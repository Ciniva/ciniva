import AppKit
import AVFoundation

private final class PlayerLayerView: NSView {
    let playerLayer: AVPlayerLayer

    init(player: AVPlayer) {
        playerLayer = AVPlayerLayer(player: player)
        playerLayer.videoGravity = .resizeAspect
        playerLayer.backgroundColor = NSColor.black.cgColor
        super.init(frame: .zero)
        wantsLayer = true
    }

    required init?(coder: NSCoder) { fatalError() }

    override func makeBackingLayer() -> CALayer { playerLayer }
}

final class AVEngine: PlaybackEngine {
    let kind = EngineKind.avfoundation
    weak var controller: PlayerController?
    var view: NSView { layerView }

    private let player = AVPlayer()
    private let layerView: PlayerLayerView
    private var item: AVPlayerItem?
    private var observations: [NSKeyValueObservation] = []
    private var timeObserver: Any?
    private var endObserver: NSObjectProtocol?
    private var audioGroup: AVMediaSelectionGroup?
    private var subtitleGroup: AVMediaSelectionGroup?
    private var started = false
    private var startAt: Double = 0

    init() {
        layerView = PlayerLayerView(player: player)
        player.appliesMediaSelectionCriteriaAutomatically = true
        player.volume = Float(UserDefaults.standard.double(forKey: Prefs.volume) / 100)
    }

    func load(url: URL, start: Double, externalSubtitles: [ExternalSubtitle]) {
        startAt = start
        let asset = AVURLAsset(url: url, options: [AVURLAssetPreferPreciseDurationAndTimingKey: false])
        let item = AVPlayerItem(asset: asset)
        item.preferredForwardBufferDuration = 30
        self.item = item

        observations = [
            item.observe(\.status, options: [.new]) { [weak self] item, _ in
                onMain { self?.statusChanged(item) }
            },
            player.observe(\.timeControlStatus, options: [.new]) { [weak self] player, _ in
                onMain {
                    guard let c = self?.controller else { return }
                    c.engineBuffering(player.timeControlStatus == .waitingToPlayAtSpecifiedRate)
                    c.enginePaused(player.timeControlStatus == .paused)
                }
            },
        ]
        timeObserver = player.addPeriodicTimeObserver(forInterval: CMTime(seconds: 0.5, preferredTimescale: 600), queue: .main) { [weak self] t in
            MainActor.assumeIsolated { self?.controller?.engineTime(t.seconds) }
        }
        endObserver = NotificationCenter.default.addObserver(forName: AVPlayerItem.didPlayToEndTimeNotification, object: item, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.controller?.engineDidEnd() }
        }
        player.replaceCurrentItem(with: item)
    }

    @MainActor
    private func statusChanged(_ item: AVPlayerItem) {
        switch item.status {
        case .readyToPlay where !started:
            started = true
            if item.duration.isNumeric { controller?.engineDuration(item.duration.seconds) }
            let begin = { [weak self] in
                self?.player.play()
                self?.controller?.engineDidLoad()
            }
            if startAt > 1 {
                player.seek(to: CMTime(seconds: startAt, preferredTimescale: 600), toleranceBefore: .zero, toleranceAfter: .zero) { _ in
                    onMain { begin() }
                }
            } else {
                begin()
            }
            Task { await loadTracks(item) }
        case .failed:
            controller?.engineDidFail(item.error?.localizedDescription ?? "AVFoundation could not open this file.")
        default:
            break
        }
    }

    @MainActor
    private func loadTracks(_ item: AVPlayerItem) async {
        audioGroup = try? await item.asset.loadMediaSelectionGroup(for: .audible)
        subtitleGroup = try? await item.asset.loadMediaSelectionGroup(for: .legible)
        publishTracks()
    }

    @MainActor
    private func publishTracks() {
        guard let item else { return }
        func tracks(_ group: AVMediaSelectionGroup?) -> [MediaTrack] {
            guard let group else { return [] }
            let current = item.currentMediaSelection.selectedMediaOption(in: group)
            return group.options.enumerated().map { i, o in
                MediaTrack(id: String(i), title: o.displayName, detail: o.extendedLanguageTag, selected: o == current)
            }
        }
        controller?.engineTracks(audio: tracks(audioGroup), subtitles: tracks(subtitleGroup))
    }

    func setPaused(_ paused: Bool) {
        paused ? player.pause() : player.play()
    }

    func seek(to seconds: Double) {
        player.seek(to: CMTime(seconds: max(0, seconds), preferredTimescale: 600), toleranceBefore: .zero, toleranceAfter: .zero)
    }

    func selectAudio(_ id: String) {
        guard let group = audioGroup, let i = Int(id), group.options.indices.contains(i) else { return }
        item?.select(group.options[i], in: group)
        onMain { [weak self] in self?.publishTracks() }
    }

    func selectSubtitle(_ id: String?) {
        guard let group = subtitleGroup else { return }
        if let id, let i = Int(id), group.options.indices.contains(i) {
            item?.select(group.options[i], in: group)
        } else {
            item?.select(nil, in: group)
        }
        onMain { [weak self] in self?.publishTracks() }
    }

    func setVolume(_ volume: Double) {
        player.volume = Float(min(volume, 100) / 100)
    }

    func shutdown() {
        player.pause()
        observations.removeAll()
        if let timeObserver { player.removeTimeObserver(timeObserver) }
        timeObserver = nil
        if let endObserver { NotificationCenter.default.removeObserver(endObserver) }
        endObserver = nil
        player.replaceCurrentItem(with: nil)
    }
}
