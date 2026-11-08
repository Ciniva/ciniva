import CinivaKit
import SwiftUI

struct PluginActionButtons: View {
    @Environment(AppModel.self) private var model
    let kind: String
    let context: PluginContext

    var body: some View {
        ForEach(model.pluginActions(kind), id: \.1.id) { plugin, action in
            Button {
                Task { await model.runPluginAction(plugin, action, context: context) }
            } label: {
                Label(action.title, systemImage: symbol(for: action.icon ?? "search"))
            }
            .help(plugin.name)
        }
    }
}

struct ResumeLine: View {
    let file: MediaFile?

    var body: some View {
        if let f = file, let p = f.progress, let pos = f.positionMs, let dur = f.durationMs {
            HStack(spacing: 10) {
                ProgressBar(progress: p, height: 5)
                    .frame(width: 180)
                    .clipShape(Capsule())
                Text("\(Fmt.clock(Double(dur - pos) / 1000)) left · stopped at \(Fmt.clock(Double(pos) / 1000))")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
        }
    }
}

struct FileDetails: View {
    let file: MediaFile

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let p = file.probe {
                HStack { ForEach(p.techBadges, id: \.self) { Badge(text: $0) } }
            }
            Text("\(file.fileName) · \(Fmt.bytes(file.size))")
                .font(.callout).foregroundStyle(.secondary).textSelection(.enabled)
            Text([file.creditsLabel, PlayerController.chooseEngine(for: file) == .avfoundation ? "plays natively (AVFoundation)" : "plays via mpv"].joined(separator: " · "))
                .font(.callout).foregroundStyle(.secondary)
            if let p = file.probe, p.audio.count > 1 || !p.subtitles.isEmpty {
                Text([
                    p.audio.count > 1 ? "\(p.audio.count) audio tracks" : nil,
                    p.subtitles.isEmpty ? nil : "\(p.subtitles.count) subtitle track\(p.subtitles.count == 1 ? "" : "s")",
                ].compactMap { $0 }.joined(separator: " · "))
                .font(.callout).foregroundStyle(.secondary)
            }
        }
    }
}

struct MovieDetailView: View {
    @Environment(AppModel.self) private var model
    let id: Int

    @State private var movie: MovieDetail?
    @State private var fileId: Int?
    @State private var error: String?

    private var file: MediaFile? { movie?.files.first { $0.id == fileId } ?? movie?.files.first }

    var body: some View {
        ScrollView {
            if let m = movie {
                VStack(alignment: .leading, spacing: 0) {
                    Hero(url: model.client?.imageURL(m.backdropPath, size: "w1280"))
                    HStack(alignment: .top, spacing: 28) {
                        RemoteImage(url: model.client?.imageURL(m.posterPath, size: "w500"))
                            .frame(width: 200, height: 300)
                            .clipShape(RoundedRectangle(cornerRadius: 10))
                            .shadow(radius: 10)
                        VStack(alignment: .leading, spacing: 14) {
                            Text(m.title).font(.largeTitle.bold()).lineLimit(2)
                            Text([m.year.map(String.init), Fmt.runtime(minutes: m.runtime), m.rating.map { String(format: "★ %.1f", $0) }, m.genres?.joined(separator: ", ")]
                                .compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: "  ·  "))
                                .foregroundStyle(.secondary)
                            if let t = m.tagline, !t.isEmpty { Text(t).italic().foregroundStyle(.secondary) }
                            actions(m)
                            if m.files.count > 1 {
                                Picker("Version", selection: Binding(get: { file?.id ?? 0 }, set: { fileId = $0 })) {
                                    ForEach(m.files) { f in
                                        Text([f.probe?.techBadges.prefix(3).joined(separator: " "), Fmt.bytes(f.size), f.fileName].compactMap { $0 }.joined(separator: " · ")).tag(f.id)
                                    }
                                }
                                .frame(maxWidth: 560)
                            }
                            ResumeLine(file: file)
                            if let f = file { FileDetails(file: f) } else { Text("No playable file.").foregroundStyle(.secondary) }
                            if let o = m.overview, !o.isEmpty {
                                Text(o).frame(maxWidth: 820, alignment: .leading).textSelection(.enabled)
                            }
                        }
                    }
                    .padding(.horizontal, 24)
                    .padding(.top, -140)
                    CastRow(cast: m.castMembers).padding(24)
                }
            }
        }
        .ignoresSafeArea(edges: .top)
        .overlay {
            if let error { ContentUnavailableView("Couldn't load movie", systemImage: "exclamationmark.triangle", description: Text(error)) }
            else if movie == nil { ProgressView() }
        }
        .navigationTitle(movie?.title ?? "")
        .task(id: model.libraryVersion) { await load() }
    }

    @ViewBuilder
    private func actions(_ m: MovieDetail) -> some View {
        let watched = m.files.contains { $0.isWatched }
        HStack {
            if let f = file {
                Button {
                    model.play(PlaybackRequest(fileId: f.id, title: m.title, subtitle: m.year.map(String.init)))
                } label: {
                    Label(f.progress != nil ? "Resume" : "Play", systemImage: "play.fill").frame(minWidth: 80)
                }
                .buttonStyle(.borderedProminent)
                .tint(accent)
                if f.progress != nil {
                    Button {
                        model.play(PlaybackRequest(fileId: f.id, title: m.title, subtitle: m.year.map(String.init), startAt: 0))
                    } label: {
                        Label("Start Over", systemImage: "arrow.counterclockwise")
                    }
                }
            }
            Button {
                Task { await model.setWatched(m.files, !watched) }
            } label: {
                Label(watched ? "Mark Unwatched" : "Mark Watched", systemImage: watched ? "eye.slash" : "checkmark")
            }
            PluginActionButtons(kind: "movie", context: PluginContext(kind: "movie", id: m.id, tmdbId: m.tmdbId, title: m.title, year: m.year))
        }
        .controlSize(.large)
    }

    private func load() async {
        do {
            movie = try await model.call { try await $0.movie(id) }
            error = nil
        } catch {
            if movie == nil { self.error = error.localizedDescription }
        }
    }
}

