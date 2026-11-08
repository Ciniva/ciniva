import AppKit
import CinivaKit
import SwiftUI

/// What a rendered plugin view needs from its host: who to call, and how to replace or re-fetch itself.
struct PluginHost {
    let pluginId: String
    let show: (PluginView) -> Void
    let reload: () async -> Void
    var dismiss: (() -> Void)?
}

struct PluginPageView: View {
    @Environment(AppModel.self) private var model
    let pluginId: String
    let nav: PluginNav

    @State private var view: PluginView?
    @State private var error: String?
    @State private var generation = 0

    var body: some View {
        ScrollView {
            if let view {
                PluginViewRenderer(view: view, host: host)
                    .padding(24)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .overlay {
            if let error, view == nil {
                ContentUnavailableView {
                    Label("Couldn't load this page", systemImage: "puzzlepiece.extension")
                } description: {
                    Text(error)
                } actions: {
                    Button("Try Again") { Task { await load() } }
                }
            } else if view == nil {
                ProgressView()
            }
        }
        .navigationTitle(view?.title ?? model.plugins.first { $0.id == pluginId }?.name ?? "Plugin")
        .task(id: generation) { await refreshLoop() }
    }

    private var host: PluginHost {
        PluginHost(pluginId: pluginId, show: { v in
            view = v
            generation += 1
        }, reload: { await load() })
    }

    private func refreshLoop() async {
        if view == nil || generation == 0 { await load() }
        while !Task.isCancelled, let seconds = view?.refresh, seconds > 0 {
            try? await Task.sleep(for: .seconds(max(1, seconds)))
            guard !Task.isCancelled else { return }
            await load()
        }
    }

    private func load() async {
        do {
            view = try await model.call { try await $0.pluginPage(plugin: pluginId, page: nav.page, params: nav.queryItems) }
            error = nil
        } catch is CancellationError {
        } catch {
            if (error as? URLError)?.code == .cancelled { return }
            self.error = error.localizedDescription
            if view != nil { model.toast(error.localizedDescription, .error) }
        }
    }
}

struct PluginSheetView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let sheet: PluginSheet
    @State private var view: PluginView?

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                PluginViewRenderer(view: view ?? sheet.view, host: PluginHost(
                    pluginId: sheet.pluginId,
                    show: { view = $0 },
                    reload: {},
                    dismiss: { dismiss() }
                ))
                .padding(24)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            Divider()
            HStack {
                Spacer()
                Button("Done") { dismiss() }.keyboardShortcut(.cancelAction)
            }
            .padding(12)
        }
        .frame(minWidth: 560, idealWidth: 680, minHeight: 360, idealHeight: 560)
    }
}

struct PluginViewRenderer: View {
    let view: PluginView
    let host: PluginHost

    var body: some View {
        VStack(alignment: .leading, spacing: 22) {
            if view.title != nil || view.subtitle != nil {
                VStack(alignment: .leading, spacing: 4) {
                    if let t = view.title { Text(t).font(.largeTitle.bold()) }
                    if let s = view.subtitle { Text(s).foregroundStyle(.secondary) }
                }
            }
            ForEach(Array(view.blocks.enumerated()), id: \.offset) { i, block in
                PluginBlockView(block: block, host: host)
                    .id(blockIdentity(i, block))
            }
        }
    }

    private func blockIdentity(_ i: Int, _ block: PluginBlock) -> String {
        if case .form(let f) = block { return "form-\(i)-\(f.id ?? "")-\(f.fields.map(\.name).joined(separator: ","))" }
        return "block-\(i)"
    }
}

struct PluginBlockView: View {
    @Environment(AppModel.self) private var model
    let block: PluginBlock
    let host: PluginHost

