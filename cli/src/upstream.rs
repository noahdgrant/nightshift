//! `ns upstream status|diff`: compare adapted skills with the upstream path each records in
//! `metadata.upstream` (`owner/repo@sha:path`).

use std::fs;
use std::io;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use serde::Serialize;
use serde_yaml::Value;

use crate::error::SfError;
use crate::{frontmatter, git, install, lint};

mod cache;

use cache::{Cache, GITHUB};

#[derive(Debug, Serialize)]
pub struct EntryStatus {
    pub upstream: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SkillStatus {
    pub skill: String,
    pub upstreams: Vec<EntryStatus>,
}

#[derive(Debug, Serialize)]
pub struct StatusReport {
    pub ok: bool,
    pub changed: usize,
    pub skills: Vec<SkillStatus>,
}

/// `metadata.upstream` of one skill's SKILL.md, as written. Empty without frontmatter or the
/// key; non-string entries come back as their YAML text so the caller can report them.
fn entries(skill_dir: &Path) -> Result<Vec<String>> {
    let md = skill_dir.join("SKILL.md");
    let text = fs::read_to_string(&md).with_context(|| format!("cannot read {}", md.display()))?;
    let fm = frontmatter::parse(&text)
        .map_err(|e| anyhow::anyhow!("{}: invalid frontmatter: {e}", md.display()))?;
    Ok(fm
        .map(|fm| {
            lint::upstream_entries(&fm)
                .into_iter()
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    v => serde_yaml::to_string(v)
                        .unwrap_or_default()
                        .trim()
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default())
}

fn parse(raw: &str) -> Result<lint::Upstream<'_>> {
    lint::parse_upstream(raw).ok_or_else(|| {
        SfError::general(format!(
            "metadata.upstream {raw:?} must look like owner/repo@<hex sha>:path"
        ))
        .into()
    })
}

/// Every `ns-*` skill under `skills_dir` with a `metadata.upstream`, and whether each entry's
/// path changed upstream since its recorded sha. A skill whose SKILL.md cannot be read or parsed
/// is one error entry.
fn status(skills_dir: &Path, cache: &mut Cache) -> Result<StatusReport> {
    let dirs =
        install::skill_dirs(skills_dir).map_err(|e| match e.downcast_ref::<io::Error>() {
            Some(io) if io.kind() == io::ErrorKind::NotFound => {
                anyhow::Error::from(no_skills_dir(skills_dir))
            }
            _ => e,
        })?;
    let mut skills = Vec::new();
    for dir in dirs {
        let upstreams = match entries(&dir) {
            Err(e) => vec![EntryStatus {
                upstream: "SKILL.md".to_string(),
                head: None,
                changed: None,
                error: Some(format!("{e:#}")),
            }],
            Ok(raws) => raws
                .into_iter()
                .map(|raw| {
                    let result = parse(&raw).and_then(|up| cache.changed(&up));
                    let (head, changed, error) = match result {
                        Ok((head, changed)) => (Some(head), Some(changed), None),
                        Err(e) => (None, None, Some(format!("{e:#}"))),
                    };
                    EntryStatus {
                        upstream: raw,
                        head,
                        changed,
                        error,
                    }
                })
                .collect(),
        };
        if upstreams.is_empty() {
            continue;
        }
        skills.push(SkillStatus {
            skill: dir.file_name().unwrap().to_string_lossy().into_owned(),
            upstreams,
        });
    }
    let all = || skills.iter().flat_map(|s| &s.upstreams);
    Ok(StatusReport {
        ok: all().all(|e| e.error.is_none()),
        changed: all().filter(|e| e.changed == Some(true)).count(),
        skills,
    })
}

#[derive(Debug, Serialize)]
pub struct EntryDiff {
    pub upstream: String,
    pub head: String,
    pub diff: String,
}

#[derive(Debug, Serialize)]
pub struct DiffReport {
    pub skill: String,
    pub upstreams: Vec<EntryDiff>,
}

fn diff(skills_dir: &Path, skill: &str, cache: &mut Cache) -> Result<DiffReport> {
    let name = if skill.starts_with("ns-") {
        skill.to_string()
    } else {
        format!("ns-{skill}")
    };
    let dir = skills_dir.join(&name);
    if !dir.join("SKILL.md").is_file() {
        if !skills_dir.is_dir() {
            return Err(no_skills_dir(skills_dir).into());
        }
        return Err(SfError::usage(
            format!("no skill {name} in {}", skills_dir.display()),
            "list the skills with an upstream:\n  ns upstream status --human",
        )
        .into());
    }
    let raws = entries(&dir)?;
    if raws.is_empty() {
        return Err(SfError::usage(
            format!("{name} records no metadata.upstream, so it has no upstream to diff"),
            "list the skills with an upstream:\n  ns upstream status --human",
        )
        .into());
    }
    let mut upstreams = Vec::new();
    for raw in raws {
        let (head, diff) = cache.diff(&parse(&raw)?)?;
        upstreams.push(EntryDiff {
            upstream: raw,
            head,
            diff,
        });
    }
    Ok(DiffReport {
        skill: name,
        upstreams,
    })
}

fn no_skills_dir(dir: &Path) -> SfError {
    SfError::usage(
        format!("skills directory {} does not exist", dir.display()),
        "run from the repo root, or pass --skills-dir:\n  ns upstream status --skills-dir path/to/skills",
    )
}

fn cache_here() -> Result<Cache> {
    let here = std::env::current_dir()?;
    if !git::ok(&here, &["rev-parse", "--git-dir"]) {
        return Err(SfError::usage(
            format!("{} is not inside a git repository", here.display()),
            "the upstream cache lives in the repo's .git; run from inside the nightshift checkout:\n  cd path/to/repo && ns upstream status",
        )
        .into());
    }
    let repo = git::Repo::discover(&here)?;
    Ok(Cache::new(
        repo.common_dir.join("ns").join("upstream"),
        GITHUB,
    ))
}

pub fn status_cli(skills_dir: &Path, human: bool) -> Result<ExitCode> {
    let report = status(skills_dir, &mut cache_here()?)?;
    if human {
        for s in &report.skills {
            for e in &s.upstreams {
                let state = match (&e.error, e.changed) {
                    (Some(err), _) => format!("error: {err}"),
                    (None, Some(true)) => "changed".to_string(),
                    _ => "unchanged".to_string(),
                };
                println!("{}: {}: {state}", s.skill, e.upstream);
            }
        }
        println!("{} upstream paths changed", report.changed);
    } else {
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    Ok(if report.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

pub fn diff_cli(skills_dir: &Path, skill: &str, human: bool) -> Result<ExitCode> {
    let report = diff(skills_dir, skill, &mut cache_here()?)?;
    if human {
        for e in &report.upstreams {
            println!("=== {} -> {}", e.upstream, e.head);
            if e.diff.is_empty() {
                println!("(no changes)");
            } else {
                println!("{}", e.diff);
            }
        }
    } else {
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::cache::{FIRST_DEEPEN, HEAD_REF};
    use super::*;
    use crate::testutil::{commit_file, g};

    /// A scratch dir holding upstream bare repos under `up/`, the cache under `cache/` and
    /// skills under `skills/`.
    struct World {
        _tmp: tempfile::TempDir,
        root: PathBuf,
    }

    impl World {
        fn new() -> World {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().canonicalize().unwrap();
            fs::create_dir_all(root.join("skills")).unwrap();
            World { _tmp: tmp, root }
        }

        fn cache(&self) -> Cache {
            Cache::new(
                self.root.join("cache"),
                &format!("file://{}", self.root.join("up").display()),
            )
        }

        fn skills(&self) -> PathBuf {
            self.root.join("skills")
        }

        /// A work repo whose `main` is pushed to the bare upstream `up/<repo>` by [`push`].
        fn upstream(&self, repo: &str) -> PathBuf {
            let work = self.root.join("work").join(repo);
            fs::create_dir_all(&work).unwrap();
            g(&work, &["init", "-q", "-b", "main"]);
            let bare = self.root.join("up").join(repo);
            fs::create_dir_all(&bare).unwrap();
            g(&bare, &["init", "-q", "--bare", "-b", "main"]);
            work
        }

        fn push(&self, work: &Path, repo: &str) {
            let bare = self.root.join("up").join(repo);
            g(
                work,
                &["push", "-q", "--force", bare.to_str().unwrap(), "HEAD:main"],
            );
        }

        fn skill(&self, name: &str, upstream_yaml: &str) {
            let dir = self.skills().join(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: x\nmetadata:\n  upstream:{upstream_yaml}\n---\nbody\n"),
            )
            .unwrap();
        }
    }

    fn short(sha: &str) -> &str {
        &sha[..12]
    }

    fn entry<'a>(r: &'a StatusReport, skill: &str) -> &'a [EntryStatus] {
        &r.skills
            .iter()
            .find(|s| s.skill == skill)
            .unwrap()
            .upstreams
    }

    #[test]
    fn status_reports_whether_each_recorded_path_changed_since_its_sha() {
        let w = World::new();
        let work = w.upstream("acme/skills");
        commit_file(&work, "README", "hi\n");
        commit_file(&work, "stable.md", "s\n");
        fs::create_dir_all(work.join("tdd")).unwrap();
        let recorded = commit_file(&work, "tdd/SKILL.md", "v1\n");
        commit_file(&work, "other.md", "o1\n");
        commit_file(&work, "tdd/SKILL.md", "v2\n");
        let head = commit_file(&work, "README", "hi again\n");
        w.push(&work, "acme/skills");
        w.skill("ns-tdd", &format!(" acme/skills@{}:tdd", short(&recorded)));
        w.skill(
            "ns-other",
            &format!(" acme/skills@{}:stable.md", short(&recorded)),
        );
        let plain = w.skills().join("ns-plain");
        fs::create_dir_all(&plain).unwrap();
        fs::write(
            plain.join("SKILL.md"),
            "---\nname: ns-plain\ndescription: x\n---\n",
        )
        .unwrap();

        let not_a_skill = w.skills().join("tdd");
        fs::create_dir_all(&not_a_skill).unwrap();
        fs::copy(
            w.skills().join("ns-tdd/SKILL.md"),
            not_a_skill.join("SKILL.md"),
        )
        .unwrap();

        let r = status(&w.skills(), &mut w.cache()).unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(r.changed, 1);
        let names: Vec<&str> = r.skills.iter().map(|s| s.skill.as_str()).collect();
        assert_eq!(
            names,
            ["ns-other", "ns-tdd"],
            "only ns-* skills with an upstream"
        );
        let tdd = entry(&r, "ns-tdd");
        assert_eq!(tdd[0].changed, Some(true));
        assert_eq!(tdd[0].head.as_deref(), Some(head.as_str()));
        assert_eq!(entry(&r, "ns-other")[0].changed, Some(false));
    }

    #[test]
    fn a_path_untouched_since_its_sha_is_unchanged() {
        let w = World::new();
        let work = w.upstream("acme/skills");
        let recorded = commit_file(&work, "a.md", "a\n");
        commit_file(&work, "b.md", "b\n");
        w.push(&work, "acme/skills");
        w.skill("ns-a", &format!(" acme/skills@{}:a.md", short(&recorded)));
        let r = status(&w.skills(), &mut w.cache()).unwrap();
        assert_eq!(entry(&r, "ns-a")[0].changed, Some(false));
        assert_eq!(r.changed, 0);
    }

    #[test]
    fn the_first_fetch_is_shallow() {
        let w = World::new();
        let work = w.upstream("acme/skills");
        commit_file(&work, "a.md", "a\n");
        let head = commit_file(&work, "b.md", "b\n");
        w.push(&work, "acme/skills");
        w.skill("ns-a", &format!(" acme/skills@{}:a.md", short(&head)));
        let mut cache = w.cache();
        let r = status(&w.skills(), &mut cache).unwrap();
        assert_eq!(entry(&r, "ns-a")[0].changed, Some(false), "{r:?}");
        let dir = cache.dir("acme/skills");
        assert_eq!(
            git::run(&dir, &["rev-list", "--count", HEAD_REF]).unwrap(),
            "1"
        );
    }

    #[test]
    fn a_sha_older_than_the_shallow_cache_is_reached_by_deepening() {
        let w = World::new();
        let work = w.upstream("acme/skills");
        let recorded = commit_file(&work, "a.md", "a\n");
        for i in 0..3 {
            commit_file(&work, "b.md", &format!("{i}\n"));
        }
        w.push(&work, "acme/skills");
        w.skill("ns-a", &format!(" acme/skills@{}:a.md", short(&recorded)));
        let mut cache = w.cache();
        let r = status(&w.skills(), &mut cache).unwrap();
        assert!(r.ok, "{r:?}");
        let dir = cache.dir("acme/skills");
        assert_eq!(
            git::run(&dir, &["rev-parse", "--is-shallow-repository"]).unwrap(),
            "false",
            "deepening past the root unshallows"
        );

        commit_file(&work, "b.md", "new\n");
        w.push(&work, "acme/skills");
        status(&w.skills(), &mut w.cache()).unwrap();
        assert_eq!(
            git::run(&dir, &["rev-parse", "--is-shallow-repository"]).unwrap(),
            "false",
            "a later fetch keeps the history it has"
        );
    }

    #[test]
    fn a_sha_deeper_than_the_first_deepen_is_reached_by_doubling_deepens() {
        let w = World::new();
        let work = w.upstream("acme/skills");
        commit_file(&work, "a.md", "a\n");
        let empty_commits = |n: u32| {
            let tree = g(&work, &["rev-parse", "HEAD^{tree}"]);
            let mut tip = g(&work, &["rev-parse", "HEAD"]);
            for i in 0..n {
                tip = g(
                    &work,
                    &["commit-tree", &tree, "-p", &tip, "-m", &i.to_string()],
                );
            }
            g(&work, &["reset", "-q", "--hard", &tip]);
        };
        empty_commits(2 * FIRST_DEEPEN);
        let recorded = g(&work, &["rev-parse", "HEAD"]);
        empty_commits(FIRST_DEEPEN + 10);
        commit_file(&work, "a.md", "changed\n");
        w.push(&work, "acme/skills");
        w.skill("ns-a", &format!(" acme/skills@{}:a.md", short(&recorded)));
        let mut cache = w.cache();
        let r = status(&w.skills(), &mut cache).unwrap();
        assert_eq!(entry(&r, "ns-a")[0].changed, Some(true), "{r:?}");
        let fetched = git::run(
            &cache.dir("acme/skills"),
            &["rev-list", "--count", HEAD_REF],
        );
        assert_eq!(
            fetched.unwrap(),
            (1 + FIRST_DEEPEN + 2 * FIRST_DEEPEN).to_string(),
            "depth 1, then deepen by {FIRST_DEEPEN}, then by twice that"
        );
    }

    #[test]
    fn a_sha_missing_upstream_is_an_error_entry_and_the_report_fails() {
        let w = World::new();
        let work = w.upstream("acme/skills");
        commit_file(&work, "a.md", "a\n");
        commit_file(&work, "a.md", "b\n");
        w.push(&work, "acme/skills");
        w.skill("ns-a", " acme/skills@0123456789ab:a.md");
        let r = status(&w.skills(), &mut w.cache()).unwrap();
        assert!(!r.ok);
        let e = &entry(&r, "ns-a")[0];
        assert_eq!(e.changed, None);
        assert!(
            e.error.as_deref().unwrap().contains("0123456789ab"),
            "{e:?}"
        );
    }

    #[test]
    fn an_unreachable_upstream_and_a_malformed_entry_are_error_entries() {
        let w = World::new();
        let work = w.upstream("acme/skills");
        let sha = commit_file(&work, "a.md", "a\n");
        w.push(&work, "acme/skills");
        w.skill(
            "ns-a",
            &format!(
                "\n    - acme/skills@{0}:a.md\n    - acme/gone@{0}:a.md\n    - acme/skills@main:a.md\n    - 1234567",
                short(&sha)
            ),
        );
        let r = status(&w.skills(), &mut w.cache()).unwrap();
        assert!(!r.ok);
        let es = entry(&r, "ns-a");
        assert_eq!(es.len(), 4);
        assert_eq!(es[0].changed, Some(false));
        assert!(
            es[1].error.as_deref().unwrap().contains("git fetch"),
            "{es:?}"
        );
        assert!(
            es[2].error.as_deref().unwrap().contains("owner/repo@"),
            "{es:?}"
        );
        assert_eq!(es[3].upstream, "1234567");
        assert!(es[3].error.is_some());
    }

    #[test]
    fn a_path_in_neither_the_recorded_sha_nor_head_is_an_error_entry() {
        let w = World::new();
        let work = w.upstream("acme/skills");
        let recorded = commit_file(&work, "a.md", "a\n");
        commit_file(&work, "a.md", "b\n");
        w.push(&work, "acme/skills");
        w.skill(
            "ns-a",
            &format!(" acme/skills@{}:typo.md", short(&recorded)),
        );
        let r = status(&w.skills(), &mut w.cache()).unwrap();
        assert!(!r.ok);
        let e = &entry(&r, "ns-a")[0];
        assert_eq!(e.changed, None);
        assert!(
            e.error
                .as_deref()
                .unwrap()
                .contains("path typo.md not found at recorded sha or HEAD"),
            "{e:?}"
        );
    }

    #[test]
    fn a_path_added_or_removed_upstream_is_changed() {
        let w = World::new();
        let work = w.upstream("acme/skills");
        let recorded = commit_file(&work, "gone.md", "g\n");
        g(&work, &["rm", "-q", "gone.md"]);
        g(&work, &["commit", "-qm", "rm"]);
        commit_file(&work, "new.md", "n\n");
        w.push(&work, "acme/skills");
        w.skill(
            "ns-a",
            &format!(
                "\n    - acme/skills@{0}:gone.md\n    - acme/skills@{0}:new.md",
                short(&recorded)
            ),
        );
        let r = status(&w.skills(), &mut w.cache()).unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(r.changed, 2);
    }

    #[test]
    fn an_owner_or_repo_of_dots_is_an_error_entry_and_touches_no_cache_dir() {
        let w = World::new();
        w.skill(
            "ns-a",
            "\n    - ../..@abc1234:x\n    - a/..@abc1234:x\n    - a/.b@abc1234:x",
        );
        let mut cache = w.cache();
        git::reset_spawns();
        let r = status(&w.skills(), &mut cache).unwrap();
        assert!(!r.ok);
        for e in entry(&r, "ns-a") {
            assert!(e.error.as_deref().unwrap().contains("owner/repo@"), "{e:?}");
        }
        assert_eq!(git::spawns(), 0);
        assert!(!w.root.join("cache").exists());
    }

    #[test]
    fn a_skill_with_unparseable_frontmatter_is_an_error_entry_and_diff_fails() {
        let w = World::new();
        let broken = w.skills().join("ns-broken");
        fs::create_dir_all(&broken).unwrap();
        fs::write(broken.join("SKILL.md"), "---\nname: [unclosed\n---\nbody\n").unwrap();
        let r = status(&w.skills(), &mut w.cache()).unwrap();
        assert!(!r.ok);
        let e = &entry(&r, "ns-broken")[0];
        assert!(
            e.error.as_deref().unwrap().contains("invalid frontmatter"),
            "{e:?}"
        );
        let e = diff(&w.skills(), "ns-broken", &mut w.cache()).unwrap_err();
        assert!(e.to_string().contains("invalid frontmatter"), "{e}");
    }

    #[test]
    fn an_unreadable_skill_md_is_an_error_entry() {
        let w = World::new();
        let dir = w.skills().join("ns-bin");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), [0xff, 0xfe, 0xfd]).unwrap();
        let r = status(&w.skills(), &mut w.cache()).unwrap();
        assert!(!r.ok);
        let e = &entry(&r, "ns-bin")[0];
        assert!(e.error.as_deref().unwrap().contains("cannot read"), "{e:?}");
    }

    #[test]
    fn a_sha_beyond_the_deepen_ceiling_is_an_error_naming_the_ceiling() {
        let w = World::new();
        let work = w.upstream("acme/skills");
        commit_file(&work, "a.md", "a\n");
        let tree = g(&work, &["rev-parse", "HEAD^{tree}"]);
        let mut tip = g(&work, &["rev-parse", "HEAD"]);
        for i in 0..300 {
            tip = g(
                &work,
                &["commit-tree", &tree, "-p", &tip, "-m", &i.to_string()],
            );
        }
        g(&work, &["reset", "-q", "--hard", &tip]);
        w.push(&work, "acme/skills");
        w.skill("ns-a", " acme/skills@0123456789ab:a.md");
        let mut cache = w.cache();
        cache.max_deepen = 100;
        let r = status(&w.skills(), &mut cache).unwrap();
        let e = &entry(&r, "ns-a")[0];
        assert!(
            e.error
                .as_deref()
                .unwrap()
                .contains("not in acme/skills's history within 100 commits"),
            "{e:?}"
        );
        assert_eq!(
            git::run(
                &cache.dir("acme/skills"),
                &["rev-list", "--count", HEAD_REF]
            )
            .unwrap(),
            "101"
        );
    }

    /// Every fetch keeps its own pack and, under the `gc` strategy on any git version, a second
    /// pack starts gc, so the deepening fetch runs one. Detached, it is still repacking when
    /// `status` returns.
    #[test]
    fn a_fetch_finishes_its_auto_gc_before_the_next_fetch_into_the_cache() {
        let w = World::new();
        let work = w.upstream("acme/skills");
        let recorded = commit_file(&work, "a.md", "a\n");
        for i in 0..3 {
            commit_file(&work, "b.md", &format!("{i}\n"));
        }
        w.push(&work, "acme/skills");
        w.skill("ns-a", &format!(" acme/skills@{}:a.md", short(&recorded)));
        let mut cache = w.cache();
        let dir = cache.dir("acme/skills");
        fs::create_dir_all(&dir).unwrap();
        g(&dir, &["init", "-q", "--bare"]);
        for (k, v) in [
            ("maintenance.strategy", "gc"),
            ("gc.auto", "1"),
            ("gc.autoPackLimit", "1"),
            ("fetch.unpackLimit", "1"),
        ] {
            g(&dir, &["config", k, v]);
        }
        let r = status(&w.skills(), &mut cache).unwrap();
        assert!(r.ok, "{r:?}");
        let counts = g(&dir, &["count-objects", "-v"]);
        assert!(
            counts.lines().any(|l| l == "packs: 1"),
            "the deepening fetch left its gc running: {counts}"
        );
    }

    #[test]
    fn two_entries_on_one_unreachable_repo_both_error_after_one_failed_fetch() {
        let w = World::new();
        w.skill("ns-a", " acme/gone@1234567:a.md");
        w.skill("ns-b", " acme/gone@1234567:b.md");
        let spawns = |skills: &Path| {
            git::reset_spawns();
            let r = status(skills, &mut w.cache()).unwrap();
            (r, git::spawns())
        };
        let (r, two) = spawns(&w.skills());
        assert!(entry(&r, "ns-a")[0].error.is_some());
        assert!(entry(&r, "ns-b")[0].error.is_some());
        fs::remove_dir_all(w.skills().join("ns-b")).unwrap();
        fs::remove_dir_all(w.root.join("cache")).unwrap();
        let (_, one) = spawns(&w.skills());
        assert_eq!(two, one, "the second entry spawned git again");
    }

    #[test]
    fn a_git_diff_failure_is_an_error_entry() {
        let w = World::new();
        let work = w.upstream("acme/skills");
        let recorded = commit_file(&work, "a.md", "a\n");
        commit_file(&work, "a.md", "b\n");
        w.push(&work, "acme/skills");
        w.skill("ns-a", &format!(" acme/skills@{}:a.md", short(&recorded)));
        let mut cache = w.cache();
        status(&w.skills(), &mut cache).unwrap();
        g(
            &cache.dir("acme/skills"),
            &["config", "diff.algorithm", "bogus"],
        );
        let r = status(&w.skills(), &mut w.cache()).unwrap();
        let e = &entry(&r, "ns-a")[0];
        assert!(
            e.error.as_deref().unwrap().contains("git diff failed"),
            "{e:?}"
        );
    }

    #[test]
    fn a_later_run_sees_new_upstream_commits() {
        let w = World::new();
        let work = w.upstream("acme/skills");
        let recorded = commit_file(&work, "a.md", "a\n");
        w.push(&work, "acme/skills");
        w.skill("ns-a", &format!(" acme/skills@{}:a.md", short(&recorded)));
        let r = status(&w.skills(), &mut w.cache()).unwrap();
        assert_eq!(r.changed, 0);

        commit_file(&work, "a.md", "changed\n");
        w.push(&work, "acme/skills");
        let r = status(&w.skills(), &mut w.cache()).unwrap();
        assert_eq!(entry(&r, "ns-a")[0].changed, Some(true), "{r:?}");
    }

    #[test]
    fn a_second_entry_on_the_same_repo_reuses_the_first_fetch() {
        let w = World::new();
        let work = w.upstream("acme/skills");
        let recorded = commit_file(&work, "a.md", "a\n");
        w.push(&work, "acme/skills");
        w.skill("ns-a", &format!(" acme/skills@{}:a.md", short(&recorded)));
        status(&w.skills(), &mut w.cache()).unwrap();
        let spawns = || {
            git::reset_spawns();
            status(&w.skills(), &mut w.cache()).unwrap();
            git::spawns()
        };
        let one = spawns();
        w.skill("ns-b", &format!(" acme/skills@{}:b.md", short(&recorded)));
        let two = spawns();
        assert!(two - one < one, "one entry: {one} git runs, two: {two}");
    }

    #[test]
    fn diff_prints_each_entrys_path_diff_from_its_sha_to_head() {
        let w = World::new();
        let work = w.upstream("acme/skills");
        commit_file(&work, "c.md", "c\n");
        let recorded = commit_file(&work, "a.md", "old a\n");
        commit_file(&work, "b.md", "b1\n");
        commit_file(&work, "a.md", "new a\n");
        let head = commit_file(&work, "b.md", "b2\n");
        w.push(&work, "acme/skills");
        w.skill(
            "ns-a",
            &format!(
                "\n    - acme/skills@{0}:a.md\n    - acme/skills@{0}:c.md",
                short(&recorded)
            ),
        );
        let r = diff(&w.skills(), "a", &mut w.cache()).unwrap();
        assert_eq!(r.skill, "ns-a");
        assert_eq!(r.upstreams.len(), 2);
        let a = &r.upstreams[0];
        assert_eq!(a.head, head);
        assert!(a.diff.contains("-old a\n+new a"), "{}", a.diff);
        assert!(!a.diff.contains("b.md"), "{}", a.diff);
        assert_eq!(r.upstreams[1].diff, "");
    }

    #[test]
    fn diff_of_a_path_in_neither_revision_is_an_error() {
        let w = World::new();
        let work = w.upstream("acme/skills");
        let recorded = commit_file(&work, "a.md", "a\n");
        commit_file(&work, "a.md", "b\n");
        w.push(&work, "acme/skills");
        w.skill(
            "ns-a",
            &format!(" acme/skills@{}:typo.md", short(&recorded)),
        );
        let e = diff(&w.skills(), "ns-a", &mut w.cache()).unwrap_err();
        assert!(
            e.to_string()
                .contains("path typo.md not found at recorded sha or HEAD"),
            "{e}"
        );
        assert_eq!(usage_code(&e), 1);
    }

    #[test]
    fn two_entries_recording_one_missing_sha_deepen_once() {
        let w = World::new();
        let work = w.upstream("acme/skills");
        commit_file(&work, "a.md", "a\n");
        commit_file(&work, "a.md", "b\n");
        w.push(&work, "acme/skills");
        w.skill("ns-a", " acme/skills@0123456789ab:a.md");
        w.skill("ns-b", " acme/skills@0123456789ab:b.md");
        let spawns = |skills: &Path| {
            git::reset_spawns();
            let r = status(skills, &mut w.cache()).unwrap();
            (r, git::spawns())
        };
        let (r, two) = spawns(&w.skills());
        assert!(entry(&r, "ns-a")[0].error.is_some());
        assert!(entry(&r, "ns-b")[0].error.is_some());
        fs::remove_dir_all(w.skills().join("ns-b")).unwrap();
        fs::remove_dir_all(w.root.join("cache")).unwrap();
        let (_, one) = spawns(&w.skills());
        assert_eq!(two, one, "the second entry deepened again");
    }

    #[test]
    fn a_skill_md_without_frontmatter_is_left_out_of_status() {
        let w = World::new();
        let dir = w.skills().join("ns-bare");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "just a body\n").unwrap();
        let r = status(&w.skills(), &mut w.cache()).unwrap();
        assert!(r.ok);
        assert!(r.skills.is_empty(), "{r:?}");
    }

    fn usage_code(e: &anyhow::Error) -> i32 {
        e.downcast_ref::<SfError>().unwrap().code
    }

    #[test]
    fn diff_of_an_unknown_skill_or_one_without_upstream_is_a_usage_error() {
        let w = World::new();
        let plain = w.skills().join("ns-plain");
        fs::create_dir_all(&plain).unwrap();
        fs::write(
            plain.join("SKILL.md"),
            "---\nname: ns-plain\ndescription: x\n---\n",
        )
        .unwrap();
        let e = diff(&w.skills(), "ns-nope", &mut w.cache()).unwrap_err();
        assert!(e.to_string().contains("no skill ns-nope"), "{e}");
        assert_eq!(usage_code(&e), 2);
        let e = diff(&w.skills(), "plain", &mut w.cache()).unwrap_err();
        assert!(e.to_string().contains("no metadata.upstream"), "{e}");
        assert_eq!(usage_code(&e), 2);
        let e = diff(&w.root.join("none"), "plain", &mut w.cache()).unwrap_err();
        assert!(e.to_string().contains("does not exist"), "{e}");
        let e = status(&w.root.join("none"), &mut w.cache()).unwrap_err();
        assert_eq!(usage_code(&e), 2);
    }

    #[test]
    fn diff_of_an_unreachable_upstream_fails() {
        let w = World::new();
        w.skill("ns-a", " acme/gone@1234567:a.md");
        let e = diff(&w.skills(), "ns-a", &mut w.cache()).unwrap_err();
        assert!(e.to_string().contains("git fetch"), "{e}");
        assert_eq!(usage_code(&e), 1);
    }
}
