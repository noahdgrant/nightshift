/// What the build script learned from git: the commit at HEAD, and whether it told cargo to
/// rerun it when HEAD moves.
pub struct Head {
    pub commit: Option<String>,
    pub watched: bool,
}

/// The commit to stamp, and whether cargo must rerun the build script on every build because
/// nothing it watches would change when a later build could find the commit.
pub fn commit(env: Option<String>, git_head: impl FnOnce() -> Head) -> (String, bool) {
    match env {
        Some(c) if !c.is_empty() => (c, false),
        _ => {
            let head = git_head();
            let rerun_always = head.commit.is_none() || !head.watched;
            (
                head.commit.unwrap_or_else(|| "unknown".into()),
                rerun_always,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{commit, Head};

    fn head(commit: Option<&str>, watched: bool) -> impl FnOnce() -> Head {
        let commit = commit.map(String::from);
        move || Head { commit, watched }
    }

    #[test]
    fn a_set_commit_wins_and_git_is_not_asked() {
        let c = commit(Some("abc".into()), || panic!("git was asked"));
        assert_eq!(c, ("abc".into(), false));
    }

    #[test]
    fn git_head_is_used_when_the_env_is_unset_or_empty() {
        assert_eq!(commit(None, head(Some("def"), true)), ("def".into(), false));
        assert_eq!(
            commit(Some(String::new()), head(Some("def"), true)),
            ("def".into(), false)
        );
    }

    #[test]
    fn with_neither_the_commit_is_unknown_and_the_script_reruns_every_build() {
        assert_eq!(commit(None, head(None, false)), ("unknown".into(), true));
        assert_eq!(commit(None, head(None, true)), ("unknown".into(), true));
    }

    #[test]
    fn a_commit_found_with_no_head_watched_reruns_every_build() {
        assert_eq!(commit(None, head(Some("def"), false)), ("def".into(), true));
    }
}
