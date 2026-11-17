//! What `ns` is told about this machine, in `~/.config/nightshift/config.toml`: not about a
//! repository, so never committed.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use toml_edit::{value, DocumentMut, Item, Table};

use crate::notify::{Move, Settings};

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

/// The command `ns` runs when a unit needs a person, and the moves it runs it for. `None` when the
/// machine has no `[notify]`. Only the machine's config can set it, so that a repository cannot
/// make a machine run a command. A bad value is an error, so that `ns` stops before it runs
/// anything.
pub fn notify() -> Result<Option<Settings>> {
    notify_parse(&document_read()?)
}

fn notify_parse(document: &DocumentMut) -> Result<Option<Settings>> {
    let Some(table) = document.get("notify") else {
        return Ok(None);
    };
    let table = table
        .as_table_like()
        .context("`notify` in the config is not a table")?;
    let command = table
        .get("command")
        .context("`notify.command` in the config is missing")?
        .as_str()
        .filter(|command| !command.trim().is_empty())
        .context("`notify.command` in the config is not a command line")?
        .to_string();
    let on = match table.get("on") {
        None => Move::ALL.to_vec(),
        Some(on) => on
            .as_array()
            .context("`notify.on` in the config is not a list")?
            .iter()
            .map(|name| {
                let name = name
                    .as_str()
                    .context("`notify.on` in the config holds something that is not a word")?;
                Move::parse(name).with_context(|| {
                    format!(
                        "`notify.on` in the config holds {name:?}, which is none of {}",
                        Move::ALL.map(Move::name).join(", ")
                    )
                })
            })
            .collect::<Result<_>>()?,
    };
    Ok(Some(Settings { command, on }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> Result<Option<Settings>> {
        notify_parse(&text.parse().unwrap())
    }

    #[test]
    fn no_notify_table_means_no_settings() {
        assert_eq!(parsed("[forge.github]\naccount = \"a\"\n").unwrap(), None);
    }

    #[test]
    fn on_defaults_to_all_four_moves() {
        let settings = parsed("[notify]\ncommand = \"say hi\"\n").unwrap().unwrap();
        assert_eq!(settings.command, "say hi");
        assert_eq!(
            settings.on,
            [
                Move::Stuck,
                Move::WaitingForHuman,
                Move::Merged,
                Move::NightEnded
            ]
        );
    }

    #[test]
    fn on_lists_the_moves_to_notify() {
        let settings = parsed("[notify]\ncommand = \"c\"\non = [\"merged\", \"stuck\"]\n")
            .unwrap()
            .unwrap();
        assert_eq!(settings.on, [Move::Merged, Move::Stuck]);
    }

    #[test]
    fn a_value_of_the_wrong_type_is_an_error_naming_the_key() {
        for (text, key) in [
            ("[notify]\ncommand = 3\n", "notify.command"),
            ("[notify]\ncommand = \"\"\n", "notify.command"),
            ("[notify]\n", "notify.command"),
            ("[notify]\ncommand = \"c\"\non = \"stuck\"\n", "notify.on"),
            ("[notify]\ncommand = \"c\"\non = [1]\n", "notify.on"),
            ("notify = 1\n", "`notify`"),
        ] {
            let err = parsed(text).unwrap_err().to_string();
            assert!(err.contains(key), "{text}: {err}");
        }
    }

    #[test]
    fn a_move_not_in_the_list_is_an_error_naming_it() {
        let err = parsed("[notify]\ncommand = \"c\"\non = [\"stuck\", \"asleep\"]\n")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("\"asleep\"") && err.contains("night_ended"),
            "{err}"
        );
    }
}