struct ShowDetailView: View {
    @Environment(AppModel.self) private var model
    let id: Int

    @State private var show: ShowDetail?
    @State private var seasonNumber: Int?
    @State private var episodes: [Episode] = []
    @State private var error: String?

    private var current: SeasonCard? { show?.seasons.first { $0.seasonNumber == seasonNumber } }
    private var upNext: Episode? { episodes.first { $0.file != nil && $0.file?.isWatched != true } }

    var body: some View {
        ScrollView {
            if let s = show {
                VStack(alignment: .leading, spacing: 0) {
                    Hero(url: model.client?.imageURL(s.backdropPath, size: "w1280"))
                    HStack(alignment: .top, spacing: 28) {
                        RemoteImage(url: model.client?.imageURL(s.posterPath, size: "w500"))
                            .frame(width: 200, height: 300)
                            .clipShape(RoundedRectangle(cornerRadius: 10))
                            .shadow(radius: 10)
                        VStack(alignment: .leading, spacing: 14) {
                            Text(s.title).font(.largeTitle.bold()).lineLimit(2)
                            Text([s.year.map(String.init), s.status, s.rating.map { String(format: "★ %.1f", $0) }, s.genres?.joined(separator: ", ")]
                                .compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: "  ·  "))
                                .foregroundStyle(.secondary)
                            actions(s)
                            if let o = s.overview, !o.isEmpty {
                                Text(o).frame(maxWidth: 820, alignment: .leading).textSelection(.enabled)
                            }
                        }
                    }
                    .padding(.horizontal, 24)
                    .padding(.top, -140)

                    if s.seasons.isEmpty {
                        Text("No episodes in the library.").foregroundStyle(.secondary).padding(24)
                    } else {
                        seasonPills(s).padding(.horizontal, 24).padding(.top, 24)
                        LazyVStack(alignment: .leading, spacing: 14) {
                            ForEach(episodes) { EpisodeRow(show: s, episode: $0) }
                        }
                        .padding(24)
                    }
                    CastRow(cast: s.castMembers).padding(24)
                }
            }
        }
        .ignoresSafeArea(edges: .top)
        .overlay {
            if let error { ContentUnavailableView("Couldn't load show", systemImage: "exclamationmark.triangle", description: Text(error)) }
            else if show == nil { ProgressView() }
        }
        .navigationTitle(show?.title ?? "")
        .task(id: model.libraryVersion) { await load() }
        .onChange(of: seasonNumber) { Task { await loadSeason() } }
    }

    @ViewBuilder
    private func actions(_ s: ShowDetail) -> some View {
        HStack {
            if let e = upNext, let f = e.file {
                Button {
                    model.play(PlaybackRequest(fileId: f.id, title: s.title, subtitle: e.label, showId: s.id))
                } label: {
                    Label("\(f.progress != nil ? "Resume" : "Play") S\(e.seasonNumber) · E\(e.episodeNumber)", systemImage: "play.fill")
                }
                .buttonStyle(.borderedProminent)
                .tint(accent)
            }
            if let c = current {
                let unwatched = c.watched < c.available
                Button {
                    Task { await model.setWatched(episodes.flatMap(\.files), unwatched) }
                } label: {
                    Label("Mark \(c.label) \(unwatched ? "Watched" : "Unwatched")", systemImage: unwatched ? "checkmark" : "eye.slash")
                }
                PluginActionButtons(kind: "show", context: PluginContext(
                    kind: "show", id: s.id, tmdbId: s.tmdbId, title: s.title, year: s.year,
                    extra: ["seasons": .array(s.seasons.map { .object(["season": .number(Double($0.seasonNumber)), "available": .number(Double($0.available)), "episodes": $0.episodeCount.map { .number(Double($0)) } ?? .null]) })]
                ))
                PluginActionButtons(kind: "season", context: PluginContext(
                    kind: "season", id: s.id, tmdbId: s.tmdbId, title: s.title, year: s.year, season: c.seasonNumber,
                    missing: episodes.filter { $0.files.isEmpty }.map { MissingEpisode(season: $0.seasonNumber, episode: $0.episodeNumber, title: $0.title) }
                ))
            }
        }
        .controlSize(.large)
    }

    private func seasonPills(_ s: ShowDetail) -> some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(s.seasons) { x in
                    Button {
                        seasonNumber = x.seasonNumber
                    } label: {
                        HStack(spacing: 6) {
                            Text(x.label)
                            Text("\(x.available)\(x.episodeCount.map { "/\($0)" } ?? "")").foregroundStyle(.secondary)
                        }
                        .padding(.horizontal, 14).padding(.vertical, 7)
                        .background(x.seasonNumber == seasonNumber ? accent.opacity(0.85) : Color.secondary.opacity(0.15), in: Capsule())
                        .foregroundStyle(x.seasonNumber == seasonNumber ? .black : .primary)
                    }
                    .buttonStyle(.plain)
                }
            }
        }
    }

    private func load() async {
        do {
            let s = try await model.call { try await $0.show(id) }
            show = s
            error = nil
            if seasonNumber == nil || !s.seasons.contains(where: { $0.seasonNumber == seasonNumber }) {
                let firstUnwatched = s.seasons.first { $0.seasonNumber > 0 && $0.watched < $0.available }
                seasonNumber = (firstUnwatched ?? s.seasons.first)?.seasonNumber
            }
            await loadSeason()
        } catch {
            if show == nil { self.error = error.localizedDescription }
        }
    }

    private func loadSeason() async {
        guard let n = seasonNumber else { return }
        if let eps = try? await model.call({ try await $0.season(show: id, number: n) }), n == seasonNumber {
            episodes = eps
        }
    }
}

