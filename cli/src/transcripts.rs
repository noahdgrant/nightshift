//! `ns transcripts`: list and read harness session transcripts, and track which were processed.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::clock;
use crate::error::SfError;

const FULL_LIMIT: usize = 2_000;
pub(crate) const COMPACT_LIMIT: usize = 200;
const HEADER_LINES: usize = 50;
const SOURCE: &str = "claude";

#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub id: String,
    pub source: &'static str,
    pub date: String,
    pub project: String,
    pub size: u64,
    pub mtime: i64,
    pub path: PathBuf,
    #[serde(skip)]
    pub key: String,
    #[serde(skip)]
    pub cwd: Option<String>,
    #[serde(skip)]
    pub dir: String,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Entry {
    User { text: String },
    Assistant { text: String },
    Tool { name: String, input: String },
    Output { text: String },
    Error { text: String },
}

pub struct Located {
    pub id: String,
    pub key: String,
    pub path: PathBuf,
    pub size: u64,
    pub mtime: i64,
}

pub trait Reader {
    fn locate(&self) -> Result<Vec<Located>>;
    fn load(&self, located: Located) -> Result<Option<Session>>;
    fn entries(&self, session: &Session, limit: usize) -> Result<Vec<Entry>>;
    fn in_project(&self, session: &Session, project: &Path) -> bool;
}

fn skippable(kind: io::ErrorKind) -> bool {
    matches!(
        kind,
        io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
    )
}

/// Claude Code: `$CLAUDE_CONFIG_DIR/projects` (default `~/.claude/projects`), one
/// `<slug>/<session-id>.jsonl` per session, where the slug is the cwd with every
/// non-alphanumeric character replaced by `-`.
pub struct Claude {
    projects: PathBuf,
}

impl Claude {
    pub fn from_env() -> Result<Self> {
        let root = match env_path("CLAUDE_CONFIG_DIR") {
            Some(dir) => dir,
            None => home()?.join(".claude"),
        };
        Ok(Self {
            projects: root.join("projects"),
        })
    }
}

fn json_values(reader: impl BufRead) -> impl Iterator<Item = io::Result<Value>> {
    reader
        .split(b'\n')
        .filter_map(|line| line.map(|l| serde_json::from_slice(&l).ok()).transpose())
}

fn json_lines(path: &Path) -> io::Result<impl Iterator<Item = io::Result<Value>>> {
    Ok(json_values(BufReader::new(fs::File::open(path)?)))
}

