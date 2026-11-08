import CinivaKit
import SwiftUI

struct HomeView: View {
    @Environment(AppModel.self) private var model
    @State private var home: HomeData?
    @State private var error: String?

    var body: some View {
        ScrollView {
            if let home {
                LazyVStack(alignment: .leading, spacing: 28) {
                    if !home.continueWatching.isEmpty {
                        Shelf(title: "Continue Watching") {
                            ForEach(home.continueWatching) { ContinueCard(item: $0) }
                        }
                    }
                    if !home.recentMovies.isEmpty {
                        Shelf(title: "Recently Added Movies") {
                            ForEach(home.recentMovies) { MovieCardView(movie: $0) }
                        }
                    }
                    if !home.recentShows.isEmpty {
                        Shelf(title: "Recently Added Shows") {
                            ForEach(home.recentShows) { ShowCardView(show: $0) }
                        }
                    }
                }
                .padding(.vertical, 24)
            }
        }
        .overlay {
            if let error {
                ContentUnavailableView("Couldn't load home", systemImage: "wifi.exclamationmark", description: Text(error))
            } else if home == nil {
                ProgressView()
            } else if let home, home.recentMovies.isEmpty, home.recentShows.isEmpty {
                ContentUnavailableView("Your library is empty", systemImage: "film.stack", description: Text("Add files to the media folder and run a scan from the web UI."))
            }
        }
        .navigationTitle("Home")
        .task(id: model.libraryVersion) { await load() }
        .refreshable { await load() }
    }

    private func load() async {
        do {
            home = try await model.call { try await $0.home() }
            error = nil
        } catch {
            if home == nil { self.error = error.localizedDescription }
        }
    }
}

enum LibraryKind: String {
    case movies, shows
}

struct LibraryGridView: View {
    @Environment(AppModel.self) private var model
    let kind: LibraryKind

    @AppStorage("sort.movies") private var movieSort = "title"
    @AppStorage("sort.shows") private var showSort = "title"
    @State private var movies: [MovieCard] = []
    @State private var shows: [ShowCard] = []
    @State private var resume: [ContinueItem] = []
    @State private var loaded = false
    @State private var error: String?

    private let sorts = [("title", "Title"), ("added", "Recently Added"), ("year", "Year"), ("rating", "Rating")]

    private var sort: Binding<String> { kind == .movies ? $movieSort : $showSort }
    private var count: Int { kind == .movies ? movies.count : shows.count }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                if !resume.isEmpty {
                    Shelf(title: "Continue Watching") {
                        ForEach(resume) { ContinueCard(item: $0) }
                    }
                }
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 150, maximum: 170), spacing: 20, alignment: .top)], spacing: 24) {
                    if kind == .movies {
                        ForEach(movies) { MovieCardView(movie: $0) }
                    } else {
                        ForEach(shows) { ShowCardView(show: $0) }
                    }
                }
                .padding(.horizontal, 24)
            }
            .padding(.vertical, 24)
        }
        .overlay {
            if let error {
                ContentUnavailableView("Couldn't load \(kind.rawValue)", systemImage: "wifi.exclamationmark", description: Text(error))
            } else if !loaded {
                ProgressView()
            } else if count == 0 {
                ContentUnavailableView("Nothing here yet", systemImage: kind == .movies ? "film" : "tv")
            }
        }
        .navigationTitle(kind == .movies ? "Movies" : "Shows")
        .navigationSubtitle(loaded ? "\(count) \(kind == .movies ? (count == 1 ? "movie" : "movies") : (count == 1 ? "show" : "shows"))" : "")
        .toolbar {
            Picker("Sort", selection: sort) {
                ForEach(sorts, id: \.0) { Text($0.1).tag($0.0) }
            }
            .pickerStyle(.menu)
        }
        .task(id: "\(sort.wrappedValue)-\(model.libraryVersion)") { await load() }
    }

    private func load() async {
        do {
            let s = sort.wrappedValue
            if kind == .movies {
                movies = try await model.call { try await $0.movies(sort: s) }
            } else {
                shows = try await model.call { try await $0.shows(sort: s) }
            }
            let home = try? await model.call { try await $0.home() }
            resume = (home?.continueWatching ?? []).filter { kind == .movies ? $0.movieId != nil : $0.showId != nil }
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
        loaded = true
    }
}

struct SearchResultsView: View {
    @Environment(AppModel.self) private var model
    let query: String
    @State private var results: SearchResults?

    private let columns = [GridItem(.adaptive(minimum: 150, maximum: 170), spacing: 20, alignment: .top)]

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                if let r = results {
                    if !r.movies.isEmpty {
                        Text("Movies").font(.title3.bold())
                        LazyVGrid(columns: columns, spacing: 24) { ForEach(r.movies) { MovieCardView(movie: $0) } }
                    }
                    if !r.shows.isEmpty {
                        Text("Shows").font(.title3.bold())
                        LazyVGrid(columns: columns, spacing: 24) { ForEach(r.shows) { ShowCardView(show: $0) } }
                    }
                }
            }
            .padding(24)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .overlay {
            if let r = results, r.movies.isEmpty, r.shows.isEmpty { ContentUnavailableView.search(text: query) }
        }
        .navigationTitle("Results for “\(query)”")
        .task(id: query) {
            try? await Task.sleep(for: .milliseconds(250))
            guard !Task.isCancelled else { return }
            let q = query.trimmingCharacters(in: .whitespaces)
            results = try? await model.call { try await $0.search(q) }
        }
    }
}
