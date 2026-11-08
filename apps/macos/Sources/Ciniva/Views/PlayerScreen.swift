import CinivaKit
import SwiftUI

struct PlayerScreen: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ZStack {
            Color.black
            if let request = model.playback {
                PlayerContainer(request: request, client: model.client).id(request.id)
            }
        }
        .ignoresSafeArea()
    }
}

private struct EngineHost: NSViewRepresentable {
    let view: NSView?

    func makeNSView(context: Context) -> NSView {
        let container = NSView()
        container.wantsLayer = true
        container.layer?.backgroundColor = NSColor.black.cgColor
        return container
    }

    func updateNSView(_ container: NSView, context: Context) {
        guard let view, view.superview !== container else { return }
        container.subviews.forEach { $0.removeFromSuperview() }
        view.frame = container.bounds
        view.autoresizingMask = [.width, .height]
        container.addSubview(view)
    }
}

private struct PlayerContainer: View {
    @Environment(AppModel.self) private var model
    @AppStorage(Prefs.seekShort) private var seekShort = 10
    @AppStorage(Prefs.seekMedium) private var seekMedium = 30
    @AppStorage(Prefs.seekLong) private var seekLong = 60
    @State private var controller: PlayerController
    @State private var controlsVisible = true
    @State private var hideTask: Task<Void, Never>?
    @State private var scrubbing: Double?
    @State private var hoverFraction: Double?
    @State private var sliderWidth: CGFloat = 1
    @State private var showInfo = false
    @State private var showEpisodes = false
    @FocusState private var focused: Bool

    init(request: PlaybackRequest, client: CinivaClient?) {
        _controller = State(initialValue: PlayerController(request: request, client: client))
    }

    var body: some View {
        ZStack {
            EngineHost(view: controller.engineView)
                .ignoresSafeArea()
                .allowsHitTesting(false)

            Color.clear
                .contentShape(Rectangle())
                .onTapGesture(count: 2) { toggleFullScreen() }
                .onTapGesture {
                    if showEpisodes { showEpisodes = false } else { controller.togglePause() }
                }

            if controller.isBuffering && controller.errorMessage == nil && controller.resumeOffer == nil {
                ProgressView().controlSize(.large).tint(.white)
            }

            if let error = controller.errorMessage {
                VStack(spacing: 12) {
                    Image(systemName: "exclamationmark.triangle.fill").font(.largeTitle)
                    Text(error).multilineTextAlignment(.center)
                    Button("Close") { close() }
                }
                .padding(24)
                .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 14))
                .padding()
            }

            if controlsVisible || controller.isPaused || showEpisodes {
                controls.transition(.opacity)
            }

