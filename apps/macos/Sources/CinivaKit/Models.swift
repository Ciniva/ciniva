import Foundation

public struct User: Codable, Hashable, Sendable {
    public let id: Int
    public let username: String
    public let admin: Bool
}

public struct LoginResponse: Decodable, Sendable {
    public let token: String
    public let user: User
}

public struct SetupState: Decodable, Sendable {
    public let needsAdmin: Bool
    public let tmdbConfigured: Bool?
    public let version: String?
}

public struct Lenient<T: Decodable & Sendable & Hashable>: Decodable, Sendable, Hashable {
    public let value: T?

    public init(from decoder: Decoder) throws {
        value = try? T(from: decoder)
    }
}

public struct MovieCard: Decodable, Identifiable, Hashable, Sendable {
    public let id: Int
    public let title: String
    public let year: Int?
    public let posterPath: String?
    public let backdropPath: String?
    public let rating: Double?
    public let addedAt: String?
    public let positionMs: Int?
    public let durationMs: Int?
    public let watched: Bool?

    public var progress: Double? {
        guard watched != true, let p = positionMs, let d = durationMs, d > 0, p > 0 else { return nil }
        return Double(p) / Double(d)
    }
}

public struct ShowCard: Decodable, Identifiable, Hashable, Sendable {
    public let id: Int
    public let title: String
    public let year: Int?
    public let posterPath: String?
    public let backdropPath: String?
    public let rating: Double?
    public let episodes: Int
    public let watchedEpisodes: Int
    public let lastAdded: String?

    public var unwatched: Int { max(0, episodes - watchedEpisodes) }
}

public struct ContinueItem: Decodable, Identifiable, Hashable, Sendable {
    public var id: Int { fileId }
    public let fileId: Int
    public let positionMs: Int
    public let durationMs: Int
    public let updatedAt: String?
    public let movieId: Int?
    public let showId: Int?
    public let title: String
    public let subtitle: String?
    public let imagePath: String?
    public let posterPath: String?
    public let nextUp: Bool?

    public var progress: Double { durationMs > 0 ? Double(positionMs) / Double(durationMs) : 0 }
}

public struct HomeData: Decodable, Sendable {
    public let continueWatching: [ContinueItem]
    public let recentMovies: [MovieCard]
    public let recentShows: [ShowCard]
}

public struct SearchResults: Decodable, Sendable {
    public let movies: [MovieCard]
    public let shows: [ShowCard]
}

public struct VideoInfo: Decodable, Hashable, Sendable {
    public let codec: String
    public let profile: String?
    public let width: Int
    public let height: Int
    public let bitDepth: Int?
    public let hdr: String?
    public let dolbyVisionProfile: Int?
    public let fps: Double?
}

public struct AudioInfo: Decodable, Hashable, Sendable {
    public let index: Int
    public let codec: String
    public let profile: String?
    public let channels: Int?
    public let language: String?
    public let title: String?
    public let atmos: Bool
    public let `default`: Bool
}

public struct SubtitleInfo: Decodable, Hashable, Sendable {
    public let index: Int
    public let codec: String
    public let language: String?
    public let title: String?
    public let forced: Bool
    public let `default`: Bool
}

public struct MediaInfo: Decodable, Hashable, Sendable {
    public let container: String?
    public let duration: Double?
    public let bitrate: Int?
    public let video: VideoInfo?
    public let audio: [AudioInfo]
    public let subtitles: [SubtitleInfo]

    public var defaultAudio: AudioInfo? { audio.first { $0.default } ?? audio.first }
}

public struct MediaFile: Decodable, Identifiable, Hashable, Sendable {
    public let id: Int
    public let path: String
    public let size: Int64
    public let duration: Double?
    public let probe: MediaInfo?
    public let addedAt: String?
    public let creditsStart: Double?
    public let creditsSource: String?
    public let positionMs: Int?
    public let durationMs: Int?
    public let watched: Bool?

