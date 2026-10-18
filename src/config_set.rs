//! Programmatic mutation of the on-disk config file from the CLI.
//!
//! Backs `voxtype config set <KEY> <VALUE>` and `voxtype config unset <KEY>`.
//! These are the same operations the TUI sections perform (see `src/tui/`),
//! exposed non-interactively so external tools (the Quickshell engine picker,
//! an Omarchy settings panel, shell scripts) can change settings without
//! rendering a TUI.
//!
//! `engine` keeps its own entry point, [`set_engine`], because it predates
//! the generic path and its error messages and exit codes are part of the
//! published CLI contract.
//!
//! Validation rules mirror the TUI:
//!   1. The key must be in the [`crate::config::schema`] allowlist.
//!   2. The value must type-check and be in range for that key.
//!   3. For keys belonging to an optionally-compiled engine, the binary must
//!      have been built with the matching Cargo feature. The TUI surfaces
//!      this as a warning; the CLI treats it as a hard error since there's
//!      no interactive escape hatch.
//!
//! Comments and unrelated fields are preserved via `toml_edit` (through
//! `ConfigEditor`). Saves go through the same atomic write + validation
//! pipeline as the TUI, so a change that would stop the daemon loading is
//! rolled back instead of written.

use std::path::{Path, PathBuf};

use crate::config::schema::{self, Found, TypedValue, ValueError};
use crate::config::TranscriptionEngine;
use crate::tui::{ConfigEditor, EditorError};

#[derive(Debug, thiserror::Error)]
pub enum ConfigSetError {
    #[error(
        "unknown engine '{0}'. Valid engines: {}",
        TranscriptionEngine::names_csv()
    )]
    UnknownEngine(String),

    #[error(
        "engine '{0}' is not compiled into this binary.\n  \
         Rebuild voxtype with the matching Cargo feature:\n    \
         cargo build --release --features {0}\n  \
         Or install a prebuilt variant that includes it (see \
         `voxtype info variants`)."
    )]
    FeatureNotCompiled(String),

    #[error(
        "unknown config key '{0}'.\n  \
         Run `voxtype config schema` to list every settable key."
    )]
    UnknownKey(String),

    #[error("{0}")]
    BadValue(#[from] ValueError),

    #[error(
        "'{key}' belongs to the '{feature}' engine, which is not compiled into \
         this binary.\n  Install a variant that includes it (see \
         `voxtype info variants`) or rebuild with --features {feature}."
    )]
    KeyFeatureNotCompiled {
        key: &'static str,
        feature: &'static str,
    },

    #[error("config editor: {0}")]
    Editor(#[from] EditorError),

    #[error("cannot read '{path}': {source}")]
    InputRead {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("'{path}' is not valid TOML: {source}")]
    InputParse {
        path: PathBuf,
        source: toml::de::Error,
    },

    #[error(
        "'{key}' in the input file is an array or a date, which no config key \
         takes.\n  Run `voxtype config schema` to see each key's type."
    )]
    UnsupportedValue { key: String },
}

impl ConfigSetError {
    /// Process exit code for this failure, matching the contract in
    /// `voxtype config set --help`: 2 for anything the user can fix by
    /// changing the command, 1 for filesystem and validation failures.
    pub fn exit_code(&self) -> i32 {
        match self {
            ConfigSetError::UnknownEngine(_)
            | ConfigSetError::FeatureNotCompiled(_)
            | ConfigSetError::UnknownKey(_)
            | ConfigSetError::BadValue(_)
            | ConfigSetError::KeyFeatureNotCompiled { .. }
            | ConfigSetError::InputParse { .. }
            | ConfigSetError::UnsupportedValue { .. } => 2,
            ConfigSetError::Editor(_) | ConfigSetError::InputRead { .. } => 1,
        }
    }
}

/// Engines `config set engine` and the settings UIs offer, in
/// [`TranscriptionEngine`] declaration order. Deliberately narrower than the
/// enum: `soniox` is configured through its own `[soniox]` table and is not
/// offered by the TUI picker or `config set engine`, so it is excluded here.
/// The `engine_names_track_the_enum` test pins this list against the enum so
/// a new variant can't be silently forgotten.
pub const ENGINE_NAMES: &[&str] = &[
    "whisper",
    "parakeet",
    "moonshine",
    "sensevoice",
    "paraformer",
    "dolphin",
    "omnilingual",
    "cohere",
    "openvino",
];

/// Is the engine name one we recognize at all?
///
/// Iterates the [`TranscriptionEngine`] variants and matches the exact
/// canonical lowercase name. Case-sensitive (so callers can detect typos
/// like `"Whisper"` before applying them to config). New engine variants
/// are picked up automatically via `strum::EnumIter`.
pub fn parse_engine(name: &str) -> Option<TranscriptionEngine> {
    use strum::IntoEnumIterator;
    TranscriptionEngine::iter().find(|e| e.name() == name)
}

