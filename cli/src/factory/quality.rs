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
    /// The remote to fetch and push the records, or `None` when they stay in the outbox.
    pub fn target(&self) -> Option<&str> {
        (self.remote != "none").then_some(self.remote.as_str())
    }

    pub(super) fn problems(&self) -> Vec<String> {
        let r = &self.remote;
        if r.trim().is_empty() {
            vec![
                "[quality] remote is empty; name a git remote, or \"none\" for the outbox only"
                    .into(),
            ]
        } else if r.starts_with('-') {
            vec![format!(
                "[quality] remote = {r:?}: a remote name may not start with '-'"
            )]
        } else {
            Vec::new()
        }
    }
}
