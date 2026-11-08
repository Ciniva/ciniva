import AppKit
@testable import Ciniva
import CinivaKit
import SwiftUI
import XCTest

func fixture(_ name: String) throws -> Data {
    let url = try XCTUnwrap(Bundle.module.url(forResource: name, withExtension: "json", subdirectory: "Fixtures"))
    return try Data(contentsOf: url)
}

final class PluginModelTests: XCTestCase {
    func testDecodesEveryBlockType() throws {
        let view = try CinivaClient.decodePluginView(fixture("plugin_view"))
        XCTAssertEqual(view.title, "Downloads")
        XCTAssertEqual(view.refresh, 5)
        XCTAssertEqual(view.blocks.count, 8)

        guard case .text(let text, let tone) = view.blocks[0] else { return XCTFail("text") }
        XCTAssertEqual(text, "Connected to qBittorrent.")
        XCTAssertEqual(tone, .success)

        guard case .stats(let stats) = view.blocks[2] else { return XCTFail("stats") }
        XCTAssertEqual(stats.map { $0.value.stringValue }, ["3", "12.4 MB/s", "1.25"])

        guard case .actions(let actions) = view.blocks[3] else { return XCTFail("actions") }
        XCTAssertEqual(actions[1].navigate?.params?["query_text"], "rick", "free-form keys must keep their snake_case")
        XCTAssertEqual(actions[1].navigate?.queryItems.map(\.description), ["query_text=rick", "season_no=1"])
        XCTAssertEqual(actions[2].confirm, "Delete all finished torrents?")
        XCTAssertEqual(actions[2].tone, .danger)
        XCTAssertEqual(actions[2].call?.body?["with_files"], .bool(false))

        guard case .form(let form) = view.blocks[4] else { return XCTFail("form") }
        XCTAssertEqual(form.fields.map(\.kind), [.text, .password, .number, .select, .toggle, .file, .textarea])
        XCTAssertEqual(form.fields[3].options?.last?.value, .number(3))

        guard case .list(let id, let items, _) = view.blocks[5] else { return XCTFail("list") }
        XCTAssertEqual(id, "torrents")
        XCTAssertEqual(items.map(\.id), ["abc123", "42"])
        XCTAssertEqual(items[0].progress, 0.58)
        XCTAssertEqual(items[0].link?.params?["hash"], "abc123")
        XCTAssertEqual(items[0].actions?.count, 2)
        XCTAssertEqual(items[1].badges?.map(\.tone), [.success, .warning, .error])

        guard case .list(_, let none, let empty) = view.blocks[6] else { return XCTFail("empty list") }
        XCTAssertTrue(none.isEmpty)
        XCTAssertEqual(empty, "No finished downloads.")

        guard case .unknown(let type) = view.blocks[7] else { return XCTFail("unknown") }
        XCTAssertEqual(type, "chart")
    }

    func testFormBodyMergesCallBodyWithValues() throws {
        let view = try CinivaClient.decodePluginView(fixture("plugin_view"))
        guard case .form(let form) = view.blocks[4] else { return XCTFail("form") }

        XCTAssertEqual(form.missing(values: [:]), ["Magnet link"])

        var values = Dictionary(form.fields.map { ($0.name, $0.initialValue) }, uniquingKeysWith: { $1 })
        values["magnet"] = "magnet:?xt=urn:btih:abc"
        values["limit_kb"] = "750"
        values["category"] = .number(3)
        values["start_paused"] = true
        values["torrent_file"] = PluginForm.fileValue(name: "a.torrent", data: Data("hello".utf8))
        XCTAssertEqual(form.missing(values: values), [])

        let body = form.body(values: values)
        XCTAssertEqual(body["source"], "ui")
        XCTAssertEqual(body["limit_kb"], .number(750), "field values win over call.body and numbers are sent as numbers")
        XCTAssertEqual(body["category"], .number(3))
        XCTAssertEqual(body["start_paused"], .bool(true))
        XCTAssertEqual(body["secret"], "")
        XCTAssertEqual(body["notes"], "line one")
        XCTAssertEqual(body["torrent_file"], ["name": "a.torrent", "data_base64": "aGVsbG8="])

        values["limit_kb"] = ""
        values["torrent_file"] = .null
        XCTAssertEqual(form.body(values: values)["limit_kb"], .null)
        XCTAssertEqual(form.body(values: values)["torrent_file"], .null)
    }