            if let upNext = controller.upNext {
                upNextCard(upNext)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomTrailing)
                    .padding(.trailing, showEpisodes ? 420 : 28)
                    .padding(.bottom, controlsVisible || controller.isPaused ? 120 : 40)
                    .transition(.move(edge: .trailing).combined(with: .opacity))
            }

            if showEpisodes, let showId = controller.request.showId, let navigator = controller.navigator {
                EpisodePanel(controller: controller, showId: showId, navigator: navigator) { showEpisodes = false }
                    .frame(width: 400)
                    .frame(maxWidth: .infinity, alignment: .trailing)
                    .transition(.move(edge: .trailing))
            }

            if let saved = controller.resumeOffer {
                resumePrompt(saved)
            }
        }
        .ignoresSafeArea()
        .focusable()
        .focusEffectDisabled()
        .focused($focused)
        .onAppear {
            focused = true
            controller.onClose = { close() }
            controller.onSwitch = { model.play($0) }
            controller.onUnauthorized = { model.sessionExpired() }
            Task { await controller.start() }
            bumpControls()
        }
        .onDisappear {
            controller.stop()
            hideTask?.cancel()
            NSCursor.setHiddenUntilMouseMoves(false)
        }
        .onContinuousHover { phase in
            if case .active = phase { bumpControls() }
        }
        .onKeyPress(.space) { controller.togglePause(); bumpControls(); return .handled }
        .onKeyPress(keys: [.leftArrow, .rightArrow]) { press in
            guard controller.resumeOffer == nil, let step = seekStep(for: press.modifiers) else { return .ignored }
            controller.seek(by: press.key == .leftArrow ? -step : step)
            bumpControls()
            return .handled
        }
        .onKeyPress(.upArrow) { controller.setVolume(controller.volume + 5); bumpControls(); return .handled }
        .onKeyPress(.downArrow) { controller.setVolume(controller.volume - 5); bumpControls(); return .handled }
        .onKeyPress(.escape) {
            if controller.resumeOffer != nil {
                close()
            } else if showEpisodes {
                showEpisodes = false
            } else if isFullScreen {
                toggleFullScreen()
            } else {
                close()
            }
            return .handled
        }
        .onKeyPress(.return) {
            guard let saved = controller.resumeOffer else { return .ignored }
            controller.resume(from: saved)
            return .handled
        }
        .onKeyPress(characters: .init(charactersIn: "fimsNPe")) { press in
            switch press.characters {
            case "f": toggleFullScreen()
            case "i": showInfo.toggle()
            case "m": controller.setVolume(controller.volume > 0 ? 0 : 100)
            case "s": cycleSubtitles()
            case "N": controller.playNext()
            case "P": controller.playPrevious()
            case "e": if controller.isEpisode { showEpisodes.toggle() }
            default: return .ignored
            }
            bumpControls()
            return .handled
        }
        .animation(.easeInOut(duration: 0.2), value: controlsVisible)
        .animation(.easeInOut(duration: 0.25), value: controller.upNext)
        .animation(.easeInOut(duration: 0.25), value: showEpisodes)
    }

    private func seekStep(for modifiers: EventModifiers) -> Double? {
        let mods = modifiers.intersection([.command, .option, .shift, .control])
        switch mods {
        case []: return Double(seekShort)
        case [.option, .shift]: return Double(seekMedium)
        case [.command, .shift]: return Double(seekLong)
        default: return nil
        }
    }

    private var playerWindow: NSWindow? {
        NSApp.keyWindow ?? NSApp.mainWindow
    }

    private var isFullScreen: Bool {
        playerWindow?.styleMask.contains(.fullScreen) == true
    }

    private func close() {
        if isFullScreen { toggleFullScreen() }
        model.closePlayer()
    }

    // MARK: Overlays

    private func resumePrompt(_ saved: Double) -> some View {
        ZStack {
            Color.black.opacity(0.55).ignoresSafeArea()
            VStack(alignment: .leading, spacing: 14) {
                Text("Continue watching?").font(.title2.bold())
                Text([controller.title, controller.subtitle].compactMap { $0 }.joined(separator: " · ")).foregroundStyle(.secondary)
                if controller.duration > 0 {
                    ProgressView(value: min(1, saved / controller.duration)).tint(accent)
                }
                Text("You stopped at \(Fmt.clock(saved))\(controller.duration > 0 ? " of \(Fmt.clock(controller.duration))" : "").")
                    .foregroundStyle(.secondary)
                HStack {
                    Button {
                        controller.resume(from: saved)
                    } label: {
                        Label("Resume from \(Fmt.clock(saved))", systemImage: "play.fill")
                    }
                    .buttonStyle(.borderedProminent)
                    .tint(accent)
                    Button("Start Over") { controller.resume(from: 0) }
                    Spacer()
                    Button("Cancel") { close() }.buttonStyle(.plain).foregroundStyle(.secondary)
                }
                .controlSize(.large)
            }
            .padding(24)
            .frame(width: 440)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 16))
            .environment(\.colorScheme, .dark)
        }
        .foregroundStyle(.white)
    }

    private func upNextCard(_ u: PlayerController.UpNext) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            if let target = u.target {
                Text("UP NEXT").font(.caption.weight(.bold)).foregroundStyle(accent)
                Text(target.label).font(.headline).lineLimit(2)
                HStack {
                    Button {
                        controller.playUpNext()
                    } label: {
                        Label(u.countdown.map { "Play in \($0)" } ?? "Play Now", systemImage: "play.fill").monospacedDigit()
                    }
                    .buttonStyle(.borderedProminent)
                    .tint(accent)
                    Button(u.atCredits ? "Watch Credits" : "Cancel") { controller.hideUpNext() }
                }
            } else {
                HStack {
                    Button("Skip Credits") { controller.skipCredits() }
                        .buttonStyle(.borderedProminent)
                        .tint(accent)
                    Button("Watch Credits") { controller.hideUpNext() }
                }
            }
        }
        .controlSize(.large)
        .padding(16)
        .frame(maxWidth: 340, alignment: .leading)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14))
        .environment(\.colorScheme, .dark)
        .foregroundStyle(.white)
    }

    private var controls: some View {
        VStack(spacing: 0) {
            HStack(alignment: .top, spacing: 14) {
                Button { close() } label: {
                    Image(systemName: "chevron.left")
                        .font(.title2.weight(.semibold))
                        .frame(width: 36, height: 36)
                        .background(.white.opacity(0.15), in: Circle())
                }
                .buttonStyle(.plain)
                .help("Back to library (Esc)")
                VStack(alignment: .leading, spacing: 2) {
                    Text(controller.title).font(.title2.weight(.semibold))
                    if let s = controller.subtitle { Text(s).foregroundStyle(.secondary) }
                }
                Spacer()
                if showInfo { infoPanel }
            }
            .padding(.horizontal, 24)
            .padding(.top, 36)
            .padding(.bottom, 40)
            .background(LinearGradient(colors: [.black.opacity(0.7), .clear], startPoint: .top, endPoint: .bottom))

            Spacer()

            VStack(spacing: 10) {
                scrubber
                HStack(spacing: 18) {
                    if controller.isEpisode {
                        Button { controller.playPrevious() } label: { Image(systemName: "backward.end.fill") }
                            .disabled(controller.previous == nil)
                            .help(controller.previous.map { "Previous: \($0.label) (⇧P)" } ?? "No previous episode")
                    }
                    Button { controller.seek(by: -Double(seekShort)) } label: { Image(systemName: Self.seekSymbol("gobackward", seekShort)) }
                    Button { controller.togglePause() } label: {
                        Image(systemName: controller.isPaused ? "play.fill" : "pause.fill").font(.title)
                    }
                    Button { controller.seek(by: Double(seekShort)) } label: { Image(systemName: Self.seekSymbol("goforward", seekShort)) }
                    if controller.isEpisode {
                        Button { controller.playNext() } label: { Image(systemName: "forward.end.fill") }
                            .disabled(controller.next == nil)
                            .help(controller.next.map { "Next: \($0.label) (⇧N)" } ?? "No next episode")
                    }

                    volumeControl

                    Spacer()

                    HStack(spacing: 6) {
                        ForEach(badges, id: \.self) { b in
                            Text(b)
                                .font(.caption2.weight(.semibold))
                                .padding(.horizontal, 6).padding(.vertical, 3)
                                .background(.white.opacity(0.15), in: RoundedRectangle(cornerRadius: 4))
                        }
                    }

                    if controller.isEpisode {
                        Button { showEpisodes.toggle() } label: { Image(systemName: "list.bullet.rectangle") }
                            .help("Episodes (E)")
                    }
                    audioMenu
                    subtitleMenu
                    Button { showInfo.toggle() } label: { Image(systemName: "info.circle") }
                    Button { toggleFullScreen() } label: { Image(systemName: "arrow.up.left.and.arrow.down.right") }
                }
                .font(.title3)
                .buttonStyle(.plain)
            }
            .padding(.horizontal, 24)
            .padding(.bottom, 20)
            .padding(.top, 50)
            .background(LinearGradient(colors: [.clear, .black.opacity(0.8)], startPoint: .top, endPoint: .bottom))
            .padding(.trailing, showEpisodes ? 400 : 0)
        }
        .foregroundStyle(.white)
    }

    private var badges: [String] {
        controller.sourceBadges.filter { ["4K", "Dolby Vision", "HDR", "HLG", "Atmos"].contains(where: $0.hasPrefix) }
    }

    private var scrubber: some View {
        HStack(spacing: 12) {
            Text(Fmt.clock(scrubbing ?? controller.position)).monospacedDigit()
            Slider(
                value: Binding(
                    get: { scrubbing ?? controller.position },
                    set: { scrubbing = $0 }
                ),
                in: 0...max(controller.duration, 1),
                onEditingChanged: { editing in
                    if !editing, let s = scrubbing {
                        controller.seek(to: s)
                        scrubbing = nil
                    }
                    bumpControls()
                }
            )
            .tint(accent)
            .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { sliderWidth = $0 }
            .background(alignment: .bottomLeading) {
                if let a = controller.availableSecs, controller.duration > 0 {
                    Capsule().fill(.white.opacity(0.35))
                        .frame(width: sliderWidth * min(1, a / controller.duration), height: 3)
                        .offset(y: 4)
                        .allowsHitTesting(false)
                }
            }
            .onContinuousHover { phase in
                if case .active(let p) = phase {
                    hoverFraction = min(1, max(0, p.x / max(1, sliderWidth)))
                } else {
                    hoverFraction = nil
                }
            }
            .overlay(alignment: .topLeading) {
                if let f = scrubbing.map({ $0 / max(controller.duration, 1) }) ?? hoverFraction, controller.duration > 0 {
                    TrickplayPreview(controller: controller, time: f * controller.duration)
                        .offset(x: min(max(0, f * sliderWidth - 90), max(0, sliderWidth - 180)), y: -130)
                        .allowsHitTesting(false)
                }
            }
            Text("-" + Fmt.clock(max(0, controller.duration - (scrubbing ?? controller.position)))).monospacedDigit()
        }
        .font(.callout)
    }

    private var volumeControl: some View {
        HStack(spacing: 6) {
            Image(systemName: controller.volume == 0 ? "speaker.slash.fill" : "speaker.wave.2.fill")
                .font(.callout)
                .frame(width: 22)
            Slider(value: Binding(get: { controller.volume }, set: { controller.setVolume($0) }), in: 0...150)
                .frame(width: 90)
                .controlSize(.small)
        }
    }

    private var audioMenu: some View {
        Menu {
            ForEach(controller.audioTracks) { t in
                Button { controller.selectAudio(t) } label: {
                    if t.selected { Label(trackLabel(t), systemImage: "checkmark") } else { Text(trackLabel(t)) }
                }
            }
        } label: {
            Image(systemName: "waveform")
        }
        .menuIndicator(.hidden)
        .menuStyle(.borderlessButton)
        .fixedSize()
        .disabled(controller.audioTracks.isEmpty)
        .help("Audio track")
    }

    private var subtitleMenu: some View {
        Menu {
            Button { controller.selectSubtitle(nil) } label: {
                if !controller.subtitleTracks.contains(where: \.selected) { Label("Off", systemImage: "checkmark") } else { Text("Off") }
            }
            Divider()
            ForEach(controller.subtitleTracks) { t in
                Button { controller.selectSubtitle(t) } label: {
                    if t.selected { Label(trackLabel(t), systemImage: "checkmark") } else { Text(trackLabel(t)) }
                }
            }
        } label: {
            Image(systemName: "captions.bubble")
        }
        .menuIndicator(.hidden)
        .menuStyle(.borderlessButton)
        .fixedSize()
        .help("Subtitles")
    }

    private var infoPanel: some View {
        VStack(alignment: .leading, spacing: 6) {
            row("Engine", controller.engineKind?.rawValue ?? "—")
            row("Output", controller.outputMode ?? "—")
            if !controller.videoInfo.isEmpty { row("Decoding", controller.videoInfo.joined(separator: " · ")) }
            row("Source", controller.sourceBadges.joined(separator: " · "))
            if let f = controller.file { row("File", f.fileName) }
            row("Stream", "Direct play (original file)")
            if let next = controller.nextEngine {
                Button("Switch to \(next.rawValue)") { controller.switchEngine() }
                    .buttonStyle(.bordered)
                    .padding(.top, 4)
            }
        }
        .font(.caption)
        .padding(12)
        .frame(maxWidth: 420, alignment: .leading)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 10))
    }

    private func row(_ k: String, _ v: String) -> some View {
        HStack(alignment: .top) {
            Text(k).foregroundStyle(.secondary).frame(width: 64, alignment: .leading)
            Text(v)
        }
    }

    private func trackLabel(_ t: MediaTrack) -> String {
        guard let d = t.detail, !d.isEmpty, !t.title.contains(d) else { return t.title }
        return "\(t.title) — \(d)"
    }

    private func cycleSubtitles() {
        let tracks = controller.subtitleTracks
        guard !tracks.isEmpty else { return }
        if let i = tracks.firstIndex(where: \.selected) {
            controller.selectSubtitle(i + 1 < tracks.count ? tracks[i + 1] : nil)
        } else {
            controller.selectSubtitle(tracks[0])
        }
    }

    private func bumpControls() {
        controlsVisible = true
        hideTask?.cancel()
        hideTask = Task {
            try? await Task.sleep(for: .seconds(3))
            guard !Task.isCancelled, scrubbing == nil, !showEpisodes, controller.resumeOffer == nil else { return }
            controlsVisible = false
            NSCursor.setHiddenUntilMouseMoves(true)
        }
    }

    private func toggleFullScreen() {
        playerWindow?.toggleFullScreen(nil)
    }

    static func seekSymbol(_ base: String, _ seconds: Int) -> String {
        [5, 10, 15, 30, 45, 60, 75, 90].contains(seconds) ? "\(base).\(seconds)" : base
    }
}

