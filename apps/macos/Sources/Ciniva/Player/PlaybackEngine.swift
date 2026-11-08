import AppKit

enum EngineKind: String {
    case mpv = "mpv"
    case avfoundation = "AVFoundation"
    case mpvWindow = "mpv gpu-next (libplacebo)"
}

struct MediaTrack: Identifiable, Hashable {
    let id: String
    let title: String
    var detail: String?
    var selected: Bool
}

struct ExternalSubtitle {
    let url: URL
    let title: String?
    let language: String?
}

protocol PlaybackEngine: AnyObject {
    var kind: EngineKind { get }
    var view: NSView { get }
    var controller: PlayerController? { get set }
    func load(url: URL, start: Double, externalSubtitles: [ExternalSubtitle])
    func setPaused(_ paused: Bool)
    func seek(to seconds: Double)
    func selectAudio(_ id: String)
    func selectSubtitle(_ id: String?)
    func setVolume(_ volume: Double)
    func shutdown()
}

func onMain(_ body: @escaping @MainActor () -> Void) {
    DispatchQueue.main.async { MainActor.assumeIsolated(body) }
}