    func testResultAndPluginList() throws {
        let r = try CinivaClient.decodePluginResult(fixture("result"))
        XCTAssertEqual(r.message, "Added 1 torrent")
        XCTAssertEqual(r.tone, .success)
        XCTAssertEqual(r.refresh, true)
        XCTAssertEqual(r.navigate?.params?["info_hash"], "abc")
        XCTAssertEqual(r.view?.title, "Search results")

        let plugins = try CinivaClient.decodePlugins(fixture("plugins"))
        XCTAssertEqual(plugins.count, 3)
        XCTAssertEqual(plugins[0].hasSettings, true)
        XCTAssertEqual(plugins[1].navPages.map(\.id), ["downloads"])
        XCTAssertEqual(plugins[1].actions(for: "season").map(\.id), ["find"])
        XCTAssertEqual(plugins[1].actions(for: "movie").count, 0)
    }

    func testContextJSON() throws {
        let c = PluginContext(kind: "season", id: 1, tmdbId: 60625, title: "Rick and Morty", year: 2013, season: 1,
                              missing: [MissingEpisode(season: 1, episode: 5, title: "Meeseeks and Destroy")])
        let json = try JSONDecoder().decode(JSONValue.self, from: c.json.encoded())
        XCTAssertEqual(json["kind"], "season")
        XCTAssertEqual(json["tmdb_id"], .number(60625))
        XCTAssertEqual(json["season"], .number(1))
        XCTAssertEqual(json["missing"]?.arrayValue?.first?["episode"], .number(5))
        let show = PluginContext(kind: "show", id: 1, tmdbId: nil, title: "X", year: nil, extra: ["seasons": [["season": 1]]])
        XCTAssertEqual(show.json["seasons"]?.arrayValue?.count, 1)
        XCTAssertEqual(show.json["tmdb_id"], .null)
    }
}

@MainActor
final class AppModelPluginTests: XCTestCase {
    func testPluginFilteringAndResultHandling() async throws {
        let model = AppModel()
        model.plugins = try CinivaClient.decodePlugins(fixture("plugins"))
        XCTAssertEqual(model.navPlugins.map(\.1.title), ["Downloads"], "offline plugins are hidden")
        XCTAssertEqual(model.pluginActions("movie").count, 0)
        XCTAssertEqual(model.pluginActions("season").map(\.1.id), ["find"])

        var shown: PluginView?
        var reloaded = false
        let view = try CinivaClient.decodePluginView(fixture("plugin_view"))

        model.handle(PluginResult(message: "Saved", tone: .success, view: view, navigate: nil, refresh: true), pluginId: "mockdl",
                     show: { shown = $0 }, reload: { reloaded = true })
        XCTAssertEqual(shown?.title, "Downloads", "a returned view replaces the current one")
        XCTAssertFalse(reloaded)
        XCTAssertEqual(model.toasts.last?.text, "Saved")

        model.handle(PluginResult(message: nil, tone: nil, view: nil, navigate: nil, refresh: true), pluginId: "mockdl", show: nil, reload: { reloaded = true })
        try await Task.sleep(for: .milliseconds(50))
        XCTAssertTrue(reloaded)

        model.handle(PluginResult(message: "Oops", tone: .error, view: view, navigate: nil, refresh: nil), pluginId: "mockdl", show: nil, reload: nil)
        XCTAssertEqual(model.pluginSheet?.view.title, "Downloads", "a view from a detail-page action opens in a sheet")
        XCTAssertEqual(model.toasts.last?.tone, .error)

        model.handle(PluginResult(message: nil, tone: nil, view: view, navigate: PluginNav(page: "torrent", params: ["hash": "x"]), refresh: nil), pluginId: "mockdl", show: nil, reload: nil)
        XCTAssertNil(model.pluginSheet, "navigating closes the sheet")
        XCTAssertEqual(model.path, [.plugin("mockdl", PluginNav(page: "torrent", params: ["hash": "x"]))])
    }

