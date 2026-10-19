//! The repository's `.nightshift/nightshift.toml`. `ns` reads and changes only the keys it knows,
//! and leaves the rest, comments included, as it found them.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use toml_edit::{value, Array, DocumentMut};

use crate::harness::Kind;

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
    let document = document_read(root)?;
    let version = document
        .get("version")
        .map(|item| {
            item.as_str()
                .context("`version` in nightshift.toml is not a string")
                .map(str::to_string)
        })
        .transpose()?;
    let harness = match document.get("harness") {
        None => vec![Kind::Claude],
        Some(item) => item
            .as_array()
            .context("`harness` in nightshift.toml is not a list")?
            .iter()
            .map(|name| Kind::parse(name.as_str().context("a harness name is not a string")?))
            .collect::<Result<_>>()?,
    };
    Ok(Pin { version, harness })
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
