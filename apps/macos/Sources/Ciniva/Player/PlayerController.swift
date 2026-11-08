import AppKit
import CinivaKit
import Observation

@MainActor
@Observable
final class PlayerController {
    struct UpNext: Equatable {
        var target: EpisodeTarget?
        var countdown: Int?
        var atCredits: Bool
    }

    let request: PlaybackRequest
    let client: CinivaClient?
    let navigator: EpisodeNavigator?
    private(set) var file: MediaFile?
    private(set) var engineKind: EngineKind?
    private(set) var engineView: NSView?

    var isPaused = false
    var isBuffering = true
    var position: Double = 0
    var duration: Double = 0
    var audioTracks: [MediaTrack] = []
    var subtitleTracks: [MediaTrack] = []
    var sourceBadges: [String] = []
    var videoInfo: [String] = []
    var outputMode: String?
    var errorMessage: String?
    var volume: Double = UserDefaults.standard.double(forKey: Prefs.volume)
    var resumeOffer: Double?
    var upNext: UpNext?
    var previous: EpisodeTarget?
    var next: EpisodeTarget?
    var trickplay: TrickplayManifest?
    var availableSecs: Double?

    @ObservationIgnored var onSwitch: ((PlaybackRequest) -> Void)?
    @ObservationIgnored var onClose: (() -> Void)?
    @ObservationIgnored var onUnauthorized: (() -> Void)?

    @ObservationIgnored private var engine: PlaybackEngine?
    @ObservationIgnored private var mediaURL: URL?
    @ObservationIgnored private var progressTask: Task<Void, Never>?
    @ObservationIgnored private var countdownTask: Task<Void, Never>?
    @ObservationIgnored private var neighboursTask: Task<Void, Never>?
    @ObservationIgnored private var downloadTask: Task<Void, Never>?
    @ObservationIgnored private var fellBack = false
    @ObservationIgnored private var stopped = false
    @ObservationIgnored private var loaded = false
    @ObservationIgnored private var startOffset: Double = 0
    @ObservationIgnored private var credits: Double?
    @ObservationIgnored private var creditsShown = false
    @ObservationIgnored private var lastReport = Date.distantPast
    @ObservationIgnored private var ended = false

    init(request: PlaybackRequest, client: CinivaClient?) {
        self.request = request
        self.client = client
        navigator = client.map { EpisodeNavigator(client: $0) }
    }

    var title: String { request.title }
    var subtitle: String? { request.subtitle }
    var isEpisode: Bool { request.showId != nil }

    func start() async {
        if let fileURL = request.fileURL {
            mediaURL = fileURL
            let native = ["mp4", "mov", "m4v"].contains(fileURL.pathExtension.lowercased())
            let pref = Prefs.enginePreference
            startEngine(pref == .mpvWindow ? .mpvWindow : (native && pref != .mpv ? .avfoundation : .mpv), at: 0)
            return
        }
        if let id = request.downloadId {
            await startDownload(id)
            return
        }
        guard let client, let fileId = request.fileId else { return }
        let file: MediaFile
        do {
            file = try await client.file(fileId)
        } catch CinivaError.unauthorized {
            onUnauthorized?()
            return
        } catch {
            errorMessage = error.localizedDescription
            isBuffering = false
            return
        }
        guard !stopped else { return }
        self.file = file
        mediaURL = client.streamURL(fileId: fileId)
        sourceBadges = file.probe?.techBadges ?? []
        duration = file.totalSeconds
        if let c = file.creditsStart, c > 0, duration <= 0 || c < duration - 5 { credits = c }
        loadNeighbours()
        Task { [weak self] in
            let t = await client.trickplay(fileId: fileId)
            self?.trickplay = t
        }

        if let at = request.startAt {
            begin(at: at)
        } else {
            let saved = file.resumeSeconds
            if saved >= 30, duration <= 0 || saved < duration - 15 {
                resumeOffer = saved
                isBuffering = false
            } else {
                begin(at: 0)
            }
        }
    }

