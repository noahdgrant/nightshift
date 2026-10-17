//! The repository's `.nightshift/nightshift.toml`. `ns` reads and changes only the tables it
//! knows, and leaves the rest, comments included, as it found them.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use toml_edit::{value, DocumentMut, Item, Table};

fn file_find(root: &Path) -> PathBuf {
    root.join(".nightshift").join("nightshift.toml")
}

/// What a skill's file holds, as it is recorded.
pub fn hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256:{hex}")
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

/// The skills as `ns init` last copied them: the hash of each skill's file, by skill name.
pub fn skills_read(root: &Path) -> Result<Vec<(String, String)>> {
    let document = document_read(root)?;
    Ok(document
        .get("skills")
        .and_then(Item::as_table)
        .into_iter()
        .flat_map(Table::iter)
        .filter(|(name, _)| *name != "version")
        .filter_map(|(name, hash)| Some((name.to_string(), hash.as_str()?.to_string())))
        .collect())
}

/// Records the version of `ns` that copied the skills, and the hash of each skill's file.
pub fn skills_write(root: &Path, version: &str, skills: &[(String, String)]) -> Result<()> {
    let mut document = document_read(root)?;
    let table = document
        .entry("skills")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
        .context("`skills` in nightshift.toml is not a table")?;
    table.insert("version", value(version));
    for (name, hash) in skills {
        table.insert(name, value(hash));
    }
    let file = file_find(root);
    std::fs::create_dir_all(file.parent().context("the manifest has no directory")?)?;
    std::fs::write(&file, document.to_string())
        .with_context(|| format!("cannot write {}", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skills_are_recorded_and_the_rest_of_the_file_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".nightshift")).unwrap();
        let original = "# my factory\nname = \"mine\"\n\n[queue]\nsource = \"github\" # why\n";
        std::fs::write(file_find(tmp.path()), original).unwrap();
        skills_write(tmp.path(), "0.2.0", &[("ns-build".into(), hash("a"))]).unwrap();
        let text = std::fs::read_to_string(file_find(tmp.path())).unwrap();
        assert!(text.starts_with(original.trim_end()), "{text}");
        assert!(text.contains("[skills]\nversion = \"0.2.0\"\nns-build = \"sha256:"));
        assert_eq!(
            skills_read(tmp.path()).unwrap(),
            [("ns-build".to_string(), hash("a"))]
        );
    }

    #[test]
    fn a_repository_with_no_manifest_has_no_skills_recorded() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(skills_read(tmp.path()).unwrap().is_empty());
    }

    #[test]
    fn the_hash_is_of_the_text() {
        assert_eq!(hash("a"), hash("a"));
        assert_ne!(hash("a"), hash("b"));
        assert!(hash("a").starts_with("sha256:") && hash("a").len() == 7 + 64);
    }
}
