//! `voxtype config set` / `voxtype config unset` — small dispatchers over
//! `config_set`.
//!
//! Every write goes through `voxtype::config_set`, which shares the TUI's
//! `ConfigEditor` (atomic write, post-save validation, rollback). This file
//! only resolves the target path, prints the outcome, and maps errors onto
//! exit codes.

use std::path::PathBuf;
use voxtype::{config, config_set};

/// Resolve the config file path the same way the daemon does — honoring
/// `--config <FILE>` first, then the existing user/system path, then the
/// XDG default. The default path is used even when nothing is on disk so
/// the file gets created in a predictable location on first write.
pub(crate) fn resolve_config_path_for_write(
    cli_override: Option<PathBuf>,
) -> anyhow::Result<PathBuf> {
    if let Some(p) = cli_override {
        return Ok(p);
    }
    if let Some(p) = config::Config::resolve_existing_path() {
        return Ok(p);
    }
    config::Config::default_path().ok_or_else(|| {
        anyhow::anyhow!(
            "Cannot determine config path. Set $XDG_CONFIG_HOME or $HOME, \
             or pass --config <FILE>."
        )
    })
}

/// Print the error on stderr and exit with the code its variant maps to
/// (2 for anything the user can fix in the command, 1 for I/O), matching
/// the contract documented in `voxtype config set --help`.
fn fail(e: config_set::ConfigSetError) -> ! {
    eprintln!("error: {}", e);
    std::process::exit(e.exit_code());
}

fn restart_hint(restart_required: bool) {
    if restart_required {
        // The way to restart differs per system; systemctl means nothing on
        // a Mac or a Windows box, and a deployment log is read by people.
        #[cfg(target_os = "linux")]
        println!("Restart voxtype to apply: systemctl --user restart voxtype");
        #[cfg(target_os = "macos")]
        println!("Restart voxtype to apply: quit Voxtype from the menu bar and reopen it");
        #[cfg(windows)]
        println!(
            "Restart voxtype to apply: quit it from the notification area and run `voxtype start`"
        );
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        println!("Restart voxtype to apply.");
    }
}

/// Dispatcher for `voxtype config set <KEY> <VALUE>`.
pub(crate) fn run_config_set(
    cli_override: Option<PathBuf>,
    key: &str,
    value: &str,
) -> anyhow::Result<()> {
    let path = resolve_config_path_for_write(cli_override)?;
    match config_set::set_key(path, key, value) {
        Ok(out) => {
            println!(
                "Set {} = {} in {}",
                out.key,
                out.value.display(),
                out.path.display()
            );
            restart_hint(out.restart_required);
            Ok(())
        }
        Err(e) => fail(e),
    }
}

/// Dispatcher for `voxtype config apply <FILE>`.
pub(crate) fn run_config_apply(
    cli_override: Option<PathBuf>,
    file: &std::path::Path,
) -> anyhow::Result<()> {
    let path = resolve_config_path_for_write(cli_override)?;
    match config_set::apply_file(path, file) {
        Ok(out) => {
            if out.differing.is_empty() {
                println!(
                    "Already up to date: {} key(s) match in {}",
                    out.considered,
                    out.path.display()
                );
            } else {
                for key in &out.differing {
                    println!("Set {}", key);
                }
                println!(
                    "Applied {} of {} key(s) to {}",
                    out.differing.len(),
                    out.considered,
                    out.path.display()
                );
                restart_hint(out.restart_required);
            }
            Ok(())
        }
        Err(e) => fail(e),
    }
}

