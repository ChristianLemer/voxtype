import SwiftUI

struct AdvancedSettingsView: View {
    @State private var autoStartEnabled: Bool = false
    @State private var daemonRunning: Bool = false
    @State private var daemonStatus: String = "Unknown"

    var body: some View {
        Form {
            Section {
                HStack {
                    VStack(alignment: .leading) {
                        Text("Configuration File")
                        Text(VoxtypePaths.display(VoxtypePaths.configFile))
                            .font(.caption)
                            .foregroundColor(.secondary)
                    }

                    Spacer()

                    Button("Open in Editor") {
                        openConfigFile()
                    }
                }

                HStack {
                    VStack(alignment: .leading) {
                        Text("Log Files")
                        Text("~/Library/Logs/voxtype/")
                            .font(.caption)
                            .foregroundColor(.secondary)
                    }

                    Spacer()

                    Button("Open Folder") {
                        openLogsFolder()
                    }
                }

                HStack {
                    VStack(alignment: .leading) {
                        Text("Models Folder")
                        Text(VoxtypePaths.display(VoxtypePaths.modelsDir))
                            .font(.caption)
                            .foregroundColor(.secondary)
                    }

                    Spacer()

                    Button("Open Folder") {
                        openModelsFolder()
                    }
                }
            } header: {
                Text("Files & Folders")
            }

            Section {
                Toggle("Start Voxtype at login", isOn: $autoStartEnabled)
                    .onChange(of: autoStartEnabled) { newValue in
                        toggleAutoStart(enabled: newValue)
                    }

                Text("Runs the Voxtype daemon automatically when you log in.")
                    .font(.caption)
                    .foregroundColor(.secondary)
            } header: {
                Text("Auto-Start")
            }

            Section {
                HStack {
                    Circle()
                        .fill(daemonRunning ? Color.green : Color.red)
                        .frame(width: 10, height: 10)
                    Text("Status: \(daemonStatus)")

                    Spacer()

                    Button("Refresh") {
                        checkDaemonStatus()
                    }
                }

                if daemonRunning {
                    Button(action: restartDaemon) {
                        Label("Restart Daemon", systemImage: "arrow.clockwise")
                    }
                } else {
                    Button(action: startDaemon) {
                        Label("Start Daemon", systemImage: "play.fill")
                    }
                }

                Button(action: stopDaemon) {
                    Label("Stop Daemon", systemImage: "stop.fill")
                }
                .foregroundColor(.red)
                .disabled(!daemonRunning)

                Button(action: runSetupCheck) {
                    Label("Run Setup Check", systemImage: "checkmark.circle")
                }
            } header: {
                Text("Daemon")
            }

            Section {
                HStack {
                    Text("Version")
                    Spacer()
                    Text(getVersion())
                        .foregroundColor(.secondary)
                }

                Link(destination: URL(string: "https://github.com/peteonrails/voxtype")!) {
                    Label("View on GitHub", systemImage: "link")
                }

                Link(destination: URL(string: "https://voxtype.io")!) {
                    Label("Documentation", systemImage: "book")
                }
            } header: {
                Text("About")
            }
        }
        .formStyle(.grouped)
        .onAppear {
            checkAutoStartStatus()
            checkDaemonStatus()
        }
    }

    private func checkDaemonStatus() {
        let result = VoxtypeCLI.run(["status"])
        let status = result.output.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()

        if status == "idle" || status == "recording" || status == "transcribing" {
            daemonRunning = true
            daemonStatus = status.capitalized
        } else if status.contains("not running") || status.isEmpty || !result.success {
            daemonRunning = false
            daemonStatus = "Not Running"
        } else {
            daemonRunning = false
            daemonStatus = status.capitalized
        }
    }

    private func startDaemon() {
        VoxtypeCLI.launchApp()
        DispatchQueue.main.asyncAfter(deadline: .now() + 2) {
            checkDaemonStatus()
        }
    }

    private func openConfigFile() {
        let path = VoxtypePaths.configFile
        NSWorkspace.shared.open(URL(fileURLWithPath: path))
    }

    private func openLogsFolder() {
        let path = VoxtypePaths.logsDir
        NSWorkspace.shared.open(URL(fileURLWithPath: path))
    }

    private func openModelsFolder() {
        let path = VoxtypePaths.modelsDir
        NSWorkspace.shared.open(URL(fileURLWithPath: path))
    }

    private func checkAutoStartStatus() {
        let plistPath = NSHomeDirectory() + "/Library/LaunchAgents/io.voxtype.daemon.plist"
        autoStartEnabled = FileManager.default.fileExists(atPath: plistPath)
    }

    private func toggleAutoStart(enabled: Bool) {
        if enabled {
            VoxtypeCLI.run(["setup", "launchd"])
        } else {
            VoxtypeCLI.run(["setup", "launchd", "--uninstall"])
        }
    }

    private func restartDaemon() {
        VoxtypeCLI.restartDaemon {
            DispatchQueue.main.asyncAfter(deadline: .now() + 2) {
                checkDaemonStatus()
            }
        }
    }

    private func stopDaemon() {
        VoxtypeCLI.stopApp()
    }

    private func runSetupCheck() {
        // Open Terminal with setup check command
        let voxtype = VoxtypeCLI.binaryPath
        let script = """
            tell application "Terminal"
                do script "\(voxtype) setup check"
                activate
            end tell
            """
        if let appleScript = NSAppleScript(source: script) {
            var error: NSDictionary?
            appleScript.executeAndReturnError(&error)
        }
    }

    private func getVersion() -> String {
        let result = VoxtypeCLI.run(["--version"])
        return result.output.trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
