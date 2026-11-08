import AppKit
import CMPV
import CinivaKit

final class MPVEngine: PlaybackEngine {
    let kind: EngineKind
    weak var controller: PlayerController?
    var view: NSView { videoView ?? placeholder }

    private let handle: OpaquePointer
    private let videoView: MPVVideoView?
    private lazy var placeholder = ExternalWindowPlaceholder()
    private var reportedReshapeNeed = false
    private let eventQueue = DispatchQueue(label: "ciniva.mpv.events")
    private var destroyed = false
    private var shutdownRequested = false
    private var pendingSubtitles: [ExternalSubtitle] = []
    private var pendingLoad: (() -> Void)?

    private var gamma: String?
    private var primaries: String?
    private var doviProfile: Int64?
    private var hwdec: String?
    private var videoFormat: String?
    private var videoSize: (Int64, Int64)?
    private var hdrActive = false
    private var hdrContent = false
    private var appliedMode: String?

    init(windowed: Bool = false) {
        kind = windowed ? .mpvWindow : .mpv
        if windowed { Self.pinVulkanDriver() }
        handle = mpv_create()
        let d = UserDefaults.standard
        var opts: [(String, String)] = windowed ? [
            ("vo", "gpu-next"),
            ("gpu-api", "vulkan"),
            ("gpu-context", "macvk"),
            ("target-colorspace-hint", "yes"),
            ("autofit", "85%"),
            ("osc", "yes"),
            ("input-default-bindings", "yes"),
            ("input-vo-keyboard", "yes"),
        ] : [
            ("vo", "libmpv"),
            ("osc", "no"),
            ("input-default-bindings", "no"),
            ("input-vo-keyboard", "no"),
            ("target-prim", "bt.709"),
            ("target-trc", "srgb"),
        ]
        opts += [
            ("hwdec", "videotoolbox"),
            ("idle", "yes"),
            ("keep-open", "no"),
            ("terminal", "no"),
            ("msg-level", "all=warn"),
            ("load-scripts", "no"),
            ("load-auto-profiles", "no"),
            ("ytdl", "no"),
            ("user-agent", "Ciniva/1.0"),
            ("cache", "yes"),
            ("demuxer-max-bytes", "1024MiB"),
            ("demuxer-max-back-bytes", "256MiB"),
            ("demuxer-readahead-secs", "60"),
            ("network-timeout", "30"),
            ("audio-channels", "auto"),
            ("tone-mapping", "auto"),
            ("hdr-compute-peak", "auto"),
            ("sub-font-size", "44"),
            ("sub-border-size", "2.5"),
            ("sub-ass-override", "scale"),
            ("volume", String(Int(d.double(forKey: Prefs.volume)))),
            ("volume-max", "150"),
        ]
        if let a = d.string(forKey: Prefs.audioLang), !a.isEmpty { opts.append(("alang", a)) }
        if let s = d.string(forKey: Prefs.subLang), !s.isEmpty { opts.append(("slang", s)) }
        if ProcessInfo.processInfo.environment["CINIVA_DEBUG"] != nil {
            opts.append(("terminal", "yes"))
            opts.append(("msg-level", "all=info,vo=v,cplayer=v"))
        }
        if d.bool(forKey: Prefs.passthrough) {
            opts.append(("audio-spdif", "ac3,eac3,dts,dts-hd,truehd"))
            opts.append(("ao", "coreaudio_exclusive,coreaudio"))
        }
        for (k, v) in opts { mpv_set_option_string(handle, k, v) }
        mpv_initialize(handle)

        videoView = windowed ? nil : MPVVideoView(mpv: handle)

        let observed: [(String, mpv_format)] = [
            ("time-pos", MPV_FORMAT_DOUBLE),
            ("duration", MPV_FORMAT_DOUBLE),
            ("pause", MPV_FORMAT_FLAG),
            ("paused-for-cache", MPV_FORMAT_FLAG),
            ("seeking", MPV_FORMAT_FLAG),
            ("track-list/count", MPV_FORMAT_INT64),
            ("aid", MPV_FORMAT_STRING),
            ("sid", MPV_FORMAT_STRING),
            ("video-params/gamma", MPV_FORMAT_STRING),
            ("video-params/primaries", MPV_FORMAT_STRING),
            ("video-params/w", MPV_FORMAT_INT64),
            ("video-params/h", MPV_FORMAT_INT64),
            ("current-tracks/video/dolby-vision-profile", MPV_FORMAT_INT64),
            ("hwdec-current", MPV_FORMAT_STRING),
            ("video-format", MPV_FORMAT_STRING),
        ]
        for (name, format) in observed { mpv_observe_property(handle, 0, name, format) }

        mpv_set_wakeup_callback(handle, { ctx in
            guard let ctx else { return }
            let engine = Unmanaged<MPVEngine>.fromOpaque(ctx).takeUnretainedValue()
            engine.eventQueue.async { engine.drainEvents() }
        }, Unmanaged.passUnretained(self).toOpaque())

        videoView?.onScreenChange = { [weak self] in MainActor.assumeIsolated { self?.applyColorMode() } }
    }

