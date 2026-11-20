import AppKit
import SwiftUI

@main
struct EditorGlassApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var model = EditorModel()

    var body: some Scene {
        WindowGroup("h2-editor (Liquid Glass mock)") {
            RootView()
                .environment(model)
                .frame(minWidth: 1180, minHeight: 760)
        }
        .defaultSize(width: 1520, height: 940)
        .commands {
            CommandGroup(after: .sidebar) {
                Button("Frame map") { withAnimation(.smooth(duration: 0.7)) { model.frameMap() } }
                    .keyboardShortcut(.home, modifiers: [])
                Button("Focus selection") { withAnimation(.smooth(duration: 0.7)) { model.focusSelection() } }
                Divider()
                Picker("Gizmo", selection: $model.gizmo.animation(.bouncy(duration: 0.45))) {
                    ForEach(GizmoMode.allCases) { Text("\($0.title) gizmo").tag($0) }
                }
                .pickerStyle(.inline)
                Toggle("Gizmo in local axes", isOn: $model.localAxes)
                Toggle("Halo 1 overlay", isOn: $model.h1Overlay)
                    .disabled(model.map.family != .halo1)
                Divider()
            }
            CommandGroup(replacing: .pasteboard) {
                Button("Duplicate") { model.duplicateSelection() }
                    .keyboardShortcut("d")
                Button("Delete") { model.deleteSelection() }
            }
        }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        // A SwiftPM executable has no bundle: ask to be a regular app with a
        // Dock icon and a menu bar.
        NSApp.setActivationPolicy(.regular)
        NSApp.activate()
        if let path = LaunchOptions.shared.snapshot {
            // An inactive window greys its tints: come to the front for the picture.
            NSApp.activate(ignoringOtherApps: true)
            Snapshot.schedule(to: path)
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}
