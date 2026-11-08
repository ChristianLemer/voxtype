//! Reaching Microsoft Teams' meeting controls through Windows UI Automation.
//!
//! The Windows counterpart of `teams_macos`: `TeamsMuter` mutes the meeting
//! while voxtype records (`audio.mute_teams`) by pressing Teams' own
//! microphone button, and `voxtype teams-ax` lists Teams' buttons or presses
//! one by its name.
//!
//! Each query fetches the names and control types of a whole window in one
//! cross-process call (`FindAllBuildCache`), since Teams' web content holds
//! hundreds of elements.

use windows::core::BSTR;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationElement,
    IUIAutomationInvokePattern, TreeScope_Children, TreeScope_Descendants,
    UIA_AutomationIdPropertyId, UIA_ButtonControlTypeId, UIA_ControlTypePropertyId,
    UIA_InvokePatternId, UIA_NamePropertyId, UIA_ProcessIdPropertyId,
};

/// The microphone button's name while the microphone is open.
const MUTE_LABELS: &[&str] = &["Mute mic"];
/// Its name while the microphone is muted.
const UNMUTE_LABELS: &[&str] = &["Unmute mic"];

/// How often the worker queries Teams while idle. A UI Automation client
/// makes Chromium build its accessibility tree, so the tree is ready by the
/// time a recording starts.
const WARM_INTERVAL: std::time::Duration = std::time::Duration::from_secs(20);

fn is_teams_process(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let mut path = [0u16; 1024];
    let mut len = path.len() as u32;
    let ok = unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return false;
        }
        let ok =
            QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, path.as_mut_ptr(), &mut len);
        CloseHandle(process);
        ok != 0
    };
    ok && String::from_utf16_lossy(&path[..len as usize])
        .to_lowercase()
        .ends_with("\\ms-teams.exe")
}

/// UI Automation, with the calling thread joined to COM.
struct Uia {
    automation: IUIAutomation,
    cache: IUIAutomationCacheRequest,
}

impl Uia {
    fn new() -> anyhow::Result<Self> {
        unsafe {
            // S_FALSE (already initialised) is fine; only a mode clash fails.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let cache = automation.CreateCacheRequest()?;
            for property in [
                UIA_NamePropertyId,
                UIA_ControlTypePropertyId,
                UIA_AutomationIdPropertyId,
                UIA_ProcessIdPropertyId,
            ] {
                cache.AddProperty(property)?;
            }
            cache.AddPattern(UIA_InvokePatternId)?;
            Ok(Self { automation, cache })
        }
    }

    fn find_all(
        &self,
        element: &IUIAutomationElement,
        scope: windows::Win32::UI::Accessibility::TreeScope,
    ) -> Vec<IUIAutomationElement> {
        unsafe {
            let Ok(condition) = self.automation.CreateTrueCondition() else {
                return Vec::new();
            };
            let Ok(found) = element.FindAllBuildCache(scope, &condition, &self.cache) else {
                return Vec::new();
            };
            let len = found.Length().unwrap_or(0);
            (0..len).filter_map(|i| found.GetElement(i).ok()).collect()
        }
    }

    /// Teams' top-level windows, with their titles.
    fn teams_windows(&self) -> Vec<(String, IUIAutomationElement)> {
        let Ok(root) = (unsafe { self.automation.GetRootElement() }) else {
            return Vec::new();
        };
        self.find_all(&root, TreeScope_Children)
            .into_iter()
            .filter(|w| {
                unsafe { w.CachedProcessId() }
                    .map(|pid| is_teams_process(pid as u32))
                    .unwrap_or(false)
            })
            .map(|w| (cached_name(&w), w))
            .collect()
    }

    /// The buttons in Teams' windows: (window title, name, automation id, element).
    fn buttons(&self) -> Vec<(String, String, String, IUIAutomationElement)> {
        let mut out = Vec::new();
        for (title, window) in self.teams_windows() {
            for element in self.find_all(&window, TreeScope_Descendants) {
                if unsafe { element.CachedControlType() }.ok() == Some(UIA_ButtonControlTypeId) {
                    let id = unsafe { element.CachedAutomationId() }
                        .map(|s| s.to_string())
                        .unwrap_or_default();
                    out.push((title.clone(), cached_name(&element), id, element));
                }
            }
        }
        out
    }