    private static func pinVulkanDriver() {
        let icd = "/opt/homebrew/etc/vulkan/icd.d/MoltenVK_icd.json"
        guard FileManager.default.fileExists(atPath: icd) else { return }
        setenv("VK_DRIVER_FILES", icd, 1)
        setenv("VK_ICD_FILENAMES", icd, 1)
    }

    @MainActor
    private static func raiseMPVWindow() {
        for delay in [0.0, 0.5, 1.5] {
            DispatchQueue.main.asyncAfter(deadline: .now() + delay) {
                NSApp.windows.first { $0.title.hasSuffix(" - mpv") && $0.isVisible }?.makeKeyAndOrderFront(nil)
            }
        }
    }

    func setTitle(_ title: String) {
        setString("force-media-title", title)
    }

    func load(url: URL, start: Double, externalSubtitles: [ExternalSubtitle]) {
        pendingSubtitles = externalSubtitles
        let go = { [weak self] in
            guard let self, !self.shutdownRequested else { return }
            self.setString("start", start > 1 ? String(format: "%.3f", start) : "none")
            self.command(["loadfile", url.absoluteString])
        }
        if let layer = videoView?.mpvLayer, !layer.isReady {
            layer.onReady = go
        } else {
            go()
        }
    }

    func setPaused(_ paused: Bool) {
        setString("pause", paused ? "yes" : "no")
    }

    func seek(to seconds: Double) {
        command(["seek", String(format: "%.3f", max(0, seconds)), "absolute+keyframes"])
    }

    func selectAudio(_ id: String) {
        setString("aid", id)
    }

    func selectSubtitle(_ id: String?) {
        setString("sid", id ?? "no")
    }

    func setVolume(_ volume: Double) {
        setString("volume", String(Int(volume)))
    }

    func shutdown() {
        guard !shutdownRequested else { return }
        shutdownRequested = true
        videoView?.onScreenChange = nil
        command(["stop"])
        videoView?.mpvLayer.destroyRenderContext()
        eventQueue.async { [self] in
            mpv_set_wakeup_callback(handle, nil, nil)
            destroyed = true
            mpv_terminate_destroy(handle)
        }
    }

    // MARK: - mpv plumbing

    // Async only: a sync call from the main thread deadlocks while macvk creates its window on main.
    private func setString(_ name: String, _ value: String) {
        guard !shutdownRequested else { return }
        value.withCString { v in
            var ptr: UnsafePointer<CChar>? = v
            _ = mpv_set_property_async(handle, 0, name, MPV_FORMAT_STRING, &ptr)
        }
    }

    private func command(_ args: [String]) {
        var cargs: [UnsafePointer<CChar>?] = args.map { UnsafePointer(strdup($0)) }
        cargs.append(nil)
        defer { cargs.forEach { free(UnsafeMutablePointer(mutating: $0)) } }
        mpv_command_async(handle, 0, &cargs)
    }

    private func getString(_ name: String) -> String? {
        guard let p = mpv_get_property_string(handle, name) else { return nil }
        defer { mpv_free(p) }
        return String(cString: p)
    }

    private func getInt(_ name: String) -> Int64? {
        var v: Int64 = 0
        return mpv_get_property(handle, name, MPV_FORMAT_INT64, &v) >= 0 ? v : nil
    }

    private func getFlag(_ name: String) -> Bool {
        var v: Int32 = 0
        return mpv_get_property(handle, name, MPV_FORMAT_FLAG, &v) >= 0 && v != 0
    }

    private func drainEvents() {
        while !destroyed {
            guard let ev = mpv_wait_event(handle, 0)?.pointee, ev.event_id != MPV_EVENT_NONE else { return }
            handle(event: ev)
            if ev.event_id == MPV_EVENT_SHUTDOWN { return }
        }
    }