    public var isWatched: Bool { watched == true }
    public var fileName: String { (path as NSString).lastPathComponent }
    public var fileExtension: String { (path as NSString).pathExtension.lowercased() }
    public var totalSeconds: Double { duration ?? probe?.duration ?? Double(durationMs ?? 0) / 1000 }

    /// Fraction watched, for progress bars; nil when there is nothing to show.
    public var progress: Double? {
        guard !isWatched, let p = positionMs, let d = durationMs, d > 0, p > 0 else { return nil }
        return Double(p) / Double(d)
    }

    public var resumeSeconds: Double {
        guard !isWatched, let p = positionMs else { return 0 }
        return Double(p) / 1000
    }

    public var creditsLabel: String {
        if let c = creditsStart, c > 0 {
            let source: String
            switch creditsSource {
            case "chapters": source = "from chapters"
            case "audio": source = "matched across the season"
            case "picture": source = "detected in the picture"
            case "manual": source = "set by you"
            default: source = "detected"
            }
            return "Credits at \(Fmt.clock(c)) (\(source))"
        }
        return creditsSource == "manual" ? "No credits (set by you)" : "Credits not detected"
    }
}

public struct CastMember: Decodable, Hashable, Sendable {
    public let name: String?
    public let character: String?
    public let profilePath: String?
}

public struct MovieDetail: Decodable, Identifiable, Hashable, Sendable {
    public let id: Int
    public let tmdbId: Int?
    public let title: String
    public let originalTitle: String?
    public let year: Int?
    public let overview: String?
    public let tagline: String?
    public let posterPath: String?
    public let backdropPath: String?
    public let runtime: Int?
    public let rating: Double?
    public let genres: [String]?
    public let releaseDate: String?
    private let cast: Lenient<[CastMember]>?
    public let files: [MediaFile]

    public var castMembers: [CastMember] { cast?.value ?? [] }
}

public struct SeasonCard: Decodable, Identifiable, Hashable, Sendable {
    public var id: Int { seasonNumber }
    public let seasonNumber: Int
    public let name: String?
    public let posterPath: String?
    public let airDate: String?
    public let episodeCount: Int?
    public let available: Int
    public let watched: Int

    public var label: String { name ?? (seasonNumber == 0 ? "Specials" : "Season \(seasonNumber)") }
}

public struct ShowDetail: Decodable, Identifiable, Hashable, Sendable {
    public let id: Int
    public let tmdbId: Int?
    public let title: String
    public let originalTitle: String?
    public let year: Int?
    public let overview: String?
    public let posterPath: String?
    public let backdropPath: String?
    public let rating: Double?
    public let genres: [String]?
    public let firstAirDate: String?
    public let status: String?
    private let cast: Lenient<[CastMember]>?
    public let seasons: [SeasonCard]

    public var castMembers: [CastMember] { cast?.value ?? [] }
}

public struct Episode: Decodable, Identifiable, Hashable, Sendable {
    public let id: Int
    public let seasonNumber: Int
    public let episodeNumber: Int
    public let title: String?
    public let overview: String?
    public let stillPath: String?
    public let airDate: String?
    public let runtime: Int?
    public let rating: Double?
    public let files: [MediaFile]

    public var file: MediaFile? { files.first }
    public var displayTitle: String { "\(episodeNumber). \(title ?? "Episode \(episodeNumber)")" }
    public var label: String { "S\(seasonNumber) · E\(episodeNumber)\(title.map { " — \($0)" } ?? "")" }
}

public struct TrickplayManifest: Decodable, Hashable, Sendable {
    public let interval: Double
    public let width: Int
    public let height: Int
    public let cols: Int
    public let rows: Int
    public let count: Int
    public let sheets: Int?

    /// Sheet number (1-based) and tile origin for time `t`.
    public func tile(at t: Double) -> (sheet: Int, x: Int, y: Int) {
        let perSheet = max(1, cols * rows)
        let i = max(0, min(Int(t / max(interval, 0.001)), count - 1))
        let within = i % perSheet
        return (i / perSheet + 1, (within % cols) * width, (within / cols) * height)
    }
}
