import Foundation

public enum CinivaError: LocalizedError, Equatable {
    case unauthorized(String?)
    case http(Int, String?)
    case invalidServerURL
    case invalidResponse

    public var errorDescription: String? {
        switch self {
        case .unauthorized(let m): return m ?? "Your session has ended. Sign in again."
        case .http(let code, let m): return m ?? "The server returned HTTP \(code)."
        case .invalidServerURL: return "Enter a server address like http://srv1.local:8484"
        case .invalidResponse: return "The server sent a response Ciniva couldn't read."
        }
    }
}

public struct CinivaClient: Sendable {
    public static let clientName = "Ciniva for Mac"
    public static let userAgent = "Ciniva-macOS/1.0"

    public let baseURL: URL
    public let token: String?

    public init(baseURL: URL, token: String?) {
        self.baseURL = baseURL
        self.token = token
    }

    public static let session: URLSession = {
        let c = URLSessionConfiguration.default
        c.timeoutIntervalForRequest = 20
        c.httpAdditionalHeaders = ["User-Agent": userAgent]
        return URLSession(configuration: c)
    }()

    static var libraryDecoder: JSONDecoder {
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        return d
    }

    // Plugin payloads carry free-form dictionaries whose keys must not be rewritten.
    static var pluginDecoder: JSONDecoder { JSONDecoder() }

    public static func normalize(_ text: String) -> URL? {
        var s = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !s.isEmpty else { return nil }
        if !s.contains("://") { s = "http://" + s }
        while s.hasSuffix("/") { s.removeLast() }
        guard let url = URL(string: s), let scheme = url.scheme, ["http", "https"].contains(scheme), url.host != nil else { return nil }
        return url
    }

    // MARK: URLs

    public func apiURL(_ path: String, query: [URLQueryItem] = [], withToken: Bool = false) -> URL {
        var raw = path
        var extra: [URLQueryItem] = []
        if let q = raw.firstIndex(of: "?") {
            extra = URLComponents(string: String(raw[q...]))?.queryItems ?? []
            raw = String(raw[..<q])
        }
        let encoded = raw.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? raw
        var c = URLComponents(url: baseURL, resolvingAgainstBaseURL: false)!
        c.percentEncodedPath = (c.percentEncodedPath.hasSuffix("/") ? String(c.percentEncodedPath.dropLast()) : c.percentEncodedPath) + "/api" + (encoded.hasPrefix("/") ? encoded : "/" + encoded)
        var items = extra + query
        if withToken, let token { items.append(URLQueryItem(name: "token", value: token)) }
        c.queryItems = items.isEmpty ? nil : items
        return c.url!
    }

    public func streamURL(fileId: Int) -> URL { apiURL("/files/\(fileId)/stream", withToken: true) }

    public func subtitleURL(fileId: Int, index: Int) -> URL { apiURL("/files/\(fileId)/subtitles/\(index).vtt", withToken: true) }

    public func trickplaySheetURL(fileId: Int, sheet: Int) -> URL { apiURL("/files/\(fileId)/trickplay/\(sheet).jpg", withToken: true) }

    public func imageURL(_ path: String?, size: String = "w342") -> URL? {
        guard var p = path, !p.isEmpty else { return nil }
        if p.hasPrefix("http://") || p.hasPrefix("https://") { return URL(string: p) }
        if p.hasPrefix("/api/") { return apiURL(String(p.dropFirst(4)), withToken: true) }
        while p.hasPrefix("/") { p.removeFirst() }
        return apiURL("/images/\(size)/\(p)", withToken: true)
    }

    // MARK: Transport

