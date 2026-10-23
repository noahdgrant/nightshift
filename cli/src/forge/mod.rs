//! Code forges (units and change requests) behind one trait. Everything specific to one lives
//! in its own file.

mod github;

use std::path::Path;

use anyhow::Result;

pub trait Forge {
    /// The login the forge sees `ns` as when it works on the repository at `repo`. An error
    /// when the forge answers with a login other than the account the config names.
    fn login_get(&self, repo: &Path) -> Result<String>;

    /// How the default branch of the repository at `repo` is protected.
    fn branch_protection_get(&self, repo: &Path) -> Result<BranchProtection>;

    /// The unit `number` of the repository at `repo`.
    fn unit_get(&self, repo: &Path, number: u64) -> Result<Unit>;

    /// The names of the labels the repository at `repo` has.
    fn labels_list(&self, repo: &Path) -> Result<Vec<String>>;

    /// Creates the label `name`, which the repository does not have.
    fn label_create(&self, repo: &Path, name: &str, description: &str) -> Result<()>;

    /// The names of the native unit types the repository at `repo` has. An error when the forge
    /// cannot say; none when it has no such types.
    fn unit_types_list(&self, repo: &Path) -> Result<Vec<String>>;

    /// Sets the native type of unit `number` to `name`, one of those `unit_types_list` names.
    fn unit_type_set(&self, repo: &Path, number: u64, name: &str) -> Result<()>;

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

    /// Adds a comment with `body` to unit `number`.
    fn unit_comment_add(&self, repo: &Path, number: u64, body: &str) -> Result<()>;

    /// Adds the account `ns` acts as to the assignees of unit `number`. Those already there stay.
    fn unit_assignee_add(&self, repo: &Path, number: u64) -> Result<()>;

    /// Closes unit `number` as not planned.
    fn unit_close(&self, repo: &Path, number: u64) -> Result<()>;

    /// Every open unit of the repository at `repo`.
    fn unit_list(&self, repo: &Path) -> Result<Vec<Unit>>;

    /// The numbers of the open units that block unit `number` by the forge's own relationship.
    fn unit_blockers_list(&self, repo: &Path, number: u64) -> Result<Vec<u64>>;

    /// Records that unit `number` is blocked by unit `blocker`. Recording one that is already
    /// there is not an error.
    fn unit_blocker_add(&self, repo: &Path, number: u64, blocker: u64) -> Result<()>;

    /// The newest change request opened from `branch`, if there is one.
    fn change_request_get(&self, repo: &Path, branch: &str) -> Result<Option<ChangeRequest>>;

    /// Opens a change request from `branch` for unit `unit`, linked to the unit so that merging
    /// it closes the unit on the forge. The change request carries `labels`.
    fn change_request_create(
        &self,
        repo: &Path,
        branch: &str,
        unit: u64,
        title: &str,
        body: &str,
        labels: &[String],
    ) -> Result<ChangeRequest>;

    /// Adds the account `ns` acts as to the assignees of change request `number`. Those already
    /// there stay.
    fn change_request_assignee_add(&self, repo: &Path, number: u64) -> Result<()>;

    /// Adds `label` to change request `number`.
    fn change_request_label_add(&self, repo: &Path, number: u64, label: &str) -> Result<()>;

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

    /// Merges change request `number` into its base as one commit, only when its head is the
    /// commit `head`. A change request that has moved on since is refused.
    fn change_request_merge(&self, repo: &Path, number: u64, head: &str) -> Result<()>;
}

/// The open units the forge says block unit `number`. A forge that cannot say, because the call
/// fails or it has no such relationship, blocks nothing; the failure is noted on stderr.
pub fn blockers_get(forge: &dyn Forge, repo: &Path, number: u64) -> Vec<u64> {
    forge.unit_blockers_list(repo, number).unwrap_or_else(|e| {
        eprintln!("ns: cannot read what blocks unit {number}: {e:#}");
        Vec::new()
    })
}

/// The native unit types the forge says the repository has. A forge that cannot say, because the
/// call fails or the repository has no such types, has none.
pub fn unit_types_get(forge: &dyn Forge, repo: &Path) -> Vec<String> {
    forge.unit_types_list(repo).unwrap_or_default()
}

/// Every post `ns` makes on the forge starts with this line.
pub const DISCLOSURE: &str = "🤖 *Posted by nightshift, an AI agent.*";

/// The default branch of a repository, and the checks its protection requires of a change
/// request before it merges.
#[derive(Debug, PartialEq, Eq)]
pub struct BranchProtection {
    pub branch: String,
    /// Whether the branch has classic protection at all.
    pub protected: bool,
    /// The names of the required checks; none when the protection does not enable status checks.
    pub required_checks: Option<Vec<String>>,
}

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

#[cfg(test)]
mod tests {
    use super::*;

