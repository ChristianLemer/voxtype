import Foundation

/// The files the daemon reads, resolved the way `Config::config_dir` and
/// `Config::data_dir` resolve them in Rust: an absolute `$XDG_*_HOME` wins,
/// then `~/.config/voxtype` and `~/.local/share/voxtype`, except that an
/// install which only has the older `~/Library/Application Support/voxtype`
/// keeps using it.
enum VoxtypePaths {
    private static let legacyDir = NSHomeDirectory() + "/Library/Application Support/voxtype"

    private static func resolve(xdgVar: String, defaultRel: String) -> String {
        if let base = ProcessInfo.processInfo.environment[xdgVar], base.hasPrefix("/") {
            return base + "/voxtype"
        }
        let xdg = NSHomeDirectory() + "/" + defaultRel + "/voxtype"
        let fm = FileManager.default
        if !fm.fileExists(atPath: xdg) && fm.fileExists(atPath: legacyDir) {
            return legacyDir
        }
        return xdg
    }

    static var configDir: String { resolve(xdgVar: "XDG_CONFIG_HOME", defaultRel: ".config") }
    static var configFile: String { configDir + "/config.toml" }
    static var dataDir: String { resolve(xdgVar: "XDG_DATA_HOME", defaultRel: ".local/share") }
    static var modelsDir: String { dataDir + "/models" }
    static let logsDir = NSHomeDirectory() + "/Library/Logs/voxtype"

    /// A path for display, with the home directory shortened to `~`.
    static func display(_ path: String) -> String {
        let home = NSHomeDirectory()
        return path.hasPrefix(home) ? "~" + path.dropFirst(home.count) : path
    }
}