/// Was this binary compiled with the feature needed to run the given engine?
///
/// Whisper and Soniox are unconditional (Soniox was un-feature-gated in
/// #441); every other engine is gated on the corresponding Cargo feature.
/// This is the source-of-truth check that matches what the TUI shows on
/// source builds (see `EngineState::refresh_binary_match` in
/// `src/tui/engine.rs`). The TUI's `compiled_features()` list in
/// `src/setup/binary.rs`, so we evaluate `cfg!` directly here rather than
/// coupling validation to its user-facing labels.
///
/// Matches `TranscriptionEngine` exhaustively so adding a new variant
/// produces a compile error here, not a silent `false` at runtime. The
/// previous wildcard arm hid `soniox` from this check for several months.
pub fn engine_feature_compiled(name: &str) -> bool {
    let Some(engine) = parse_engine(name) else {
        return false;
    };
    match engine {
        TranscriptionEngine::Whisper => true,
        TranscriptionEngine::Soniox => true,
        TranscriptionEngine::Parakeet => cfg!(feature = "parakeet"),
        TranscriptionEngine::Moonshine => cfg!(feature = "moonshine"),
        TranscriptionEngine::SenseVoice => cfg!(feature = "sensevoice"),
        TranscriptionEngine::Paraformer => cfg!(feature = "paraformer"),
        TranscriptionEngine::Dolphin => cfg!(feature = "dolphin"),
        TranscriptionEngine::Omnilingual => cfg!(feature = "omnilingual"),
        TranscriptionEngine::Cohere => cfg!(feature = "cohere"),
        TranscriptionEngine::OpenVino => cfg!(feature = "openvino-whisper"),
    }
}

/// Set the active engine in the config file at `path`.
///
/// Validates the name and the compiled-feature gate before touching disk.
/// If the file doesn't exist, an empty document is created and `engine = ".."`
/// is written at the root. If it exists, `toml_edit` updates only the
/// `engine` key, preserving comments and other fields.
pub fn set_engine(path: PathBuf, name: &str) -> Result<PathBuf, ConfigSetError> {
    if parse_engine(name).is_none() {
        return Err(ConfigSetError::UnknownEngine(name.to_string()));
    }
    if !engine_feature_compiled(name) {
        return Err(ConfigSetError::FeatureNotCompiled(name.to_string()));
    }

    let mut editor = ConfigEditor::load_from_path(path)?;
    editor.set_string("", "engine", name);
    editor.save()?;
    Ok(editor.path().to_path_buf())
}

/// Outcome of a successful generic set: what was written and where.
#[derive(Debug, Clone)]
pub struct SetOutcome {
    /// The dotted key as the user would type it — for a map entry that's the
    /// concrete path (`text.replacements.btw`), not the schema placeholder.
    pub key: String,
    pub value: TypedValue,
    pub path: PathBuf,
    pub restart_required: bool,
}

/// Look up `key` in the allowlist and reject it if its engine feature is
/// missing from this build.
fn lookup(key: &str) -> Result<Found, ConfigSetError> {
    let found = schema::find_key(key).ok_or_else(|| ConfigSetError::UnknownKey(key.to_string()))?;
    let spec = found.spec();
    if let Some(feature) = spec.requires_feature {
        if !schema::feature_compiled(feature) {
            return Err(ConfigSetError::KeyFeatureNotCompiled {
                key: spec.key,
                feature,
            });
        }
    }
    Ok(found)
}

/// Set any allowlisted key in the config file at `path`.
///
/// `engine` is routed to [`set_engine`] so its long-standing error messages
/// and exit codes are unchanged.
pub fn set_key(path: PathBuf, key: &str, raw: &str) -> Result<SetOutcome, ConfigSetError> {
    if key == "engine" {
        let written = set_engine(path, raw)?;
        return Ok(SetOutcome {
            key: "engine".to_string(),
            value: TypedValue::Str(raw.to_string()),
            path: written,
            restart_required: true,
        });
    }

    let found = lookup(key)?;
    let spec = found.spec();
    let value = schema::validate_value(spec, raw)?;

    let mut editor = ConfigEditor::load_from_path(path)?;
    schema::apply(&mut editor, &found, &value);
    editor.save()?;

    Ok(SetOutcome {
        key: found.dotted_key(),
        value,
        path: editor.path().to_path_buf(),
        restart_required: spec.restart_required,
    })
}

