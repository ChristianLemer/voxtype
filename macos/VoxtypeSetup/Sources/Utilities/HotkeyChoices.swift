import Foundation

/// Hotkeys offered in the pickers: keys the macOS listener recognizes
/// (`parse_key_name` in src/hotkey_macos.rs). A PC keyboard's Print Screen,
/// Scroll Lock and Pause reach macOS as F13, F14 and F15.
enum HotkeyChoices {
    static let keys: [(String, String)] = [
        ("Globe / fn", "FN"),
        ("Right Option (⌥)", "RIGHTALT"),
        ("Right Command (⌘)", "RIGHTMETA"),
        ("Right Control (⌃)", "RIGHTCTRL"),
        ("Left Option (⌥)", "LEFTALT"),
        ("Left Command (⌘)", "LEFTMETA"),
        ("Left Control (⌃)", "LEFTCTRL"),
        ("Page Down", "PAGEDOWN"),
        ("Page Up", "PAGEUP"),
        ("Home", "HOME"),
        ("End", "END"),
        ("Insert / Help", "INSERT"),
        ("Pause (F15)", "PAUSE"),
        ("Scroll Lock (F14)", "SCROLLLOCK"),
        ("F13", "F13"),
        ("F16", "F16"),
        ("F17", "F17"),
        ("F18", "F18"),
        ("F19", "F19"),
        ("F20", "F20"),
    ]
}
