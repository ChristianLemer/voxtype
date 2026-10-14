//! Installing a model from a local directory, with no network.
//!
//! Backs `voxtype setup model --from <DIR> --model <NAME>`, for provisioning a
//! machine that cannot reach Hugging Face — behind a proxy that fails closed,
//! or on a fleet where the model travels inside a kit on a shared drive.
//!
//! The point is bytes, not names. A copy that arrived half-way over a syncing
//! share keeps the right name and the right place while holding the wrong
//! contents, and a check that only looks for files would call it installed.
//! So every file is hashed:
//!
//!   * against `.voxtype-manifest.json` in the source, when the kit was sealed
//!     with [`seal`] — the strong case, which proves the source itself is
//!     intact;
//!   * and from source to destination on every copy, which proves this install
//!     did not truncate anything.
//!
//! Nothing here opens a socket, prompts, or needs a terminal, and a second run
//! over an identical destination writes nothing.

use std::path::{Path, PathBuf};

use super::manifest::{read_cached_manifest, write_cached_manifest, Manifest, ManifestFile};
use super::model::{registry_snapshot, sha256_file};

#[derive(Debug, thiserror::Error)]
pub enum LocalInstallError {
    #[error(
        "unknown model '{0}'.\n  Run `voxtype setup model --list` to see what is \
         installed, or `voxtype info engines` for the engines this binary supports."
    )]
    UnknownModel(String),

    #[error(
        "'{0}' is a single-file Whisper model, which --from does not handle yet.\n  \
         Copy it into the models directory by hand."
    )]
    NotInRegistry(String),

    #[error("source directory '{0}' does not exist")]
    NoSource(PathBuf),

    #[error(
        "'{file}' is missing from '{dir}'.\n  The source must hold every file of \
         the model: {expected}"
    )]
    MissingFile {
        file: String,
        dir: PathBuf,
        expected: String,
    },

    #[error(
        "'{file}' in the source does not match the sealed manifest.\n  \
         expected sha256 {expected}\n  found    sha256 {found}\n  \
         The copy in the kit is damaged — re-stage it."
    )]
    SourceCorrupt {
        file: String,
        expected: String,
        found: String,
    },

    #[error(
        "'{file}' did not survive the copy: sha256 {expected} became {found}.\n  \
         Nothing was installed under that name."
    )]
    CopyCorrupt {
        file: String,
        expected: String,
        found: String,
    },

    #[error("{context}: {source}")]
    Io {
        context: String,
        source: std::io::Error,
    },
}

impl LocalInstallError {
    /// 2 for anything the caller can fix (wrong name, missing or damaged
    /// source), 1 for filesystem trouble — the same split `config set` uses.
    pub fn exit_code(&self) -> i32 {
        match self {
            LocalInstallError::Io { .. } => 1,
            _ => 2,
        }
    }
}

fn io<E: Into<std::io::Error>>(context: impl Into<String>) -> impl FnOnce(E) -> LocalInstallError {
    move |e| LocalInstallError::Io {
        context: context.into(),
        source: e.into(),
    }
}

/// What an install did, so the caller can report it without guessing.
#[derive(Debug, Clone, Default)]
pub struct Installed {
    pub model: String,
    pub engine: String,
    pub dest: PathBuf,
    /// Files copied in, because the destination was absent or differed.
    pub copied: Vec<String>,
    /// Files already byte-identical at the destination, so left alone.
    pub unchanged: Vec<String>,
    /// Whether the source carried a sealed manifest, which is what lets the
    /// source itself be vouched for rather than merely copied faithfully.
    pub source_was_sealed: bool,
}

/// The engine prefix and expected file list for a model name.
fn lookup(model: &str) -> Result<(String, Vec<String>), LocalInstallError> {
    let entry = registry_snapshot()
        .into_iter()
        .find(|e| e.name == model)
        .ok_or_else(|| LocalInstallError::UnknownModel(model.to_string()))?;
    let files: Vec<String> = entry.files.iter().map(|f| f.local_path.clone()).collect();
    if files.is_empty() {
        return Err(LocalInstallError::NotInRegistry(model.to_string()));
    }
    Ok((entry.engine_prefix.to_string(), files))
}

