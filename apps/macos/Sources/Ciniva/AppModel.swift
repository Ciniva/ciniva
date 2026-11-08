import AppKit
import CinivaKit
import Observation

struct PlaybackRequest: Identifiable, Equatable {
    let id = UUID()
    var fileId: Int?
    var title: String
    var subtitle: String?
    var showId: Int?
    var startAt: Double?
    var fileURL: URL?

    static func episode(_ t: EpisodeTarget, startAt: Double?) -> PlaybackRequest {
        PlaybackRequest(fileId: t.fileId, title: t.showTitle, subtitle: t.label, showId: t.showId, startAt: startAt)
    }
}

struct Toast: Identifiable, Equatable {
    let id = UUID()
    let text: String
    let tone: Tone
}

enum Route: Hashable {
    case movie(Int)
    case show(Int)
    case plugin(String, PluginNav)
}

enum SidebarItem: Hashable {
    case home
    case movies
    case shows
    case plugin(String, String)
}

struct PluginSheet: Identifiable {
    let id = UUID()
    let pluginId: String
    var view: PluginView
}

@MainActor
@Observable
final class AppModel {
    enum Phase: Equatable {
        case loading
        case signedOut
        case ready
    }

    static let defaultServer = "http://srv1.local:8484"
    private static let serverKey = "ciniva.server"
    private static let usernameKey = "ciniva.username"
    private static let tokenAccount = "session"

    var phase: Phase = .loading
    var client: CinivaClient?
    var user: User?
    var serverText = UserDefaults.standard.string(forKey: AppModel.serverKey) ?? AppModel.defaultServer
    var usernameText = UserDefaults.standard.string(forKey: AppModel.usernameKey) ?? ""
    var errorMessage: String?
    var isSigningIn = false
    var hasStoredSession = false
    var playback: PlaybackRequest?
    var libraryVersion = 0
    var plugins: [PluginInfo] = []
    var toasts: [Toast] = []
    var selection: SidebarItem? = .home
    var path: [Route] = []
    var pluginSheet: PluginSheet?

    @ObservationIgnored private var eventsTask: Task<Void, Never>?

    // MARK: Session

    func bootstrap() async {
        errorMessage = nil
        guard let base = CinivaClient.normalize(serverText), let token = Keychain.read(Self.tokenAccount) else {
            hasStoredSession = false
            phase = .signedOut
            return
        }
        hasStoredSession = true
        let c = CinivaClient(baseURL: base, token: token)
        do {
            guard let u = try await c.me() else { throw CinivaError.unauthorized(nil) }
            becomeReady(c, u)
        } catch CinivaError.unauthorized {
            sessionExpired()
        } catch {
            errorMessage = "Couldn't reach \(base.absoluteString): \(error.localizedDescription)"
            phase = .signedOut
        }
    }

