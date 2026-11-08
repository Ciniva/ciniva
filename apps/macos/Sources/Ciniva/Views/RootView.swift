import CinivaKit
import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Group {
            switch model.phase {
            case .loading:
                ProgressView("Connecting to Ciniva…").frame(maxWidth: .infinity, maxHeight: .infinity)
            case .signedOut:
                SignInView()
            case .ready:
                BrowserView()
            }
        }
        .overlay {
            if model.playback != nil {
                PlayerScreen().transition(.opacity)
            }
        }
        .overlay { ToastOverlay() }
        .animation(.easeInOut(duration: 0.2), value: model.playback?.id)
        .task {
            let args = CommandLine.arguments
            if let i = args.firstIndex(of: "--play"), i + 1 < args.count {
                model.playFile(URL(fileURLWithPath: args[i + 1]))
            }
            if model.phase == .loading { await model.bootstrap() }
        }
    }
}

struct SignInView: View {
    @Environment(AppModel.self) private var model
    @State private var password = ""
    @FocusState private var focus: Field?

    private enum Field { case server, username, password }

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 18) {
            Image(systemName: "play.rectangle.on.rectangle.fill")
                .font(.system(size: 60))
                .foregroundStyle(accent)
            Text("Ciniva").font(.largeTitle.bold())
            Text("Sign in to your Ciniva server").foregroundStyle(.secondary)

            VStack(spacing: 10) {
                TextField("Server", text: $model.serverText, prompt: Text(AppModel.defaultServer))
                    .focused($focus, equals: .server)
                    .onSubmit { focus = .username }
                TextField("Username", text: $model.usernameText)
                    .textContentType(.username)
                    .focused($focus, equals: .username)
                    .onSubmit { focus = .password }
                SecureField("Password", text: $password)
                    .textContentType(.password)
                    .focused($focus, equals: .password)
                    .onSubmit(submit)
            }
            .textFieldStyle(.roundedBorder)
            .frame(width: 320)
            .disabled(model.isSigningIn)

            if model.isSigningIn {
                ProgressView().controlSize(.small)
            } else {
                Button(action: submit) { Text("Sign In").frame(minWidth: 200) }
                    .buttonStyle(.borderedProminent)
                    .tint(accent)
                    .controlSize(.large)
                    .keyboardShortcut(.defaultAction)
            }
            if model.hasStoredSession && !model.isSigningIn {
                Button("Try Again") { Task { await model.bootstrap() } }
            }
            if let e = model.errorMessage {
                Text(e).foregroundStyle(.red).multilineTextAlignment(.center).frame(maxWidth: 420)
            }
        }
        .padding(40)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear { focus = model.usernameText.isEmpty ? .username : .password }
    }

    private func submit() {
        let pw = password
        Task {
            await model.signIn(password: pw)
            if model.phase == .ready { password = "" }
        }
    }
}

struct BrowserView: View {
    @Environment(AppModel.self) private var model
    @State private var query = ""

    var body: some View {
        @Bindable var model = model
        NavigationSplitView {
            List(selection: $model.selection) {
                Label("Home", systemImage: "house").tag(SidebarItem.home)
                Section("Library") {
                    Label("Movies", systemImage: "film").tag(SidebarItem.movies)
                    Label("Shows", systemImage: "tv").tag(SidebarItem.shows)
                }
                if !model.navPlugins.isEmpty {
                    Section("Plugins") {
                        ForEach(model.navPlugins, id: \.1.id) { p, page in
                            Label(page.title, systemImage: symbol(for: page.icon ?? p.icon))
                                .help(p.name)
                                .tag(SidebarItem.plugin(p.id, page.id))
                        }
                    }
                }
            }
            .navigationSplitViewColumnWidth(min: 180, ideal: 210)
            .safeAreaInset(edge: .bottom) { accountMenu }
        } detail: {
            NavigationStack(path: $model.path) {
                Group {
                    if !query.trimmingCharacters(in: .whitespaces).isEmpty {
                        SearchResultsView(query: query)
                    } else {
                        switch model.selection {
                        case .movies: LibraryGridView(kind: .movies).id("movies")
                        case .shows: LibraryGridView(kind: .shows).id("shows")
                        case .plugin(let id, let page): PluginPageView(pluginId: id, nav: PluginNav(page: page)).id("\(id)/\(page)")
                        default: HomeView()
                        }
                    }
                }
                .navigationDestination(for: Route.self) { route in
                    switch route {
                    case .movie(let id): MovieDetailView(id: id)
                    case .show(let id): ShowDetailView(id: id)
                    case .plugin(let id, let nav): PluginPageView(pluginId: id, nav: nav)
                    }
                }
            }
        }
        .searchable(text: $query, placement: .toolbar, prompt: "Search movies & shows")
        .toolbar(model.playback == nil ? .automatic : .hidden, for: .windowToolbar)
        .onChange(of: model.selection) { model.path = [] }
        .sheet(item: $model.pluginSheet) { sheet in
            PluginSheetView(sheet: sheet)
        }
    }

    private var accountMenu: some View {
        Menu {
            if let host = model.client?.baseURL.host() { Text(host) }
            Divider()
            SettingsLink { Text("Settings…") }
            Button("Reload Plugins") { Task { await model.loadPlugins() } }
            Divider()
            Button("Sign Out") { model.signOut() }
        } label: {
            Label(model.user?.username ?? "Account", systemImage: "person.crop.circle")
        }
        .menuStyle(.borderlessButton)
        .padding(12)
    }
}
