import AppKit
import SwiftUI

/// Closing the window quits the settings app.
///
/// A WindowGroup app stays alive with no windows once its last one is closed,
/// and macOS answers a second "Settings…" by merely activating that windowless
/// process: nothing appears on screen and the menu item looks broken. Quitting
/// instead means every click opens a fresh window. A settings panel reached
/// from the menu bar has nothing to stay running for.
final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }

    /// Belt and braces: should this process ever outlive its window anyway,
    /// a reopen must put a window back rather than silently do nothing.
    func applicationShouldHandleReopen(
        _ sender: NSApplication,
        hasVisibleWindows: Bool
    ) -> Bool {
        !hasVisibleWindows
    }
}

@main
struct VoxtypeSetupApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        WindowGroup {
            SettingsView()
        }
        .windowStyle(.hiddenTitleBar)
        .defaultSize(width: 700, height: 500)
    }
}

/// Main settings view with sidebar navigation
struct SettingsView: View {
    @State private var selectedSection: SettingsSection = .general

    var body: some View {
        NavigationSplitView {
            List(SettingsSection.allCases, selection: $selectedSection) { section in
                Label(section.title, systemImage: section.icon)
                    .tag(section)
            }
            .listStyle(.sidebar)
            .navigationSplitViewColumnWidth(min: 180, ideal: 200)
        } detail: {
            selectedSection.view
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .padding()
        }
        .navigationTitle("Voxtype Settings")
        .onAppear {
            // On first launch, go to Permissions so user can grant access
            if isFirstLaunch() {
                selectedSection = .permissions
            }
        }
    }

    private func isFirstLaunch() -> Bool {
        let key = "HasLaunchedBefore"
        let hasLaunched = UserDefaults.standard.bool(forKey: key)
        if !hasLaunched {
            UserDefaults.standard.set(true, forKey: key)
            return true
        }
        return false
    }
}

/// Settings sections
enum SettingsSection: String, CaseIterable, Identifiable {
    case general
    case hotkey
    case audio
    case models
    case whisper
    case output
    case textProcessing
    case notifications
    case permissions
    case advanced

    var id: String { rawValue }

    var title: String {
        switch self {
        case .general: return "General"
        case .hotkey: return "Hotkey"
        case .audio: return "Audio"
        case .models: return "Models"
        case .whisper: return "Whisper"
        case .output: return "Output"
        case .textProcessing: return "Text Processing"
        case .notifications: return "Notifications"
        case .permissions: return "Permissions"
        case .advanced: return "Advanced"
        }
    }

    var icon: String {
        switch self {
        case .general: return "gearshape"
        case .hotkey: return "keyboard"
        case .audio: return "mic"
        case .models: return "cpu"
        case .whisper: return "waveform"
        case .output: return "text.cursor"
        case .textProcessing: return "text.quote"
        case .notifications: return "bell"
        case .permissions: return "lock.shield"
        case .advanced: return "wrench.and.screwdriver"
        }
    }

    @ViewBuilder
    var view: some View {
        switch self {
        case .general: GeneralSettingsView()
        case .hotkey: HotkeySettingsView()
        case .audio: AudioSettingsView()
        case .models: ModelsSettingsView()
        case .whisper: WhisperSettingsView()
        case .output: OutputSettingsView()
        case .textProcessing: TextProcessingSettingsView()
        case .notifications: NotificationSettingsView()
        case .permissions: PermissionsSettingsView()
        case .advanced: AdvancedSettingsView()
        }
    }
}