    private func startDownload(_ id: String) async {
        guard let client else { return }
        let item: DownloadItem
        do {
            item = try await client.download(id)
        } catch CinivaError.unauthorized {
            onUnauthorized?()
            return
        } catch {
            errorMessage = error.localizedDescription
            isBuffering = false
            return
        }
        guard !stopped else { return }
        mediaURL = client.downloadStreamURL(id)
        sourceBadges = item.probe?.techBadges ?? []
        duration = item.duration ?? 0
        availableSecs = item.availableSecs
        let windowed = Prefs.enginePreference == .mpvWindow || item.probe?.video?.dolbyVisionProfile == 5
        startEngine(windowed ? .mpvWindow : .mpv, at: request.startAt ?? 0)
        // The item disappears once qBittorrent finishes; from then on the whole file is seekable.
        downloadTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(5))
                guard let self, !self.stopped else { return }
                do {
                    self.availableSecs = try await client.download(id).availableSecs
                } catch CinivaError.http(404, _) {
                    self.availableSecs = nil
                    return
                } catch {}
            }
        }
    }

    func resume(from seconds: Double) {
        resumeOffer = nil
        begin(at: seconds)
    }

    private func begin(at seconds: Double) {
        var at = max(0, seconds)
        if duration > 0, at > duration - 15 { at = 0 }
        startOffset = at
        isBuffering = true
        startEngine(file.map(Self.chooseEngine) ?? .mpv, at: at)
        startProgressLoop()
    }

    private func loadNeighbours() {
        guard let navigator, let showId = request.showId, let fileId = request.fileId else { return }
        neighboursTask = Task { [weak self] in
            async let p = try? navigator.neighbour(showId: showId, fileId: fileId, step: -1)
            async let n = try? navigator.neighbour(showId: showId, fileId: fileId, step: 1)
            let (prev, next) = await (p, n)
            self?.previous = prev ?? nil
            self?.next = next ?? nil
        }
    }

    // MARK: Engine choice

    nonisolated static func avFoundationCompatible(_ f: MediaFile) -> Bool {
        guard ["mp4", "mov", "m4v"].contains(f.fileExtension), let p = f.probe, let v = p.video else { return false }
        guard ["h264", "hevc", "av1"].contains(v.codec.lowercased()) else { return false }
        let audio = (p.defaultAudio?.codec ?? "").lowercased()
        return audio.isEmpty || ["aac", "ac3", "eac3", "alac", "mp3", "flac", "opus"].contains(audio) || audio.hasPrefix("pcm")
    }

    nonisolated static func chooseEngine(for f: MediaFile) -> EngineKind {
        let mpvKind: EngineKind = f.probe?.video?.dolbyVisionProfile == 5 ? .mpvWindow : .mpv
        switch Prefs.enginePreference {
        case .mpvWindow: return .mpvWindow
        case .mpv: return mpvKind
        case .avfoundation, .automatic: return avFoundationCompatible(f) ? .avfoundation : mpvKind
        }
    }

    private func makeEngine(_ kind: EngineKind) -> PlaybackEngine {
        switch kind {
        case .mpv:
            return MPVEngine()
        case .mpvWindow:
            let e = MPVEngine(windowed: true)
            e.setTitle([title, subtitle].compactMap { $0 }.joined(separator: " — "))
            return e
        case .avfoundation:
            return AVEngine()
        }
    }

    private func startEngine(_ kind: EngineKind, at start: Double) {
        guard let mediaURL, !stopped else { return }
        engine?.shutdown()
        let e = makeEngine(kind)
        e.controller = self
        engine = e
        engineKind = kind
        engineView = e.view
        loaded = false
        isBuffering = true
        audioTracks = []
        subtitleTracks = []
        videoInfo = []
        outputMode = kind == .avfoundation ? "Native (AVFoundation)" : nil
        e.load(url: mediaURL, start: start, externalSubtitles: [])
    }

    // MARK: User actions

    func togglePause() {
        guard engine != nil else { return }
        engine?.setPaused(!isPaused)
        isPaused.toggle()
        if isPaused { report(force: true) }
    }

    func seek(to seconds: Double) {
        guard engine != nil else { return }
        var upper = max(0, duration - 1)
        if let a = availableSecs { upper = min(upper, max(0, a - 10)) }
        let target = min(max(0, seconds), upper)
        position = target
        engine?.seek(to: target)
        checkCredits(target)
    }

    func seek(by delta: Double) {
        seek(to: position + delta)
    }

    func selectAudio(_ track: MediaTrack) {
        engine?.selectAudio(track.id)
    }

    func selectSubtitle(_ track: MediaTrack?) {
        engine?.selectSubtitle(track?.id)
    }

    func setVolume(_ v: Double) {
        volume = min(max(0, v), 150)
        engine?.setVolume(volume)
        UserDefaults.standard.set(volume, forKey: Prefs.volume)
    }

    var nextEngine: EngineKind? {
        guard let current = engineKind else { return nil }
        let native = file.map(Self.avFoundationCompatible)
            ?? ["mp4", "mov", "m4v"].contains(request.fileURL?.pathExtension.lowercased() ?? "")
        switch current {
        case .mpv: return .mpvWindow
        case .mpvWindow: return native ? .avfoundation : .mpv
        case .avfoundation: return .mpv
        }
    }

    func switchEngine() {
        guard let next = nextEngine else { return }
        startEngine(next, at: position)
    }

    func play(_ target: EpisodeTarget, startAt: Double? = nil) {
        onSwitch?(.episode(target, startAt: startAt))
    }

    func playPrevious() {
        if let previous { play(previous) }
    }

    func playNext() {
        if let next { play(next) }
    }

    func close() {
        onClose?()
    }

    func stop() {
        guard !stopped else { return }
        report(force: true)
        stopped = true
        progressTask?.cancel()
        countdownTask?.cancel()
        neighboursTask?.cancel()
        downloadTask?.cancel()
        engine?.shutdown()
        engine = nil
    }

    // MARK: Up next / credits

    private func checkCredits(_ t: Double) {
        guard let credits, loaded, !ended else { return }
        if t < credits - 1 {
            if creditsShown { hideUpNext() }
            creditsShown = false
        } else if !creditsShown, duration <= 0 || t < duration - 3 {
            creditsShown = true
            Task { await showUpNext(atCredits: true) }
        }
    }

    private func showUpNext(atCredits: Bool) async {
        await neighboursTask?.value
        guard !stopped else { return }
        guard let target = next else {
            if atCredits {
                upNext = UpNext(target: nil, countdown: nil, atCredits: true)
            } else {
                close()
            }
            return
        }
        let autoplay = UserDefaults.standard.bool(forKey: Prefs.autoplayNext)
        upNext = UpNext(target: target, countdown: autoplay ? 10 : nil, atCredits: atCredits)
        countdownTask?.cancel()
        guard autoplay else { return }
        countdownTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(1))
                guard !Task.isCancelled, let self, var u = self.upNext, let c = u.countdown else { return }
                if c <= 1 {
                    self.playUpNext()
                    return
                }
                u.countdown = c - 1
                self.upNext = u
            }
        }
    }

    func playUpNext() {
        countdownTask?.cancel()
        guard let target = upNext?.target ?? next else { return }
        upNext = nil
        play(target, startAt: 0)
    }

    func hideUpNext() {
        countdownTask?.cancel()
        upNext = nil
    }

    func skipCredits() {
        hideUpNext()
        reportFinished()
        ended = true
        close()
    }

    // MARK: Engine callbacks

    func engineDidLoad() {
        loaded = true
        errorMessage = nil
    }

    func engineTime(_ t: Double) {
        guard t.isFinite else { return }
        position = t
        checkCredits(t)
    }

    func engineDuration(_ d: Double) {
        guard d.isFinite, d > 0 else { return }
        if duration <= 0 || abs(duration - d) > 2 { duration = d }
    }

    func enginePaused(_ p: Bool) {
        if isPaused != p {
            isPaused = p
            if p { report(force: true) }
        }
    }

    func engineBuffering(_ b: Bool) {
        isBuffering = b
    }

    func engineTracks(audio: [MediaTrack], subtitles: [MediaTrack]) {
        audioTracks = audio
        subtitleTracks = subtitles
    }

    func engineVideoInfo(_ info: [String]) {
        videoInfo = info
    }

    func engineDidFail(_ message: String) {
        if engineKind == .avfoundation, !fellBack, !loaded {
            fellBack = true
            startEngine(.mpv, at: max(position, startOffset))
            return
        }
        errorMessage = message
        isBuffering = false
    }

    func engineDidQuit() {
        close()
    }

    func engineNeedsDolbyVisionReshaping() {
        guard engineKind == .mpv else { return }
        startEngine(.mpvWindow, at: max(position, startOffset))
    }

    func engineDidEnd() {
        guard !stopped, !ended else { return }
        ended = true
        position = duration
        isBuffering = false
        reportFinished()
        if upNext?.countdown != nil { return }
        if upNext != nil, upNext?.target == nil {
            close()
            return
        }
        Task { await showUpNext(atCredits: false) }
    }

    // MARK: Progress

    private func startProgressLoop() {
        progressTask?.cancel()
        progressTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(10))
                guard let self, !Task.isCancelled else { return }
                if !self.isPaused { self.report(force: true) }
            }
        }
    }

    private func report(force: Bool) {
        guard loaded, !stopped, !ended, let client, let fileId = request.fileId, duration > 0, position >= 1 else { return }
        guard force || Date().timeIntervalSince(lastReport) >= 10 else { return }
        lastReport = Date()
        let pos = Int(position * 1000), dur = Int(duration * 1000)
        Task { try? await client.saveProgress(fileId: fileId, positionMs: pos, durationMs: dur) }
    }

    private func reportFinished() {
        guard let client, let fileId = request.fileId, duration > 0 else { return }
        let dur = Int(duration * 1000)
        Task { try? await client.saveProgress(fileId: fileId, positionMs: dur, durationMs: dur) }
    }
}
