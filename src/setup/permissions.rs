//! What macOS currently lets this binary do.
//!
//! Backs `voxtype permissions`. Dictation needs two distinct grants, and the
//! difference matters when one is missing:
//!
//!   * **Input Monitoring** — seeing the hotkey. Without it the daemon starts,
//!     announces that it is listening, and never fires.
//!   * **Accessibility** — posting the transcribed text. Without it the
//!     transcription succeeds and the text lands nowhere.
//!
//! Both are reported by CoreGraphics preflight calls, which read the current
//! state without prompting — the property that makes this usable from an
//! unattended installer. A live event-tap probe is reported alongside them,
//! because a grant can be present and still not work: TCC binds an ad hoc
//! signed app's grant to the binary's cdhash, so the switch in System
//! Settings stays on while macOS quietly refuses the rebuilt binary. The
//! preflight answers "were we granted"; the tap answers "does it work".
//!
//! The microphone is deliberately absent: there is no non-prompting way to
//! read it without an Objective-C bridge this crate does not have, and
//! guessing would be worse than saying so.

/// What macOS reports, with the live probe beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    /// Input Monitoring: may this process observe key events?
    pub listen_event: bool,
    /// Accessibility: may this process post key events?
    pub post_event: bool,
    /// Does an event tap actually open right now?
    pub event_tap_works: bool,
}

impl Report {
    /// Is dictation able to work end to end?
    pub fn usable(&self) -> bool {
        self.listen_event && self.post_event && self.event_tap_works
    }

    /// What to do about it, or `None` when there is nothing to fix.
    pub fn remedy(&self) -> Option<&'static str> {
        if self.usable() {
            return None;
        }
        if (self.listen_event || self.post_event) && !self.event_tap_works {
            // Granted on paper, refused in practice: the signature changed.
            return Some(
                "Voxtype is listed as allowed but macOS is refusing it, which happens \
                 when the app was rebuilt: an ad hoc signature ties the grant to the \
                 exact binary. Remove Voxtype from Privacy & Security and add it back, \
                 or sign the app with a stable identity.",
            );
        }
        Some(
            "Grant access in System Settings > Privacy & Security, under both \
             Accessibility and Input Monitoring.",
        )
    }
}

#[cfg(target_os = "macos")]
pub fn check() -> Report {
    // Plain C entry points in CoreGraphics. The `Preflight` pair reads the
    // current state and never prompts, unlike their `Request` counterparts.
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightListenEventAccess() -> bool;
        fn CGPreflightPostEventAccess() -> bool;
    }

    Report {
        listen_event: unsafe { CGPreflightListenEventAccess() },
        post_event: unsafe { CGPreflightPostEventAccess() },
        event_tap_works: crate::hotkey_macos::is_accessibility_granted(),
    }
}

/// Print the report and say whether dictation can work.
///
/// Returns the process exit code: 0 when every grant is in place, 1 when one
/// is missing — a result, not a failure, so nothing goes to stderr.
#[cfg(target_os = "macos")]
pub fn report() -> i32 {
    let r = check();
    let mark = |ok: bool| if ok { "ok" } else { "DENIED" };

    println!(
        "Input Monitoring (see the hotkey):  {}",
        mark(r.listen_event)
    );
    println!("Accessibility (type the text):      {}", mark(r.post_event));
    println!(
        "Event tap opens right now:          {}",
        mark(r.event_tap_works)
    );
    println!("Microphone:                         not checkable without prompting");

    match r.remedy() {
        None => {
            println!("\nDictation can work.");
            0
        }
        Some(advice) => {
            println!("\nDictation cannot work. {}", advice);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Report;

    fn report(listen: bool, post: bool, tap: bool) -> Report {
        Report {
            listen_event: listen,
            post_event: post,
            event_tap_works: tap,
        }
    }

    #[test]
    fn all_three_means_usable() {
        assert!(report(true, true, true).usable());
        assert!(report(true, true, true).remedy().is_none());
    }

    #[test]
    fn any_missing_grant_means_unusable() {
        for (l, p, t) in [
            (false, true, true),
            (true, false, true),
            (true, true, false),
            (false, false, false),
        ] {
            let r = report(l, p, t);
            assert!(!r.usable(), "{l} {p} {t} should not be usable");
            assert!(r.remedy().is_some(), "{l} {p} {t} needs advice");
        }
    }

    /// Granted on paper but refused in practice is the signature problem, and
    /// deserves its own advice: re-granting in System Settings is the fix,
    /// not granting for the first time.
    #[test]
    fn granted_but_refused_names_the_rebuild() {
        let advice = report(true, true, false).remedy().unwrap();
        assert!(advice.contains("rebuilt"), "{advice}");

        let fresh = report(false, false, false).remedy().unwrap();
        assert!(!fresh.contains("rebuilt"), "{fresh}");
    }
}
