//! Reaching Microsoft Teams' meeting controls through macOS Accessibility.
//!
//! Some organisations hide Teams' local third-party API, so this reads the
//! meeting window's buttons the way a screen reader does. `voxtype teams-ax`
//! lists them, to learn what the microphone button is called in a given
//! Teams version and language, and `--press` presses one by its label.

use core_foundation::array::CFArray;
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::string::CFString;
use std::ffi::c_void;

type AXUIElementRef = *const c_void;
type AXError = i32;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: core_foundation::string::CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: core_foundation::string::CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
    fn AXUIElementPerformAction(
        element: AXUIElementRef,
        action: core_foundation::string::CFStringRef,
    ) -> AXError;
}

const MAX_DEPTH: usize = 80;
const MAX_NODES: usize = 50_000;

fn attribute(element: &CFType, name: &str) -> Option<CFType> {
    let name = CFString::new(name);
    let mut value: CFTypeRef = std::ptr::null();
    let err = unsafe {
        AXUIElementCopyAttributeValue(
            element.as_CFTypeRef(),
            name.as_concrete_TypeRef(),
            &mut value,
        )
    };
    (err == 0 && !value.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(value) })
}

fn string_attribute(element: &CFType, name: &str) -> Option<String> {
    let s = attribute(element, name)?
        .downcast::<CFString>()?
        .to_string();
    (!s.is_empty()).then_some(s)
}

fn children(element: &CFType, name: &str) -> Vec<CFType> {
    attribute(element, name)
        .and_then(|v| v.downcast::<CFArray>())
        .map(|a| {
            a.iter()
                .map(|item| unsafe { CFType::wrap_under_get_rule(*item as CFTypeRef) })
                .collect()
        })
        .unwrap_or_default()
}

/// Process ids of the new Teams (`MSTeams`) and of classic Teams.
fn teams_pids() -> Vec<i32> {
    ["MSTeams", "Microsoft Teams"]
        .iter()
        .filter_map(|name| {
            std::process::Command::new("pgrep")
                .args(["-x", name])
                .output()
                .ok()
        })
        .flat_map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|l| l.trim().parse().ok())
                .collect::<Vec<i32>>()
        })
        .collect()
}

/// A labelled control found in a Teams window.
struct Control {
    window: String,
    role: String,
    label: String,
    element: CFType,
}

fn label_of(element: &CFType) -> Option<String> {
    let parts: Vec<String> = ["AXTitle", "AXDescription", "AXHelp", "AXIdentifier"]
        .iter()
        .filter_map(|a| string_attribute(element, a).map(|v| format!("{a}={v:?}")))
        .collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn collect(element: &CFType, window: &str, depth: usize, seen: &mut usize, out: &mut Vec<Control>) {
    if depth > MAX_DEPTH || *seen > MAX_NODES {
        return;
    }
    *seen += 1;
    let role = string_attribute(element, "AXRole").unwrap_or_default();
    if matches!(
        role.as_str(),
        "AXButton" | "AXCheckBox" | "AXMenuButton" | "AXToggle" | "AXPopUpButton"
    ) {
        if let Some(label) = label_of(element) {
            out.push(Control {
                window: window.to_string(),
                role: role.clone(),
                label,
                element: element.clone(),
            });
        }
    }
    for child in children(element, "AXChildren") {
        collect(&child, window, depth + 1, seen, out);
    }
}

fn controls() -> anyhow::Result<Vec<Control>> {
    let pids = teams_pids();
    if pids.is_empty() {
        anyhow::bail!("Teams is not running (no MSTeams process)");
    }
    let mut out = Vec::new();
    for pid in pids {
        let app = unsafe { AXUIElementCreateApplication(pid) };
        if app.is_null() {
            continue;
        }
        let app = unsafe { CFType::wrap_under_create_rule(app as CFTypeRef) };
        // Web content only builds its accessibility tree once asked to.
        for attr in ["AXManualAccessibility", "AXEnhancedUserInterface"] {
            let name = CFString::new(attr);
            unsafe {
                AXUIElementSetAttributeValue(
                    app.as_CFTypeRef(),
                    name.as_concrete_TypeRef(),
                    CFBoolean::true_value().as_CFTypeRef(),
                );
            }
        }
        let windows = children(&app, "AXWindows");
        println!("Teams process {pid}: {} window(s)", windows.len());
        for window in windows {
            let title = string_attribute(&window, "AXTitle").unwrap_or_else(|| "(untitled)".into());
            let mut seen = 0;
            collect(&window, &title, 0, &mut seen, &mut out);
            println!("  window {title:?}: {seen} elements read");
        }
    }
    Ok(out)
}

/// `voxtype teams-ax [--press LABEL]`
pub fn probe(press: Option<String>) -> anyhow::Result<()> {
    if !crate::hotkey_macos::check_accessibility_permission() {
        anyhow::bail!(
            "Accessibility is not granted to the app running this command (the Terminal).\n  \
             Allow it in System Settings > Privacy & Security > Accessibility, then run again."
        );
    }
    let found = controls()?;
    match press {
        None => {
            println!("\n{} labelled control(s):", found.len());
            for c in &found {
                println!("[{}] {} {}", c.window, c.role, c.label);
            }
        }
        Some(needle) => {
            let needle = needle.to_lowercase();
            let Some(c) = found
                .iter()
                .find(|c| c.label.to_lowercase().contains(&needle))
            else {
                anyhow::bail!("No control whose label contains {needle:?}");
            };
            println!("Pressing [{}] {} {}", c.window, c.role, c.label);
            let action = CFString::new("AXPress");
            let err = unsafe {
                AXUIElementPerformAction(c.element.as_CFTypeRef(), action.as_concrete_TypeRef())
            };
            if err != 0 {
                anyhow::bail!("AXPress failed with AXError {err}");
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
            let after = label_of(&c.element).unwrap_or_default();
            println!("Label after pressing: {after}");
        }
    }
    Ok(())
}
