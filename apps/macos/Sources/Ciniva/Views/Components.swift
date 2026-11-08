import CinivaKit
import SwiftUI

let accent = Color(red: 0.96, green: 0.62, blue: 0.18)

@MainActor
final class ImageCache {
    static let shared = ImageCache()
    private let cache = NSCache<NSURL, NSImage>()

    init() {
        cache.countLimit = 600
    }

    func image(for url: URL) -> NSImage? { cache.object(forKey: url as NSURL) }

    func load(_ url: URL) async -> NSImage? {
        if let img = image(for: url) { return img }
        guard let (data, resp) = try? await CinivaClient.session.data(from: url),
              (resp as? HTTPURLResponse)?.statusCode ?? 200 < 400,
              let img = NSImage(data: data) else { return nil }
        cache.setObject(img, forKey: url as NSURL)
        return img
    }
}

struct RemoteImage: View {
    let url: URL?
    var contentMode: ContentMode = .fill
    @State private var image: NSImage?

    var body: some View {
        Rectangle()
            .fill(.quaternary)
            .overlay {
                if let image {
                    Image(nsImage: image)
                        .resizable()
                        .aspectRatio(contentMode: contentMode)
                        .transition(.opacity)
                }
            }
            .clipped()
            .task(id: url) {
                guard let url else { image = nil; return }
                if let cached = ImageCache.shared.image(for: url) {
                    image = cached
                    return
                }
                let loaded = await ImageCache.shared.load(url)
                withAnimation(.easeOut(duration: 0.15)) { image = loaded }
            }
    }
}

struct ProgressBar: View {
    let progress: Double?
    var height: CGFloat = 4

    var body: some View {
        if let progress, progress > 0 {
            GeometryReader { g in
                ZStack(alignment: .leading) {
                    Rectangle().fill(.black.opacity(0.6))
                    Rectangle().fill(accent).frame(width: g.size.width * min(1, max(0, progress)))
                }
            }
            .frame(height: height)
        }
    }
}

struct WatchedMark: View {
    var body: some View {
        Image(systemName: "checkmark.circle.fill")
            .symbolRenderingMode(.palette)
            .foregroundStyle(.white, accent)
            .font(.title3)
            .padding(6)
    }
}

struct CountBadge: View {
    let count: Int

    var body: some View {
        Text("\(count)")
            .font(.caption2.bold())
            .padding(.horizontal, 6).padding(.vertical, 2)
            .background(accent, in: Capsule())
            .foregroundStyle(.black)
            .padding(6)
    }
}

struct Badge: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.caption.weight(.semibold))
            .padding(.horizontal, 7).padding(.vertical, 3)
            .overlay(RoundedRectangle(cornerRadius: 4).stroke(.secondary.opacity(0.6)))
    }
}

struct PosterCard: View {
    let title: String
    var subtitle: String?
    let image: URL?
    var progress: Double?
    var watched = false
    var unwatched: Int?
    var width: CGFloat = 150

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            RemoteImage(url: image)
                .frame(width: width, height: width * 1.5)
                .clipShape(RoundedRectangle(cornerRadius: 8))
                .overlay(alignment: .topTrailing) {
                    if watched { WatchedMark() } else if let unwatched, unwatched > 0 { CountBadge(count: unwatched) }
                }
                .overlay(alignment: .bottom) { ProgressBar(progress: progress).clipShape(RoundedRectangle(cornerRadius: 2)) }
                .shadow(color: .black.opacity(0.25), radius: 4, y: 2)
            Text(title).font(.callout.weight(.medium)).lineLimit(1)
            if let subtitle { Text(subtitle).font(.caption).foregroundStyle(.secondary).lineLimit(1) }
        }
        .frame(width: width)
        .contentShape(Rectangle())
    }
}

struct MovieCardView: View {
    @Environment(AppModel.self) private var model
    let movie: MovieCard
    var width: CGFloat = 150

    var body: some View {
        NavigationLink(value: Route.movie(movie.id)) {
            PosterCard(title: movie.title, subtitle: movie.year.map(String.init), image: model.client?.imageURL(movie.posterPath, size: "w342"),
                       progress: movie.progress, watched: movie.watched == true, width: width)
        }
        .buttonStyle(.plain)
    }
}

struct ShowCardView: View {
    @Environment(AppModel.self) private var model
    let show: ShowCard
    var width: CGFloat = 150

    var body: some View {
        NavigationLink(value: Route.show(show.id)) {
            PosterCard(title: show.title,
                       subtitle: [show.year.map(String.init), "\(show.episodes) episode\(show.episodes == 1 ? "" : "s")"].compactMap { $0 }.joined(separator: " · "),
                       image: model.client?.imageURL(show.posterPath, size: "w342"),
                       watched: show.episodes > 0 && show.unwatched == 0, unwatched: show.unwatched, width: width)
        }
        .buttonStyle(.plain)
    }
}

struct ContinueCard: View {
    @Environment(AppModel.self) private var model
    let item: ContinueItem
    var width: CGFloat = 280

