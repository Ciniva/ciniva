import SwiftUI
import UniformTypeIdentifiers

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        NSApp.activate()
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}

@main
struct CinivaApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var model = AppModel()

    init() {
        Prefs.register()
        URLCache.shared = URLCache(memoryCapacity: 64 << 20, diskCapacity: 512 << 20)
    }

    var body: some Scene {
        Window("Ciniva", id: "main") {
            RootView()
                .environment(model)
                .frame(minWidth: 960, minHeight: 620)
        }
        .defaultSize(width: 1280, height: 820)
        .commands {
            CommandGroup(after: .newItem) {
                OpenFileCommand(model: model)
            }
        }

        Settings {
            SettingsView().environment(model)
        }
    }
}

private struct OpenFileCommand: View {
    let model: AppModel

    var body: some View {
        Button("Open File…") {
            let panel = NSOpenPanel()
            panel.allowedContentTypes = [.movie, .video, .audiovisualContent, .data]
            guard panel.runModal() == .OK, let url = panel.url else { return }
            model.playFile(url)
        }
        .keyboardShortcut("o")
    }
}
