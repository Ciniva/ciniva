import CinivaKit
import Foundation

// Signs in to a Ciniva server and exercises the API client end to end.
// Usage: ciniva-check [server] [username] [password]   (or CINIVA_URL / CINIVA_USER / CINIVA_PASSWORD)

let env = ProcessInfo.processInfo.environment
let args = Array(CommandLine.arguments.dropFirst()).filter { !$0.hasPrefix("--") }
let server = args.count > 0 ? args[0] : env["CINIVA_URL"] ?? "http://127.0.0.1:18600"
let username = args.count > 1 ? args[1] : env["CINIVA_USER"] ?? "admin"
let password = args.count > 2 ? args[2] : env["CINIVA_PASSWORD"] ?? ""

var failures = 0

func check(_ name: String, _ ok: Bool, _ detail: @autoclosure () -> String = "") {
    print("\(ok ? "PASS" : "FAIL")  \(name)\(detail().isEmpty ? "" : "  — \(detail())")")
    if !ok { failures += 1 }
}

func step<T>(_ name: String, _ body: () async throws -> T) async -> T? {
    do {
        return try await body()
    } catch {
        check(name, false, error.localizedDescription)
        return nil
    }
}

func head(_ url: URL, range: String? = nil) async -> (Int, String?) {
    var req = URLRequest(url: url)
    if let range { req.setValue(range, forHTTPHeaderField: "Range") }
    guard let (_, resp) = try? await CinivaClient.session.data(for: req), let http = resp as? HTTPURLResponse else { return (0, nil) }
    return (http.statusCode, http.value(forHTTPHeaderField: "Content-Type"))
}

