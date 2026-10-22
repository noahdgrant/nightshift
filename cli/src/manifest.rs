//! The repository's `.nightshift/nightshift.toml`. `ns` reads and changes only the keys it knows,
//! and leaves the rest, comments included, as it found them.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use toml_edit::{value, Array, DocumentMut};

use crate::harness::Kind;
use crate::merge::HumanReview;

fn file_find(root: &Path) -> PathBuf {
    root.join(".nightshift").join("nightshift.toml")
}

fn document_read(root: &Path) -> Result<DocumentMut> {
    let file = file_find(root);
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", file.display())),
    };
    text.parse()
        .with_context(|| format!("{} is not valid TOML", file.display()))
}

/// What the repository says about how it uses nightshift.
#[derive(Debug, PartialEq, Eq)]
pub struct Pin {
    /// The version of `ns` the repository uses. `None` before `ns init`.
    pub version: Option<String>,
    /// The harnesses whose skills the repository wants. `claude` when it names none.
    pub harness: Vec<Kind>,
}

pub fn pin_read(root: &Path) -> Result<Pin> {
    Ok(Pin {
        version: version_read(root)?,
        harness: harness_read(root)?,
    })
}

/// The version the repository is pinned to, without looking at its harnesses.
pub fn version_read(root: &Path) -> Result<Option<String>> {
    document_read(root)?
        .get("version")
        .map(|item| {
            item.as_str()
                .context("`version` in nightshift.toml is not a string")
                .map(str::to_string)
        })
        .transpose()
}

/// The harnesses the repository names, without looking at its version.
pub fn harness_read(root: &Path) -> Result<Vec<Kind>> {
    let document = document_read(root)?;
    match document.get("harness") {
        None => Ok(vec![Kind::Claude]),
        Some(item) => item
            .as_array()
            .context("`harness` in nightshift.toml is not a list")?
            .iter()
            .map(|name| Kind::parse(name.as_str().context("a harness name is not a string")?))
            .collect(),
    }
}

/// The files whose change a person must merge: `[merge] human_review`, or the built-in default
/// when the key is missing. An empty list means every change may be merged.
pub fn human_review_read(root: &Path) -> Result<HumanReview> {
    let document = document_read(root)?;
    let Some(item) = document.get("merge").and_then(|m| m.get("human_review")) else {
        return Ok(HumanReview::default_globs());
    };
    let globs = item
        .as_array()
        .context("`merge.human_review` in nightshift.toml is not a list")?
        .iter()
        .map(|g| {
            g.as_str()
                .map(str::to_string)
                .context("a `merge.human_review` glob is not a string")
        })
        .collect::<Result<_>>()?;
    Ok(HumanReview::new(globs))
}

/// How many times a state may run before the unit is stuck, when the person gives no number.
const MAX_ATTEMPTS_DEFAULT: u32 = 3;

/// The attempts a state gets: `[run] max_attempts`, or the built-in default when the key is
/// missing. A value that is not a whole number of at least 1 is an error.
pub fn max_attempts_read(root: &Path) -> Result<u32> {
    let document = document_read(root)?;
    let Some(item) = document.get("run").and_then(|r| r.get("max_attempts")) else {
        return Ok(MAX_ATTEMPTS_DEFAULT);
    };
    item.as_integer()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|&n| n >= 1)
        .with_context(|| {
            format!(
                "`run.max_attempts` in {} is not a whole number of at least 1",
                file_find(root).display()
            )
        })
}

/// How many days a transcript is kept, when the person gives no number.
const TRANSCRIPT_DAYS_DEFAULT: u32 = 7;

/// How long `ns` keeps a transcript.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    Forever,
    Days(u32),
}

