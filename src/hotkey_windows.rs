//! Windows global hotkey via a low-level keyboard hook
//!
//! A `WH_KEYBOARD_LL` hook sees every keystroke on the desktop, so the hook
//! procedure does the least it can: compare virtual-key codes and hand an event
//! to the daemon without blocking. rdev, used on macOS, translates every key to
//! text inside its hook; on Windows that stalls the input queue and disturbs
//! dead-key layouts, which scrambled the text voxtype itself was typing.
//!
//! Keystrokes flagged as injected are ignored, so the SendInput driver never
//! reaches the hotkey logic. Auto-repeat is folded: holding the key yields one
//! `Pressed`, releasing it one `Released`.

use crate::config::HotkeyConfig;
use crate::error::{HotkeyError, Result};
use std::sync::Mutex;
use tokio::sync::mpsc;
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
    HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT,
    WM_SYSKEYDOWN, WM_SYSKEYUP,
};

/// Hotkey events that can be sent from the listener
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyEvent {
    /// Same shape as the Linux variant so the daemon match arms stay
    /// platform-agnostic; the overrides are not surfaced on Windows yet.
    Pressed {
        model_override: Option<String>,
        profile_override: Option<String>,
    },
    Released,
    Cancel,
}

/// Hotkey listener trait for Windows
pub trait HotkeyListener: Send {
    /// Start listening for hotkey events
    fn start(&mut self) -> Result<mpsc::Receiver<HotkeyEvent>>;

    /// Stop listening
    fn stop(&mut self) -> Result<()>;
}

/// What the hook procedure matches against. The procedure is a bare
/// `extern "system" fn`, so its state lives in a static.
struct HookState {
    target: &'static [u32],
    cancel: &'static [u32],
    tx: mpsc::Sender<HotkeyEvent>,
    held: bool,
}

static HOOK_STATE: Mutex<Option<HookState>> = Mutex::new(None);

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for WH_KEYBOARD_LL with HC_ACTION, lparam points to a
        // KBDLLHOOKSTRUCT valid for the duration of the call.
        let kb = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        if kb.flags & LLKHF_INJECTED == 0 {
            if let Ok(mut guard) = HOOK_STATE.try_lock() {
                if let Some(state) = guard.as_mut() {
                    handle_key(state, kb.vkCode, wparam as u32);
                }
            }
        }
    }
    // SAFETY: passing the hook arguments through unchanged.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

fn handle_key(state: &mut HookState, vk: u32, message: u32) {
    let down = matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN);
    let up = matches!(message, WM_KEYUP | WM_SYSKEYUP);

    if state.target.contains(&vk) {
        if down && !state.held {
            state.held = true;
            let _ = state.tx.try_send(HotkeyEvent::Pressed {
                model_override: None,
                profile_override: None,
            });
        } else if up && state.held {
            state.held = false;
            let _ = state.tx.try_send(HotkeyEvent::Released);
        }
    } else if down && state.cancel.contains(&vk) {
        let _ = state.tx.try_send(HotkeyEvent::Cancel);
    }
}

/// Low-level keyboard hook listener
pub struct LowLevelHookListener {
    target: &'static [u32],
    cancel: &'static [u32],
    thread_id: Option<u32>,
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl LowLevelHookListener {
    pub fn new(config: &HotkeyConfig) -> Result<Self> {
        let target = parse_key_name(&config.key)
            .ok_or_else(|| HotkeyError::UnknownKey(config.key.clone()))?;
        let cancel = config
            .cancel_key
            .as_deref()
            .and_then(parse_key_name)
            .unwrap_or(&[]);
        Ok(Self {
            target,
            cancel,
            thread_id: None,
            thread_handle: None,
        })
    }
}

impl HotkeyListener for LowLevelHookListener {
    fn start(&mut self) -> Result<mpsc::Receiver<HotkeyEvent>> {
        let (tx, rx) = mpsc::channel(32);
        *HOOK_STATE.lock().unwrap() = Some(HookState {
            target: self.target,
            cancel: self.cancel,
            tx,
            held: false,
        });

        let (id_tx, id_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            // SAFETY: a null module name yields this executable's handle; the
            // hook is removed on the same thread before it exits.
            unsafe {
                let module = GetModuleHandleW(std::ptr::null());
                let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0);
                if hook.is_null() {
                    tracing::error!(
                        "Failed to install keyboard hook: {}",
                        std::io::Error::last_os_error()
                    );
                    let _ = id_tx.send(None);
                    return;
                }
                let _ = id_tx.send(Some(GetCurrentThreadId()));

                // Low-level hooks are delivered through this thread's message loop.
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {}

                UnhookWindowsHookEx(hook);
            }
        });

        match id_rx.recv() {
            Ok(Some(id)) => {
                self.thread_id = Some(id);
                self.thread_handle = Some(handle);
                Ok(rx)
            }
            _ => Err(HotkeyError::WindowsHook(std::io::Error::last_os_error().to_string()).into()),
        }
    }

    fn stop(&mut self) -> Result<()> {
        if let Some(id) = self.thread_id.take() {
            // SAFETY: posting WM_QUIT to a thread id we created ends its loop.
            unsafe { PostThreadMessageW(id, WM_QUIT, 0, 0) };
        }
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
        *HOOK_STATE.lock().unwrap() = None;
        Ok(())
    }
}

