import Foundation

public enum Fmt {
    public static func clock(_ seconds: Double) -> String {
        guard seconds.isFinite else { return "0:00" }
        let t = max(0, Int(seconds))
        return t >= 3600
            ? String(format: "%d:%02d:%02d", t / 3600, t % 3600 / 60, t % 60)
            : String(format: "%d:%02d", t / 60, t % 60)
    }

    public static func runtime(minutes: Int?) -> String? {
        guard let m = minutes, m > 0 else { return nil }
        return m >= 60 ? "\(m / 60)h \(m % 60)m" : "\(m)m"
    }

    public static func bytes(_ n: Int64) -> String {
        ByteCountFormatter.string(fromByteCount: n, countStyle: .file)
    }

    public static func language(_ code: String?) -> String? {
        guard let code, !code.isEmpty, code != "und" else { return nil }
        return Locale.current.localizedString(forLanguageCode: code) ?? code
    }

    public static func audioName(codec: String, profile: String?, channels: Int?) -> String {
        let name: String
        switch codec.lowercased() {
        case "truehd": name = "TrueHD"
        case "eac3": name = "Dolby Digital+"
        case "ac3": name = "Dolby Digital"
        case "dca", "dts":
            if let p = profile?.lowercased(), p.contains("ma") { name = "DTS-HD MA" }
            else if let p = profile?.lowercased(), p.contains("x") { name = "DTS:X" }
            else { name = "DTS" }
        case "aac": name = "AAC"
        case "flac": name = "FLAC"
        case "opus": name = "Opus"
        case "mp3": name = "MP3"
        case let c where c.hasPrefix("pcm"): name = "PCM"
        default: name = codec.uppercased()
        }
        guard let channels, channels > 0 else { return name }
        let layout: String
        switch channels {
        case 1: layout = "1.0"
        case 2: layout = "2.0"
        case 6: layout = "5.1"
        case 8: layout = "7.1"
        default: layout = "\(channels)ch"
        }
        return "\(name) \(layout)"
    }
}

extension AudioInfo {
    public var label: String { Fmt.audioName(codec: codec, profile: profile, channels: channels) }
}

extension MediaInfo {
    /// Resolution, HDR format, video codec and the default audio track, like the web client.
    public var techBadges: [String] {
        var out: [String] = []
        if let v = video {
            let res: String
            if v.height >= 1600 || v.width >= 3200 { res = "4K" }
            else if v.height >= 1000 || v.width >= 1800 { res = "1080p" }
            else if v.height >= 700 { res = "720p" }
            else { res = "\(v.height)p" }
            out.append(res)
            if let p = v.dolbyVisionProfile { out.append("Dolby Vision P\(p)") }
            if let h = v.hdr, h != "dolby_vision" {
                out.append(h == "hdr10" ? "HDR10" : h == "hdr10+" ? "HDR10+" : h.uppercased())
            }
            let names = ["hevc": "HEVC", "h264": "H.264", "av1": "AV1", "vp9": "VP9", "mpeg2video": "MPEG-2", "mpeg4": "MPEG-4"]
            out.append(names[v.codec] ?? v.codec.uppercased())
        }
        if let a = defaultAudio {
            if a.atmos { out.append("Atmos") }
            out.append(a.label)
        }
        return out
    }
}
