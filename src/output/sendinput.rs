//! Windows text output via the SendInput API
//!
//! Each character is injected as a Unicode keystroke (`KEYEVENTF_UNICODE`), so
//! the active keyboard layout never alters what gets typed and accents, CJK and
//! emoji need no key mapping. Newlines are sent as real Enter presses, since
//! most editors ignore a Unicode line feed.
//!
//! Keystrokes are paced one character at a time. Some targets, the Windows 11
//! Notepad among them, drop or scramble Unicode keystrokes delivered in a single
//! burst; measured on Notepad, 3 ms between characters is clean and 1 ms is not.
//!
//! Windows refuses injection into a window running at a higher integrity level
//! (User Interface Privilege Isolation): an ordinary daemon cannot type into an
//! elevated application.

use super::TextOutput;
use crate::error::OutputError;
use std::time::Duration;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    VIRTUAL_KEY, VK_RETURN,
};

/// Pause between characters when `type_delay_ms` is 0. See the module docs.
const DEFAULT_CHAR_DELAY_MS: u32 = 4;

/// SendInput-based text output for Windows
pub struct SendInputOutput {
    /// Delay between keypresses in milliseconds
    type_delay_ms: u32,
    /// Delay before typing starts in milliseconds
    pre_type_delay_ms: u32,
    /// Whether to send Enter key after output
    auto_submit: bool,
    /// Text appended after the transcription (e.g. a space)
    append_text: Option<String>,
}

impl SendInputOutput {
    pub fn new(
        type_delay_ms: u32,
        pre_type_delay_ms: u32,
        auto_submit: bool,
        append_text: Option<String>,
    ) -> Self {
        Self {
            type_delay_ms,
            pre_type_delay_ms,
            auto_submit,
            append_text,
        }
    }

    fn type_text_blocking(
        text: String,
        type_delay_ms: u32,
        auto_submit: bool,
    ) -> Result<(), OutputError> {
        let mut characters: Vec<Vec<[INPUT; 2]>> = text.chars().map(keystrokes_for).collect();
        if auto_submit {
            characters.push(vec![virtual_key(VK_RETURN)]);
        }

        let delay_ms = if type_delay_ms == 0 {
            DEFAULT_CHAR_DELAY_MS
        } else {
            type_delay_ms
        };
        let delay = Duration::from_millis(delay_ms as u64);
        for strokes in &characters {
            // A surrogate pair must reach the target as one unit.
            let flat: Vec<INPUT> = strokes.iter().flatten().copied().collect();
            send(&flat)?;
            std::thread::sleep(delay);
        }
        Ok(())
    }
}

fn keyboard_input(vk: VIRTUAL_KEY, scan: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Down/up pair for a virtual key.
fn virtual_key(vk: VIRTUAL_KEY) -> [INPUT; 2] {
    [
        keyboard_input(vk, 0, 0),
        keyboard_input(vk, 0, KEYEVENTF_KEYUP),
    ]
}

/// Down/up pairs for one character: one pair per UTF-16 unit, so characters
/// outside the BMP go out as a surrogate pair.
fn keystrokes_for(ch: char) -> Vec<[INPUT; 2]> {
    match ch {
        '\n' => vec![virtual_key(VK_RETURN)],
        '\r' => Vec::new(),
        _ => {
            let mut buf = [0u16; 2];
            ch.encode_utf16(&mut buf)
                .iter()
                .map(|&unit| {
                    [
                        keyboard_input(0, unit, KEYEVENTF_UNICODE),
                        keyboard_input(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
                    ]
                })
                .collect()
        }
    }
}

fn send(inputs: &[INPUT]) -> Result<(), OutputError> {
    if inputs.is_empty() {
        return Ok(());
    }
    // SAFETY: `inputs` is a live slice of fully initialised INPUT structs and
    // the size argument matches the element type.
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
    if sent as usize != inputs.len() {
        return Err(OutputError::InjectionFailed(format!(
            "SendInput injected {} of {} events: {}\n  \
             If the focused window runs as administrator, Windows blocks typing into it \
             from a non-elevated process.",
            sent,
            inputs.len(),
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
}

#[async_trait::async_trait]
impl TextOutput for SendInputOutput {
    async fn output(&self, text: &str) -> Result<(), OutputError> {
        if text.is_empty() {
            return Ok(());
        }

        if self.pre_type_delay_ms > 0 {
            tracing::debug!(
                "sendinput: waiting {}ms before typing",
                self.pre_type_delay_ms
            );
            tokio::time::sleep(Duration::from_millis(self.pre_type_delay_ms as u64)).await;
        }

        let mut full = text.to_string();
        if let Some(ref append) = self.append_text {
            full.push_str(append);
        }
        let chars = full.chars().count();
        tracing::debug!("sendinput: typing {} chars", chars);

        let type_delay_ms = self.type_delay_ms;
        let auto_submit = self.auto_submit;
        tokio::task::spawn_blocking(move || {
            SendInputOutput::type_text_blocking(full, type_delay_ms, auto_submit)
        })
        .await
        .map_err(|e| OutputError::InjectionFailed(format!("Task join error: {}", e)))??;

        tracing::info!("Text typed via SendInput ({} chars)", chars);
        Ok(())
    }

    async fn is_available(&self) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "sendinput (Windows native)"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unicode_units(strokes: &[[INPUT; 2]]) -> Vec<u16> {
        strokes
            .iter()
            .map(|pair| unsafe { pair[0].Anonymous.ki.wScan })
            .collect()
    }

    #[test]
    fn accented_char_is_one_unicode_keystroke() {
        let strokes = keystrokes_for('é');
        assert_eq!(unicode_units(&strokes), vec![0x00E9]);
    }

    #[test]
    fn astral_char_is_a_surrogate_pair() {
        let strokes = keystrokes_for('🎙');
        assert_eq!(unicode_units(&strokes), vec![0xD83C, 0xDF99]);
    }

    #[test]
    fn newline_is_an_enter_press_and_cr_is_dropped() {
        let strokes = keystrokes_for('\n');
        assert_eq!(strokes.len(), 1);
        assert_eq!(unsafe { strokes[0][0].Anonymous.ki.wVk }, VK_RETURN);
        assert!(keystrokes_for('\r').is_empty());
    }
}
