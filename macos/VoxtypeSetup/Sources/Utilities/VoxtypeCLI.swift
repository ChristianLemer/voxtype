import Foundation

/// Helper to run voxtype CLI commands
enum VoxtypeCLI {
    /// Voxtype.app: the bundle this app is embedded in
    /// (Voxtype.app/Contents/MacOS/VoxtypeSetup.app), else /Applications.
    static var appBundlePath: String {
        let enclosing = Bundle.main.bundleURL
            .deletingLastPathComponent()  // MacOS
            .deletingLastPathComponent()  // Contents
            .deletingLastPathComponent()  // Voxtype.app
        if enclosing.pathExtension == "app"
            && FileManager.default.fileExists(atPath: enclosing.appendingPathComponent("Contents/MacOS/voxtype-bin").path) {
            return enclosing.path
        }
        return "/Applications/Voxtype.app"
    }

    /// Path to voxtype binary
    static var binaryPath: String {
        let bundled = appBundlePath + "/Contents/MacOS/voxtype-bin"
        if FileManager.default.fileExists(atPath: bundled) {
            return bundled
        }

        // Try homebrew symlink
        let homebrewPath = "/opt/homebrew/bin/voxtype"
        if FileManager.default.fileExists(atPath: homebrewPath) {
            return homebrewPath
        }

        // Try ~/.local/bin
        let localBinPath = NSHomeDirectory() + "/.local/bin/voxtype"
        if FileManager.default.fileExists(atPath: localBinPath) {
            return localBinPath
        }

        // Fallback to PATH
        return "voxtype"
    }

    /// Run a voxtype command
    @discardableResult
    static func run(_ arguments: [String], wait: Bool = true) -> (output: String, success: Bool) {
        let task = Process()
        task.launchPath = binaryPath
        task.arguments = arguments

        let pipe = Pipe()
        task.standardOutput = pipe
        task.standardError = pipe

        do {
            try task.run()

            if wait {
                task.waitUntilExit()
                let data = pipe.fileHandleForReading.readDataToEndOfFile()
                let output = String(data: data, encoding: .utf8) ?? ""
                return (output, task.terminationStatus == 0)
            } else {
                return ("", true)
            }
        } catch {
            return ("Error: \(error.localizedDescription)", false)
        }
    }

    /// The last non-empty line of a command's output. The CLI's log lines
    /// share the pipe, so `status` prints e.g. an INFO line before "idle".
    static func lastLine(_ output: String) -> String {
        output.components(separatedBy: .newlines)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .last { !$0.isEmpty } ?? ""
    }

    /// Get daemon status
    static func getStatus() -> String {
        let result = run(["status"])
        return lastLine(result.output)
    }

    /// Check if daemon is running
    static func isDaemonRunning() -> Bool {
        let result = run(["status"])
        let status = lastLine(result.output).lowercased()
        return status == "idle" || status == "recording" || status == "transcribing"
    }

    /// Start Voxtype.app, which runs the daemon and the menu bar icon.
    static func launchApp() {
        let task = Process()
        task.launchPath = "/usr/bin/open"
        task.arguments = [appBundlePath]
        try? task.run()
    }

    /// Stop Voxtype.app: the menu bar process, the daemon and the
    /// recording card all run from its voxtype-bin.
    static func stopApp() {
        let task = Process()
        task.launchPath = "/usr/bin/pkill"
        task.arguments = ["-f", appBundlePath + "/Contents/MacOS/voxtype-bin"]
        task.standardOutput = FileHandle.nullDevice
        task.standardError = FileHandle.nullDevice
        try? task.run()
        task.waitUntilExit()
    }

    /// Restart the daemon by restarting Voxtype.app, so the menu bar icon
    /// comes back with it. Also the way to start it: when the daemon has
    /// died but the menu bar process lives, `open` alone only activates the
    /// running app and starts nothing.
    static func restartDaemon(completion: (() -> Void)? = nil) {
        DispatchQueue.global().async {
            stopApp()

            // Give the processes time to exit and release the lock
            Thread.sleep(forTimeInterval: 1.0)

            DispatchQueue.main.async {
                launchApp()
                completion?()
            }
        }
    }
}
