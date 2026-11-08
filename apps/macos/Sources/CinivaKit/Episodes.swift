import Foundation

public struct EpisodeTarget: Hashable, Sendable {
    public let fileId: Int
    public let showId: Int
    public let showTitle: String
    public let label: String
    public let seasonNumber: Int
    public let episodeNumber: Int

    public init(show: ShowDetail, episode: Episode, fileId: Int) {
        self.fileId = fileId
        showId = show.id
        showTitle = show.title
        label = episode.label
        seasonNumber = episode.seasonNumber
        episodeNumber = episode.episodeNumber
    }
}

public enum EpisodeOrder {
    /// Regular seasons in order, specials last.
    public static func seasons(_ seasons: [SeasonCard]) -> [SeasonCard] {
        seasons.sorted { a, b in
            if (a.seasonNumber == 0) != (b.seasonNumber == 0) { return b.seasonNumber == 0 }
            return a.seasonNumber < b.seasonNumber
        }
    }

    /// The playable episode `step` places away from the one holding `fileId`, crossing seasons but not into or
    /// out of specials.
    public static func neighbour(in episodes: [Episode], fileId: Int, step: Int) -> Episode? {
        let playable = episodes.filter { !$0.files.isEmpty }
        guard let at = playable.firstIndex(where: { $0.files.contains { $0.id == fileId } }) else { return nil }
        let i = at + step
        guard playable.indices.contains(i) else { return nil }
        let here = playable[at], target = playable[i]
        guard (here.seasonNumber == 0) == (target.seasonNumber == 0) else { return nil }
        return target
    }
}

public actor EpisodeNavigator {
    private let client: CinivaClient
    private var shows: [Int: ShowDetail] = [:]
    private var seasons: [String: [Episode]] = [:]

    public init(client: CinivaClient) {
        self.client = client
    }

    public func show(_ id: Int) async throws -> ShowDetail {
        if let s = shows[id] { return s }
        let s = try await client.show(id)
        shows[id] = s
        return s
    }

    public func season(show: Int, number: Int) async throws -> [Episode] {
        let key = "\(show)/\(number)"
        if let e = seasons[key] { return e }
        let e = try await client.season(show: show, number: number)
        seasons[key] = e
        return e
    }

    public func neighbour(showId: Int, fileId: Int, step: Int) async throws -> EpisodeTarget? {
        let s = try await show(showId)
        var all: [Episode] = []
        var found = false
        for x in EpisodeOrder.seasons(s.seasons) {
            let eps = try await season(show: showId, number: x.seasonNumber).filter { !$0.files.isEmpty }
            all += eps
            if !found { found = eps.contains { $0.files.contains { $0.id == fileId } } }
            if found, step > 0, let at = all.firstIndex(where: { $0.files.contains { $0.id == fileId } }), all.count > at + step { break }
        }
        guard let e = EpisodeOrder.neighbour(in: all, fileId: fileId, step: step), let f = e.files.first else { return nil }
        return EpisodeTarget(show: s, episode: e, fileId: f.id)
    }

    public func locate(showId: Int, fileId: Int) async throws -> Int? {
        let s = try await show(showId)
        for x in EpisodeOrder.seasons(s.seasons) {
            if try await season(show: showId, number: x.seasonNumber).contains(where: { $0.files.contains { $0.id == fileId } }) {
                return x.seasonNumber
            }
        }
        return nil
    }
}