    private func handle(event ev: mpv_event) {
        switch ev.event_id {
        case MPV_EVENT_PROPERTY_CHANGE:
            guard let p = ev.data?.assumingMemoryBound(to: mpv_event_property.self).pointee else { return }
            propertyChanged(String(cString: p.name), format: p.format, data: p.data)

        case MPV_EVENT_FILE_LOADED:
            for s in pendingSubtitles {
                command(["sub-add", s.url.absoluteString, "auto", s.title ?? "External", s.language ?? ""])
            }
            pendingSubtitles = []
            publishTracks()
            let windowed = videoView == nil
            onMain { [weak self] in
                if windowed { Self.raiseMPVWindow() }
                self?.controller?.engineDidLoad()
            }

        case MPV_EVENT_END_FILE:
            guard let e = ev.data?.assumingMemoryBound(to: mpv_event_end_file.self).pointee else { return }
            if e.reason == MPV_END_FILE_REASON_EOF {
                onMain { [weak self] in self?.controller?.engineDidEnd() }
            } else if e.reason == MPV_END_FILE_REASON_ERROR {
                let msg = String(cString: mpv_error_string(e.error))
                onMain { [weak self] in self?.controller?.engineDidFail("mpv: \(msg)") }
            }

        case MPV_EVENT_SHUTDOWN:
            onMain { [weak self] in
                guard let self, !self.shutdownRequested else { return }
                self.controller?.engineDidQuit()
            }

        default:
            break
        }
    }

    private func propertyChanged(_ name: String, format: mpv_format, data: UnsafeMutableRawPointer?) {
        let double = format == MPV_FORMAT_DOUBLE ? data?.load(as: Double.self) : nil
        let flag = format == MPV_FORMAT_FLAG ? (data?.load(as: Int32.self)).map { $0 != 0 } : nil
        let int = format == MPV_FORMAT_INT64 ? data?.load(as: Int64.self) : nil
        let string = format == MPV_FORMAT_STRING
            ? data?.load(as: UnsafePointer<CChar>?.self).map { String(cString: $0) }
            : nil

        switch name {
        case "time-pos":
            if let double { onMain { [weak self] in self?.controller?.engineTime(double) } }
        case "duration":
            if let double { onMain { [weak self] in self?.controller?.engineDuration(double) } }
        case "pause":
            let paused = flag ?? false
            onMain { [weak self] in self?.controller?.enginePaused(paused) }
        case "paused-for-cache", "seeking":
            let buffering = getFlag("paused-for-cache") || getFlag("seeking")
            onMain { [weak self] in self?.controller?.engineBuffering(buffering) }
        case "track-list/count", "aid", "sid":
            publishTracks()
        case "video-params/gamma":
            gamma = string
            publishVideoInfo()
        case "video-params/primaries":
            primaries = string
            publishVideoInfo()
        case "video-params/w":
            videoSize = int.map { ($0, videoSize?.1 ?? 0) }
            publishVideoInfo()
        case "video-params/h":
            videoSize = int.map { (videoSize?.0 ?? 0, $0) }
            publishVideoInfo()
        case "current-tracks/video/dolby-vision-profile":
            doviProfile = int
            publishVideoInfo()
        case "hwdec-current":
            hwdec = string
            publishVideoInfo()
        case "video-format":
            videoFormat = string
            publishVideoInfo()
        default:
            break
        }
    }

    private func publishTracks() {
        let count = Int(getInt("track-list/count") ?? 0)
        var audio: [MediaTrack] = []
        var subs: [MediaTrack] = []
        for i in 0..<count {
            let p = "track-list/\(i)/"
            guard let type = getString(p + "type"), let id = getInt(p + "id") else { continue }
            let lang = getString(p + "lang")
            let title = getString(p + "title")
            let codec = getString(p + "codec")
            let selected = getFlag(p + "selected")
            let langName = lang.flatMap { Locale.current.localizedString(forLanguageCode: $0) } ?? lang
            switch type {
            case "audio":
                let channels = getInt(p + "demux-channel-count").map(Int.init)
                let profile = getString(p + "codec-profile")
                let tech = codec.map { Fmt.audioName(codec: $0, profile: profile, channels: channels) }
                audio.append(MediaTrack(
                    id: String(id),
                    title: [langName, tech].compactMap { $0 }.joined(separator: " · ").nonEmpty ?? "Track \(id)",
                    detail: title,
                    selected: selected
                ))
            case "sub":
                var bits = [langName ?? "Unknown"]
                if getFlag(p + "forced") { bits.append("Forced") }
                if let codec { bits.append(Self.subtitleCodecName(codec)) }
                if getFlag(p + "external") { bits.append("External") }
                subs.append(MediaTrack(id: String(id), title: bits.joined(separator: " · "), detail: title, selected: selected))
            default:
                break
            }
        }
        onMain { [weak self] in self?.controller?.engineTracks(audio: audio, subtitles: subs) }
    }

