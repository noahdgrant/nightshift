//! Fix main first: while main's CI is red, `ns watch` files one issue to fix it, or reuses the
//! open one, and starts no other unit (docs/FACTORY.md, "Main's CI").

use anyhow::Result;

use super::ci::{Check, MainCi};
use super::{Forge, Issue};
use crate::factory::Factory;
use crate::git::{self, Repo};

/// Every fix issue's title starts so, whatever sha and check it names: one open is reused.
pub(super) const TITLE_PREFIX: &str = "fix: main is red at";

/// The labels a fix issue is filed with, besides the queue's ready label.
const LABELS: [&str; 2] = ["type:fix", "priority:critical"];

pub(super) const DISCLAIMER: &str =
    "_This issue was written by an AI agent (nightshift `ns watch`), not a person._";

/// The lines of each check's log the body quotes, at most, and the bytes.
const LOG_LINES: usize = 40;
const LOG_BYTES: usize = 3000;
/// The commits since the last green run the body lists, at most.
const COMMITS: usize = 30;

/// The issue that fixes red `main`: `filed`, the one filed earlier tonight, while it is open,
/// since GitHub may not list it yet; else the oldest open team-authored one whose title starts
/// with [`TITLE_PREFIX`]; else one filed now. True when filed now.
pub(super) fn issue(
    repo: &Repo,
    forge: &dyn Forge,
    fac: &Factory,
    main: &MainCi,
    filed: Option<&Issue>,
) -> Result<(Issue, bool)> {
    if let Some(i) = filed.filter(|i| matches!(forge.is_closed(i.number), Ok(false))) {
        return Ok((i.clone(), false));
    }
    let mut open: Vec<Issue> = forge
        .open_issues(None)?
        .into_iter()
        .filter(|i| i.team && i.title.starts_with(TITLE_PREFIX))
        .collect();
    open.sort_by_key(|i| i.number);
    if let Some(i) = open.into_iter().next() {
        return Ok((i, false));
    }
    let title = title(main);
    let body = body(repo, forge, main);
    let mut labels: Vec<&str> = LABELS.to_vec();
    labels.push(&fac.queue.ready_label);
    let number = forge.create_issue(&title, &body, &labels)?;
    let issue = Issue {
        number,
        title,
        labels: labels.into_iter().map(String::from).collect(),
        body,
        team: true,
    };
    Ok((issue, true))
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

/// `fix: main is red at <short sha>: <first failing check>`, with how many others failed.
fn title(main: &MainCi) -> String {
    let sha = short(main.sha.as_deref().unwrap_or("?"));
    let first = main.failing.first().map_or("?", |c| c.name.as_str());
    match main.failing.len() {
        0 | 1 => format!("{TITLE_PREFIX} {sha}: {first}"),
        n => format!("{TITLE_PREFIX} {sha}: {first} (+{} more)", n - 1),
    }
}

/// The failing checks, the end of each one's log, the commits since the last green run, and
/// the authorship line. A part the forge can't give says why instead.
fn body(repo: &Repo, forge: &dyn Forge, main: &MainCi) -> String {
    let sha = main.sha.as_deref().unwrap_or("?");
    let mut out = format!("CI on the default branch fails at {sha}.\n\n## Failing checks\n");
    for c in &main.failing {
        out.push_str(&format!("\n### {}\n\n{}\n", c.name, excerpt(forge, c)));
    }
    out.push_str("\n## Commits since the last green run\n\n");
    out.push_str(&commits(repo, forge, sha));
    out.push_str(&format!("\n\n{DISCLAIMER}\n"));
    out
}

/// The last [`LOG_LINES`] lines of the check's log, at most [`LOG_BYTES`], redacted and fenced.
fn excerpt(forge: &dyn Forge, c: &Check) -> String {
    let Some(at) = &c.log else {
        return "No log: the forge keeps none for this check.".into();
    };
    let log = match forge.check_log(at) {
        Ok(l) => l,
        Err(e) => {
            return format!(
                "No log: {}",
                crate::phase_output::redact_text(&format!("{e:#}"))
            )
        }
    };
    let lines: Vec<&str> = log.lines().collect();
    let tail = lines[lines.len().saturating_sub(LOG_LINES)..].join("\n");
    let start = tail.len().saturating_sub(LOG_BYTES);
    let start = (start..=tail.len())
        .find(|i| tail.is_char_boundary(*i))
        .unwrap_or(0);
    let text = crate::phase_output::redact_text(&tail[start..]).replace("```", "'''");
    format!("```\n{text}\n```")
}

/// `git log --oneline` from the last green run to `sha`, at most [`COMMITS`] of them.
fn commits(repo: &Repo, forge: &dyn Forge, sha: &str) -> String {
    let green = match forge.last_green() {
        Ok(Some(g)) => g,
        Ok(None) => return "No green run on the default branch found.".into(),
        Err(e) => return format!("Cannot find the last green run: {e:#}"),
    };
    let _ = git::run(&repo.root, &["fetch", "--quiet", "origin"]);
    let range = format!("{green}..{sha}");
    let max = format!("--max-count={}", COMMITS + 1);
    match git::run(
        &repo.root,
        &["log", "--oneline", "--no-decorate", &max, &range],
    ) {
        Ok(log) => {
            let lines: Vec<&str> = log.lines().collect();
            let mut list = format!("Since {}:\n", short(&green));
            for l in lines.iter().take(COMMITS) {
                list.push_str(&format!("\n- {l}"));
            }
            if lines.len() > COMMITS {
                list.push_str("\n- ...");
            }
            list
        }
        Err(e) => format!("Cannot list the commits since {}: {e:#}", short(&green)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watch::ci::{HeadChecks, Outcome, State};
    use crate::watch::tests::Fake;

    fn failed(name: &str, log: Option<&str>) -> Check {
        Check {
            name: name.into(),
            outcome: Outcome::Fail,
            required: true,
            log: log.map(String::from),
        }
    }

    fn red(failing: Vec<Check>) -> MainCi {
        MainCi {
            state: State::Red,
            sha: Some("abcdef0123456789abcdef0123456789abcdef01".into()),
            failing,
        }
    }

    fn issue(n: u64, title: &str, team: bool) -> Issue {
        Issue {
            number: n,
            title: title.into(),
            labels: Vec::new(),
            body: String::new(),
            team,
        }
    }

    fn repo() -> Repo {
        Repo {
            root: "/nonexistent".into(),
            common_dir: "/nonexistent".into(),
        }
    }

    #[test]
    fn the_title_names_the_short_sha_and_the_first_failing_check() {
        assert_eq!(
            title(&red(vec![failed("test", None)])),
            "fix: main is red at abcdef0: test"
        );
        assert_eq!(
            title(&red(vec![
                failed("test", None),
                failed("lint", None),
                failed("docs", None)
            ])),
            "fix: main is red at abcdef0: test (+2 more)"
        );
        assert!(title(&red(vec![failed("a", None)])).starts_with(TITLE_PREFIX));
    }

    #[test]
    fn the_excerpt_is_the_redacted_end_of_the_log() {
        let lines: Vec<String> = (1..=100).map(|i| format!("line {i}")).collect();
        let mut forge = Fake::default();
        forge.logs.insert("1".into(), lines.join("\n"));
        forge.logs.insert(
            "2".into(),
            "GH_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123\n```\nok".into(),
        );
        forge.logs.insert("3".into(), "é".repeat(LOG_BYTES));
        let tail = excerpt(&forge, &failed("t", Some("1")));
        assert!(tail.starts_with("```\nline 61\n"), "{tail}");
        assert!(tail.ends_with("line 100\n```"), "{tail}");
        assert!(!tail.contains("line 60\n"), "{tail}");
        let secret = excerpt(&forge, &failed("t", Some("2")));
        assert!(!secret.contains("ghp_"), "{secret}");
        assert_eq!(secret.matches("```").count(), 2, "{secret}");
        let wide = excerpt(&forge, &failed("t", Some("3")));
        assert!(wide.len() <= LOG_BYTES + 8, "{}", wide.len());
        assert!(excerpt(&forge, &failed("t", None)).contains("keeps none"));
        assert!(excerpt(&forge, &failed("t", Some("9"))).contains("No log: no log at 9"));
    }

    #[test]
    fn the_oldest_open_team_issue_with_the_prefix_is_reused() {
        let forge = Fake {
            issues: vec![
                issue(3, "fix: main is red at 1111111: x", false),
                issue(9, "fix: main is red at 2222222: y", true),
                issue(5, "fix: main is red at 3333333: z", true),
                issue(4, "fix: main is broken", true),
            ],
            ..Fake::default()
        };
        let (i, filed) =
            super::issue(&repo(), &forge, &Factory::default(), &red(vec![]), None).unwrap();
        assert_eq!((i.number, filed), (5, false));
        assert!(forge.filed.borrow().is_empty());
    }

    #[test]
    fn with_none_open_one_is_filed_ready_with_the_body() {
        let forge = Fake {
            issues: vec![issue(3, "fix: main is red at 1111111: x", false)],
            main: HeadChecks::default(),
            ..Fake::default()
        };
        let main = red(vec![failed("test", None)]);
        let (i, filed) = super::issue(&repo(), &forge, &Factory::default(), &main, None).unwrap();
        assert_eq!((i.number, filed), (100, true));
        assert_eq!(i.title, "fix: main is red at abcdef0: test");
        let all = forge.filed.borrow();
        let (title, body, labels) = &all[0];
        assert_eq!(title, &i.title);
        assert_eq!(
            labels,
            &["type:fix", "priority:critical", "status:ready-for-agent"]
        );
        assert_eq!(i.labels, *labels);
        assert!(body.contains("### test"), "{body}");
        assert!(
            body.contains("No green run on the default branch found"),
            "{body}"
        );
        assert!(body.ends_with(&format!("{DISCLAIMER}\n")), "{body}");
    }

    fn git_in(dir: &std::path::Path, args: &[&str]) -> String {
        let mut all = vec!["-c", "user.name=t", "-c", "user.email=t@t"];
        all.extend(args);
        git::run(dir, &all).unwrap().trim().to_string()
    }

    #[test]
    fn commits_list_at_most_the_cap_newest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_in(root, &["init", "-q"]);
        git_in(root, &["commit", "-q", "--allow-empty", "-m", "green"]);
        let green = git_in(root, &["rev-parse", "HEAD"]);
        let repo = Repo {
            root: root.into(),
            common_dir: root.join(".git"),
        };
        let forge = Fake {
            green: Some(green.clone()),
            ..Fake::default()
        };
        let list = |name: &str, n: usize| {
            for i in 0..n {
                git_in(
                    root,
                    &["commit", "-q", "--allow-empty", "-m", &format!("{name}{i}")],
                );
            }
            commits(&repo, &forge, &git_in(root, &["rev-parse", "HEAD"]))
        };
        let text = list("c", COMMITS);
        assert!(
            text.starts_with(&format!("Since {}:", &green[..7])),
            "{text}"
        );
        assert_eq!(text.matches("\n- ").count(), COMMITS, "{text}");
        assert!(text.contains(" c29") && !text.contains("..."), "{text}");
        let text = list("new", 1);
        assert_eq!(text.matches("\n- ").count(), COMMITS + 1, "{text}");
        assert!(
            text.ends_with("\n- ...") && text.contains(" new0\n") && !text.contains(" c0\n"),
            "{text}"
        );
    }

    #[test]
    fn the_issue_filed_tonight_is_reused_while_open_even_unlisted() {
        let mut forge = Fake::default();
        let tonight = issue(7, "fix: main is red at 1111111: x", true);
        let main = red(vec![failed("test", None)]);
        let fac = Factory::default();
        let (i, filed) = super::issue(&repo(), &forge, &fac, &main, Some(&tonight)).unwrap();
        assert_eq!((i.number, filed), (7, false));
        assert!(forge.filed.borrow().is_empty());
        forge.closed.insert(7);
        let (i, filed) = super::issue(&repo(), &forge, &fac, &main, Some(&tonight)).unwrap();
        assert_eq!((i.number, filed), (100, true));
    }

    #[test]
    fn commits_say_why_when_they_cannot_be_listed() {
        let forge = Fake {
            green: Some("1234567890".into()),
            ..Fake::default()
        };
        let text = commits(&repo(), &forge, "abc");
        assert!(
            text.starts_with("Cannot list the commits since 1234567"),
            "{text}"
        );
    }
}