/// Dispatcher for `voxtype config check <FILE>`.
///
/// Exits 1 on divergence. That is a result, not a failure, so it is reported
/// on stdout and the process exits quietly rather than printing `error:`.
pub(crate) fn run_config_check(
    cli_override: Option<PathBuf>,
    file: &std::path::Path,
) -> anyhow::Result<()> {
    let path = resolve_config_path_for_write(cli_override)?;
    match config_set::check_file(path, file) {
        Ok(out) if out.differing.is_empty() => {
            println!(
                "Matches: {} key(s) as expected in {}",
                out.considered,
                out.path.display()
            );
            Ok(())
        }
        Ok(out) => {
            for key in &out.differing {
                println!("Differs: {}", key);
            }
            println!(
                "{} of {} key(s) differ in {}",
                out.differing.len(),
                out.considered,
                out.path.display()
            );
            std::process::exit(1);
        }
        Err(e) => fail(e),
    }
}

/// Dispatcher for `voxtype config unset <KEY>`.
pub(crate) fn run_config_unset(cli_override: Option<PathBuf>, key: &str) -> anyhow::Result<()> {
    let path = resolve_config_path_for_write(cli_override)?;
    match config_set::unset_key(path, key) {
        Ok(out) => {
            println!("Unset {} in {}", out.key, out.path.display());
            restart_hint(out.restart_required);
            Ok(())
        }
        Err(e) => fail(e),
    }
}

/// Dispatcher for `voxtype setup model --from <DIR> --model <NAME>`.
pub(crate) fn run_model_install(model: &str, from: &std::path::Path) -> anyhow::Result<()> {
    use voxtype::setup::local_install;

    let models_dir = config::Config::models_dir();
    match local_install::install_from_dir(model, from, &models_dir) {
        Ok(out) => {
            if out.copied.is_empty() {
                println!(
                    "Already installed: {} file(s) of {} match in {}",
                    out.unchanged.len(),
                    out.model,
                    out.dest.display()
                );
            } else {
                for name in &out.copied {
                    println!("Installed {}", name);
                }
                println!(
                    "Installed {} of {} file(s) of {} into {}",
                    out.copied.len(),
                    out.copied.len() + out.unchanged.len(),
                    out.model,
                    out.dest.display()
                );
            }
            // Say which guarantee was actually obtained, rather than letting
            // "verified" stand for two different strengths of evidence.
            if !out.source_was_sealed {
                println!(
                    "Note: the source carries no sealed manifest, so the copy was \
                     verified against it but the source itself could not be \
                     vouched for. Seal the kit with `voxtype setup model --seal`."
                );
            }
            Ok(())
        }
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(e.exit_code());
        }
    }
}

/// Dispatcher for `voxtype setup model --seal <DIR> --model <NAME>`.
pub(crate) fn run_model_seal(model: &str, dir: &std::path::Path) -> anyhow::Result<()> {
    use voxtype::setup::local_install;

    match local_install::seal(model, dir) {
        Ok(manifest) => {
            let total: u64 = manifest.files.iter().map(|f| f.size).sum();
            println!(
                "Sealed {} file(s), {:.1} MB, in {}",
                manifest.files.len(),
                total as f64 / 1_048_576.0,
                dir.display()
            );
            Ok(())
        }
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(e.exit_code());
        }
    }
}

/// Dispatcher for `voxtype setup model --from <DIR> --model <NAME> --check`.
///
/// Exits 1 on divergence. That is a result, not a failure, so it goes to
/// stdout and the process exits quietly rather than printing `error:`.
pub(crate) fn run_model_check(model: &str, from: &std::path::Path) -> anyhow::Result<()> {
    use voxtype::setup::local_install;

    let models_dir = config::Config::models_dir();
    match local_install::check_from_dir(model, from, &models_dir) {
        Ok(out) if out.copied.is_empty() => {
            println!(
                "Matches: {} file(s) of {} as expected in {}",
                out.unchanged.len(),
                out.model,
                out.dest.display()
            );
            Ok(())
        }
        Ok(out) => {
            for name in &out.copied {
                println!("Differs: {}", name);
            }
            println!(
                "{} of {} file(s) of {} differ in {}",
                out.copied.len(),
                out.copied.len() + out.unchanged.len(),
                out.model,
                out.dest.display()
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(e.exit_code());
        }
    }
}