/// Hash every file of `model` in `dir` and write `.voxtype-manifest.json`
/// beside them, so a kit staged from this directory can be verified on
/// arrival rather than merely unpacked.
///
/// Run once, on a machine where the download succeeded. Models downloaded by
/// recent voxtype versions are already sealed; older ones are not.
pub fn seal(model: &str, dir: &Path) -> Result<Manifest, LocalInstallError> {
    let (engine, expected) = lookup(model)?;
    if !dir.is_dir() {
        return Err(LocalInstallError::NoSource(dir.to_path_buf()));
    }

    let mut files = Vec::with_capacity(expected.len());
    for name in &expected {
        let path = dir.join(name);
        if !path.is_file() {
            return Err(LocalInstallError::MissingFile {
                file: name.clone(),
                dir: dir.to_path_buf(),
                expected: expected.join(", "),
            });
        }
        let size = std::fs::metadata(&path)
            .map_err(io(format!("reading {}", path.display())))?
            .len();
        let sha256 = sha256_file(&path).map_err(|e| LocalInstallError::Io {
            context: format!("hashing {}", path.display()),
            source: std::io::Error::other(e.to_string()),
        })?;
        files.push(ManifestFile {
            path: name.clone(),
            size,
            sha256,
        });
    }

    let manifest = Manifest {
        version: 1,
        model: model.to_string(),
        engine,
        files,
    };
    write_cached_manifest(dir, &manifest);
    Ok(manifest)
}

/// Install `model` from `source` into `models_dir`, verifying bytes.
pub fn install_from_dir(
    model: &str,
    source: &Path,
    models_dir: &Path,
) -> Result<Installed, LocalInstallError> {
    let (engine, expected) = lookup(model)?;
    if !source.is_dir() {
        return Err(LocalInstallError::NoSource(source.to_path_buf()));
    }

    let sealed = read_cached_manifest(source).filter(|m| m.model == model);
    let dest = models_dir.join(model);
    std::fs::create_dir_all(&dest).map_err(io(format!("creating {}", dest.display())))?;

    let mut out = Installed {
        model: model.to_string(),
        engine: engine.clone(),
        dest: dest.clone(),
        source_was_sealed: sealed.is_some(),
        ..Default::default()
    };

    let mut hashes = Vec::with_capacity(expected.len());

    for name in &expected {
        let from = source.join(name);
        if !from.is_file() {
            return Err(LocalInstallError::MissingFile {
                file: name.clone(),
                dir: source.to_path_buf(),
                expected: expected.join(", "),
            });
        }

        let source_hash = hash(&from)?;
        let size = std::fs::metadata(&from)
            .map_err(io(format!("reading {}", from.display())))?
            .len();

        // The strong check: does the source still hold what it held when the
        // kit was sealed? Only possible when a manifest travelled with it.
        if let Some(entry) = sealed
            .as_ref()
            .and_then(|m| m.files.iter().find(|f| f.path == *name))
        {
            if entry.sha256 != source_hash || entry.size != size {
                return Err(LocalInstallError::SourceCorrupt {
                    file: name.clone(),
                    expected: entry.sha256.clone(),
                    found: source_hash,
                });
            }
        }

        hashes.push(ManifestFile {
            path: name.clone(),
            size,
            sha256: source_hash.clone(),
        });

        // Already right? Leave it, and say nothing. A replay must write
        // nothing at all.
        let to = dest.join(name);
        if to.is_file() && hash(&to)? == source_hash {
            out.unchanged.push(name.clone());
            continue;
        }

        copy_verified(&from, &to, &source_hash, name)?;
        out.copied.push(name.clone());
    }

    // Record what the destination now holds, so a later check — ours or
    // anyone's — has hashes to compare against even if the source is gone.
    write_cached_manifest(
        &dest,
        &Manifest {
            version: 1,
            model: model.to_string(),
            engine,
            files: hashes,
        },
    );

    Ok(out)
}

fn hash(path: &Path) -> Result<String, LocalInstallError> {
    sha256_file(path).map_err(|e| LocalInstallError::Io {
        context: format!("hashing {}", path.display()),
        source: std::io::Error::other(e.to_string()),
    })
}