    struct Blockers(Result<Vec<u64>, &'static str>);

    struct Types(Result<Vec<String>, &'static str>);

    impl Forge for Types {
        fn unit_types_list(&self, _: &Path) -> Result<Vec<String>> {
            self.0.clone().map_err(|e| anyhow::anyhow!(e))
        }
        fn unit_type_set(&self, _: &Path, _: u64, _: &str) -> Result<()> {
            unreachable!()
        }
        fn unit_blockers_list(&self, _: &Path, _: u64) -> Result<Vec<u64>> {
            unreachable!()
        }
        fn unit_blocker_add(&self, _: &Path, _: u64, _: u64) -> Result<()> {
            unreachable!()
        }
        fn login_get(&self, _: &Path) -> Result<String> {
            unreachable!()
        }
        fn branch_protection_get(&self, _: &Path) -> Result<BranchProtection> {
            unreachable!()
        }
        fn unit_get(&self, _: &Path, _: u64) -> Result<Unit> {
            unreachable!()
        }
        fn labels_list(&self, _: &Path) -> Result<Vec<String>> {
            unreachable!()
        }
        fn label_create(&self, _: &Path, _: &str, _: &str) -> Result<()> {
            unreachable!()
        }
        fn unit_labels_set(&self, _: &Path, _: u64, _: &[String], _: &[String]) -> Result<()> {
            unreachable!()
        }
        fn unit_body_set(&self, _: &Path, _: u64, _: &str) -> Result<()> {
            unreachable!()
        }
        fn unit_comment_add(&self, _: &Path, _: u64, _: &str) -> Result<()> {
            unreachable!()
        }
        fn unit_assignee_add(&self, _: &Path, _: u64) -> Result<()> {
            unreachable!()
        }
        fn unit_close(&self, _: &Path, _: u64) -> Result<()> {
            unreachable!()
        }
        fn unit_list(&self, _: &Path) -> Result<Vec<Unit>> {
            unreachable!()
        }
        fn change_request_get(&self, _: &Path, _: &str) -> Result<Option<ChangeRequest>> {
            unreachable!()
        }
        fn change_request_create(
            &self,
            _: &Path,
            _: &str,
            _: u64,
            _: &str,
            _: &str,
            _: &[String],
        ) -> Result<ChangeRequest> {
            unreachable!()
        }
        fn change_request_assignee_add(&self, _: &Path, _: u64) -> Result<()> {
            unreachable!()
        }
        fn change_request_label_add(&self, _: &Path, _: u64, _: &str) -> Result<()> {
            unreachable!()
        }
        fn change_request_checks_get(&self, _: &Path, _: u64) -> Result<Checks> {
            unreachable!()
        }
        fn release_download(&self, _: &str, _: &str, _: &[&str], _: &Path) -> Result<()> {
            unreachable!()
        }
        fn change_request_merge(&self, _: &Path, _: u64, _: &str) -> Result<()> {
            unreachable!()
        }
    }

    impl Forge for Blockers {
        fn unit_types_list(&self, _: &Path) -> Result<Vec<String>> {
            unreachable!()
        }
        fn unit_type_set(&self, _: &Path, _: u64, _: &str) -> Result<()> {
            unreachable!()
        }
        fn unit_blockers_list(&self, _: &Path, _: u64) -> Result<Vec<u64>> {
            self.0.clone().map_err(|e| anyhow::anyhow!(e))
        }
        fn login_get(&self, _: &Path) -> Result<String> {
            unreachable!()
        }
        fn branch_protection_get(&self, _: &Path) -> Result<BranchProtection> {
            unreachable!()
        }
        fn unit_blocker_add(&self, _: &Path, _: u64, _: u64) -> Result<()> {
            unreachable!()
        }
        fn unit_get(&self, _: &Path, _: u64) -> Result<Unit> {
            unreachable!()
        }
        fn labels_list(&self, _: &Path) -> Result<Vec<String>> {
            unreachable!()
        }
        fn label_create(&self, _: &Path, _: &str, _: &str) -> Result<()> {
            unreachable!()
        }
        fn unit_labels_set(&self, _: &Path, _: u64, _: &[String], _: &[String]) -> Result<()> {
            unreachable!()
        }
        fn unit_body_set(&self, _: &Path, _: u64, _: &str) -> Result<()> {
            unreachable!()
        }
        fn unit_comment_add(&self, _: &Path, _: u64, _: &str) -> Result<()> {
            unreachable!()
        }
        fn unit_assignee_add(&self, _: &Path, _: u64) -> Result<()> {
            unreachable!()
        }
        fn unit_close(&self, _: &Path, _: u64) -> Result<()> {
            unreachable!()
        }
        fn unit_list(&self, _: &Path) -> Result<Vec<Unit>> {
            unreachable!()
        }
        fn change_request_get(&self, _: &Path, _: &str) -> Result<Option<ChangeRequest>> {
            unreachable!()
        }
        fn change_request_create(
            &self,
            _: &Path,
            _: &str,
            _: u64,
            _: &str,
            _: &str,
            _: &[String],
        ) -> Result<ChangeRequest> {
            unreachable!()
        }
        fn change_request_assignee_add(&self, _: &Path, _: u64) -> Result<()> {
            unreachable!()
        }
        fn change_request_label_add(&self, _: &Path, _: u64, _: &str) -> Result<()> {
            unreachable!()
        }
        fn change_request_checks_get(&self, _: &Path, _: u64) -> Result<Checks> {
            unreachable!()
        }
        fn release_download(&self, _: &str, _: &str, _: &[&str], _: &Path) -> Result<()> {
            unreachable!()
        }
        fn change_request_merge(&self, _: &Path, _: u64, _: &str) -> Result<()> {
            unreachable!()
        }
    }

    #[test]
    fn the_forges_blockers_are_returned() {
        let forge = Blockers(Ok(vec![3, 5]));
        assert_eq!(blockers_get(&forge, Path::new("."), 1), [3, 5]);
    }

    #[test]
    fn a_forge_that_cannot_say_blocks_nothing() {
        let forge = Blockers(Err("404"));
        assert_eq!(blockers_get(&forge, Path::new("."), 1), Vec::<u64>::new());
    }

    #[test]
    fn the_native_types_the_forge_names_are_returned() {
        let forge = Types(Ok(vec!["Bug".into(), "Task".into()]));
        assert_eq!(unit_types_get(&forge, Path::new(".")), ["Bug", "Task"]);
    }

    #[test]
    fn a_failed_lookup_of_native_types_means_none() {
        let forge = Types(Err("404"));
        assert_eq!(unit_types_get(&forge, Path::new(".")), Vec::<String>::new());
    }
}