    /// Press the first Teams button named one of `labels`.
    fn press_first(&self, labels: &[&str]) -> bool {
        let started = std::time::Instant::now();
        let Some((_, _, _, button)) = self
            .buttons()
            .into_iter()
            .find(|(_, name, _, _)| labels.contains(&name.as_str()))
        else {
            tracing::debug!(?labels, "No Teams button found");
            return false;
        };
        let pressed = invoke(&button);
        tracing::debug!(?labels, pressed, elapsed = ?started.elapsed(), "Pressed Teams button");
        pressed
    }
}

fn cached_name(element: &IUIAutomationElement) -> String {
    unsafe { element.CachedName() }
        .map(|s: BSTR| s.to_string())
        .unwrap_or_default()
}

fn invoke(element: &IUIAutomationElement) -> bool {
    unsafe {
        element
            .GetCachedPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
            .or_else(|_| {
                element.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
            })
            .and_then(|pattern| pattern.Invoke())
            .is_ok()
    }
}

enum Request {
    Mute,
    Restore,
}

/// Mutes the Teams meeting while voxtype records, and unmutes it afterwards
/// only if it was voxtype that muted it. Same contract as on macOS: a thread
/// of its own, requests handled in order, never holding up the recording.
pub struct TeamsMuter {
    tx: std::sync::mpsc::Sender<Request>,
}

impl TeamsMuter {
    pub fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("teams-mute".into())
            .spawn(move || worker(rx))
            .expect("spawn the Teams mute thread");
        Self { tx }
    }

    /// Mute the meeting's microphone if Teams is in a meeting with it open.
    pub fn mute(&self) {
        let _ = self.tx.send(Request::Mute);
    }

    /// Unmute it again if `mute` muted it.
    pub fn restore(&self) {
        let _ = self.tx.send(Request::Restore);
    }
}

impl Default for TeamsMuter {
    fn default() -> Self {
        Self::new()
    }
}

fn worker(rx: std::sync::mpsc::Receiver<Request>) {
    use std::sync::mpsc::RecvTimeoutError;

    let uia = match Uia::new() {
        Ok(uia) => uia,
        Err(e) => {
            tracing::warn!("Teams mute unavailable: UI Automation failed to start: {e}");
            return;
        }
    };
    let mut muted_by_us = false;
    let _ = uia.teams_windows();
    loop {
        match rx.recv_timeout(WARM_INTERVAL) {
            Ok(Request::Mute) => {
                muted_by_us = uia.press_first(MUTE_LABELS);
                if muted_by_us {
                    tracing::info!("Muted the Teams meeting while recording");
                }
            }
            Ok(Request::Restore) => {
                if std::mem::take(&mut muted_by_us) && uia.press_first(UNMUTE_LABELS) {
                    tracing::info!("Unmuted the Teams meeting");
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                // Keep Teams' accessibility tree built between recordings.
                let _ = uia.buttons();
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// `voxtype teams-ax [--press NAME | --dump]`
pub fn probe(press: Option<String>, dump: bool) -> anyhow::Result<()> {
    let uia = Uia::new()?;
    let windows = uia.teams_windows();
    if windows.is_empty() {
        anyhow::bail!("Teams is not running (no ms-teams.exe window)");
    }
    for (title, _) in &windows {
        println!("Teams window: {title:?}");
    }
    // Chromium builds its tree for the first UI Automation client that asks.
    std::thread::sleep(std::time::Duration::from_secs(2));
    let buttons = uia.buttons();

    match press {
        None => {
            println!("\n{} button(s):", buttons.len());
            for (window, name, id, _) in &buttons {
                if dump || !name.is_empty() {
                    println!("[{window}] Button Name={name:?} AutomationId={id:?}");
                }
            }
        }
        Some(needle) => {
            let Some((window, name, _, button)) =
                buttons.iter().find(|(_, name, _, _)| *name == needle)
            else {
                anyhow::bail!("No Teams button named {needle:?}");
            };
            println!("Pressing [{window}] Button {name:?}");
            if !invoke(button) {
                anyhow::bail!("The button could not be pressed");
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
            let after = unsafe { button.CurrentName() }
                .map(|s| s.to_string())
                .unwrap_or_default();
            println!("Name after pressing: {after:?}");
        }
    }
    Ok(())
}