    var body: some View {
        Button {
            model.play(PlaybackRequest(fileId: item.fileId, title: item.title, subtitle: item.subtitle, showId: item.showId))
        } label: {
            VStack(alignment: .leading, spacing: 6) {
                RemoteImage(url: model.client?.imageURL(item.imagePath ?? item.posterPath, size: "w780"))
                    .frame(width: width, height: width * 9 / 16)
                    .clipShape(RoundedRectangle(cornerRadius: 8))
                    .overlay {
                        Image(systemName: "play.circle.fill").font(.system(size: 36)).foregroundStyle(.white.opacity(0.9)).shadow(radius: 4)
                    }
                    .overlay(alignment: .bottom) { ProgressBar(progress: item.progress) }
                Text(item.title).font(.callout.weight(.medium)).lineLimit(1)
                Text([item.nextUp == true ? "Up next" : nil, item.subtitle].compactMap { $0 }.joined(separator: " · ").nilIfEmpty
                     ?? "\(Fmt.clock(Double(item.durationMs - item.positionMs) / 1000)) left")
                    .font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            .frame(width: width)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .contextMenu {
            if let id = item.movieId { Button("Go to Movie") { model.path.append(.movie(id)) } }
            if let id = item.showId { Button("Go to Show") { model.path.append(.show(id)) } }
        }
    }
}

struct Shelf<Content: View>: View {
    let title: String
    @ViewBuilder let content: () -> Content

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(title).font(.title3.bold()).padding(.horizontal, 24)
            ScrollView(.horizontal, showsIndicators: false) {
                LazyHStack(alignment: .top, spacing: 16) { content() }
                    .padding(.horizontal, 24)
            }
        }
    }
}

struct CastRow: View {
    @Environment(AppModel.self) private var model
    let cast: [CastMember]

    var body: some View {
        if !cast.isEmpty {
            VStack(alignment: .leading, spacing: 10) {
                Text("Cast").font(.title3.bold())
                ScrollView(.horizontal, showsIndicators: false) {
                    LazyHStack(alignment: .top, spacing: 14) {
                        ForEach(Array(cast.prefix(24).enumerated()), id: \.offset) { _, c in
                            VStack(spacing: 4) {
                                RemoteImage(url: model.client?.imageURL(c.profilePath, size: "w185"))
                                    .frame(width: 80, height: 80)
                                    .clipShape(Circle())
                                Text(c.name ?? "").font(.caption.weight(.medium)).lineLimit(1)
                                if let r = c.character { Text(r).font(.caption2).foregroundStyle(.secondary).lineLimit(1) }
                            }
                            .frame(width: 96)
                        }
                    }
                }
            }
        }
    }
}

struct Hero: View {
    let url: URL?

    var body: some View {
        RemoteImage(url: url)
            .frame(maxWidth: .infinity)
            .frame(height: 380)
            .clipped()
            .overlay(LinearGradient(colors: [.clear, Color(nsColor: .windowBackgroundColor)], startPoint: .center, endPoint: .bottom))
    }
}

struct ToastOverlay: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(spacing: 8) {
            ForEach(model.toasts) { t in
                HStack(spacing: 8) {
                    Image(systemName: t.tone == .error ? "exclamationmark.triangle.fill" : t.tone == .success ? "checkmark.circle.fill" : "info.circle.fill")
                        .foregroundStyle(t.tone == .error ? .red : t.tone == .success ? .green : accent)
                    Text(t.text).lineLimit(3)
                }
                .padding(.horizontal, 14).padding(.vertical, 10)
                .background(.regularMaterial, in: Capsule())
                .shadow(radius: 6)
                .transition(.move(edge: .bottom).combined(with: .opacity))
                .onTapGesture { model.toasts.removeAll { $0.id == t.id } }
            }
        }
        .padding(.bottom, 24)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottom)
        .animation(.easeInOut(duration: 0.2), value: model.toasts)
        .allowsHitTesting(!model.toasts.isEmpty)
    }
}

func symbol(for icon: String?) -> String {
    switch icon?.lowercased() ?? "" {
    case "download": return "arrow.down.circle"
    case "upload": return "arrow.up.circle"
    case "search": return "magnifyingglass"
    case "play": return "play.fill"
    case "pause": return "pause.fill"
    case "stop": return "stop.fill"
    case "trash", "delete", "remove": return "trash"
    case "refresh", "reload": return "arrow.clockwise"
    case "check", "done": return "checkmark"
    case "x", "close", "cancel": return "xmark"
    case "plus", "add": return "plus"
    case "gear", "settings": return "gearshape"
    case "list": return "list.bullet"
    case "folder": return "folder"
    case "link": return "link"
    case "star": return "star"
    case "info": return "info.circle"
    case "warning", "alert": return "exclamationmark.triangle"
    case "film", "movie", "movies": return "film"
    case "tv", "show", "shows": return "tv"
    case "wrench", "fix": return "wrench.and.screwdriver"
    case "subtitles", "captions": return "captions.bubble"
    case "music", "audio": return "music.note"
    case "calendar": return "calendar"
    case "rss", "feed": return "dot.radiowaves.up.forward"
    default: return "puzzlepiece.extension"
    }
}

func toneColor(_ tone: Tone?) -> Color? {
    switch tone {
    case .error, .danger: return .red
    case .success: return .green
    case .warning: return .orange
    case .info: return .blue
    case .primary: return accent
    default: return nil
    }
}

extension String {
    var nilIfEmpty: String? { isEmpty ? nil : self }
}
