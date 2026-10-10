pub fn commit(env: Option<String>, git_head: impl FnOnce() -> Option<String>) -> String {
    match env {
        Some(c) if !c.is_empty() => c,
        _ => git_head().unwrap_or_else(|| "unknown".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::commit;

    #[test]
    fn a_set_commit_wins_and_git_is_not_asked() {
        let c = commit(Some("abc".into()), || panic!("git was asked"));
        assert_eq!(c, "abc");
    }

    #[test]
    fn git_head_is_used_when_the_env_is_unset_or_empty() {
        assert_eq!(commit(None, || Some("def".into())), "def");
        assert_eq!(commit(Some(String::new()), || Some("def".into())), "def");
    }

    #[test]
    fn with_neither_the_commit_is_unknown() {
        assert_eq!(commit(None, || None), "unknown");
    }
}