    /// Renders the fixture page natively and writes a PNG (to $CINIVA_RENDER_DIR when set) for a visual check.
    func testRenderFixturePage() throws {
        let view = try CinivaClient.decodePluginView(fixture("plugin_view"))
        let model = AppModel()
        model.client = CinivaClient(baseURL: URL(string: "http://127.0.0.1:9")!, token: "t")
        let content = PluginViewRenderer(view: view, host: PluginHost(pluginId: "mockdl", show: { _ in }, reload: {}))
            .environment(model)
            .padding(24)
            .frame(width: 900, alignment: .topLeading)
            .background(Color(nsColor: .windowBackgroundColor))
            .environment(\.colorScheme, .dark)
        let hosting = NSHostingView(rootView: content)
        hosting.appearance = NSAppearance(named: .darkAqua)
        let size = hosting.fittingSize
        XCTAssertGreaterThan(size.height, 500)
        hosting.frame = NSRect(origin: .zero, size: size)
        let window = NSWindow(contentRect: hosting.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.contentView = hosting
        hosting.layoutSubtreeIfNeeded()
        RunLoop.main.run(until: Date().addingTimeInterval(0.5))
        let rep = try XCTUnwrap(hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds))
        hosting.cacheDisplay(in: hosting.bounds, to: rep)
        let png = try XCTUnwrap(rep.representation(using: .png, properties: [:]))
        let dir = ProcessInfo.processInfo.environment["CINIVA_RENDER_DIR"].map { URL(fileURLWithPath: $0) } ?? FileManager.default.temporaryDirectory
        try png.write(to: dir.appendingPathComponent("plugin_fixture.png"))
        XCTAssertGreaterThan(png.count, 10_000)
    }
}

