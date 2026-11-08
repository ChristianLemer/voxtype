//! Microsoft Teams' local third-party device API (new Teams, macOS and Windows).
//!
//! Teams listens on a local WebSocket once "Manage API" is enabled in its
//! privacy settings. It pushes `meetingUpdate` messages with the meeting state
//! and accepts actions such as `toggle-mute`. The first action from a new app
//! makes Teams ask the user to allow it; Teams then sends a `tokenRefresh`
//! token, which later connections pass to skip the prompt.
//!
//! There is no absolute "mute" action, only a toggle, so callers read
//! `isMuted` first and only toggle back what they toggled themselves.

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

const ENDPOINT: &str = "ws://127.0.0.1:8124";
const PROTOCOL_VERSION: &str = "2.0.0";

/// Where the pairing token is kept between runs.
fn token_path() -> Option<PathBuf> {
    crate::config::Config::config_dir().map(|d| d.join("teams-token"))
}

fn load_token() -> Option<String> {
    let token = std::fs::read_to_string(token_path()?).ok()?;
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_string())
}

fn save_token(token: &str) {
    if let Some(path) = token_path() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, token);
    }
}

fn url(token: Option<&str>) -> String {
    format!(
        "{ENDPOINT}?token={}&protocol-version={PROTOCOL_VERSION}&manufacturer=Voxtype&device=Voxtype&app=Voxtype&app-version={}",
        token.unwrap_or(""),
        env!("CARGO_PKG_VERSION"),
    )
}

/// The parts of a `meetingUpdate` that matter for muting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeetingState {
    pub in_meeting: bool,
    pub muted: bool,
    pub can_toggle_mute: bool,
}

pub fn parse_meeting_update(v: &Value) -> Option<MeetingState> {
    let update = v.get("meetingUpdate")?;
    let state = update.get("meetingState")?;
    let flag = |obj: Option<&Value>, key: &str| {
        obj.and_then(|o| o.get(key))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    Some(MeetingState {
        in_meeting: flag(Some(state), "isInMeeting"),
        muted: flag(Some(state), "isMuted"),
        can_toggle_mute: flag(update.get("meetingPermissions"), "canToggleMute"),
    })
}

fn action(name: &str, request_id: u64) -> String {
    json!({ "action": name, "parameters": {}, "requestId": request_id }).to_string()
}

/// `voxtype teams-probe`: connect, print everything Teams sends, and once in
/// a meeting with the microphone open, mute it and unmute it again.
pub async fn probe() -> anyhow::Result<()> {
    let token = load_token();
    println!(
        "Connecting to {ENDPOINT} ({} saved token)",
        if token.is_some() { "with a" } else { "no" }
    );
    let (ws, _) = tokio::time::timeout(
        Duration::from_secs(5),
        tokio_tungstenite::connect_async(url(token.as_deref())),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Teams did not answer within 5 s"))?
    .map_err(|e| {
        anyhow::anyhow!(
            "Cannot reach Teams at {ENDPOINT}: {e}\n  Is the new Teams running, with \
             Settings > Privacy > Manage API turned on?"
        )
    })?;
    println!("Connected. Join a meeting (or a test call) and keep your microphone on.");

    let (mut write, mut read) = ws.split();
    let mut request_id = 0u64;
    // 0: waiting for an open mic, 1: sent mute, 2: sent unmute, 3: done
    let mut step = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);

    loop {
        let msg = match tokio::time::timeout_at(deadline, read.next()).await {
            Err(_) => anyhow::bail!("Gave up after 5 minutes"),
            Ok(None) => anyhow::bail!("Teams closed the connection"),
            Ok(Some(msg)) => msg?,
        };
        let Message::Text(text) = msg else { continue };
        println!("<- {text}");
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            continue;
        };

        if let Some(token) = v.get("tokenRefresh").and_then(Value::as_str) {
            save_token(token);
            println!("   Paired: token saved for next time.");
        }

        let Some(state) = parse_meeting_update(&v) else {
            continue;
        };
        match step {
            0 if state.in_meeting && !state.muted => {
                request_id += 1;
                println!(
                    "-> toggle-mute (to mute). If Teams asks whether to allow Voxtype, allow it."
                );
                write
                    .send(Message::Text(action("toggle-mute", request_id)))
                    .await?;
                step = 1;
            }
            0 if state.in_meeting => println!("   Your microphone is muted: unmute it to test."),
            1 if state.muted => {
                println!("   MUTED by Voxtype. Unmuting in 3 s...");
                tokio::time::sleep(Duration::from_secs(3)).await;
                request_id += 1;
                println!("-> toggle-mute (to unmute)");
                write
                    .send(Message::Text(action("toggle-mute", request_id)))
                    .await?;
                step = 2;
            }
            2 if !state.muted => {
                println!("   UNMUTED again. The test worked.");
                step = 3;
            }
            _ => {}
        }
        if step == 3 {
            // Linger briefly so a late tokenRefresh still gets saved.
            let _ = tokio::time::timeout(Duration::from_secs(3), async {
                while let Some(Ok(Message::Text(text))) = read.next().await {
                    println!("<- {text}");
                    if let Some(token) = serde_json::from_str::<Value>(&text)
                        .ok()
                        .and_then(|v| v.get("tokenRefresh")?.as_str().map(String::from))
                    {
                        save_token(&token);
                        println!("   Paired: token saved for next time.");
                    }
                }
            })
            .await;
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_meeting_update() {
        let v: Value = serde_json::from_str(
            r#"{"meetingUpdate":{"meetingState":{"isMuted":false,"isInMeeting":true},
                "meetingPermissions":{"canToggleMute":true}}}"#,
        )
        .unwrap();
        assert_eq!(
            parse_meeting_update(&v),
            Some(MeetingState {
                in_meeting: true,
                muted: false,
                can_toggle_mute: true
            })
        );
    }

    #[test]
    fn ignores_other_messages() {
        let v: Value = serde_json::from_str(r#"{"tokenRefresh":"abc"}"#).unwrap();
        assert_eq!(parse_meeting_update(&v), None);
    }

    #[test]
    fn url_without_token_has_empty_token() {
        assert!(url(None).starts_with("ws://127.0.0.1:8124?token=&protocol-version=2.0.0"));
    }
}