    var body: some View {
        switch block {
        case .text(let text, let tone):
            Text(text)
                .foregroundStyle(toneColor(tone) ?? .primary)
                .frame(maxWidth: 820, alignment: .leading)
                .textSelection(.enabled)
        case .stats(let items):
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 140), spacing: 12)], spacing: 12) {
                ForEach(Array(items.enumerated()), id: \.offset) { _, s in
                    VStack(alignment: .leading, spacing: 4) {
                        Text(s.value.stringValue ?? "").font(.title2.bold()).monospacedDigit()
                        Text(s.label).font(.caption).foregroundStyle(.secondary)
                    }
                    .padding(14)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(.quaternary.opacity(0.6), in: RoundedRectangle(cornerRadius: 10))
                }
            }
        case .actions(let items):
            HStack(spacing: 10) {
                ForEach(Array(items.enumerated()), id: \.offset) { _, a in
                    PluginActionButton(action: a, host: host)
                }
            }
            .controlSize(.large)
        case .form(let form):
            PluginFormView(form: form) { values in
                guard let call = form.submit.call else { return }
                await PluginRunner.perform(model: model, host: host, call: call, body: values)
            }
            .frame(maxWidth: 640, alignment: .leading)
        case .list(_, let items, let empty):
            if items.isEmpty {
                Text(empty ?? "Nothing here.").foregroundStyle(.secondary).padding(.vertical, 24)
            } else {
                LazyVStack(spacing: 6) {
                    ForEach(items) { PluginListRow(item: $0, host: host) }
                }
            }
        case .unknown:
            EmptyView()
        }
    }
}

@MainActor
enum PluginRunner {
    static func perform(model: AppModel, host: PluginHost, action: PluginUIAction) async {
        if let nav = action.navigate {
            host.dismiss?()
            model.navigatePlugin(host.pluginId, nav)
            return
        }
        guard let call = action.call else { return }
        await perform(model: model, host: host, call: call, body: nil)
    }

    static func perform(model: AppModel, host: PluginHost, call: PluginCall, body: [String: JSONValue]?) async {
        do {
            let result = try await model.call { try await $0.pluginCall(plugin: host.pluginId, call, body: body) }
            if result.navigate != nil { host.dismiss?() }
            model.handle(result, pluginId: host.pluginId, show: host.show, reload: host.reload)
        } catch {
            model.toast(error.localizedDescription, .error)
        }
    }
}

struct PluginActionButton: View {
    @Environment(AppModel.self) private var model
    let action: PluginUIAction
    let host: PluginHost
    var small = false
    @State private var confirming = false
    @State private var running = false

    var body: some View {
        Group {
            if action.tone == .primary {
                button.buttonStyle(.borderedProminent).tint(accent)
            } else if action.tone == .danger {
                button.buttonStyle(.bordered).tint(.red)
            } else {
                button.buttonStyle(.bordered)
            }
        }
        .controlSize(small ? .small : .regular)
        .disabled(running)
        .alert(action.confirm ?? "", isPresented: $confirming) {
            Button("Cancel", role: .cancel) {}
            Button(action.label, role: action.tone == .danger ? .destructive : nil) { run() }
        }
    }

    private var button: some View {
        Button {
            if action.confirm?.isEmpty == false { confirming = true } else { run() }
        } label: {
            if let icon = action.icon {
                Label(action.label, systemImage: symbol(for: icon))
            } else {
                Text(action.label)
            }
        }
    }

    private func run() {
        running = true
        Task {
            await PluginRunner.perform(model: model, host: host, action: action)
            running = false
        }
    }
}

struct PluginListRow: View {
    @Environment(AppModel.self) private var model
    let item: PluginListItem
    let host: PluginHost
    @State private var hovering = false

    var body: some View {
        HStack(alignment: .center, spacing: 14) {
            HStack(alignment: .center, spacing: 14) {
                if let image = item.image {
                    RemoteImage(url: model.client?.imageURL(image, size: "w185"))
                        .frame(width: 46, height: 69)
                        .clipShape(RoundedRectangle(cornerRadius: 5))
                }
                VStack(alignment: .leading, spacing: 6) {
                    Text(item.title).font(.headline).lineLimit(2)
                    if let s = item.subtitle { Text(s).font(.callout).foregroundStyle(.secondary).lineLimit(2) }
                    if let p = item.progress {
                        ProgressView(value: min(1, max(0, p))).tint(accent).frame(maxWidth: 360)
                    }
                    if let badges = item.badges, !badges.isEmpty {
                        HStack(spacing: 6) {
                            ForEach(Array(badges.enumerated()), id: \.offset) { _, b in
                                Text(b.text)
                                    .font(.caption2.weight(.semibold))
                                    .padding(.horizontal, 7).padding(.vertical, 3)
                                    .background((toneColor(b.tone) ?? .secondary).opacity(0.22), in: RoundedRectangle(cornerRadius: 4))
                                    .foregroundStyle(toneColor(b.tone) ?? .primary)
                            }
                        }
                    }
                }
                Spacer(minLength: 0)
            }
            .contentShape(Rectangle())
            .onTapGesture {
                if let link = item.link {
                    host.dismiss?()
                    model.navigatePlugin(host.pluginId, link)
                }
            }
            if let actions = item.actions, !actions.isEmpty {
                HStack(spacing: 6) {
                    ForEach(Array(actions.enumerated()), id: \.offset) { _, a in
                        PluginActionButton(action: a, host: host, small: true)
                    }
                }
            }
        }
        .padding(.horizontal, 14).padding(.vertical, 12)
        .background(.quaternary.opacity(hovering && item.link != nil ? 0.9 : 0.5), in: RoundedRectangle(cornerRadius: 10))
        .onHover { hovering = $0 }
    }
}