/// The transcripts' keep time: `[run] transcript_days`, or the built-in default when the key is
/// missing. `0` is for ever. A value that is not a whole number of at least 0 is an error.
pub fn transcript_days_read(root: &Path) -> Result<Keep> {
    let document = document_read(root)?;
    let Some(item) = document.get("run").and_then(|r| r.get("transcript_days")) else {
        return Ok(Keep::Days(TRANSCRIPT_DAYS_DEFAULT));
    };
    item.as_integer()
        .and_then(|n| u32::try_from(n).ok())
        .map(|n| if n == 0 { Keep::Forever } else { Keep::Days(n) })
        .with_context(|| {
            format!(
                "`run.transcript_days` in {} is not a whole number of at least 0",
                file_find(root).display()
            )
        })
}

/// Records the version of `ns` the repository uses, and its harnesses. A `[skills]` table from an
/// `ns` that copied skills into the repository is dropped.
pub fn pin_write(root: &Path, pin: &Pin) -> Result<()> {
    let mut document = document_read(root)?;
    if let Some(version) = &pin.version {
        document.insert("version", value(version));
    }
    let names: Array = pin.harness.iter().map(|kind| kind.name()).collect();
    document.insert("harness", value(names));
    document.remove("skills");
    let file = file_find(root);
    std::fs::create_dir_all(file.parent().context("the manifest has no directory")?)?;
    std::fs::write(&file, document.to_string())
        .with_context(|| format!("cannot write {}", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin(version: &str) -> Pin {
        Pin {
            version: Some(version.into()),
            harness: vec![Kind::Claude],
        }
    }

    #[test]
    fn the_pin_is_recorded_and_the_rest_of_the_file_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".nightshift")).unwrap();
        let original = "# my factory\nname = \"mine\"\n\n[queue]\nsource = \"github\" # why\n";
        std::fs::write(file_find(tmp.path()), original).unwrap();
        pin_write(tmp.path(), &pin("0.2.0")).unwrap();
        let text = std::fs::read_to_string(file_find(tmp.path())).unwrap();
        assert!(text.contains("# my factory"), "{text}");
        assert!(
            text.contains("[queue]\nsource = \"github\" # why"),
            "{text}"
        );
        assert!(text.contains("version = \"0.2.0\""), "{text}");
        assert!(text.contains("harness = [\"claude\"]"), "{text}");
        assert_eq!(pin_read(tmp.path()).unwrap(), pin("0.2.0"));
    }

    #[test]
    fn a_repository_with_no_manifest_has_no_version_and_uses_claude() {
        let tmp = tempfile::tempdir().unwrap();
        let read = pin_read(tmp.path()).unwrap();
        assert_eq!(read.version, None);
        assert_eq!(read.harness, vec![Kind::Claude]);
    }

    #[test]
    fn writing_the_pin_drops_the_skills_table_an_older_ns_left() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".nightshift")).unwrap();
        std::fs::write(
            file_find(tmp.path()),
            "[skills]\nversion = \"0.1.0\"\nns-build = \"sha256:x\"\n",
        )
        .unwrap();
        pin_write(tmp.path(), &pin("0.2.0")).unwrap();
        let text = std::fs::read_to_string(file_find(tmp.path())).unwrap();
        assert!(
            !text.contains("[skills]") && !text.contains("sha256"),
            "{text}"
        );
    }

    #[test]
    fn human_review_is_the_default_unless_the_key_is_set() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            human_review_read(tmp.path()).unwrap(),
            HumanReview::default_globs()
        );
        std::fs::create_dir_all(tmp.path().join(".nightshift")).unwrap();
        std::fs::write(
            file_find(tmp.path()),
            "[merge]\nhuman_review = [\"a/**\"]\n",
        )
        .unwrap();
        assert_eq!(
            human_review_read(tmp.path()).unwrap(),
            HumanReview::new(vec!["a/**".into()])
        );
        std::fs::write(file_find(tmp.path()), "[merge]\nhuman_review = []\n").unwrap();
        assert_eq!(
            human_review_read(tmp.path()).unwrap(),
            HumanReview::new(Vec::new())
        );
    }

    #[test]
    fn human_review_that_is_not_a_list_of_strings_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".nightshift")).unwrap();
        for (text, message) in [
            ("[merge]\nhuman_review = \"a\"\n", "is not a list"),
            ("[merge]\nhuman_review = [1]\n", "is not a string"),
        ] {
            std::fs::write(file_find(tmp.path()), text).unwrap();
            let err = human_review_read(tmp.path()).unwrap_err();
            assert!(err.to_string().contains(message), "{err}");
        }
    }

    #[test]
    fn max_attempts_is_the_setting_or_the_default() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(max_attempts_read(tmp.path()).unwrap(), 3);
        std::fs::create_dir_all(tmp.path().join(".nightshift")).unwrap();
        std::fs::write(file_find(tmp.path()), "[run]\nmax_attempts = 5\n").unwrap();
        assert_eq!(max_attempts_read(tmp.path()).unwrap(), 5);
        std::fs::write(file_find(tmp.path()), "[run]\nmax_attempts = 1\n").unwrap();
        assert_eq!(max_attempts_read(tmp.path()).unwrap(), 1);
        std::fs::write(file_find(tmp.path()), "[merge]\nhuman_review = []\n").unwrap();
        assert_eq!(max_attempts_read(tmp.path()).unwrap(), 3);
    }

    #[test]
    fn max_attempts_that_is_not_a_whole_number_of_at_least_one_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".nightshift")).unwrap();
        for text in [
            "[run]\nmax_attempts = 0\n",
            "[run]\nmax_attempts = -2\n",
            "[run]\nmax_attempts = \"3\"\n",
            "[run]\nmax_attempts = 2.5\n",
            "[run]\nmax_attempts = 4294967296\n",
        ] {
            std::fs::write(file_find(tmp.path()), text).unwrap();
            let err = max_attempts_read(tmp.path()).unwrap_err().to_string();
            assert!(
                err.contains("run.max_attempts") && err.contains("nightshift.toml"),
                "{text}: {err}"
            );
        }
    }

    #[test]
    fn transcript_days_is_the_setting_or_the_default() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(transcript_days_read(tmp.path()).unwrap(), Keep::Days(7));
        std::fs::create_dir_all(tmp.path().join(".nightshift")).unwrap();
        for (text, keep) in [
            ("[run]\ntranscript_days = 0\n", Keep::Forever),
            ("[run]\ntranscript_days = 7\n", Keep::Days(7)),
            ("[run]\ntranscript_days = 30\n", Keep::Days(30)),
            ("[run]\nmax_attempts = 2\n", Keep::Days(7)),
        ] {
            std::fs::write(file_find(tmp.path()), text).unwrap();
            assert_eq!(transcript_days_read(tmp.path()).unwrap(), keep, "{text}");
        }
    }

    #[test]
    fn transcript_days_that_is_not_a_whole_number_of_at_least_zero_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".nightshift")).unwrap();
        for text in [
            "[run]\ntranscript_days = -1\n",
            "[run]\ntranscript_days = \"7\"\n",
            "[run]\ntranscript_days = 2.5\n",
        ] {
            std::fs::write(file_find(tmp.path()), text).unwrap();
            let err = transcript_days_read(tmp.path()).unwrap_err().to_string();
            assert!(
                err.contains("run.transcript_days") && err.contains("nightshift.toml"),
                "{text}: {err}"
            );
        }
    }

    #[test]
    fn an_unknown_harness_is_an_error_and_codex_is_not_supported_yet() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".nightshift")).unwrap();
        for (name, message) in [
            ("codex", "not supported yet"),
            ("vim", "no harness named vim"),
        ] {
            std::fs::write(file_find(tmp.path()), format!("harness = [\"{name}\"]\n")).unwrap();
            let err = pin_read(tmp.path()).unwrap_err();
            assert!(err.to_string().contains(message), "{err}");
        }
    }
}