private struct TrickplayPreview: View {
    let controller: PlayerController
    let time: Double
    @State private var sheet: NSImage?
    @State private var sheetNumber = 0

    var body: some View {
        VStack(spacing: 4) {
            if let tp = controller.trickplay, let image = tile(tp) {
                Image(nsImage: image)
                    .resizable()
                    .aspectRatio(CGFloat(tp.width) / CGFloat(max(1, tp.height)), contentMode: .fit)
                    .frame(width: 180)
                    .clipShape(RoundedRectangle(cornerRadius: 6))
                    .overlay(RoundedRectangle(cornerRadius: 6).stroke(.white.opacity(0.6)))
            }
            Text(Fmt.clock(time))
                .font(.caption.weight(.semibold)).monospacedDigit()
                .padding(.horizontal, 6).padding(.vertical, 2)
                .background(.black.opacity(0.7), in: Capsule())
        }
        .frame(width: 180, height: 124, alignment: .bottom)
        .task(id: controller.trickplay.map { $0.tile(at: time).sheet } ?? 0) {
            guard let tp = controller.trickplay, let fileId = controller.request.fileId, let client = controller.client else { return }
            let n = tp.tile(at: time).sheet
            guard n != sheetNumber || sheet == nil else { return }
            sheet = await ImageCache.shared.load(client.trickplaySheetURL(fileId: fileId, sheet: n))
            sheetNumber = n
        }
    }

