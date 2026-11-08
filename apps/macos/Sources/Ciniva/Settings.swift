import CinivaKit
import SwiftUI

enum EnginePreference: String, CaseIterable, Identifiable {
    case automatic
    case mpv
    case avfoundation
    case mpvWindow

    var id: String { rawValue }

    var label: String {
        switch self {
        case .automatic: return "Automatic (native for Dolby Vision / Atmos when possible)"
        case .mpv: return "Always mpv (every codec & container)"
        case .avfoundation: return "Prefer AVFoundation (native DV / Atmos)"
        case .mpvWindow: return "Always libplacebo HDR window (reference quality, separate window)"
        }
    }
}

enum Prefs {
    static let engine = "pref.engine"
    static let hdr = "pref.hdr"
    static let peakNits = "pref.peakNits"
    static let passthrough = "pref.passthrough"
    static let audioLang = "pref.audioLang"
    static let subLang = "pref.subLang"
    static let volume = "pref.volume"
    static let autoplayNext = "pref.autoplayNext"
    static let seekShort = "pref.seekShort"
    static let seekMedium = "pref.seekMedium"
    static let seekLong = "pref.seekLong"

    static func register() {
        UserDefaults.standard.register(defaults: [
            engine: EnginePreference.automatic.rawValue,
            hdr: true,
            peakNits: 0,
            passthrough: false,
            audioLang: "",
            subLang: "",
            volume: 100.0,
            autoplayNext: true,
            seekShort: 10,
            seekMedium: 30,
            seekLong: 60,
        ])
    }

    static var enginePreference: EnginePreference {
        EnginePreference(rawValue: UserDefaults.standard.string(forKey: engine) ?? "") ?? .automatic
    }
}

struct SettingsView: View {
    var body: some View {
        TabView {
            PlaybackSettings().tabItem { Label("Playback", systemImage: "play.rectangle") }
            AccountSettings().tabItem { Label("Account", systemImage: "person.crop.circle") }
            PluginSettings().tabItem { Label("Plugins", systemImage: "puzzlepiece.extension") }
        }
        .frame(width: 580, height: 680)
    }
}

private struct PlaybackSettings: View {
    @AppStorage(Prefs.engine) private var engine = EnginePreference.automatic.rawValue
    @AppStorage(Prefs.hdr) private var hdr = true
    @AppStorage(Prefs.peakNits) private var peakNits = 0
    @AppStorage(Prefs.passthrough) private var passthrough = false
    @AppStorage(Prefs.audioLang) private var audioLang = ""
    @AppStorage(Prefs.subLang) private var subLang = ""
    @AppStorage(Prefs.autoplayNext) private var autoplayNext = true
    @AppStorage(Prefs.seekShort) private var seekShort = 10
    @AppStorage(Prefs.seekMedium) private var seekMedium = 30
    @AppStorage(Prefs.seekLong) private var seekLong = 60

    private let seekChoices = [5, 10, 15, 20, 30, 45, 60, 90, 120, 180, 300, 600]

    private func seekPicker(_ title: String, _ value: Binding<Int>) -> some View {
        Picker(title, selection: value) {
            ForEach(seekChoices, id: \.self) { s in
                Text(s < 60 ? "\(s) seconds" : (s % 60 == 0 ? "\(s / 60) min" : "\(s / 60) min \(s % 60) s")).tag(s)
            }
        }
    }