fn mtime_secs(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn first_timestamp_and_cwd(path: &Path) -> io::Result<(Option<String>, Option<String>)> {
    let (mut date, mut cwd) = (None, None);
    for v in json_lines(path)?.take(HEADER_LINES) {
        let v = v?;
        date = date.or_else(|| v["timestamp"].as_str().map(str::to_string));
        cwd = cwd.or_else(|| v["cwd"].as_str().map(str::to_string));
        if date.is_some() && cwd.is_some() {
            break;
        }
    }
    Ok((date, cwd))
}

pub fn claude_slug(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn project_candidates(p: &Path) -> Vec<PathBuf> {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let plain: PathBuf = abs.components().collect();
    let mut out = vec![plain.clone()];
    if let Ok(real) = plain.canonicalize() {
        if real != plain {
            out.push(real);
        }
    }
    out
}

fn tolerate<T>(result: io::Result<T>, path: &Path) -> Result<Option<T>> {
    match result {
        Ok(v) => Ok(Some(v)),
        Err(e) if skippable(e.kind()) => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

fn read_dir_paths(dir: &Path) -> Result<Vec<PathBuf>> {
    let Some(entries) = tolerate(fs::read_dir(dir), dir)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => out.push(entry.path()),
            Err(e) if skippable(e.kind()) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(out)
}

fn located_from(dir: &Path, path: PathBuf) -> Option<Located> {
    if path.extension().is_none_or(|e| e != "jsonl") {
        return None;
    }
    let meta = fs::metadata(&path).ok().filter(fs::Metadata::is_file)?;
    let id = path.file_stem().unwrap_or_default().to_string_lossy();
    Some(Located {
        key: format!("{SOURCE}:{}/{id}", name_of(dir)),
        id: id.into_owned(),
        size: meta.len(),
        mtime: mtime_secs(&meta),
        path,
    })
}

impl Reader for Claude {
    fn locate(&self) -> Result<Vec<Located>> {
        let mut out = Vec::new();
        for dir in read_dir_paths(&self.projects)? {
            if dir.is_dir() {
                let files = read_dir_paths(&dir)?;
                out.extend(files.into_iter().filter_map(|p| located_from(&dir, p)));
            }
        }
        Ok(out)
    }

    fn load(&self, located: Located) -> Result<Option<Session>> {
        let Located {
            id,
            key,
            path,
            size,
            mtime,
        } = located;
        let Some((date, cwd)) = tolerate(first_timestamp_and_cwd(&path), &path)? else {
            return Ok(None);
        };
        let dir = path.parent().map(name_of).unwrap_or_default();
        Ok(Some(Session {
            id,
            source: SOURCE,
            date: date.unwrap_or_else(|| clock::iso(mtime)),
            project: cwd.clone().unwrap_or_else(|| dir.clone()),
            size,
            mtime,
            key,
            cwd,
            dir,
            path,
        }))
    }

    fn entries(&self, session: &Session, limit: usize) -> Result<Vec<Entry>> {
        let path = &session.path;
        let lines = json_lines(path).with_context(|| format!("open {}", path.display()))?;
        let mut out = Vec::new();
        for v in lines {
            let v = v.with_context(|| format!("read {}", path.display()))?;
            if v["isMeta"] != true {
                out.extend(line_entries(&v, limit));
            }
        }
        Ok(out)
    }

    fn in_project(&self, session: &Session, project: &Path) -> bool {
        let candidates = project_candidates(project);
        match &session.cwd {
            Some(cwd) => candidates.iter().any(|p| Path::new(cwd) == p),
            None => candidates.iter().any(|p| session.dir == claude_slug(p)),
        }
    }
}

struct Capped {
    text: String,
    kept: usize,
    dropped: usize,
    limit: usize,
}

impl Capped {
    fn new(limit: usize) -> Self {
        Capped {
            text: String::new(),
            kept: 0,
            dropped: 0,
            limit,
        }
    }

    fn push(&mut self, piece: &str) {
        for c in piece.chars() {
            if self.kept < self.limit {
                self.text.push(c);
                self.kept += 1;
            } else {
                self.dropped += 1;
            }
        }
    }

    fn finish(self) -> String {
        if self.dropped == 0 {
            self.text
        } else {
            format!("{}…[truncated {} chars]", self.text, self.dropped)
        }
    }
}

impl io::Write for Capped {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.push(&String::from_utf8_lossy(buf));
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn capped_json(v: &Value, limit: usize) -> String {
    let mut out = Capped::new(limit);
    let _ = serde_json::to_writer(&mut out, v);
    out.finish()
}

pub(crate) fn line_entries(v: &Value, limit: usize) -> Vec<Entry> {
    let content = &v["message"]["content"];
    match v["type"].as_str() {
        Some("user") => user_entries(content, limit),
        Some("assistant") => assistant_entries(content, limit),
        Some("system") if v["level"] == "error" => vec![Entry::Error {
            text: text_of(&v["content"], limit),
        }],
        _ => Vec::new(),
    }
}

fn user_entries(content: &Value, limit: usize) -> Vec<Entry> {
    if content.is_string() {
        return vec![Entry::User {
            text: text_of(content, limit),
        }];
    }
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| match item["type"].as_str() {
            Some("text") => Some(Entry::User {
                text: text_of(&item["text"], limit),
            }),
            Some("tool_result") => {
                let text = text_of(&item["content"], limit);
                Some(if item["is_error"] == true {
                    Entry::Error { text }
                } else {
                    Entry::Output { text }
                })
            }
            _ => None,
        })
        .collect()
}

fn assistant_entries(content: &Value, limit: usize) -> Vec<Entry> {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| match item["type"].as_str() {
            Some("text") => Some(Entry::Assistant {
                text: text_of(&item["text"], limit),
            }),
            Some("tool_use") => Some(Entry::Tool {
                name: text_of(&item["name"], limit),
                input: capped_json(&item["input"], limit),
            }),
            _ => None,
        })
        .collect()
}

fn text_of(v: &Value, limit: usize) -> String {
    let mut out = Capped::new(limit);
    match v {
        Value::String(s) => out.push(s),
        Value::Array(items) => {
            for (n, text) in items.iter().filter_map(|i| i["text"].as_str()).enumerate() {
                if n > 0 {
                    out.push("\n");
                }
                out.push(text);
            }
        }
        Value::Null => {}
        other => {
            let _ = serde_json::to_writer(&mut out, other);
        }
    }
    out.finish()
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn home() -> Result<PathBuf> {
    env_path("HOME").ok_or_else(|| {
        SfError::general("HOME is not set")
            .hint("set HOME, or CLAUDE_CONFIG_DIR and XDG_DATA_HOME")
            .into()
    })
}

pub fn index_path() -> Result<PathBuf> {
    let data = match env_path("XDG_DATA_HOME") {
        Some(dir) => dir,
        None => home()?.join(".local/share"),
    };
    Ok(data.join("nightshift/transcripts-index.json"))
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    sessions: BTreeMap<String, Seen>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Seen {
    size: u64,
    mtime: i64,
}

impl Seen {
    fn of(l: &Located) -> Self {
        Self {
            size: l.size,
            mtime: l.mtime,
        }
    }
}

impl Index {
    fn load(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).with_context(|| format!("parse {}", path.display())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
        }
    }

    fn save(&self, path: &Path) -> Result<()> {
        let dir = path.parent().context("index path has no parent")?;
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        fs::rename(&tmp, path).with_context(|| format!("write {}", path.display()))
    }

    fn record(path: &Path, seen: Vec<(String, Seen)>) -> Result<()> {
        let mut fresh = Self::load(path)?;
        fresh.sessions.extend(seen);
        fresh.save(path)
    }

    fn is_new(&self, l: &Located) -> bool {
        self.sessions.get(&l.key) != Some(&Seen::of(l))
    }
}

pub struct ListArgs {
    pub since: Option<String>,
    pub project: Option<PathBuf>,
    pub new: bool,
    pub mark: bool,
}

fn before_since(timestamp: &str, since: &str) -> bool {
    timestamp.get(..10).is_none_or(|day| day < since)
}

fn precheck(args: &ListArgs, index: &Index, located: &Located) -> bool {
    if args.new && !index.is_new(located) {
        return false;
    }
    args.since
        .as_deref()
        .is_none_or(|since| !before_since(&clock::iso(located.mtime), since))
}

fn postcheck(args: &ListArgs, reader: &dyn Reader, session: &Session) -> bool {
    if args
        .since
        .as_deref()
        .is_some_and(|since| before_since(&session.date, since))
    {
        return false;
    }
    args.project
        .as_deref()
        .is_none_or(|project| reader.in_project(session, project))
}

fn scan(reader: &dyn Reader, args: &ListArgs, index: &Index) -> Result<Vec<(Session, Seen)>> {
    let mut out = Vec::new();
    for located in reader.locate()? {
        if !precheck(args, index, &located) {
            continue;
        }
        let seen = Seen::of(&located);
        let Some(session) = reader.load(located)? else {
            continue;
        };
        if postcheck(args, reader, &session) {
            out.push((session, seen));
        }
    }
    Ok(out)
}

fn check_since(since: &str) -> Result<()> {
    if since.len() == 10 && clock::parse_iso(since).is_some() {
        return Ok(());
    }
    Err(SfError::usage(
        format!("--since takes a date as YYYY-MM-DD, got {since:?}"),
        "ns transcripts list --since 2026-10-01",
    )
    .into())
}

pub fn list(args: &ListArgs) -> Result<ExitCode> {
    if let Some(since) = &args.since {
        check_since(since)?;
    }
    let index_path = index_path()?;
    let index = if args.new || args.mark {
        Index::load(&index_path)?
    } else {
        Index::default()
    };
    let mut sessions = Vec::new();
    let mut seen = Vec::new();
    for r in readers()? {
        for (session, seen_as) in scan(r.as_ref(), args, &index)? {
            seen.push((session.key.clone(), seen_as));
            sessions.push(session);
        }
    }
    sessions.sort_by(|a, b| (&a.date, &a.id, &a.key).cmp(&(&b.date, &b.id, &b.key)));
    if args.mark {
        Index::record(&index_path, seen)?;
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "index": index_path,
            "marked": args.mark,
            "sessions": sessions,
        }))?
    );
    Ok(ExitCode::SUCCESS)
}

fn no_session(id: &str) -> anyhow::Error {
    SfError::general(format!("no session {id:?}"))
        .hint("list sessions:  ns transcripts list")
        .into()
}

fn resolve<'a>(
    mut found: Vec<(&'a dyn Reader, Located)>,
    id: &str,
) -> Result<(&'a dyn Reader, Located)> {
    if found.iter().any(|(_, l)| l.id == id) {
        found.retain(|(_, l)| l.id == id);
    } else {
        found.retain(|(_, l)| l.id.starts_with(id));
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(no_session(id)),
        n => {
            let shown: Vec<_> = found
                .iter()
                .map(|(_, l)| format!("{} ({})", l.id, l.path.display()))
                .collect();
            let first = &found[0].1.id;
            let hint = if found.iter().all(|(_, l)| l.id == *first) {
                "the id exists in several project directories; remove or rename one copy"
                    .to_string()
            } else {
                format!("ns transcripts read {first}")
            };
            Err(SfError::usage(
                format!("{id:?} matches {n} sessions: {}", shown.join(", ")),
                hint,
            )
            .into())
        }
    }
}

pub fn read(id: &str, compact: bool) -> Result<ExitCode> {
    if id.is_empty() {
        return Err(
            SfError::usage("session id is empty", "list sessions:  ns transcripts list").into(),
        );
    }
    let limit = if compact { COMPACT_LIMIT } else { FULL_LIMIT };
    let readers = readers()?;
    let mut found = Vec::new();
    for r in &readers {
        found.extend(r.locate()?.into_iter().map(|l| (r.as_ref(), l)));
    }
    let (reader, located) = resolve(found, id)?;
    let session = reader.load(located)?.ok_or_else(|| no_session(id))?;
    let entries = reader.entries(&session, limit)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "id": session.id,
            "source": session.source,
            "date": session.date,
            "project": session.project,
            "path": session.path,
            "entries": entries,
        }))?
    );
    Ok(ExitCode::SUCCESS)
}

