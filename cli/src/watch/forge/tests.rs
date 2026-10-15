use super::*;

#[test]
fn a_post_opens_with_the_disclosure_then_a_blank_line_then_the_text() {
    let post = Post::new("Fixed in #3.\n");
    let mut lines = post.as_str().lines();
    assert_eq!(
        lines.next(),
        Some("🤖 *Posted by nightshift, an AI agent.*")
    );
    assert_eq!(lines.next(), Some(""));
    assert_eq!(lines.next(), Some("Fixed in #3."));
    assert_eq!(lines.next(), None);
}

#[test]
fn parse_issue_link_reads_the_title_and_the_named_url() {
    let v = serde_json::json!({"title": "Fix it", "url": "u1", "web_url": "u2"});
    let link = |title: &str, url: &str| IssueLink {
        title: title.into(),
        url: url.into(),
    };
    assert_eq!(parse_issue_link(&v, "url"), link("Fix it", "u1"));
    assert_eq!(parse_issue_link(&v, "web_url"), link("Fix it", "u2"));
    assert_eq!(
        parse_issue_link(&serde_json::json!({}), "url"),
        link("", "")
    );
}

#[test]
fn source_is_inferred_from_the_origin_host() {
    let cases = [
        (Some("https://gitlab.com/o/r.git"), "gitlab"),
        (Some("git@gitlab.example:o/r.git"), "gitlab"),
        (Some("https://gitlab.corp.example/o/r"), "gitlab"),
        (Some("https://GitLab.com/o/r"), "gitlab"),
        (Some("https://github.com/o/r"), "github"),
        (Some("/srv/git/gitlab-mirror.git"), "github"),
        (None, "github"),
    ];
    for (origin, want) in cases {
        assert_eq!(inferred(origin, None).name(), want, "{origin:?}");
    }
    assert_eq!(
        check(Some("github"), Some("https://gitlab.com/o/r"), None)
            .unwrap()
            .name(),
        "github"
    );
}

#[test]
fn source_is_gitlab_on_the_configured_gitlab_host() {
    let origin = Some("git@Code.example.com:o/r.git");
    for h in ["code.example.com", "https://CODE.example.com"] {
        assert_eq!(inferred(origin, Some(h)).name(), "gitlab", "{h}");
    }
    assert_eq!(inferred(origin, Some("other.example")).name(), "github");
    assert_eq!(
        check(Some("github"), origin, Some("code.example.com"))
            .unwrap()
            .name(),
        "github"
    );
}

#[test]
fn check_takes_github_and_gitlab_and_refuses_unknown_with_the_hint() {
    assert_eq!(
        check(None, Some("https://github.com/o/r"), None).unwrap(),
        ForgeKind::Github
    );
    assert_eq!(
        check(Some("github"), None, None).unwrap(),
        ForgeKind::Github
    );
    assert_eq!(
        check(Some("gitlab"), None, None).unwrap(),
        ForgeKind::Gitlab
    );
    assert_eq!(
        check(None, Some("https://gitlab.com/o/r"), None).unwrap(),
        ForgeKind::Gitlab
    );
    for (s, o) in [
        (Some("bitbucket"), None),
        (Some("svn"), Some("https://github.com/o/r")),
    ] {
        let e = check(s, o, None).unwrap_err().to_string();
        assert!(e.contains("[queue] source"), "{e}");
    }
}

#[test]
fn last_green_skips_runs_on_the_red_head() {
    let runs = serde_json::json!([
        {"headSha": "aaaaaaa"}, {"headSha": "aaaaaaa"}, {"headSha": "bbbbbbb"},
        {"headSha": "ccccccc"}
    ]);
    assert_eq!(
        first_other(&runs, "headSha", "aaaaaaa").as_deref(),
        Some("bbbbbbb")
    );
    let only = serde_json::json!([{"headSha": "aaaaaaa"}]);
    assert_eq!(first_other(&only, "headSha", "aaaaaaa"), None);
    assert_eq!(first_other(&serde_json::json!([]), "headSha", "red"), None);
}

#[test]
fn commit_prs_refuses_what_is_not_a_commit_id_before_calling_the_forge() {
    let root = std::env::temp_dir();
    let github = GitHub { root: root.clone() };
    let gitlab = gitlab::GitLab { root };
    for sha in ["--output=x", "abc1234", &"a".repeat(41)] {
        for forge in [&github as &dyn Forge, &gitlab] {
            let e = forge.commit_prs(sha).unwrap_err();
            assert!(format!("{e:#}").contains("not a commit id"), "{sha}: {e:#}");
        }
    }
}