/// Copy through a `.part` file and rename only once the destination's own
/// hash matches, so an interrupted or truncated copy never appears under the
/// real name.
fn copy_verified(
    from: &Path,
    to: &Path,
    expected: &str,
    name: &str,
) -> Result<(), LocalInstallError> {
    let part = to.with_extension("voxtype-part");
    let _ = std::fs::remove_file(&part);

    std::fs::copy(from, &part).map_err(io(format!(
        "copying {} to {}",
        from.display(),
        part.display()
    )))?;

    let got = hash(&part)?;
    if got != expected {
        let _ = std::fs::remove_file(&part);
        return Err(LocalInstallError::CopyCorrupt {
            file: name.to_string(),
            expected: expected.to_string(),
            found: got,
        });
    }

    std::fs::rename(&part, to).map_err(io(format!("renaming {}", part.display())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real registry model, so the expected file list is the production one.
    const MODEL: &str = "parakeet-tdt-0.6b-v3-int8";

    fn fake_source() -> (tempfile::TempDir, PathBuf, Vec<String>) {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("kit");
        std::fs::create_dir_all(&src).unwrap();
        let (_engine, files) = lookup(MODEL).unwrap();
        for (i, name) in files.iter().enumerate() {
            if let Some(parent) = src.join(name).parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(src.join(name), format!("contents of {name} #{i}")).unwrap();
        }
        (dir, src, files)
    }

    #[test]
    fn unknown_model_is_named_and_fixable() {
        let err = lookup("not-a-model").unwrap_err();
        assert!(matches!(err, LocalInstallError::UnknownModel(_)));
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn installs_every_expected_file() {
        let (dir, src, files) = fake_source();
        let models = dir.path().join("models");
        let out = install_from_dir(MODEL, &src, &models).unwrap();

        assert_eq!(out.copied.len(), files.len());
        assert!(out.unchanged.is_empty());
        assert!(!out.source_was_sealed, "no manifest was staged");
        for name in &files {
            assert!(models.join(MODEL).join(name).is_file(), "{name} missing");
        }
    }

    #[test]
    fn a_replay_copies_nothing() {
        let (dir, src, files) = fake_source();
        let models = dir.path().join("models");
        install_from_dir(MODEL, &src, &models).unwrap();

        let again = install_from_dir(MODEL, &src, &models).unwrap();
        assert!(again.copied.is_empty(), "copied {:?}", again.copied);
        assert_eq!(again.unchanged.len(), files.len());
    }

    /// The case that motivates the whole module: a destination with the right
    /// name and the wrong bytes must be replaced, not accepted.
    #[test]
    fn a_truncated_destination_is_replaced() {
        let (dir, src, files) = fake_source();
        let models = dir.path().join("models");
        install_from_dir(MODEL, &src, &models).unwrap();

        let victim = models.join(MODEL).join(&files[0]);
        std::fs::write(&victim, b"half a file").unwrap();

        let out = install_from_dir(MODEL, &src, &models).unwrap();
        assert_eq!(out.copied, [files[0].clone()], "only the damaged one");
        assert_eq!(
            std::fs::read_to_string(&victim).unwrap(),
            std::fs::read_to_string(src.join(&files[0])).unwrap()
        );
    }

    #[test]
    fn a_missing_source_file_is_named() {
        let (dir, src, files) = fake_source();
        std::fs::remove_file(src.join(&files[0])).unwrap();
        let err = install_from_dir(MODEL, &src, &dir.path().join("models")).unwrap_err();
        match err {
            LocalInstallError::MissingFile { file, .. } => assert_eq!(file, files[0]),
            other => panic!("expected MissingFile, got {other:?}"),
        }
    }

    #[test]
    fn sealing_then_installing_vouches_for_the_source() {
        let (dir, src, files) = fake_source();
        let manifest = seal(MODEL, &src).unwrap();
        assert_eq!(manifest.files.len(), files.len());
        assert!(manifest.files.iter().all(|f| f.sha256.len() == 64));

        let out = install_from_dir(MODEL, &src, &dir.path().join("models")).unwrap();
        assert!(out.source_was_sealed, "the seal must be noticed");
    }

    /// A kit damaged *before* this machine saw it. Only a sealed manifest can
    /// catch this, and it must refuse rather than install the damage.
    #[test]
    fn a_damaged_sealed_source_is_refused() {
        let (dir, src, files) = fake_source();
        seal(MODEL, &src).unwrap();
        std::fs::write(src.join(&files[0]), b"corrupted in transit").unwrap();

        let models = dir.path().join("models");
        let err = install_from_dir(MODEL, &src, &models).unwrap_err();
        match err {
            LocalInstallError::SourceCorrupt { file, .. } => assert_eq!(file, files[0]),
            other => panic!("expected SourceCorrupt, got {other:?}"),
        }
        assert!(
            !models.join(MODEL).join(&files[0]).is_file(),
            "nothing may be installed from a damaged kit"
        );
    }

    #[test]
    fn the_destination_is_left_sealed_for_later_checks() {
        let (dir, src, _files) = fake_source();
        let models = dir.path().join("models");
        install_from_dir(MODEL, &src, &models).unwrap();
        let recorded = read_cached_manifest(&models.join(MODEL)).expect("manifest written");
        assert_eq!(recorded.model, MODEL);
        assert!(recorded.files.iter().all(|f| f.sha256.len() == 64));
    }

    #[test]
    fn no_part_files_are_left_behind() {
        let (dir, src, _files) = fake_source();
        let models = dir.path().join("models");
        install_from_dir(MODEL, &src, &models).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(models.join(MODEL))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains("voxtype-part"))
            .collect();
        assert!(leftovers.is_empty(), "left {leftovers:?}");
    }
}