    func signIn(password: String) async {
        errorMessage = nil
        guard let base = CinivaClient.normalize(serverText) else {
            errorMessage = CinivaError.invalidServerURL.localizedDescription
            return
        }
        let username = usernameText.trimmingCharacters(in: .whitespaces)
        guard !username.isEmpty, !password.isEmpty else {
            errorMessage = "Enter your username and password."
            return
        }
        isSigningIn = true
        defer { isSigningIn = false }
        do {
            let setup = try await CinivaClient.setupState(baseURL: base)
            if setup.needsAdmin {
                errorMessage = "This server isn't set up yet. Open \(base.absoluteString) in a browser to create the administrator account, then sign in here."
                return
            }
            let (c, u) = try await CinivaClient.login(baseURL: base, username: username, password: password)
            Keychain.save(c.token ?? "", for: Self.tokenAccount)
            serverText = base.absoluteString
            UserDefaults.standard.set(base.absoluteString, forKey: Self.serverKey)
            UserDefaults.standard.set(username, forKey: Self.usernameKey)
            hasStoredSession = true
            becomeReady(c, u)
        } catch CinivaError.unauthorized(let m) {
            errorMessage = m ?? "Wrong username or password."
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func becomeReady(_ c: CinivaClient, _ u: User) {
        client = c
        user = u
        errorMessage = nil
        selection = .home
        path = []
        phase = .ready
        Task { await loadPlugins() }
        startEvents()
    }

    func signOut() {
        let c = client
        Task { await c?.logout() }
        clearSession()
        errorMessage = nil
    }

    func sessionExpired() {
        guard client != nil || hasStoredSession else { return }
        clearSession()
        errorMessage = "Your session has ended. Sign in again."
    }

    private func clearSession() {
        Keychain.delete(Self.tokenAccount)
        eventsTask?.cancel()
        eventsTask = nil
        hasStoredSession = false
        client = nil
        user = nil
        playback = nil
        plugins = []
        path = []
        pluginSheet = nil
        phase = .signedOut
    }

    /// Runs an API call, sending the user back to sign-in when the server rejects the session.
    func call<T>(_ op: (CinivaClient) async throws -> T) async throws -> T {
        guard let client else { throw CinivaError.unauthorized(nil) }
        do {
            return try await op(client)
        } catch CinivaError.unauthorized(let m) {
            sessionExpired()
            throw CinivaError.unauthorized(m)
        }
    }

    // MARK: Playback

    func play(_ request: PlaybackRequest) {
        playback = request
    }

    func closePlayer() {
        playback = nil
        libraryVersion += 1
    }

    func playFile(_ url: URL) {
        playback = PlaybackRequest(title: url.lastPathComponent, fileURL: url)
    }

    func setWatched(_ files: [MediaFile], _ watched: Bool) async {
        do {
            try await call { c in
                for f in files { try await c.setWatched(fileId: f.id, watched) }
            }
        } catch {
            toast(error.localizedDescription, .error)
        }
        libraryVersion += 1
    }

    // MARK: Toasts & events

    func toast(_ text: String, _ tone: Tone = .info) {
        let t = Toast(text: text, tone: tone)
        toasts.append(t)
        Task {
            try? await Task.sleep(for: .seconds(tone == .error ? 6 : 4))
            toasts.removeAll { $0.id == t.id }
        }
    }

    private func startEvents() {
        eventsTask?.cancel()
        guard let client else { return }
        eventsTask = Task { [weak self] in
            while !Task.isCancelled {
                do {
                    for try await (event, data) in client.events() {
                        guard let self else { return }
                        switch event {
                        case "notification":
                            let text = [data["title"]?.stringValue, data["body"]?.stringValue].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " · ")
                            if !text.isEmpty { self.toast(text, data["tone"]?.stringValue == "error" ? .error : .info) }
                        case "library":
                            self.libraryVersion += 1
                        default:
                            break
                        }
                    }
                } catch CinivaError.unauthorized {
                    self?.sessionExpired()
                    return
                } catch {}
                try? await Task.sleep(for: .seconds(15))
            }
        }
    }

    // MARK: Plugins

    func loadPlugins() async {
        plugins = (try? await call { try await $0.plugins() }) ?? []
    }

    var navPlugins: [(PluginInfo, PluginPageRef)] {
        plugins.filter { $0.online != false }.flatMap { p in p.navPages.map { (p, $0) } }
    }

    func pluginActions(_ kind: String) -> [(PluginInfo, PluginActionRef)] {
        plugins.filter { $0.online != false }.flatMap { p in p.actions(for: kind).map { (p, $0) } }
    }

    func runPluginAction(_ plugin: PluginInfo, _ action: PluginActionRef, context: PluginContext) async {
        do {
            let result = try await call { try await $0.pluginAction(plugin: plugin.id, action: action.id, context: context) }
            handle(result, pluginId: plugin.id, show: nil, reload: nil)
        } catch {
            toast(error.localizedDescription, .error)
        }
    }

    func navigatePlugin(_ pluginId: String, _ nav: PluginNav) {
        pluginSheet = nil
        path.append(.plugin(pluginId, nav))
    }

    /// Applies a plugin `Result`: toast, then navigate, show a view, or refresh.
    func handle(_ result: PluginResult, pluginId: String, show: ((PluginView) -> Void)?, reload: (() async -> Void)?) {
        if let m = result.message, !m.isEmpty {
            toast(m, result.tone == .error ? .error : result.tone == .success ? .success : .info)
        }
        if let nav = result.navigate {
            navigatePlugin(pluginId, nav)
        } else if let view = result.view {
            if let show { show(view) } else { pluginSheet = PluginSheet(pluginId: pluginId, view: view) }
        } else if result.refresh == true {
            if let reload { Task { await reload() } } else { libraryVersion += 1 }
        }
    }
}