/// Create a hotkey listener for Windows
pub fn create_listener(
    config: &HotkeyConfig,
    _secondary_model: Option<String>,
) -> Result<Box<dyn HotkeyListener>> {
    Ok(Box::new(LowLevelHookListener::new(config)?))
}

/// Map a config key name to the virtual-key codes that satisfy it. Generic
/// modifier names match either side; a low-level hook reports them sided.
fn parse_key_name(name: &str) -> Option<&'static [u32]> {
    let upper = name.to_uppercase();
    let key = upper.strip_prefix("KEY_").unwrap_or(&upper);
    let vks: &'static [u32] = match key {
        "F1" => &[0x70],
        "F2" => &[0x71],
        "F3" => &[0x72],
        "F4" => &[0x73],
        "F5" => &[0x74],
        "F6" => &[0x75],
        "F7" => &[0x76],
        "F8" => &[0x77],
        "F9" => &[0x78],
        "F10" => &[0x79],
        "F11" => &[0x7A],
        "F12" => &[0x7B],
        "F13" => &[0x7C],
        "F14" => &[0x7D],
        "F15" => &[0x7E],
        "F16" => &[0x7F],
        "F17" => &[0x80],
        "F18" => &[0x81],
        "F19" => &[0x82],
        "F20" => &[0x83],
        "F21" => &[0x84],
        "F22" => &[0x85],
        "F23" => &[0x86],
        "F24" => &[0x87],
        "PAUSE" => &[0x13],
        "SCROLLLOCK" => &[0x91],
        "NUMLOCK" => &[0x90],
        "CAPSLOCK" => &[0x14],
        "PRINTSCREEN" | "SYSRQ" => &[0x2C],
        "INSERT" => &[0x2D],
        "DELETE" => &[0x2E],
        "HOME" => &[0x24],
        "END" => &[0x23],
        "PAGEUP" => &[0x21],
        "PAGEDOWN" => &[0x22],
        "UP" | "UPARROW" => &[0x26],
        "DOWN" | "DOWNARROW" => &[0x28],
        "LEFT" | "LEFTARROW" => &[0x25],
        "RIGHT" | "RIGHTARROW" => &[0x27],
        "ESC" | "ESCAPE" => &[0x1B],
        "SPACE" => &[0x20],
        "TAB" => &[0x09],
        "ENTER" | "RETURN" => &[0x0D],
        "BACKSPACE" => &[0x08],
        "MENU" | "COMPOSE" => &[0x5D],
        "LEFTSHIFT" => &[0xA0],
        "RIGHTSHIFT" => &[0xA1],
        "SHIFT" => &[0xA0, 0xA1],
        "LEFTCTRL" | "LEFTCONTROL" => &[0xA2],
        "RIGHTCTRL" | "RIGHTCONTROL" => &[0xA3],
        "CTRL" | "CONTROL" => &[0xA2, 0xA3],
        "LEFTALT" => &[0xA4],
        "RIGHTALT" | "ALTGR" => &[0xA5],
        "ALT" => &[0xA4, 0xA5],
        "LEFTMETA" | "LEFTWIN" => &[0x5B],
        "RIGHTMETA" | "RIGHTWIN" => &[0x5C],
        "META" | "WIN" => &[0x5B, 0x5C],
        "A" => &[0x41],
        "B" => &[0x42],
        "C" => &[0x43],
        "D" => &[0x44],
        "E" => &[0x45],
        "F" => &[0x46],
        "G" => &[0x47],
        "H" => &[0x48],
        "I" => &[0x49],
        "J" => &[0x4A],
        "K" => &[0x4B],
        "L" => &[0x4C],
        "M" => &[0x4D],
        "N" => &[0x4E],
        "O" => &[0x4F],
        "P" => &[0x50],
        "Q" => &[0x51],
        "R" => &[0x52],
        "S" => &[0x53],
        "T" => &[0x54],
        "U" => &[0x55],
        "V" => &[0x56],
        "W" => &[0x57],
        "X" => &[0x58],
        "Y" => &[0x59],
        "Z" => &[0x5A],
        _ => return None,
    };
    Some(vks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(target: &'static [u32]) -> (HookState, mpsc::Receiver<HotkeyEvent>) {
        let (tx, rx) = mpsc::channel(8);
        (
            HookState {
                target,
                cancel: &[0x1B],
                tx,
                held: false,
            },
            rx,
        )
    }

    #[test]
    fn auto_repeat_yields_one_press_and_one_release() {
        let (mut s, mut rx) = state(&[0x22]);
        for _ in 0..5 {
            handle_key(&mut s, 0x22, WM_KEYDOWN);
        }
        handle_key(&mut s, 0x22, WM_KEYUP);
        assert!(matches!(rx.try_recv(), Ok(HotkeyEvent::Pressed { .. })));
        assert_eq!(rx.try_recv(), Ok(HotkeyEvent::Released));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn other_keys_and_cancel() {
        let (mut s, mut rx) = state(&[0x70]);
        handle_key(&mut s, 0x41, WM_KEYDOWN);
        handle_key(&mut s, 0x1B, WM_KEYDOWN);
        assert_eq!(rx.try_recv(), Ok(HotkeyEvent::Cancel));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn key_names() {
        assert_eq!(parse_key_name("pagedown"), Some(&[0x22u32][..]));
        assert_eq!(parse_key_name("KEY_F1"), Some(&[0x70u32][..]));
        assert_eq!(parse_key_name("ctrl"), Some(&[0xA2u32, 0xA3][..]));
        assert_eq!(parse_key_name("nope"), None);
    }
}