/// Remove an allowlisted key from the config file, falling back to its
/// built-in default. Removing a key that isn't present succeeds.
pub fn unset_key(path: PathBuf, key: &str) -> Result<SetOutcome, ConfigSetError> {
    let found = lookup(key)?;
    let spec = found.spec();
    let (table, field) = found.target();

    let mut editor = ConfigEditor::load_from_path(path)?;
    editor.unset(table, field);
    editor.save()?;

    Ok(SetOutcome {
        key: found.dotted_key(),
        value: TypedValue::Str(String::new()),
        path: editor.path().to_path_buf(),
        restart_required: spec.restart_required,
    })
}

// ---------------------------------------------------------------------------
// Applying a whole file: `voxtype config apply` / `voxtype config check`
// ---------------------------------------------------------------------------
//
// These exist for unattended provisioning: a fleet tool stages a TOML file
// holding only the keys it cares about, applies it, and later asks whether the
// machine still matches. Both are deliberately silent on everything the file
// does not mention, so a user's own settings survive a deployment.
//
// Neither prompts, opens a socket, or needs a terminal: an atom runs with
// nobody at the keyboard, and behind a proxy that fails closed.

/// Flatten a parsed TOML document into (dotted key, raw value) pairs, in
/// document order, so errors name keys in the order the file lists them.
///
/// Values become the same strings `config set` takes on the command line,
/// which is what [`schema::validate_value`] checks. Nested tables recurse,
/// giving both `output.notification.show_engine_icon` and the map entries of
/// `text.replacements`.
fn flatten(
    value: &toml::Value,
    prefix: &str,
    out: &mut Vec<(String, String)>,
) -> Result<(), String> {
    let table = match value {
        toml::Value::Table(t) => t,
        _ => return Ok(()),
    };
    for (key, child) in table {
        let dotted = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        let raw = match child {
            toml::Value::Table(_) => {
                flatten(child, &dotted, out)?;
                continue;
            }
            toml::Value::String(s) => s.clone(),
            toml::Value::Integer(n) => n.to_string(),
            toml::Value::Float(f) => f.to_string(),
            toml::Value::Boolean(b) => b.to_string(),
            toml::Value::Array(_) | toml::Value::Datetime(_) => return Err(dotted),
        };
        out.push((dotted, raw));
    }
    Ok(())
}

/// What a file would change, or did change.
#[derive(Debug, Clone)]
pub struct FileOutcome {
    /// The config file acted on.
    pub path: PathBuf,
    /// Keys whose on-disk value differs from the file's, in document order.
    /// Empty means the machine already matches.
    pub differing: Vec<String>,
    /// How many keys the input file asked about, matched or not.
    pub considered: usize,
    /// Whether any differing key needs a daemon restart to take effect.
    pub restart_required: bool,
}

/// Load the config, apply every key the input file names, and report what
/// moved — without saving. `apply_file` saves the result; `check_file` throws
/// it away, which is what makes check "apply replayed without recording".
fn stage(
    config_path: PathBuf,
    input: &Path,
) -> Result<(ConfigEditor, FileOutcome), ConfigSetError> {
    let text = std::fs::read_to_string(input).map_err(|source| ConfigSetError::InputRead {
        path: input.to_path_buf(),
        source,
    })?;
    let parsed: toml::Value =
        toml::from_str(&text).map_err(|source| ConfigSetError::InputParse {
            path: input.to_path_buf(),
            source,
        })?;

    let mut desired = Vec::new();
    flatten(&parsed, "", &mut desired).map_err(|key| ConfigSetError::UnsupportedValue { key })?;

    let mut editor = ConfigEditor::load_from_path(config_path)?;
    let mut outcome = FileOutcome {
        path: editor.path().to_path_buf(),
        differing: Vec::new(),
        considered: desired.len(),
        restart_required: false,
    };

    for (key, raw) in &desired {
        // `engine` is allowlisted, but validate_value only checks the name is
        // a known engine. Writing one this binary cannot run would leave a
        // config the daemon refuses, so apply the same gate `set_engine` does.
        if key == "engine" && !engine_feature_compiled(raw) {
            return Err(ConfigSetError::FeatureNotCompiled(raw.clone()));
        }

        let found = lookup(key)?;
        let spec = found.spec();
        let value = schema::validate_value(spec, raw)?;
        let (table, field) = found.target();

        // Compare values, not their rendering. toml_edit keeps each value's
        // original decoration, so `0.50` and `0.5` differ as text while being
        // one value — and comparing the rendered form reported every key of
        // the file as changed on a replay, which is the opposite of
        // idempotent. Apply regardless: that keeps the required-sibling
        // defaults `config set` writes, and the result is thrown away unless
        // something actually differs.
        let already = match &value {
            TypedValue::Bool(b) => editor.get_bool(table, field) == Some(*b),
            TypedValue::Int(n) => editor.get_int(table, field) == Some(*n),
            TypedValue::Float(f) => editor.get_float(table, field) == Some(*f),
            TypedValue::Str(s) => editor.get_string(table, field).as_deref() == Some(s.as_str()),
        };

        schema::apply(&mut editor, &found, &value);

        if !already {
            outcome.differing.push(found.dotted_key());
            outcome.restart_required |= spec.restart_required;
        }
    }

    Ok((editor, outcome))
}

