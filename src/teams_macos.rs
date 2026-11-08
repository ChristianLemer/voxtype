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
    fn AXUIElementCopyAttributeNames(
        element: AXUIElementRef,
        names: *mut core_foundation::array::CFArrayRef,
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

fn value_text(element: &CFType) -> Option<String> {
    use core_foundation::number::CFNumber;
    let v = attribute(element, "AXValue")?;
    if let Some(s) = v.downcast::<CFString>() {
        return Some(format!("{:?}", s.to_string()));
    }
    if let Some(b) = v.downcast::<CFBoolean>() {
        return Some(bool::from(b).to_string());
    }
    v.downcast::<CFNumber>()
        .and_then(|n| n.to_f64())
        .map(|n| n.to_string())
}

fn attribute_names(element: &CFType) -> Vec<String> {
    let mut names: core_foundation::array::CFArrayRef = std::ptr::null();
    let err = unsafe { AXUIElementCopyAttributeNames(element.as_CFTypeRef(), &mut names) };
    if err != 0 || names.is_null() {
        return Vec::new();
    }
    let names: CFArray = unsafe { CFArray::wrap_under_create_rule(names) };
    names
        .iter()
        .map(|n| unsafe { CFString::wrap_under_get_rule(*n as _) }.to_string())
        .collect()
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

/// Teams' application elements, with web content asked to expose itself.
fn teams_apps() -> anyhow::Result<Vec<(i32, CFType)>> {
    let pids = teams_pids();
    if pids.is_empty() {
        anyhow::bail!("Teams is not running (no MSTeams process)");
    }
    let mut apps = Vec::new();
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
        apps.push((pid, app));
    }
    // Give the web views a moment to build the tree they were just asked for.
    std::thread::sleep(std::time::Duration::from_secs(2));
    Ok(apps)
}

fn controls() -> anyhow::Result<Vec<Control>> {
    let mut out = Vec::new();
    for (pid, app) in teams_apps()? {
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

fn print_tree(
    element: &CFType,
    depth: usize,
    max_depth: usize,
    with_names: bool,
    seen: &mut usize,
) {
    if depth > max_depth || *seen > 3000 {
        return;
    }
    *seen += 1;
    let role = string_attribute(element, "AXRole").unwrap_or_default();
    let subrole = string_attribute(element, "AXSubrole")
        .map(|s| format!("/{s}"))
        .unwrap_or_default();
    let label = label_of(element).unwrap_or_default();
    let value = value_text(element)
        .map(|v| format!(" AXValue={v}"))
        .unwrap_or_default();
    let kids = children(element, "AXChildren");
    println!(
        "{}{role}{subrole} {label}{value} ({} children)",
        "  ".repeat(depth),
        kids.len()
    );
    if with_names {
        println!(
            "{}  attributes: {}",
            "  ".repeat(depth),
            attribute_names(element).join(", ")
        );
    }
    for child in &kids {
        print_tree(child, depth + 1, max_depth, with_names, seen);
    }
}

/// `voxtype teams-ax --dump`: the menu bar, and every element of the meeting
/// window(s), with their attribute names, to see where the controls live.
fn dump() -> anyhow::Result<()> {
    for (pid, app) in teams_apps()? {
        println!("== Teams process {pid}");
        println!(
            "-- application attributes: {}",
            attribute_names(&app).join(", ")
        );
        if let Some(menu_bar) = attribute(&app, "AXMenuBar") {
            println!("-- menu bar");
            let mut seen = 0;
            print_tree(&menu_bar, 0, 4, false, &mut seen);
        }
        for window in children(&app, "AXWindows") {
            let title = string_attribute(&window, "AXTitle").unwrap_or_default();
            let lower = title.to_lowercase();
            if !["meeting", "réunion", "call", "appel"]
                .iter()
                .any(|w| lower.contains(w))
            {
                println!("-- window {title:?} (skipped)");
                continue;
            }
            println!("-- window {title:?}");
            let mut seen = 0;
            print_tree(&window, 0, MAX_DEPTH, true, &mut seen);
        }
    }
    Ok(())
}

/// `voxtype teams-ax [--press LABEL | --dump]`
pub fn probe(press: Option<String>, dump_tree: bool) -> anyhow::Result<()> {
    if !crate::hotkey_macos::check_accessibility_permission() {
        anyhow::bail!(
            "Accessibility is not granted to the app running this command (the Terminal).\n  \
             Allow it in System Settings > Privacy & Security > Accessibility, then run again."
        );
    }
    if dump_tree {
        return dump();
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
