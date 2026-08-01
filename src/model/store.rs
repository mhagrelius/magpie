//! Reading and writing the two JSON files Magpie owns.
//!
//! Both files are small, both are rewritten whole, and both must survive the
//! machine losing power halfway through: temp file, fsync, rename. A rename
//! within a directory is atomic, so a reader either sees the whole old file or
//! the whole new one, never a truncated mixture.
//!
//! The other half of that promise is what happens when a file *is* damaged —
//! by an older version, a disk error, or a hand edit. Refusing to start is the
//! wrong answer, and so is deleting it: [`load`] moves the bad file aside and
//! reports where it went, so the queue comes back empty but the user can still
//! find what was in it.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// What happened when a file was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Read as expected.
    Loaded,
    /// There was no file. A first run, and not an error.
    Fresh,
    /// The file could not be parsed and was moved aside.
    Recovered { backup: PathBuf },
}

/// Something went wrong that the user needs to hear about.
#[derive(Debug)]
pub enum Error {
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    Encode(serde_json::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Error::Write { path, source } => write!(f, "cannot write {}: {source}", path.display()),
            Error::Encode(source) => write!(f, "cannot encode: {source}"),
        }
    }
}

impl std::error::Error for Error {}

/// Read a JSON file, falling back to a default rather than failing.
///
/// Loading never writes. A missing file is not created here, and a recovered
/// one is not replaced — the next save does that, if there is one. Anything
/// else would mean opening the preferences dialog could overwrite a config file
/// the user was in the middle of editing by hand.
pub fn load<T>(path: &Path) -> Result<(T, Outcome), Error>
where
    T: serde::de::DeserializeOwned + Default,
{
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((T::default(), Outcome::Fresh));
        }
        Err(source) => {
            return Err(Error::Read {
                path: path.to_path_buf(),
                source,
            })
        }
    };

    match serde_json::from_str(&text) {
        Ok(value) => Ok((value, Outcome::Loaded)),
        Err(_) => {
            let backup = path.with_extension("json.corrupt");
            // Best effort: if even the rename fails there is nothing useful to
            // do about it, and the caller still gets a working default.
            let _ = fs::rename(path, &backup);
            Ok((T::default(), Outcome::Recovered { backup }))
        }
    }
}

/// Write a JSON file so that it is either wholly old or wholly new.
pub fn save<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| Error::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let json = serde_json::to_vec_pretty(value).map_err(Error::Encode)?;
    let temporary = path.with_extension("json.tmp");

    let write = |path: &Path| -> std::io::Result<()> {
        let mut file = fs::File::create(path)?;
        file.write_all(&json)?;
        file.write_all(b"\n")?;
        // Without the sync, the rename can land before the bytes do, and a
        // power cut leaves a file that is intact by name and empty by content.
        file.sync_all()
    };

    write(&temporary).map_err(|source| Error::Write {
        path: temporary.clone(),
        source,
    })?;

    fs::rename(&temporary, path).map_err(|source| Error::Write {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    struct Thing {
        name: String,
        count: u32,
    }

    #[test]
    fn a_first_run_is_not_an_error_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("thing.json");

        let (thing, outcome) = load::<Thing>(&path).expect("a default");
        assert_eq!(outcome, Outcome::Fresh);
        assert_eq!(thing, Thing::default());
        assert!(!path.exists(), "loading must not create the file");
    }

    #[test]
    fn a_saved_file_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("thing.json");
        let thing = Thing {
            name: "magpie".into(),
            count: 3,
        };

        save(&path, &thing).expect("saves, creating the directory");
        let (read_back, outcome) = load::<Thing>(&path).expect("loads");
        assert_eq!(outcome, Outcome::Loaded);
        assert_eq!(read_back, thing);
    }

    #[test]
    fn a_truncated_file_is_moved_aside_rather_than_deleted() {
        // A power cut mid-write, or an older version's format. Refusing to
        // start is the wrong answer and so is silently binning the file: the
        // user gets an empty queue and a path to what was in it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("thing.json");
        fs::write(&path, r#"{"name": "half a fi"#).unwrap();

        let (thing, outcome) = load::<Thing>(&path).expect("recovers");
        assert_eq!(thing, Thing::default());
        let Outcome::Recovered { backup } = outcome else {
            panic!("expected recovery, got {outcome:?}");
        };
        assert!(
            backup.exists(),
            "the damaged file is still there to look at"
        );
        assert!(!path.exists());
    }

    #[test]
    fn no_temporary_file_is_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("thing.json");
        save(&path, &Thing::default()).unwrap();

        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn a_second_save_replaces_the_first_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("thing.json");
        save(
            &path,
            &Thing {
                name: "a-long-name".into(),
                count: 1,
            },
        )
        .unwrap();
        save(
            &path,
            &Thing {
                name: "b".into(),
                count: 2,
            },
        )
        .unwrap();

        let (thing, _) = load::<Thing>(&path).unwrap();
        assert_eq!(thing.name, "b", "no remnant of the longer first write");
    }
}