struct EpisodeRow: View {
    @Environment(AppModel.self) private var model
    let show: ShowDetail
    let episode: Episode

    var body: some View {
        let f = episode.file
        HStack(alignment: .top, spacing: 16) {
            Button {
                if let f { model.play(PlaybackRequest(fileId: f.id, title: show.title, subtitle: episode.label, showId: show.id)) }
            } label: {
                RemoteImage(url: model.client?.imageURL(episode.stillPath, size: "w300"))
                    .frame(width: 240, height: 135)
                    .clipShape(RoundedRectangle(cornerRadius: 8))
                    .overlay(alignment: .bottom) { ProgressBar(progress: f?.progress) }
                    .overlay(alignment: .topTrailing) { if f?.isWatched == true { WatchedMark() } }
                    .overlay {
                        if f != nil {
                            Image(systemName: "play.circle.fill").font(.system(size: 40)).foregroundStyle(.white.opacity(0.9)).shadow(radius: 4)
                        }
                    }
            }
            .buttonStyle(.plain)
            .disabled(f == nil)

            VStack(alignment: .leading, spacing: 4) {
                Text(episode.displayTitle).font(.headline)
                Text([
                    episode.airDate,
                    episode.runtime.map { "\($0)m" },
                    f.map { $0.probe?.techBadges.prefix(2).joined(separator: " ") ?? "" } ?? "Not in library",
                    f.flatMap { file in file.progress.map { _ in "\(Fmt.clock(Double((file.durationMs ?? 0) - (file.positionMs ?? 0)) / 1000)) left" } },
                ].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " · "))
                .font(.caption)
                .foregroundStyle(.secondary)
                if let o = episode.overview, !o.isEmpty {
                    Text(o).font(.callout).foregroundStyle(.secondary).lineLimit(3)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            if let f {
                Button {
                    Task { await model.setWatched(episode.files, !f.isWatched) }
                } label: {
                    Image(systemName: f.isWatched ? "checkmark.circle.fill" : "checkmark.circle")
                        .font(.title2)
                        .foregroundStyle(f.isWatched ? accent : .secondary)
                }
                .buttonStyle(.plain)
                .help(f.isWatched ? "Mark unwatched" : "Mark watched")
            }
        }
        .opacity(f == nil ? 0.5 : 1)
        .contextMenu {
            if let f {
                Button("Play from Start") { model.play(PlaybackRequest(fileId: f.id, title: show.title, subtitle: episode.label, showId: show.id, startAt: 0)) }
                Button(f.isWatched ? "Mark Unwatched" : "Mark Watched") { Task { await model.setWatched(episode.files, !f.isWatched) } }
            }
        }
    }
}
