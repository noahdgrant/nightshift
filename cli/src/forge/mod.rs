//! Code forges (units and change requests) behind one trait. Everything specific to one lives
//! in its own file.

mod github;

use std::path::Path;

use anyhow::Result;

pub trait Forge {
    /// The login the forge sees `ns` as when it works on the repository at `repo`. An error
    /// when the forge answers with a login other than the account the config names.
    fn login_get(&self, repo: &Path) -> Result<String>;

    /// The unit `number` of the repository at `repo`.
    fn unit_get(&self, repo: &Path, number: u64) -> Result<Unit>;

    /// The names of the labels the repository at `repo` has.
    fn labels_list(&self, repo: &Path) -> Result<Vec<String>>;

    /// Creates the label `name`, which the repository does not have.
    fn label_create(&self, repo: &Path, name: &str, description: &str) -> Result<()>;

    /// Adds and removes labels on unit `number`.
    fn unit_labels_set(
        &self,
        repo: &Path,
        number: u64,
        add: &[String],
        remove: &[String],
    ) -> Result<()>;

    /// Replaces the description of unit `number` with `body`.
    fn unit_body_set(&self, repo: &Path, number: u64, body: &str) -> Result<()>;

    /// Closes unit `number` as not planned.
    fn unit_close(&self, repo: &Path, number: u64) -> Result<()>;

    /// Every open unit of the repository at `repo`.
    fn unit_list(&self, repo: &Path) -> Result<Vec<Unit>>;

    /// The newest change request opened from `branch`, if there is one.
    fn change_request_get(&self, repo: &Path, branch: &str) -> Result<Option<ChangeRequest>>;

    /// Opens a change request from `branch` for unit `unit`, linked to the unit so that merging
    /// it closes the unit on the forge.
    fn change_request_create(
        &self,
        repo: &Path,
        branch: &str,
        unit: u64,
        title: &str,
        body: &str,
    ) -> Result<ChangeRequest>;

    /// How the checks on change request `number` stand.
    fn change_request_checks_get(&self, repo: &Path, number: u64) -> Result<Checks>;

    /// Downloads the files of release `tag` of the repository `source` ("owner/name") whose names
    /// match one of `patterns` into `dir`.
    fn release_download(
        &self,
        source: &str,
        tag: &str,
        patterns: &[&str],
        dir: &Path,
    ) -> Result<()>;

    /// Merges change request `number` into its base as one commit.
    fn change_request_merge(&self, repo: &Path, number: u64) -> Result<()>;
}

/// Every post `ns` makes on the forge starts with this line.
pub const DISCLOSURE: &str = "🤖 *Posted by nightshift, an AI agent.*";

#[derive(Debug, PartialEq, Eq)]
pub struct Unit {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    /// Whether the unit's author is on the repository's team. Only the team's units are worked.
    pub from_team: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ChangeRequest {
    pub number: u64,
    pub url: String,
    pub state: ChangeState,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Checks {
    /// Some check has not finished.
    Pending,
    /// No check is reported: none has started yet, or the repository has none.
    NoneReported,
    Passed,
    /// The names of the checks that failed.
    Failed(Vec<String>),
}

#[derive(Debug, PartialEq, Eq)]
pub enum ChangeState {
    Open,
    Merged,
    Closed,
}

/// The forge `ns` uses. The only place one is chosen.
pub fn get() -> Box<dyn Forge> {
    Box::new(github::GitHub::new())
}