    public func send(_ method: String, _ path: String, query: [URLQueryItem] = [], body: JSONValue? = nil) async throws -> Data {
        var req = URLRequest(url: apiURL(path, query: query))
        req.httpMethod = method
        req.setValue("application/json", forHTTPHeaderField: "Accept")
        if let token { req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization") }
        if let body {
            req.httpBody = try body.encoded()
            req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        let (data, resp) = try await Self.session.data(for: req)
        guard let http = resp as? HTTPURLResponse else { throw CinivaError.invalidResponse }
        if (200..<300).contains(http.statusCode) { return data }
        let message = (try? JSONDecoder().decode([String: String].self, from: data))?["error"]
        if http.statusCode == 401 { throw CinivaError.unauthorized(message) }
        throw CinivaError.http(http.statusCode, message)
    }

    public func get<T: Decodable>(_ path: String, query: [URLQueryItem] = [], as type: T.Type = T.self) async throws -> T {
        let data = try await send("GET", path, query: query)
        do {
            return try Self.libraryDecoder.decode(T.self, from: data)
        } catch {
            throw CinivaError.invalidResponse
        }
    }

    // MARK: Account

    public static func setupState(baseURL: URL) async throws -> SetupState {
        try await CinivaClient(baseURL: baseURL, token: nil).get("/setup")
    }

    public static func login(baseURL: URL, username: String, password: String) async throws -> (CinivaClient, User) {
        let c = CinivaClient(baseURL: baseURL, token: nil)
        let data = try await c.send("POST", "/auth/login", body: [
            "username": .string(username),
            "password": .string(password),
            "client": .string(clientName),
        ])
        guard let r = try? libraryDecoder.decode(LoginResponse.self, from: data) else { throw CinivaError.invalidResponse }
        return (CinivaClient(baseURL: baseURL, token: r.token), r.user)
    }

    public func me() async throws -> User? {
        struct Me: Decodable { let user: User? }
        return try await get("/me", as: Me.self).user
    }

    public func logout() async {
        _ = try? await send("POST", "/auth/logout")
    }

    // MARK: Library

    public func home() async throws -> HomeData { try await get("/home") }

    public func movies(sort: String) async throws -> [MovieCard] {
        try await get("/movies", query: [URLQueryItem(name: "sort", value: sort)])
    }

    public func shows(sort: String) async throws -> [ShowCard] {
        try await get("/shows", query: [URLQueryItem(name: "sort", value: sort)])
    }

    public func movie(_ id: Int) async throws -> MovieDetail { try await get("/movies/\(id)") }

    public func show(_ id: Int) async throws -> ShowDetail { try await get("/shows/\(id)") }

    public func season(show: Int, number: Int) async throws -> [Episode] { try await get("/shows/\(show)/seasons/\(number)") }

    public func search(_ q: String) async throws -> SearchResults {
        try await get("/search", query: [URLQueryItem(name: "q", value: q)])
    }

    public func file(_ id: Int) async throws -> MediaFile { try await get("/files/\(id)") }

    public func downloads() async throws -> [DownloadItem] { try await get("/downloads") }

    public func download(_ id: String) async throws -> DownloadItem { try await get("/downloads/\(id)") }

    public func downloadStreamURL(_ id: String) -> URL { apiURL("/downloads/\(id)/stream", withToken: true) }

    public func trickplay(fileId: Int) async -> TrickplayManifest? {
        try? await get("/files/\(fileId)/trickplay")
    }

    public func saveProgress(fileId: Int, positionMs: Int, durationMs: Int) async throws {
        _ = try await send("PUT", "/files/\(fileId)/progress", body: [
            "position_ms": .number(Double(positionMs)),
            "duration_ms": .number(Double(durationMs)),
        ])
    }

    public func setWatched(fileId: Int, _ watched: Bool) async throws {
        _ = try await send("POST", "/files/\(fileId)/watched", body: ["watched": .bool(watched)])
    }

    // MARK: Plugins

    /// Installed plugins; empty when the server has no plugin support yet.
    public func plugins() async throws -> [PluginInfo] {
        let data: Data
        do {
            data = try await send("GET", "/plugins")
        } catch CinivaError.http(404, _) {
            return []
        }
        return (try? Self.pluginDecoder.decode([PluginInfo].self, from: data)) ?? []
    }

    public func pluginPage(plugin: String, page: String, params: [URLQueryItem] = []) async throws -> PluginView {
        let data = try await send("GET", "/plugins/\(plugin)/ui/pages/\(page)", query: params)
        do {
            return try Self.pluginDecoder.decode(PluginView.self, from: data)
        } catch {
            throw CinivaError.invalidResponse
        }
    }

    public func pluginCall(plugin: String, _ call: PluginCall, body: [String: JSONValue]? = nil) async throws -> PluginResult {
        let method = call.method.uppercased()
        let payload = body ?? call.body
        var path = call.path
        while path.hasPrefix("/") { path.removeFirst() }
        let query = method == "GET" ? (payload ?? [:]).sorted { $0.key < $1.key }.compactMap { k, v in v.stringValue.map { URLQueryItem(name: k, value: $0) } } : []
        let data = try await send(method, "/plugins/\(plugin)/ui/\(path)", query: query, body: method == "GET" ? nil : payload.map(JSONValue.object) ?? .object([:]))
        return Self.decodeResult(data)
    }

    public func pluginAction(plugin: String, action: String, context: PluginContext) async throws -> PluginResult {
        let data = try await send("POST", "/plugins/\(plugin)/ui/actions/\(action)", body: ["context": context.json])
        return Self.decodeResult(data)
    }

    public func pluginSettings(plugin: String) async throws -> [PluginField] {
        struct Settings: Decodable { let fields: [PluginField] }
        let data = try await send("GET", "/plugins/\(plugin)/settings")
        guard let s = try? Self.pluginDecoder.decode(Settings.self, from: data) else { throw CinivaError.invalidResponse }
        return s.fields
    }

    public func savePluginSettings(plugin: String, values: [String: JSONValue]) async throws {
        _ = try await send("PUT", "/plugins/\(plugin)/settings", body: ["values": .object(values)])
    }

    /// Server-Sent Events from `/api/events` as (event name, JSON data) pairs until the connection drops.
    public func events() -> AsyncThrowingStream<(String, JSONValue), Error> {
        var req = URLRequest(url: apiURL("/events"))
        req.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        req.timeoutInterval = 3600
        if let token { req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization") }
        let request = req
        return AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    let (bytes, resp) = try await Self.session.bytes(for: request)
                    if let http = resp as? HTTPURLResponse, http.statusCode == 401 { throw CinivaError.unauthorized(nil) }
                    if let http = resp as? HTTPURLResponse, !(200..<300).contains(http.statusCode) { throw CinivaError.http(http.statusCode, nil) }
                    var event = "message"
                    var data = ""
                    for try await line in bytes.lines {
                        if line.hasPrefix("event:") {
                            event = line.dropFirst(6).trimmingCharacters(in: .whitespaces)
                        } else if line.hasPrefix("data:") {
                            data += (data.isEmpty ? "" : "\n") + line.dropFirst(5).trimmingCharacters(in: .whitespaces)
                        }
                        // URLSession's line reader drops blank lines, so each data line completes an event.
                        if line.hasPrefix("data:") {
                            let value = (try? JSONDecoder().decode(JSONValue.self, from: Data(data.utf8))) ?? .string(data)
                            continuation.yield((event, value))
                            event = "message"
                            data = ""
                        }
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    static func decodeResult(_ data: Data) -> PluginResult {
        guard !data.isEmpty else { return .empty }
        return (try? pluginDecoder.decode(PluginResult.self, from: data)) ?? .empty
    }

    public static func decodePluginView(_ data: Data) throws -> PluginView {
        try pluginDecoder.decode(PluginView.self, from: data)
    }

    public static func decodePluginResult(_ data: Data) throws -> PluginResult {
        try pluginDecoder.decode(PluginResult.self, from: data)
    }

    public static func decodePlugins(_ data: Data) throws -> [PluginInfo] {
        try pluginDecoder.decode([PluginInfo].self, from: data)
    }
}