    private static func subtitleCodecName(_ codec: String) -> String {
        switch codec {
        case "hdmv_pgs_subtitle": return "PGS"
        case "dvd_subtitle": return "VobSub"
        case "subrip": return "SRT"
        case "ass", "ssa": return "ASS"
        case "mov_text": return "TX3G"
        case "webvtt": return "WebVTT"
        default: return codec.uppercased()
        }
    }

    private func publishVideoInfo() {
        var bits: [String] = []
        if let videoFormat { bits.append(videoFormat.uppercased()) }
        if let (w, h) = videoSize, w > 0, h > 0 { bits.append("\(w)×\(h)") }
        if let doviProfile { bits.append("Dolby Vision P\(doviProfile)") }
        switch gamma {
        case "pq": if doviProfile == nil { bits.append("HDR10 (PQ)") }
        case "hlg": bits.append("HLG")
        default: break
        }
        if let hwdec, hwdec != "no" { bits.append("HW decode") } else if videoFormat != nil { bits.append("SW decode") }
        let info = bits
        debugLog("video: \(bits.joined(separator: " | ")) gamma=\(gamma ?? "-") prim=\(primaries ?? "-")")
        let isHDR = gamma == "pq" || gamma == "hlg" || doviProfile != nil
        if doviProfile == 5, videoView != nil, !reportedReshapeNeed {
            reportedReshapeNeed = true
            onMain { [weak self] in self?.controller?.engineNeedsDolbyVisionReshaping() }
        }
        onMain { [weak self] in
            self?.hdrContent = isHDR
            self?.applyColorMode()
            self?.controller?.engineVideoInfo(info)
        }
    }

    @MainActor
    private func applyColorMode() {
        guard !shutdownRequested, let videoView else {
            if kind == .mpvWindow { controller?.outputMode = "libplacebo · macOS HDR (separate window)" }
            return
        }
        let isHDRContent = hdrContent
        let screen = videoView.window?.screen ?? NSScreen.main
        let potential = screen?.maximumPotentialExtendedDynamicRangeColorComponentValue ?? 1
        let enable = isHDRContent && UserDefaults.standard.bool(forKey: Prefs.hdr) && potential > 1.0

        let pref = UserDefaults.standard.integer(forKey: Prefs.peakNits)
        let peak = pref > 0 ? pref : Int(min(10000, max(400, potential * 100)))
        let mode = enable ? "hdr-\(peak)" : "sdr-\(isHDRContent)"
        guard mode != appliedMode else { return }
        appliedMode = mode

        if enable {
            setString("target-prim", "bt.2020")
            setString("target-trc", "pq")
            setString("target-peak", String(peak))
            setString("tone-mapping", peak >= 10000 ? "clip" : "auto")
            controller?.outputMode = "EDR · PQ · \(peak) nits"
            debugLog("HDR output on: potential EDR \(potential), target-peak \(peak)")
        } else {
            setString("target-prim", "bt.709")
            setString("target-trc", "srgb")
            setString("target-peak", "auto")
            setString("tone-mapping", "auto")
            controller?.outputMode = isHDRContent ? "SDR (tone-mapped)" : "SDR"
            debugLog("SDR output (hdrContent=\(isHDRContent), potential EDR \(potential))")
        }
        if enable != hdrActive {
            hdrActive = enable
            videoView.mpvLayer.setColorMode(hdr: enable)
        }
    }
}

func debugLog(_ s: @autoclosure () -> String) {
    guard ProcessInfo.processInfo.environment["CINIVA_DEBUG"] != nil else { return }
    FileHandle.standardError.write(("[ciniva] " + s() + "\n").data(using: .utf8)!)
}

extension String {
    var nonEmpty: String? { isEmpty ? nil : self }
}

final class ExternalWindowPlaceholder: NSView {
    init() {
        super.init(frame: .zero)
        wantsLayer = true
        layer?.backgroundColor = NSColor.black.cgColor
        let label = NSTextField(wrappingLabelWithString: "Playing in the libplacebo HDR window.\nUsed for Dolby Vision profile 5 so colors are reshaped correctly.\n\nThese controls still work. Close that window or press Q in it to stop.")
        label.alignment = .center
        label.textColor = .secondaryLabelColor
        label.font = .systemFont(ofSize: 15)
        label.translatesAutoresizingMaskIntoConstraints = false
        addSubview(label)
        NSLayoutConstraint.activate([
            label.centerXAnchor.constraint(equalTo: centerXAnchor),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
            label.widthAnchor.constraint(lessThanOrEqualToConstant: 460),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }
}
