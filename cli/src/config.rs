//! What `ns` is told about this machine, in `~/.config/nightshift/config.toml`: not about a
//! repository, so never committed.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use toml_edit::{value, DocumentMut, Item, Table};

/// `$NS_CONFIG`, or `$XDG_CONFIG_HOME/nightshift/config.toml`, or under `~/.config`.
pub fn file_find() -> Result<PathBuf> {
    if let Some(file) = std::env::var_os("NS_CONFIG") {
        return Ok(PathBuf::from(file));
    }
    let dir = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".config"),
    };
    Ok(dir.join("nightshift").join("config.toml"))
}

fn document_read() -> Result<DocumentMut> {
    let file = file_find()?;
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", file.display())),
    };
    text.parse()
        .with_context(|| format!("{} is not valid TOML", file.display()))
}

/// The GitHub account whose login `ns` uses for the forge, if one is named.
pub fn github_account() -> Result<Option<String>> {
    Ok(document_read()?
        .get("forge")
        .and_then(|forge| forge.get("github"))
        .and_then(|github| github.get("account"))
        .and_then(Item::as_str)
        .map(str::to_string))
}

/// Names the GitHub account whose login `ns` uses for the forge.
pub fn github_account_set(account: &str) -> Result<PathBuf> {
    let valid = !account.is_empty()
        && account
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-');
    if !valid {
        bail!("{account:?} is not a GitHub account name");
    }
    let mut document = document_read()?;
    let forge = document
        .entry("forge")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
        .context("`forge` in the config is not a table")?;
    let github = forge
        .entry("github")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
        .context("`forge.github` in the config is not a table")?;
    github.insert("account", value(account));
    let file = file_find()?;
    std::fs::create_dir_all(file.parent().context("the config has no directory")?)?;
    std::fs::write(&file, document.to_string())
        .with_context(|| format!("cannot write {}", file.display()))?;
    Ok(file)
}