fn readers() -> Result<Vec<Box<dyn Reader>>> {
    Ok(vec![Box::new(Claude::from_env()?)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tolerate_skips_missing_and_permission_denied_but_not_other_errors() {
        let path = Path::new("/x");
        for kind in [io::ErrorKind::NotFound, io::ErrorKind::PermissionDenied] {
            assert!(tolerate::<u8>(Err(kind.into()), path).unwrap().is_none());
        }
        assert_eq!(tolerate(Ok(7), path).unwrap(), Some(7));
        let err = tolerate::<u8>(Err(io::ErrorKind::InvalidData.into()), path).unwrap_err();
        assert!(err.to_string().contains("read /x"), "{err}");
    }

    #[test]
    fn slug_replaces_every_non_alphanumeric() {
        assert_eq!(
            claude_slug(Path::new("/work/.dotfiles/my_repo")),
            "-work--dotfiles-my-repo"
        );
    }

    #[test]
    fn truncate_marks_the_dropped_count_on_a_char_boundary() {
        assert_eq!(
            text_of(&json!("héllo wörld"), 4),
            "héll…[truncated 7 chars]"
        );
        assert_eq!(text_of(&json!("ok"), 4), "ok");
        assert_eq!(text_of(&json!("ééé"), 3), "ééé");
        let arr = json!([{"text": "ab"}, {"text": "cd"}]);
        assert_eq!(text_of(&arr, 3), "ab\n…[truncated 2 chars]");
        assert_eq!(
            text_of(&json!("😀😀😀😀😀"), 3),
            "😀😀😀…[truncated 2 chars]"
        );
    }

    #[test]
    fn capped_json_cuts_the_serialized_input_on_a_char_boundary() {
        let v = json!({"k": "héllo"});
        assert_eq!(capped_json(&v, 100), v.to_string());
        assert_eq!(capped_json(&v, 8), "{\"k\":\"hé…[truncated 5 chars]");
    }

    #[test]
    fn record_merges_into_what_is_on_disk_now() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("idx/index.json");
        let mut other = Index::default();
        other
            .sessions
            .insert("a".into(), Seen { size: 1, mtime: 1 });
        other.save(&path).unwrap();
        Index::record(&path, vec![("b".into(), Seen { size: 2, mtime: 2 })]).unwrap();
        let merged = Index::load(&path).unwrap();
        assert_eq!(merged.sessions.keys().collect::<Vec<_>>(), ["a", "b"]);
    }

    #[test]
    fn text_of_joins_content_blocks() {
        let v = json!([{"type": "text", "text": "a"}, {"type": "text", "text": "b"}]);
        assert_eq!(text_of(&v, 100), "a\nb");
        assert_eq!(text_of(&json!("s"), 100), "s");
    }

    struct FailsAfter(&'static [u8]);

    impl io::Read for FailsAfter {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.0.is_empty() {
                return Err(io::Error::other("disk gone"));
            }
            let n = self.0.len().min(buf.len());
            buf[..n].copy_from_slice(&self.0[..n]);
            self.0 = &self.0[n..];
            Ok(n)
        }
    }

    #[test]
    fn json_values_skips_bad_lines_but_surfaces_read_errors() {
        let reader = BufReader::new(FailsAfter(b"{\"a\":1}\nnot json\n{\"b\":2}\n"));
        let items: Vec<_> = json_values(reader).take(3).collect();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].as_ref().unwrap()["a"], 1);
        assert_eq!(items[1].as_ref().unwrap()["b"], 2);
        assert_eq!(items[2].as_ref().unwrap_err().to_string(), "disk gone");
    }
}