    var body: some View {
        Form {
            Section("Playback engine") {
                Picker("Engine", selection: $engine) {
                    ForEach(EnginePreference.allCases) { Text($0.label).tag($0.rawValue) }
                }
                .pickerStyle(.radioGroup)
                Text("Ciniva plays the original file directly. AVFoundation gives true Dolby Vision and Dolby Atmos but only plays MP4/MOV. mpv plays everything (MKV, AV1, VP9, TrueHD, DTS-HD…) and renders HDR10, HLG and Dolby Vision 7/8 in EDR. Dolby Vision profile 5 automatically uses the libplacebo window, which reshapes it to correct colors.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Toggle("Count down and play the next episode automatically", isOn: $autoplayNext)
            }
            Section("Seeking (← / →)") {
                seekPicker("Arrow", $seekShort)
                seekPicker("⌥⇧ Arrow", $seekMedium)
                seekPicker("⌘⇧ Arrow", $seekLong)
                Button("Reset to 10 s / 30 s / 60 s") {
                    seekShort = 10
                    seekMedium = 30
                    seekLong = 60
                }
            }
            Section("HDR (mpv)") {
                Toggle("Output HDR to EDR-capable displays", isOn: $hdr)
                Picker("Display peak", selection: $peakNits) {
                    Text("Automatic").tag(0)
                    Text("400 nits").tag(400)
                    Text("600 nits").tag(600)
                    Text("1000 nits").tag(1000)
                    Text("1600 nits (XDR)").tag(1600)
                    Text("No tone mapping").tag(10000)
                }
            }
            Section("Audio (mpv)") {
                Toggle("Bitstream Dolby / DTS to receiver (HDMI passthrough)", isOn: $passthrough)
                Text("Leave off for Mac speakers, AirPods and headphones. Turn on only when connected to an AV receiver over HDMI.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Section("Languages") {
                TextField("Preferred audio (e.g. eng,jpn)", text: $audioLang)
                TextField("Preferred subtitles (e.g. eng)", text: $subLang)
            }
        }
        .formStyle(.grouped)
    }
}

private struct AccountSettings: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Form {
            Section("Server") {
                LabeledContent("Address", value: model.client?.baseURL.absoluteString ?? model.serverText)
                LabeledContent("Signed in as", value: model.user.map { "\($0.username)\($0.admin ? " (administrator)" : "")" } ?? "Not signed in")
            }
            if model.phase == .ready {
                Section {
                    Button("Sign Out", role: .destructive) { model.signOut() }
                }
            }
        }
        .formStyle(.grouped)
    }
}

private struct PluginSettings: View {
    @Environment(AppModel.self) private var model
    @State private var forms: [(PluginInfo, [PluginField])] = []
    @State private var loaded = false

    var body: some View {
        Form {
            if model.plugins.isEmpty {
                Text("No plugins are registered with this server.").foregroundStyle(.secondary)
            }
            ForEach(model.plugins) { p in
                Section {
                    if let d = p.description { Text(d).font(.caption).foregroundStyle(.secondary) }
                    if let fields = forms.first(where: { $0.0.id == p.id })?.1 {
                        PluginFormView(form: PluginForm(fields: fields, submit: PluginSubmit(label: "Save settings"))) { values in
                            do {
                                try await model.call { try await $0.savePluginSettings(plugin: p.id, values: values) }
                                model.toast("\(p.name) settings saved", .success)
                            } catch {
                                model.toast(error.localizedDescription, .error)
                            }
                        }
                        .id("\(p.id)-\(fields.hashValue)")
                    } else if p.hasSettings == true, model.user?.admin != true {
                        Text("Only administrators can change plugin settings.").font(.caption).foregroundStyle(.secondary)
                    }
                } header: {
                    HStack {
                        Text(p.name)
                        Text(p.online == false ? "offline" : "running")
                            .font(.caption2.weight(.semibold))
                            .padding(.horizontal, 6).padding(.vertical, 2)
                            .background((p.online == false ? Color.red : Color.green).opacity(0.25), in: Capsule())
                        if let v = p.version { Text("v\(v)").font(.caption).foregroundStyle(.secondary) }
                    }
                }
            }
        }
        .formStyle(.grouped)
        .task(id: model.plugins.map(\.id)) { await load() }
    }

    private func load() async {
        guard model.user?.admin == true else { return }
        var out: [(PluginInfo, [PluginField])] = []
        for p in model.plugins where p.hasSettings == true {
            if let fields = try? await model.call({ try await $0.pluginSettings(plugin: p.id) }) { out.append((p, fields)) }
        }
        forms = out
    }
}