final class LibraryTests: XCTestCase {
    private func episode(_ s: Int, _ e: Int, file: Int?) throws -> Episode {
        let files = file.map { #"[{"id":\#($0),"path":"/m/x.mkv","size":1,"duration":60,"probe":null,"added_at":null,"credits_start":null,"credits_source":null,"position_ms":null,"duration_ms":null,"watched":null}]"# } ?? "[]"
        let json = #"{"id":\#(s * 100 + e),"season_number":\#(s),"episode_number":\#(e),"title":"T","overview":null,"still_path":null,"air_date":null,"runtime":22,"rating":null,"files":\#(files)}"#
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        return try d.decode(Episode.self, from: Data(json.utf8))
    }

    func testNeighboursCrossSeasonsButNotSpecials() throws {
        let all = try [
            episode(1, 1, file: 11), episode(1, 2, file: nil), episode(1, 3, file: 13),
            episode(2, 1, file: 21),
            episode(0, 1, file: 1), episode(0, 2, file: 2),
        ]
        XCTAssertEqual(EpisodeOrder.neighbour(in: all, fileId: 11, step: 1)?.episodeNumber, 3, "skips episodes without files")
        XCTAssertEqual(EpisodeOrder.neighbour(in: all, fileId: 13, step: 1)?.seasonNumber, 2, "crosses into the next season")
        XCTAssertEqual(EpisodeOrder.neighbour(in: all, fileId: 21, step: -1)?.seasonNumber, 1, "crosses back")
        XCTAssertNil(EpisodeOrder.neighbour(in: all, fileId: 21, step: 1), "does not run into specials")
        XCTAssertNil(EpisodeOrder.neighbour(in: all, fileId: 1, step: -1), "does not run out of specials")
        XCTAssertEqual(EpisodeOrder.neighbour(in: all, fileId: 1, step: 1)?.episodeNumber, 2)
        XCTAssertNil(EpisodeOrder.neighbour(in: all, fileId: 11, step: -1))
    }

    func testURLs() {
        let c = CinivaClient(baseURL: CinivaClient.normalize("srv1.local:8484/")!, token: "tok")
        XCTAssertEqual(c.baseURL.absoluteString, "http://srv1.local:8484")
        XCTAssertEqual(c.streamURL(fileId: 4).absoluteString, "http://srv1.local:8484/api/files/4/stream?token=tok")
        XCTAssertEqual(c.imageURL("/abc.jpg", size: "w300")?.absoluteString, "http://srv1.local:8484/api/images/w300/abc.jpg?token=tok")
        XCTAssertEqual(c.imageURL("https://x/y.png")?.absoluteString, "https://x/y.png")
        XCTAssertEqual(c.apiURL("/plugins/q/ui/pages/search?x=1", query: [URLQueryItem(name: "y", value: "a b")]).absoluteString,
                       "http://srv1.local:8484/api/plugins/q/ui/pages/search?x=1&y=a%20b")
        XCTAssertNil(CinivaClient.normalize("  "))
        XCTAssertNil(CinivaClient.normalize("ftp://x"))
    }

    func testTrickplayTiles() throws {
        let m = try JSONDecoder().decode(TrickplayManifest.self, from: Data(#"{"interval":10,"width":320,"height":180,"cols":10,"rows":10,"count":150,"sheets":2}"#.utf8))
        XCTAssertEqual(m.tile(at: 0).sheet, 1)
        XCTAssertEqual(m.tile(at: 125).x, 2 * 320)
        XCTAssertEqual(m.tile(at: 125).y, 1 * 180)
        XCTAssertEqual(m.tile(at: 1005).sheet, 2)
        XCTAssertEqual(m.tile(at: 99999).sheet, 2)
    }

    func testEngineChoice() throws {
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        func file(_ path: String, video: String, audio: String, dv: Int? = nil) throws -> MediaFile {
            let json = #"{"id":1,"path":"\#(path)","size":1,"duration":60,"probe":{"container":"x","duration":60,"bitrate":1,"video":{"codec":"\#(video)","profile":null,"width":1920,"height":1080,"bit_depth":10,"hdr":null,"dolby_vision_profile":\#(dv.map(String.init) ?? "null"),"fps":24},"audio":[{"index":1,"codec":"\#(audio)","profile":null,"channels":6,"language":"eng","title":null,"atmos":false,"default":true}],"subtitles":[]},"added_at":null,"credits_start":null,"credits_source":null,"position_ms":null,"duration_ms":null,"watched":null}"#
            return try d.decode(MediaFile.self, from: Data(json.utf8))
        }
        UserDefaults.standard.removeObject(forKey: Prefs.engine)
        Prefs.register()
        XCTAssertEqual(PlayerController.chooseEngine(for: try file("/a.mp4", video: "hevc", audio: "eac3")), .avfoundation)
        XCTAssertEqual(PlayerController.chooseEngine(for: try file("/a.mkv", video: "hevc", audio: "eac3")), .mpv)
        XCTAssertEqual(PlayerController.chooseEngine(for: try file("/a.mp4", video: "hevc", audio: "truehd")), .mpv)
        XCTAssertEqual(PlayerController.chooseEngine(for: try file("/a.mkv", video: "hevc", audio: "truehd", dv: 5)), .mpvWindow)
        XCTAssertEqual(try file("/a.mkv", video: "hevc", audio: "eac3", dv: 8).probe?.techBadges, ["1080p", "Dolby Vision P8", "HEVC", "Dolby Digital+ 5.1"])
    }
}