func run() async {
    guard let base = CinivaClient.normalize(server) else {
        check("server URL", false, server)
        return
    }
    print("Server \(base.absoluteString) as \(username)")

    if let setup = await step("setup state", { try await CinivaClient.setupState(baseURL: base) }) {
        check("setup state", !setup.needsAdmin, "needs_admin=\(setup.needsAdmin) version=\(setup.version ?? "?")")
    }

    let anon = CinivaClient(baseURL: base, token: nil)
    do {
        _ = try await anon.home()
        check("home without token is rejected", false)
    } catch CinivaError.unauthorized {
        check("home without token is rejected", true, "401 → unauthorized")
    } catch {
        check("home without token is rejected", false, error.localizedDescription)
    }

    do {
        _ = try await CinivaClient.login(baseURL: base, username: username, password: password + "-wrong")
        check("wrong password is rejected", false)
    } catch CinivaError.unauthorized(let m) {
        check("wrong password is rejected", true, m ?? "")
    } catch {
        check("wrong password is rejected", false, error.localizedDescription)
    }

    guard let (client, user) = await step("login", { try await CinivaClient.login(baseURL: base, username: username, password: password) }) else { return }
    check("login", true, "user \(user.username) admin=\(user.admin)")

    if let me = await step("me", { try await client.me() }) {
        check("me", me?.id == user.id, me?.username ?? "nil")
    }

    if let home = await step("home", { try await client.home() }) {
        check("home", true, "continue=\(home.continueWatching.count) movies=\(home.recentMovies.count) shows=\(home.recentShows.count)")
        if let m = home.recentMovies.first, let url = client.imageURL(m.posterPath, size: "w185") {
            let (code, type) = await head(url)
            check("poster image", code == 200 && (type ?? "").hasPrefix("image/"), "\(code) \(type ?? "")")
        }
    }

    for sort in ["title", "added", "year", "rating"] {
        if let m = await step("movies sort=\(sort)", { try await client.movies(sort: sort) }),
           let s = await step("shows sort=\(sort)", { try await client.shows(sort: sort) }) {
            check("movies/shows sort=\(sort)", true, "\(m.map(\.title).joined(separator: ", ")) | \(s.map(\.title).joined(separator: ", "))")
        }
    }

    if let r = await step("search", { try await client.search("ri") }) {
        check("search 'ri'", !r.shows.isEmpty || !r.movies.isEmpty, "movies=\(r.movies.count) shows=\(r.shows.count)")
    }

    if let movies = try? await client.movies(sort: "title"), let first = movies.first,
       let m = await step("movie detail", { try await client.movie(first.id) }) {
        check("movie detail", !m.files.isEmpty, "\(m.title): \(m.files.count) file(s), \(m.castMembers.count) cast, badges \(m.files.first?.probe?.techBadges.joined(separator: " ") ?? "-")")
    }

    guard let shows = try? await client.shows(sort: "title"), let firstShow = shows.first,
          let show = await step("show detail", { try await client.show(firstShow.id) }) else { return }
    check("show detail", !show.seasons.isEmpty, "\(show.title): seasons \(show.seasons.map(\.label).joined(separator: ", "))")

    guard let seasonNo = EpisodeOrder.seasons(show.seasons).first?.seasonNumber,
          let episodes = await step("season", { try await client.season(show: show.id, number: seasonNo) }) else { return }
    check("season \(seasonNo)", !episodes.isEmpty, episodes.map { "\($0.episodeNumber):\($0.files.count)f" }.joined(separator: " "))

    let playable = episodes.filter { !$0.files.isEmpty }
    let nav = EpisodeNavigator(client: client)
    if playable.count >= 2 {
        let a = playable[0].files[0].id, b = playable[1].files[0].id
        let next = (try? await nav.neighbour(showId: show.id, fileId: a, step: 1)) ?? nil
        let prev = (try? await nav.neighbour(showId: show.id, fileId: b, step: -1)) ?? nil
        let none = (try? await nav.neighbour(showId: show.id, fileId: a, step: -1)) ?? nil
        check("episode neighbours", next?.fileId == b && prev?.fileId == a && none == nil,
              "next(\(a))=\(next?.label ?? "nil") prev(\(b))=\(prev?.label ?? "nil") prev(\(a))=\(none?.label ?? "nil")")
        let located = try? await nav.locate(showId: show.id, fileId: b)
        check("locate episode", located == seasonNo, "season \(String(describing: located))")
    }

    guard let target = playable.first?.files.first,
          let file = await step("file", { try await client.file(target.id) }) else { return }
    check("file", file.probe != nil, "\(file.fileName) \(Fmt.clock(file.totalSeconds)) credits=\(file.creditsStart.map { Fmt.clock($0) } ?? "none") pos=\(file.positionMs ?? -1) watched=\(String(describing: file.watched))")

    let (code, type) = await head(client.streamURL(fileId: file.id), range: "bytes=0-1023")
    check("stream (Range, ?token=)", code == 206, "\(code) \(type ?? "")")
    if let tp = await client.trickplay(fileId: file.id) {
        let (c2, _) = await head(client.trickplaySheetURL(fileId: file.id, sheet: 1))
        check("trickplay", c2 == 200, "\(tp.count) thumbs every \(tp.interval)s, sheet 1 → \(c2)")
    } else {
        print("INFO  no trickplay manifest for file \(file.id)")
    }

    let originalPos = file.positionMs ?? 0, originalDur = file.durationMs ?? 0, originalWatched = file.watched ?? false
    let durMs = Int(file.totalSeconds * 1000)
    let testPos = min(45_000, durMs / 4)
    if await step("PUT progress", { try await client.saveProgress(fileId: file.id, positionMs: testPos, durationMs: durMs) }) != nil,
       let after = try? await client.file(file.id) {
        check("PUT progress", after.positionMs == testPos && after.watched == false, "position_ms=\(after.positionMs ?? -1) watched=\(String(describing: after.watched))")
        if let home = try? await client.home() {
            check("progress shows in continue watching", home.continueWatching.contains { $0.fileId == file.id } == (testPos > 30_000), "\(home.continueWatching.map(\.title))")
        }
    }

    _ = await step("watched toggle", { try await client.setWatched(fileId: file.id, true) })
    if let w = try? await client.file(file.id) { check("mark watched", w.watched == true) }

    _ = await step("restore", {
        try await client.setWatched(fileId: file.id, originalWatched)
        if !originalWatched, originalPos > 0 {
            try await client.saveProgress(fileId: file.id, positionMs: originalPos, durationMs: originalDur)
        }
    })
    if let r = try? await client.file(file.id) {
        check("progress restored", (r.watched ?? false) == originalWatched && (r.positionMs ?? 0) == (originalWatched ? 0 : originalPos),
              "position_ms=\(r.positionMs ?? -1) watched=\(String(describing: r.watched))")
    }

    if let plugins = await step("plugins", { try await client.plugins() }) {
        check("plugins", true, plugins.isEmpty ? "none registered (or server has no plugin API yet)" : plugins.map { "\($0.name) [\($0.navPages.map(\.title).joined(separator: ", "))]" }.joined(separator: "; "))
    }

    await client.logout()
    do {
        _ = try await client.home()
        check("logout ends the session", false)
    } catch CinivaError.unauthorized {
        check("logout ends the session", true)
    } catch {
        check("logout ends the session", false, error.localizedDescription)
    }
}

