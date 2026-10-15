//! `[quality]`: where the quality records live (docs/factory/quality.md, "Where the history
//! lives").

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Quality {
    /// The git remote that carries the `nightshift/quality` branch, or `none` for the outbox only.
    pub remote: String,
}

impl Default for Quality {
    fn default() -> Self {
        Self {
            remote: "origin".into(),
        }
    }
}

impl Quality {
    /// The remote to fetch and push the records, `None` when they stay in the outbox, or why the
    /// name cannot be a remote name: it reaches git as an argument and inside a ref.
    pub fn target(&self) -> Result<Option<&str>, String> {
        let r = self.remote.as_str();
        if r == "none" {
            return Ok(None);
        }
        if r.trim().is_empty() {
            return Err(
                "[quality] remote is empty; name a git remote, or \"none\" for the outbox only"
                    .into(),
            );
        }
        if r.starts_with('-') {
            return Err(format!(
                "[quality] remote = {r:?}: a remote name may not start with '-'"
            ));
        }
        let ref_name = format!("refs/remotes/{r}/x");
        let ok = crate::git::command()
            .args(["check-ref-format", &ref_name])
            .output()
            .is_ok_and(|o| o.status.success());
        if !ok {
            return Err(format!(
                "[quality] remote = {r:?} is not a valid remote name"
            ));
        }
        Ok(Some(r))
    }

    pub(super) fn problems(&self) -> Vec<String> {
        self.target().err().into_iter().collect()
    }
}