    private func tile(_ tp: TrickplayManifest) -> NSImage? {
        let t = tp.tile(at: time)
        guard t.sheet == sheetNumber, let sheet, let cg = sheet.cgImage(forProposedRect: nil, context: nil, hints: nil) else { return nil }
        let scale = CGFloat(cg.width) / max(1, sheet.size.width)
        let rect = CGRect(x: CGFloat(t.x) * scale, y: CGFloat(t.y) * scale, width: CGFloat(tp.width) * scale, height: CGFloat(tp.height) * scale)
        guard let cropped = cg.cropping(to: rect) else { return nil }
        return NSImage(cgImage: cropped, size: NSSize(width: tp.width, height: tp.height))
    }
}

private struct EpisodePanel: View {
    @Environment(AppModel.self) private var model
    let controller: PlayerController
    let showId: Int
    let navigator: EpisodeNavigator
    let onClose: () -> Void

    @State private var show: ShowDetail?
    @State private var season: Int?
    @State private var episodes: [Episode] = []
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(show?.title ?? controller.title).font(.headline).lineLimit(1)
                Spacer()
                if let show {
                    Picker("Season", selection: Binding(get: { season ?? -1 }, set: { season = $0 })) {
                        ForEach(EpisodeOrder.seasons(show.seasons)) { Text($0.label).tag($0.seasonNumber) }
                    }
                    .labelsHidden()
                    .fixedSize()
                }
                Button(action: onClose) { Image(systemName: "xmark") }.buttonStyle(.plain)
            }
            .padding(16)
            Divider()
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(spacing: 4) {
                        ForEach(episodes) { e in row(e).id(e.id) }
                    }
                    .padding(10)
                }
                .onChange(of: episodes) {
                    if let current = episodes.first(where: { isCurrent($0) }) { proxy.scrollTo(current.id, anchor: .center) }
                }
            }
            .overlay {
                if let error { Text(error).foregroundStyle(.secondary).padding() }
                else if show == nil { ProgressView() }
            }
        }
        .frame(maxHeight: .infinity)
        .background(.regularMaterial)
        .environment(\.colorScheme, .dark)
        .foregroundStyle(.white)
        .task { await load() }
        .onChange(of: season) { Task { await loadSeason() } }
    }

    private func isCurrent(_ e: Episode) -> Bool {
        e.files.contains { $0.id == controller.request.fileId }
    }

    private func row(_ e: Episode) -> some View {
        let f = e.file
        let current = isCurrent(e)
        return Button {
            guard let f, let show else { return }
            if current { onClose() } else { controller.play(EpisodeTarget(show: show, episode: e, fileId: f.id)) }
        } label: {
            HStack(spacing: 12) {
                RemoteImage(url: model.client?.imageURL(e.stillPath, size: "w300"))
                    .frame(width: 128, height: 72)
                    .clipShape(RoundedRectangle(cornerRadius: 6))
                    .overlay(alignment: .bottom) { ProgressBar(progress: f?.progress, height: 3) }
                    .overlay(alignment: .topTrailing) {
                        if f?.isWatched == true {
                            Image(systemName: "checkmark.circle.fill").symbolRenderingMode(.palette).foregroundStyle(.white, accent).padding(3)
                        }
                    }
                VStack(alignment: .leading, spacing: 3) {
                    Text(e.displayTitle).font(.callout.weight(.semibold)).lineLimit(2)
                    Text(current ? "Playing now" : f == nil ? "Not in library" : [
                        e.runtime.map { "\($0) min" },
                        f.flatMap { file in file.progress.map { _ in "\(Fmt.clock(Double((file.durationMs ?? 0) - (file.positionMs ?? 0)) / 1000)) left" } },
                    ].compactMap { $0 }.joined(separator: " · "))
                    .font(.caption)
                    .foregroundStyle(current ? accent : .secondary)
                }
                Spacer(minLength: 0)
            }
            .padding(8)
            .background(current ? accent.opacity(0.18) : .clear, in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).stroke(current ? accent : .clear))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .opacity(f == nil ? 0.5 : 1)
        .disabled(f == nil)
    }

    private func load() async {
        do {
            let s = try await navigator.show(showId)
            show = s
            let fileId = controller.request.fileId ?? -1
            season = (try? await navigator.locate(showId: showId, fileId: fileId)) ?? EpisodeOrder.seasons(s.seasons).first?.seasonNumber
            await loadSeason()
        } catch {
            self.error = "Could not load episodes: \(error.localizedDescription)"
        }
    }

    private func loadSeason() async {
        guard let n = season else { return }
        do {
            let eps = try await model.call { try await $0.season(show: showId, number: n) }
            if n == season { episodes = eps }
        } catch {
            self.error = error.localizedDescription
        }
    }
}