/// Exercises the plugin wire format against a mock server: `ciniva-check --plugin-wire http://127.0.0.1:PORT`.
func pluginWire(_ base: URL) async {
    let client = CinivaClient(baseURL: base, token: "wire-token")
    guard let plugins = await step("plugin list", { try await client.plugins() }) else { return }
    check("plugin list", plugins.map(\.id) == ["mockdl"], plugins.map(\.id).joined(separator: ","))
    let nav = PluginNav(page: "downloads", params: ["query_text": "rick", "season_no": 1])
    if let view = await step("page", { try await client.pluginPage(plugin: "mockdl", page: nav.page, params: nav.queryItems) }) {
        check("page", view.blocks.count == 8, "\(view.title ?? "") with \(view.blocks.count) blocks")
        if case .form(let form) = view.blocks[4], let call = form.submit.call {
            var values: [String: JSONValue] = ["magnet": "magnet:?xt=abc", "limit_kb": "750"]
            values["torrent_file"] = PluginForm.fileValue(name: "a.torrent", data: Data("hello".utf8))
            let r = await step("form submit", { try await client.pluginCall(plugin: "mockdl", call, body: form.body(values: values)) })
            check("form submit", r?.message == "ok POST /api/plugins/mockdl/ui/torrents/add", r?.message ?? "")
        }
    }
    let g = await step("GET call", { try await client.pluginCall(plugin: "mockdl", PluginCall(method: "GET", path: "/search", body: ["q": "rick morty"])) })
    check("GET call puts body in the query", g?.message == "ok GET /api/plugins/mockdl/ui/search?q=rick+morty" || g?.message == "ok GET /api/plugins/mockdl/ui/search?q=rick%20morty", g?.message ?? "")
    let d = await step("DELETE call", { try await client.pluginCall(plugin: "mockdl", PluginCall(method: "DELETE", path: "torrents/abc123")) })
    check("DELETE call", d?.message == "ok DELETE /api/plugins/mockdl/ui/torrents/abc123", d?.message ?? "")
    let ctx = PluginContext(kind: "season", id: 1, tmdbId: 60625, title: "Rick and Morty", year: 2013, season: 1,
                            missing: [MissingEpisode(season: 1, episode: 5, title: "Meeseeks and Destroy")])
    let a = await step("action", { try await client.pluginAction(plugin: "mockdl", action: "find", context: ctx) })
    check("action", a?.message == "ok POST /api/plugins/mockdl/ui/actions/find" && a?.view != nil, a?.message ?? "")
}

if let i = CommandLine.arguments.firstIndex(of: "--plugin-wire"), i + 1 < CommandLine.arguments.count, let u = URL(string: CommandLine.arguments[i + 1]) {
    await pluginWire(u)
} else {
    await run()
}
print(failures == 0 ? "\nAll checks passed." : "\n\(failures) check(s) failed.")
exit(failures == 0 ? 0 : 1)