#[test]
fn last_green_skips_what_is_not_a_commit_id() {
    let runs = serde_json::json!([
        {"headSha": "--output=x"}, {"headSha": "abc"}, {"headSha": "zzzzzzzz"},
        {"headSha": "é".repeat(8)}, {"headSha": "a".repeat(65)}, {"headSha": "AbC1234"}
    ]);
    assert_eq!(
        first_other(&runs, "headSha", "red").as_deref(),
        Some("AbC1234")
    );
}

#[test]
fn path_reads_url_and_scp_forms_and_no_path_from_a_local_path() {
    assert_eq!(path("https://gitlab.com/o/r.git"), Some("o/r.git"));
    assert_eq!(path("ssh://git@gitlab.example:2222/o/r"), Some("o/r"));
    assert_eq!(path("git@github.com:o/r.git"), Some("o/r.git"));
    assert_eq!(path("https://gitlab.com"), None);
    assert_eq!(path("/srv/git/gitlab:mirror.git"), None);
}

#[test]
fn host_reads_url_and_scp_forms_and_no_host_from_a_path() {
    assert_eq!(host("https://gitlab.com/o/r.git"), Some("gitlab.com"));
    assert_eq!(
        host("ssh://git@gitlab.example:2222/o/r"),
        Some("gitlab.example")
    );
    assert_eq!(host("https://user:pw@github.com/o/r"), Some("github.com"));
    assert_eq!(host("git@github.com:o/r.git"), Some("github.com"));
    assert_eq!(host("/srv/git/gitlab:mirror.git"), None);
    assert_eq!(host("../gitlab-mirror"), None);
}

/// Where a forge's CLI may run: the forges here, the credential checks, and `Proc` itself.
const FORGE_CALLERS: &[&str] = &["watch/forge/", "forge.rs", "forge/", "proc.rs", "proc/"];

/// The line up to a `//` that is not inside a string literal.
fn outside_comment(line: &str) -> &str {
    let (mut in_string, mut escaped, mut prev) = (false, false, '\0');
    for (i, c) in line.char_indices() {
        if in_string {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
        } else if c == '"' {
            in_string = true;
        } else if c == '/' && prev == '/' {
            return &line[..i - 1];
        }
        prev = c;
    }
    line
}

/// What starts `gh` or `glab`, or calls `gh()` or `gh_json()`.
fn calls_a_forge(line: &str) -> bool {
    let code = outside_comment(line);
    let spawns = ["Proc::", "Command::new"].iter().any(|s| code.contains(s))
        && ["(\"gh\"", "(\"glab\""].iter().any(|p| code.contains(p));
    static HELPER: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"(^|[^\w."])gh(_json)?\("#).unwrap());
    spawns || code.contains("Proc::forge(") || HELPER.is_match(code)
}

#[test]
fn calls_a_forge_finds_each_way_to_reach_one() {
    for yes in [
        "    let out = Proc::forge(\"gh\")",
        "crate::proc::Proc::tool(\"glab\").args(x)",
        "let v = gh_json(wt, &[\"pr\"])?;",
        "    gh(&self.root, &args)",
        "Proc::forge(program)",
        "std::process::Command::new(\"gh\").args(x)",
        "Command::new(\"glab\")",
        "let u = \"https://x\"; gh(wt, &a);",
        "let u = \"a\\\"//\"; Command::new(\"gh\")",
    ] {
        assert!(calls_a_forge(yes), "{yes}");
    }
    for no in [
        "// gh(wt) ran here once",
        "fn high(x)",
        "self.gh(x)",
        "let ghost = 1;",
        "\"gh\" => 1,",
        "    \"gh(\",",
        "Command::new(\"git\") // gh(wt)",
        "let u = \"https://x\"; // Command::new(\"gh\")",
    ] {
        assert!(!calls_a_forge(no), "{no}");
    }
}

/// Every forge call goes through [`Forge`]: no file outside [`FORGE_CALLERS`] starts `gh` or
/// `glab`, in the unit tests under `src/` too. Integration tests in `tests/` are not scanned.
#[test]
fn no_forge_cli_runs_outside_the_forge_modules() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    let mut dirs = vec![src.clone()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let rel = path
                .strip_prefix(&src)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            if FORGE_CALLERS.iter().any(|c| rel.starts_with(c)) {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for (i, line) in text.lines().enumerate() {
                if calls_a_forge(line) {
                    found.push(format!("{rel}:{}: {}", i + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "forge calls outside the forge modules; add a Forge method instead:\n{}",
        found.join("\n")
    );
}
