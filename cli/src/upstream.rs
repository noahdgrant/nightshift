//! `ns upstream status|diff`: compare adapted skills with the upstream path each records in
//! `metadata.upstream` (`owner/repo@sha:path`).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use serde::Serialize;
use serde_yaml::Value;

use crate::error::SfError;
use crate::{frontmatter, git, lint};

/// Where `owner/repo` is fetched from: `<base>/owner/repo`. git's `url.<base>.insteadOf`
/// redirects it, to a mirror or, in tests, to a local bare repo.
pub const GITHUB: &str = "https://github.com";

/// The ref each cached repo keeps upstream HEAD in.
const HEAD_REF: &str = "refs/ns/upstream-head";

/// Commits the first deepen adds when a recorded sha is older than the cached history.
const FIRST_DEEPEN: u32 = 64;

/// Bare clones of upstream repos under `<git-common-dir>/ns/upstream/<owner>/<repo>`, each
/// fetched at most once per invocation.
pub struct Cache {
    root: PathBuf,
    base: String,
    heads: HashMap<String, Result<String, String>>,
}

impl Cache {
    pub fn new(root: PathBuf, base: &str) -> Cache {
        Cache {
            root,
            base: base.to_string(),
            heads: HashMap::new(),
        }
    }

    fn dir(&self, repo: &str) -> PathBuf {
        self.root.join(repo)
    }