/// Merge every key of `input` into the config file, leaving the rest alone.
///
/// Idempotent: a second run writes nothing, because nothing differs. Comments
/// and unmentioned fields survive, and the single save at the end goes through
/// the same atomic write, post-save validation and rollback as `config set`.
pub fn apply_file(config_path: PathBuf, input: &Path) -> Result<FileOutcome, ConfigSetError> {
    let (mut editor, outcome) = stage(config_path, input)?;
    if !outcome.differing.is_empty() {
        editor.save()?;
    }
    Ok(outcome)
}

/// Report whether the config file already holds every key of `input`, without
/// writing anything.
pub fn check_file(config_path: PathBuf, input: &Path) -> Result<FileOutcome, ConfigSetError> {
    let (_discarded, outcome) = stage(config_path, input)?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use strum::IntoEnumIterator;

    /// ENGINE_NAMES is a deliberate subset of the enum; this pins both
    /// directions so a new TranscriptionEngine variant must either be added
    /// to the list or join the documented exclusions below.
    #[test]
    fn engine_names_track_the_enum() {
        for name in ENGINE_NAMES {
            assert!(
                parse_engine(name).is_some(),
                "{name} in ENGINE_NAMES is not a TranscriptionEngine variant"
            );
        }
        let excluded: Vec<&&str> = TranscriptionEngine::names()
            .iter()
            .filter(|n| !ENGINE_NAMES.contains(n))
            .collect();
        assert_eq!(
            excluded,
            [&"soniox"],
            "new engine variants must be added to ENGINE_NAMES or documented as excluded"
        );
    }

    fn temp_config(contents: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        (dir, path)
    }

    #[test]
    fn parse_engine_accepts_known_names() {
        for engine in TranscriptionEngine::iter() {
            let name = engine.name();
            assert!(parse_engine(name).is_some(), "should accept '{}'", name);
        }
    }

    /// Pins the user-facing error message to the enum so a new variant can't
    /// land without showing up in `voxtype config set engine <bogus>` output.
    /// Caught the post-#476 drift where `soniox` was missing from this list.
    #[test]
    fn unknown_engine_error_lists_every_variant() {
        let display = format!("{}", ConfigSetError::UnknownEngine("bogus".to_string()));
        for engine in TranscriptionEngine::iter() {
            assert!(
                display.contains(engine.name()),
                "ConfigSetError::UnknownEngine display is missing variant '{}': {}",
                engine.name(),
                display
            );
        }
    }

    #[test]
    fn parse_engine_rejects_unknown() {
        assert!(parse_engine("nope").is_none());
        assert!(parse_engine("Whisper").is_none(), "case-sensitive");
        assert!(parse_engine("").is_none());
    }

    #[test]
    fn engine_feature_whisper_always_compiled() {
        assert!(engine_feature_compiled("whisper"));
    }

    #[test]
    fn engine_feature_unknown_returns_false() {
        assert!(!engine_feature_compiled("not-a-real-engine"));
    }

    #[test]
    fn set_engine_rejects_unknown_name() {
        let (_dir, path) = temp_config("");
        let err = set_engine(path, "fakeengine").unwrap_err();
        match err {
            ConfigSetError::UnknownEngine(n) => assert_eq!(n, "fakeengine"),
            other => panic!("expected UnknownEngine, got {:?}", other),
        }
    }

    #[test]
    fn set_engine_whisper_succeeds_against_full_config() {
        // Use the production default config so load_config's strict
        // deserialization passes after the write. (A bare `engine = ...`
        // file would fail validation by design — voxtype's serde struct
        // requires every top-level table to be present.)
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, crate::config::default_config_content()).unwrap();
        let written = set_engine(path.clone(), "whisper").expect("set whisper");
        assert_eq!(written, path);
        let contents = fs::read_to_string(&path).unwrap();
        assert!(
            contents.contains("engine = \"whisper\""),
            "missing engine line in {contents:?}"
        );
    }

    #[test]
    fn set_engine_preserves_comments_and_adjacent_fields() {
        // ConfigEditor's round-trip is the exact mechanism the TUI uses;
        // verify the CLI path doesn't disturb non-engine content.
        //
        // Use the production default config (which is a complete,
        // commented TOML document) and then sprinkle a custom marker
        // comment + adjacent field we expect to survive the round-trip.
        let mut base = crate::config::default_config_content();
        // Inject a marker comment near the top so we can prove comments
        // are preserved. Insert after the first newline so it lands
        // inside the document body rather than ahead of any header.
        let marker = "\n# VOXTYPE-TEST-MARKER: keep this comment\n";
        let insert_at = base.find('\n').map(|i| i + 1).unwrap_or(0);
        base.insert_str(insert_at, marker);

        let (_dir, path) = temp_config(&base);
        // Switching to whisper is always safe regardless of feature flags.
        set_engine(path.clone(), "whisper").expect("set engine");

        let after = fs::read_to_string(&path).unwrap();
        assert!(
            after.contains("# VOXTYPE-TEST-MARKER: keep this comment"),
            "marker comment lost after round-trip: {after}"
        );
        assert!(
            after.contains("engine = \"whisper\""),
            "engine not updated: {after}"
        );
        // [hotkey] table from the default config should still be present.
        assert!(
            after.contains("[hotkey]"),
            "hotkey table lost after round-trip: {after}"
        );
    }

    #[test]
    fn set_engine_in_memory_round_trip_preserves_comments() {
        // Pure ConfigEditor exercise (no full-config validation) — proves
        // that the toml_edit mutation we perform is the comment-preserving
        // one. Mirrors `round_trip_preserves_comments` in config_editor.rs.
        let (_dir, path) =
            temp_config("# top comment\nengine = \"parakeet\"\n# trailing comment\n");
        let mut ed = crate::tui::ConfigEditor::load_from_path(path).unwrap();
        ed.set_string("", "engine", "whisper");
        // We can't call ed.save() here without a full config schema, so
        // read the document directly via get_string for the round-trip
        // check.
        assert_eq!(ed.get_string("", "engine").as_deref(), Some("whisper"));
    }

    // Engines other than whisper/parakeet aren't enumerated in the default
    // feature set, so on a default `cargo test` run they'll fail the feature
    // gate. Exercise that path with a non-whisper engine and check the
    // error variant — but only if the feature isn't enabled, otherwise the
    // engine is legitimately available and this test would be misleading.
    #[test]
    fn set_engine_rejects_uncompiled_engine() {
        // Pick the first non-whisper engine whose feature is NOT compiled
        // into this test binary. Skip the test entirely if every engine is
        // compiled in (e.g. a maximalist CI build).
        let target = TranscriptionEngine::iter()
            .map(|e| e.name())
            .find(|n| *n != "whisper" && !engine_feature_compiled(n));
        let Some(name) = target else {
            eprintln!("skipping: all engine features are compiled in this build");
            return;
        };
        let (_dir, path) = temp_config("");
        let err = set_engine(path, name).unwrap_err();
        match err {
            ConfigSetError::FeatureNotCompiled(n) => assert_eq!(n, name),
            other => panic!("expected FeatureNotCompiled, got {:?}", other),
        }
    }

    // ---------------------------------------------------------------------
    // Generic set/unset
    // ---------------------------------------------------------------------

    fn full_config() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, crate::config::default_config_content()).unwrap();
        (dir, path)
    }

    fn reload(path: &std::path::Path) -> crate::config::Config {
        crate::config::load_config(Some(path)).expect("reload")
    }

    #[test]
    fn set_key_writes_each_scalar_type() {
        let (_dir, path) = full_config();

        set_key(path.clone(), "hotkey.enabled", "false").unwrap();
        set_key(path.clone(), "audio.max_duration_secs", "120").unwrap();
        set_key(path.clone(), "audio.feedback.volume", "0.25").unwrap();
        set_key(path.clone(), "output.mode", "clipboard").unwrap();
        set_key(path.clone(), "whisper.initial_prompt", "Voxtype, Omarchy").unwrap();

        let cfg = reload(&path);
        assert!(!cfg.hotkey.enabled);
        assert_eq!(cfg.audio.max_duration_secs, 120);
        assert_eq!(cfg.audio.feedback.volume, 0.25);
        assert_eq!(cfg.output.mode, crate::config::OutputMode::Clipboard);
        assert_eq!(
            cfg.whisper.initial_prompt.as_deref(),
            Some("Voxtype, Omarchy")
        );
    }

    /// The OSD styling keys (#501) must be reachable through `config set`,
    /// not just by hand-editing the TOML: a style package path, the
    /// development plugin_path (set and unset), and the nested [osd.frame]
    /// table.
    #[test]
    fn set_key_covers_the_osd_styling_keys() {
        let (_dir, path) = full_config();

        set_key(path.clone(), "osd.style", "~/.config/voxtype/osd/aegis-hud").unwrap();
        set_key(path.clone(), "osd.plugin_path", "~/dev/my-style").unwrap();
        set_key(path.clone(), "osd.frame.border", "accent").unwrap();
        set_key(path.clone(), "osd.frame.glow", "false").unwrap();

        let cfg = reload(&path);
        assert_eq!(cfg.osd.style, "~/.config/voxtype/osd/aegis-hud");
        assert_eq!(
            cfg.osd.plugin_path.as_deref(),
            Some(std::path::Path::new("~/dev/my-style"))
        );
        assert_eq!(cfg.osd.frame.border, "accent");
        assert!(!cfg.osd.frame.glow);

        unset_key(path.clone(), "osd.plugin_path").unwrap();
        let cfg = reload(&path);
        assert_eq!(
            cfg.osd.plugin_path, None,
            "unset must fall back to the serde default"
        );
    }

    #[test]
    fn set_key_reports_the_canonical_key_and_path() {
        let (_dir, path) = full_config();
        let out = set_key(path.clone(), "vad.threshold", "0.75").unwrap();
        assert_eq!(out.key, "vad.threshold");
        assert_eq!(out.path, path);
        assert!(out.restart_required);
        assert_eq!(out.value, TypedValue::Float(0.75));
    }

    /// Floats must not be written as quoted strings (#451) — the daemon
    /// refuses to load its own output if they are.
    #[test]
    fn set_key_writes_floats_as_toml_numbers() {
        let (_dir, path) = full_config();
        set_key(path.clone(), "vad.threshold", "0.25").unwrap();
        set_key(path.clone(), "audio.feedback.volume", "0.5").unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("threshold = 0.25"), "{}", text);
        assert!(text.contains("volume = 0.5"), "{}", text);
        assert!(!text.contains("threshold = \"0.25\""));
        assert!(!text.contains("volume = \"0.5\""));
    }

    #[test]
    fn set_key_rejects_unknown_keys() {
        let (_dir, path) = full_config();
        let err = set_key(path, "hotkey.nonexistent", "x").unwrap_err();
        assert!(matches!(err, ConfigSetError::UnknownKey(_)));
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn set_key_rejects_bad_values() {
        let (_dir, path) = full_config();
        for (key, value) in [
            ("hotkey.enabled", "maybe"),
            ("audio.feedback.volume", "11"),
            ("output.mode", "telepathy"),
            ("audio.max_duration_secs", "notanumber"),
        ] {
            let err = set_key(path.clone(), key, value).unwrap_err();
            assert!(
                matches!(err, ConfigSetError::BadValue(_)),
                "{} = {} gave {:?}",
                key,
                value,
                err
            );
            assert_eq!(err.exit_code(), 2);
        }
    }

    /// A rejected set must not touch the file.
    #[test]
    fn rejected_set_leaves_the_file_alone() {
        let (_dir, path) = full_config();
        let before = fs::read_to_string(&path).unwrap();
        assert!(set_key(path.clone(), "output.mode", "telepathy").is_err());
        assert!(set_key(path.clone(), "no.such.key", "1").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn set_key_rejects_keys_of_uncompiled_engines() {
        let target = ENGINE_NAMES
            .iter()
            .find(|n| **n != "whisper" && !engine_feature_compiled(n));
        let Some(engine) = target else {
            eprintln!("skipping: all engine features are compiled in this build");
            return;
        };
        let (_dir, path) = full_config();
        let key = format!("{}.on_demand_loading", engine);
        let err = set_key(path, &key, "true").unwrap_err();
        match err {
            ConfigSetError::KeyFeatureNotCompiled { feature, .. } => {
                assert_eq!(feature, *engine)
            }
            other => panic!("expected KeyFeatureNotCompiled, got {:?}", other),
        }
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn set_key_routes_engine_through_set_engine() {
        let (_dir, path) = full_config();
        let out = set_key(path.clone(), "engine", "whisper").unwrap();
        assert_eq!(out.key, "engine");
        assert!(fs::read_to_string(&path)
            .unwrap()
            .contains("engine = \"whisper\""));

        // Same errors as the dedicated subcommand used to produce.
        let err = set_key(path.clone(), "engine", "fakeengine").unwrap_err();
        assert!(matches!(err, ConfigSetError::UnknownEngine(_)));
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn unset_key_restores_the_default() {
        let (_dir, path) = full_config();
        set_key(path.clone(), "hotkey.mode", "toggle").unwrap();
        assert_eq!(
            reload(&path).hotkey.mode,
            crate::config::ActivationMode::Toggle
        );

        unset_key(path.clone(), "hotkey.mode").unwrap();
        assert_eq!(
            reload(&path).hotkey.mode,
            crate::config::ActivationMode::PushToTalk,
            "unset should fall back to the built-in default"
        );
    }

    #[test]
    fn unset_key_is_idempotent() {
        let (_dir, path) = full_config();
        unset_key(path.clone(), "whisper.initial_prompt").unwrap();
        unset_key(path.clone(), "whisper.initial_prompt").unwrap();
        assert!(reload(&path).whisper.initial_prompt.is_none());
    }

    #[test]
    fn unset_key_rejects_unknown_keys() {
        let (_dir, path) = full_config();
        let err = unset_key(path, "nope.nope").unwrap_err();
        assert!(matches!(err, ConfigSetError::UnknownKey(_)));
    }

    #[test]
    fn replacements_map_entries_set_and_unset() {
        let (_dir, path) = full_config();
        set_key(path.clone(), "text.replacements.btw", "by the way").unwrap();
        set_key(path.clone(), "text.replacements.omw", "on my way").unwrap();

        let cfg = reload(&path);
        assert_eq!(
            cfg.text.replacements.get("btw").map(String::as_str),
            Some("by the way")
        );
        assert_eq!(
            cfg.text.replacements.get("omw").map(String::as_str),
            Some("on my way")
        );

        unset_key(path.clone(), "text.replacements.btw").unwrap();
        let cfg = reload(&path);
        assert!(!cfg.text.replacements.contains_key("btw"));
        assert!(
            cfg.text.replacements.contains_key("omw"),
            "unsetting one entry must not disturb the others"
        );
    }

    #[test]
    fn set_key_preserves_comments() {
        let mut base = crate::config::default_config_content();
        let marker = "\n# VOXTYPE-TEST-MARKER: keep this comment\n";
        let at = base.find('\n').map(|i| i + 1).unwrap_or(0);
        base.insert_str(at, marker);
        let (_dir, path) = temp_config(&base);

        set_key(path.clone(), "osd.position", "top-right").unwrap();
        let after = fs::read_to_string(&path).unwrap();
        assert!(after.contains("# VOXTYPE-TEST-MARKER: keep this comment"));
        assert!(after.contains("position = \"top-right\""));
    }

    #[test]
    fn set_key_creates_missing_tables() {
        // A user config with only [hotkey] must still accept a key in a
        // table that isn't there yet.
        let (_dir, path) = temp_config("[hotkey]\nkey = \"HOME\"\n");
        set_key(path.clone(), "osd.opacity", "0.5").unwrap();
        let cfg = reload(&path);
        assert_eq!(cfg.osd.opacity, 0.5);
        assert_eq!(cfg.hotkey.key, "HOME", "existing settings must survive");
    }

    // ---------------------------------------------------------------------
    // apply / check over a whole file
    // ---------------------------------------------------------------------

    fn kit(contents: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kit.toml");
        fs::write(&path, contents).unwrap();
        (dir, path)
    }

    #[test]
    fn apply_writes_only_the_keys_the_file_names() {
        let (_d, cfg_path) = full_config();
        let (_k, kit_path) = kit("[hotkey]\nmode = \"toggle\"\n");

        // A setting the kit says nothing about, which must survive.
        set_key(cfg_path.clone(), "audio.max_duration_secs", "99").unwrap();

        let out = apply_file(cfg_path.clone(), &kit_path).unwrap();
        assert_eq!(out.differing, ["hotkey.mode"]);
        assert_eq!(out.considered, 1);

        let cfg = reload(&cfg_path);
        assert_eq!(cfg.hotkey.mode, crate::config::ActivationMode::Toggle);
        assert_eq!(
            cfg.audio.max_duration_secs, 99,
            "a key the kit never mentions must be left alone"
        );
    }

    #[test]
    fn apply_is_idempotent_and_leaves_the_file_byte_identical() {
        let (_d, cfg_path) = full_config();
        let (_k, kit_path) = kit(
            "engine = \"whisper\"\n[hotkey]\nmode = \"toggle\"\n[audio]\nmax_duration_secs = 120\n",
        );

        let first = apply_file(cfg_path.clone(), &kit_path).unwrap();
        assert_eq!(first.differing.len(), 3);
        let after_first = fs::read_to_string(&cfg_path).unwrap();

        let second = apply_file(cfg_path.clone(), &kit_path).unwrap();
        assert!(
            second.differing.is_empty(),
            "second run changed {:?}",
            second.differing
        );
        assert_eq!(
            fs::read_to_string(&cfg_path).unwrap(),
            after_first,
            "a replay must not rewrite the file"
        );
    }

    /// Saul's rule for Talos: `check` is `apply` replayed without recording.
    #[test]
    fn check_agrees_with_apply_and_never_writes() {
        let (_d, cfg_path) = full_config();
        let (_k, kit_path) = kit("[hotkey]\nmode = \"toggle\"\n");

        let before = fs::read_to_string(&cfg_path).unwrap();
        let checked = check_file(cfg_path.clone(), &kit_path).unwrap();
        assert_eq!(checked.differing, ["hotkey.mode"]);
        assert_eq!(
            fs::read_to_string(&cfg_path).unwrap(),
            before,
            "check must not touch the file"
        );

        let applied = apply_file(cfg_path.clone(), &kit_path).unwrap();
        assert_eq!(
            applied.differing, checked.differing,
            "check must predict exactly what apply does"
        );

        let after = check_file(cfg_path.clone(), &kit_path).unwrap();
        assert!(after.differing.is_empty(), "check must pass once applied");
    }

    #[test]
    fn apply_preserves_comments() {
        let mut base = crate::config::default_config_content();
        let marker = "\n# VOXTYPE-TEST-MARKER: keep me\n";
        let at = base.find('\n').map(|i| i + 1).unwrap_or(0);
        base.insert_str(at, marker);
        let (_d, cfg_path) = temp_config(&base);
        let (_k, kit_path) = kit("[osd]\nposition = \"top-right\"\n");

        apply_file(cfg_path.clone(), &kit_path).unwrap();
        let after = fs::read_to_string(&cfg_path).unwrap();
        assert!(after.contains("# VOXTYPE-TEST-MARKER: keep me"));
        assert!(after.contains("position = \"top-right\""));
    }

    #[test]
    fn nested_tables_and_map_entries_both_resolve() {
        let (_d, cfg_path) = full_config();
        let (_k, kit_path) = kit(
            "[output.notification]\nshow_engine_icon = false\n\n[text.replacements]\nbtw = \"by the way\"\n",
        );
        let out = apply_file(cfg_path.clone(), &kit_path).unwrap();
        assert_eq!(out.considered, 2, "both leaves must be seen");

        let cfg = reload(&cfg_path);
        assert!(!cfg.output.notification.show_engine_icon);
        assert_eq!(
            cfg.text.replacements.get("btw").map(String::as_str),
            Some("by the way")
        );
    }

    /// Equivalent numbers are one value: check must not demand a rewrite it
    /// would not perform.
    #[test]
    fn equivalent_numbers_do_not_count_as_a_difference() {
        let (_d, cfg_path) = full_config();
        set_key(cfg_path.clone(), "audio.feedback.volume", "0.5").unwrap();
        let (_k, kit_path) = kit("[audio.feedback]\nvolume = 0.50\n");
        let out = check_file(cfg_path.clone(), &kit_path).unwrap();
        assert!(out.differing.is_empty(), "0.50 and 0.5 are the same value");
    }

    #[test]
    fn a_bad_key_or_value_writes_nothing() {
        let (_d, cfg_path) = full_config();
        let before = fs::read_to_string(&cfg_path).unwrap();

        for (contents, label) in [
            (
                "[hotkey]\nmode = \"toggle\"\n[nope]\nwhat = 1\n",
                "unknown key",
            ),
            ("[hotkey]\nmode = \"sideways\"\n", "invalid value"),
        ] {
            let (_k, kit_path) = kit(contents);
            let err = apply_file(cfg_path.clone(), &kit_path).unwrap_err();
            assert_eq!(err.exit_code(), 2, "{label} should be a user-fixable error");
            assert_eq!(
                fs::read_to_string(&cfg_path).unwrap(),
                before,
                "{label} must leave the config untouched"
            );
        }
    }

    #[test]
    fn malformed_toml_is_reported_as_fixable() {
        let (_d, cfg_path) = full_config();
        let (_k, kit_path) = kit("this is not toml = = =\n");
        let err = apply_file(cfg_path, &kit_path).unwrap_err();
        assert!(matches!(err, ConfigSetError::InputParse { .. }));
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn a_missing_input_file_is_an_io_error() {
        let (_d, cfg_path) = full_config();
        let err = apply_file(cfg_path, std::path::Path::new("/nonexistent/kit.toml")).unwrap_err();
        assert!(matches!(err, ConfigSetError::InputRead { .. }));
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn arrays_are_rejected_by_name() {
        let (_d, cfg_path) = full_config();
        let (_k, kit_path) = kit("[output]\ndriver_order = [\"wtype\"]\n");
        let err = apply_file(cfg_path, &kit_path).unwrap_err();
        match err {
            ConfigSetError::UnsupportedValue { key } => assert_eq!(key, "output.driver_order"),
            other => panic!("expected UnsupportedValue, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_file_is_a_no_op() {
        let (_d, cfg_path) = full_config();
        let before = fs::read_to_string(&cfg_path).unwrap();
        let (_k, kit_path) = kit("# nothing to say\n");
        let out = apply_file(cfg_path.clone(), &kit_path).unwrap();
        assert_eq!(out.considered, 0);
        assert!(out.differing.is_empty());
        assert_eq!(fs::read_to_string(&cfg_path).unwrap(), before);
    }

    #[test]
    fn apply_reports_whether_a_restart_is_needed() {
        let (_d, cfg_path) = full_config();
        let (_k, kit_path) = kit("[hotkey]\nmode = \"toggle\"\n");
        let out = apply_file(cfg_path, &kit_path).unwrap();
        assert!(out.restart_required, "hotkey.mode needs a restart");
    }
}