struct PluginFormView: View {
    @Environment(AppModel.self) private var model
    let form: PluginForm
    let onSubmit: ([String: JSONValue]) async -> Void

    @State private var values: [String: JSONValue]
    @State private var submitting = false

    init(form: PluginForm, onSubmit: @escaping ([String: JSONValue]) async -> Void) {
        self.form = form
        self.onSubmit = onSubmit
        _values = State(initialValue: Dictionary(form.fields.map { ($0.name, $0.initialValue) }, uniquingKeysWith: { $1 }))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            ForEach(form.fields) { f in
                field(f)
            }
            Button {
                submit()
            } label: {
                if submitting { ProgressView().controlSize(.small) } else { Text(form.submit.label) }
            }
            .buttonStyle(.borderedProminent)
            .tint(accent)
            .disabled(submitting)
        }
    }

    private func string(_ name: String) -> Binding<String> {
        Binding(get: { values[name]?.stringValue ?? "" }, set: { values[name] = .string($0) })
    }

    @ViewBuilder
    private func field(_ f: PluginField) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            if f.kind != .toggle {
                Text(f.label + (f.required == true ? " *" : "")).font(.callout.weight(.semibold)).foregroundStyle(.secondary)
            }
            switch f.kind {
            case .text, .number:
                TextField(f.label, text: string(f.name), prompt: f.placeholder.map { Text($0) })
                    .textFieldStyle(.roundedBorder)
                    .labelsHidden()
            case .password:
                SecureField(f.label, text: string(f.name), prompt: f.placeholder.map { Text($0) })
                    .textFieldStyle(.roundedBorder)
                    .labelsHidden()
            case .textarea:
                TextEditor(text: string(f.name))
                    .font(.body)
                    .frame(minHeight: 70, maxHeight: 140)
                    .overlay(RoundedRectangle(cornerRadius: 6).stroke(.secondary.opacity(0.4)))
            case .toggle:
                Toggle(f.label, isOn: Binding(get: { values[f.name]?.boolValue ?? false }, set: { values[f.name] = .bool($0) }))
            case .select:
                Picker(f.label, selection: Binding(get: { values[f.name] ?? .null }, set: { values[f.name] = $0 })) {
                    ForEach(Array((f.options ?? []).enumerated()), id: \.offset) { _, o in
                        Text(o.label).tag(o.value)
                    }
                }
                .labelsHidden()
                .frame(maxWidth: 360, alignment: .leading)
            case .file:
                HStack {
                    Button("Choose File…") { chooseFile(f) }
                    Text(values[f.name]?["name"]?.stringValue ?? "No file chosen").foregroundStyle(.secondary).lineLimit(1)
                    if values[f.name]?["name"] != nil {
                        Button { values[f.name] = .null } label: { Image(systemName: "xmark.circle.fill") }.buttonStyle(.plain).foregroundStyle(.secondary)
                    }
                }
            }
            if let help = f.help { Text(help).font(.caption).foregroundStyle(.secondary) }
        }
    }

    private func chooseFile(_ f: PluginField) {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do {
            let data = try Data(contentsOf: url)
            values[f.name] = PluginForm.fileValue(name: url.lastPathComponent, data: data)
        } catch {
            model.toast("Couldn't read \(url.lastPathComponent): \(error.localizedDescription)", .error)
        }
    }

    private func submit() {
        let missing = form.missing(values: values)
        guard missing.isEmpty else {
            model.toast("Fill in \(missing.joined(separator: ", "))", .error)
            return
        }
        let body = form.body(values: values)
        submitting = true
        Task {
            await onSubmit(body)
            submitting = false
        }
    }
}