    /// `git fetch` of upstream HEAD into [`HEAD_REF`], with `extra` options.
    fn fetch(&self, repo: &str, extra: &[&str]) -> Result<()> {
        let url = format!("{}/{repo}", self.base);
        let refspec = format!("+HEAD:{HEAD_REF}");
        let mut args = vec!["fetch", "--quiet"];
        args.extend_from_slice(extra);
        args.extend([url.as_str(), refspec.as_str()]);
        let out = git::net(&self.dir(repo), &args).output()?;
        if !out.status.success() {
            return Err(SfError::general(format!(
                "git fetch {url} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ))
            .into());
        }
        Ok(())
    }

    /// Upstream HEAD's full sha. The first fetch into a new cache is shallow (`--depth=1`);
    /// later ones fetch only what is new.
    fn head(&mut self, repo: &str) -> Result<String> {
        if !self.heads.contains_key(repo) {
            let fetched = self.fetch_head(repo).map_err(|e| format!("{e:#}"));
            self.heads.insert(repo.to_string(), fetched);
        }
        self.heads[repo]
            .clone()
            .map_err(|e| SfError::general(e).into())
    }

    fn fetch_head(&self, repo: &str) -> Result<String> {
        let dir = self.dir(repo);
        if !dir.join("HEAD").is_file() {
            fs::create_dir_all(&dir)?;
            git::run(&dir, &["init", "--bare", "--quiet"])?;
        }
        let cached = git::ok(&dir, &["rev-parse", "--verify", "--quiet", HEAD_REF]);
        self.fetch(repo, if cached { &[] } else { &["--depth=1"] })?;
        git::run(&dir, &["rev-parse", HEAD_REF])
    }

    /// The full sha of `sha`, deepening the shallow cache until it holds the commit or the
    /// whole history.
    fn resolve(&self, repo: &str, sha: &str) -> Result<String> {
        let dir = self.dir(repo);
        let commit = format!("{sha}^{{commit}}");
        let mut depth = FIRST_DEEPEN;
        loop {
            if let Ok(full) = git::run(&dir, &["rev-parse", "--verify", "--quiet", &commit]) {
                return Ok(full);
            }
            if git::run(&dir, &["rev-parse", "--is-shallow-repository"])? != "true" {
                return Err(SfError::general(format!(
                    "recorded sha {sha} is not in {repo}'s history from HEAD"
                ))
                .into());
            }
            self.fetch(repo, &[&format!("--deepen={depth}")])?;
            depth = depth.saturating_mul(2);
        }
    }

    /// Fetch `up`'s repo and return `(recorded sha, upstream head)`, both full.
    fn range(&mut self, up: &lint::Upstream) -> Result<(String, String)> {
        let head = self.head(up.repo)?;
        let from = self.resolve(up.repo, up.sha)?;
        Ok((from, head))
    }

    /// Whether `up.path` differs between the recorded sha and upstream HEAD.
    fn changed(&mut self, up: &lint::Upstream) -> Result<(String, bool)> {
        let (from, head) = self.range(up)?;
        let out = git::command()
            .arg("-C")
            .arg(self.dir(up.repo))
            .args(["diff", "--quiet", &from, &head, "--", up.path])
            .output()?;
        match out.status.code() {
            Some(0) => Ok((head, false)),
            Some(1) => Ok((head, true)),
            _ => Err(SfError::general(format!(
                "git diff failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ))
            .into()),
        }
    }

    /// The diff of `up.path` from the recorded sha to upstream HEAD.
    fn diff(&mut self, up: &lint::Upstream) -> Result<(String, String)> {
        let (from, head) = self.range(up)?;
        let dir = self.dir(up.repo);
        let diff = git::run(&dir, &["diff", "--no-color", &from, &head, "--", up.path])?;
        Ok((head, diff))
    }
}

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

/// `metadata.upstream` of one skill's SKILL.md, as written. `None` without SKILL.md or valid
/// frontmatter; non-string entries come back as their YAML text so the caller can report them.
fn entries(skill_dir: &Path) -> Option<Vec<String>> {
    let text = fs::read_to_string(skill_dir.join("SKILL.md")).ok()?;
    let fm = frontmatter::parse(&text).ok()??;
    Some(
        lint::upstream_entries(&fm)
            .into_iter()
            .map(|v| match v {
                Value::String(s) => s.clone(),
                v => serde_yaml::to_string(v)
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            })
            .collect(),
    )
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
/// path changed upstream since its recorded sha.
pub fn status(skills_dir: &Path, cache: &mut Cache) -> Result<StatusReport> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(skills_dir)
        .map_err(|_| no_skills_dir(skills_dir))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("ns-"))
        })
        .collect();
    dirs.sort();
    let mut skills = Vec::new();
    for dir in dirs {
        let raws = entries(&dir).unwrap_or_default();
        if raws.is_empty() {
            continue;
        }
        let upstreams = raws
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
            .collect();
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

/// The upstream diff of each of `skill`'s `metadata.upstream` paths, recorded sha to HEAD.
pub fn diff(skills_dir: &Path, skill: &str, cache: &mut Cache) -> Result<DiffReport> {
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
    let raws = entries(&dir).unwrap_or_default();
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
    let repo = git::Repo::discover(&std::env::current_dir()?)?;
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
        fs::create_dir_all(work.join("tdd")).unwrap();
        let recorded = commit_file(&work, "tdd/SKILL.md", "v1\n");
        commit_file(&work, "other.md", "o1\n");
        commit_file(&work, "tdd/SKILL.md", "v2\n");
        let head = commit_file(&work, "README", "hi again\n");
        w.push(&work, "acme/skills");
        w.skill("ns-tdd", &format!(" acme/skills@{}:tdd", short(&recorded)));
        w.skill(
            "ns-other",
            &format!(" acme/skills@{}:README-missing", short(&recorded)),
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

    fn usage_code(e: anyhow::Error) -> i32 {
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
        assert_eq!(usage_code(e), 2);
        let e = diff(&w.skills(), "plain", &mut w.cache()).unwrap_err();
        assert!(e.to_string().contains("no metadata.upstream"), "{e}");
        assert_eq!(usage_code(e), 2);
        let e = diff(&w.root.join("none"), "plain", &mut w.cache()).unwrap_err();
        assert!(e.to_string().contains("does not exist"), "{e}");
        let e = status(&w.root.join("none"), &mut w.cache()).unwrap_err();
        assert_eq!(usage_code(e), 2);
    }

    #[test]
    fn diff_of_an_unreachable_upstream_fails() {
        let w = World::new();
        w.skill("ns-a", " acme/gone@1234567:a.md");
        let e = diff(&w.skills(), "ns-a", &mut w.cache()).unwrap_err();
        assert!(e.to_string().contains("git fetch"), "{e}");
        assert_eq!(usage_code(e), 1);
    }
}
