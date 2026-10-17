//! Code forges (units and change requests) behind one trait. Everything specific to one lives
//! in its own file.

mod github;

use std::path::Path;

use anyhow::Result;

pub trait Forge {
    /// The unit `number` of the repository at `repo`.
    fn unit_get(&self, repo: &Path, number: u64) -> Result<Unit>;

    /// The newest change request opened from `branch`, if there is one.
    fn change_request_get(&self, repo: &Path, branch: &str) -> Result<Option<ChangeRequest>>;

    /// Merges change request `number` into its base as one commit.
    fn change_request_merge(&self, repo: &Path, number: u64) -> Result<()>;
}

#[derive(Debug, PartialEq, Eq)]
pub struct Unit {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ChangeRequest {
    pub number: u64,
    pub url: String,
    pub state: ChangeState,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ChangeState {
    Open,
    Merged,
    Closed,
}

/// The forge `ns` uses. The only place one is chosen.
pub fn current() -> Box<dyn Forge> {
    Box::new(github::GitHub::new())
}
