//! The tamper check around each phase (docs/factory/run.md, "Tamper check"): `ns run` records
//! every protected path before the harness starts, compares after it ends, puts back any change
//! and fails the phase as `tampered`. It reads files, not commands, so no way of writing them
//! gets past it; the hook guard only fails a few literal forms early.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::git;
use crate::pin;

/// The git config keys a phase may change: `git push -u` and other units' pushes set a branch's
/// upstream in the shared config, and no `branch.*` key moves or skips a hook.
const FREE_CONFIG_PREFIX: &str = "branch.";

/// The most changed paths a reason names.
const SHOWN: usize = 3;

/// The largest file the check holds to put back: a larger one is compared by its size and mode,
/// so a phase can't make `ns` read a file of any size into memory.
const MAX_FILE: u64 = 16 * 1024 * 1024;

/// One path as it was read.
#[derive(Debug, Clone, PartialEq)]
enum Node {
    Dir(u32),
    File(Vec<u8>, u32),
    Link(PathBuf),
    /// A file larger than [`MAX_FILE`]: its size and mode.
    Large(u64, u32),
    /// It exists but could not be read. Unread before the phase, it and what is under it are
    /// never compared; unread after, it changed.
    Unread,
}

/// A protected path, read with everything under it.
#[derive(Debug, Clone)]
struct Root {
    path: PathBuf,
    kind: Kind,
    /// `ns` may write here to put a change back. False for a path outside the worktree and the
    /// unit's own files, which are reported and left as they are.
    restore: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Tree,
    /// A tree git tracks in the worktree: a change the phase committed (or rebased in) is the
    /// repo's own, and only what differs from HEAD and the index is tampering.
    Tracked,
    /// A git config file, compared by its entries without the `branch.*` ones.
    Config,
}

/// What one root's read may hold: past any of these the root is not read whole.
#[derive(Debug, Clone, Copy)]
struct Limits {
    entries: usize,
    bytes: u64,
    depth: usize,
}

const LIMITS: Limits = Limits {
    entries: 20_000,
    bytes: 64 * 1024 * 1024,
    depth: 32,
};

/// The protected paths as a phase found them.
pub(super) struct Protected {
    roots: Vec<Root>,
    limits: Limits,
    nodes: BTreeMap<PathBuf, Node>,
    /// A config file's entries, `key\nvalue` each, without the `branch.*` ones.
    views: BTreeMap<PathBuf, Vec<String>>,
    /// The roots whose read went past a cap. Their nodes are one [`Node::Unread`].
    over: BTreeSet<PathBuf>,
}

/// What a phase changed among the protected paths.
#[derive(Debug)]
pub(super) struct Tampered {
    /// Each path it changed, added or removed, shallowest first.
    pub paths: Vec<PathBuf>,
    /// Why putting them back failed.
    pub restore_error: Option<String>,
}

impl Tampered {
    /// The reason `phase` fails, naming the paths as `show` writes them.
    pub fn reason(&self, phase: &str, show: impl Fn(&Path) -> String) -> String {
        let mut named: Vec<String> = self.paths.iter().take(SHOWN).map(|p| show(p)).collect();
        if self.paths.len() > SHOWN {
            named.push(format!("{} more", self.paths.len() - SHOWN));
        }
        let what = format!(
            "{phase} tampered with a protected path ({})",
            named.join(", ")
        );
        match &self.restore_error {
            None => format!("{what}; ns put it back"),
            Some(e) => format!("{what}; ns could not put it back: {e}"),
        }
    }
}

/// The paths a phase of `unit` must leave as they were: the common git config and any
/// `config.worktree`, `.git/hooks/`, the worktree's `.githooks/`, the unit's skill snapshot and
/// phase settings, and each skills link with the directory it points at. That directory lies
/// outside the unit, so a change there is reported and not put back.
pub(super) fn protected_paths(
    worktree: &Path,
    common: &Path,
    artifacts: &Path,
    unit: &str,
) -> Protected {
    let root = |path: PathBuf, kind, restore| Root {
        path,
        kind,
        restore,
    };
    let tree = |path: PathBuf| root(path, Kind::Tree, true);
    let config = |path: PathBuf| root(path, Kind::Config, true);
    let mut roots = vec![
        config(common.join("config")),
        config(common.join("config.worktree")),
        tree(common.join("hooks")),
        root(worktree.join(".githooks"), Kind::Tracked, true),
        tree(artifacts.join(pin::SKILLS_DIR)),
        tree(pin::settings_path(common, unit)),
    ];
    if let Ok(dir) = git::run(worktree, &["rev-parse", "--absolute-git-dir"]) {
        let own = PathBuf::from(dir).join("config.worktree");
        if own != common.join("config.worktree") {
            roots.push(config(own));
        }
    }
    let snapshot = fs::canonicalize(artifacts.join(pin::SKILLS_DIR)).ok();
    for link in pin::LINKS {
        let path = worktree.join(link);
        if !fs::symlink_metadata(&path).is_ok_and(|m| m.is_symlink()) {
            continue;
        }
        if let Ok(target) = fs::canonicalize(&path) {
            if Some(&target) != snapshot.as_ref() {
                roots.push(root(target, Kind::Tree, false));
            }
        }
        roots.push(tree(path));
    }
    Protected::read(roots)
}

impl Protected {
    fn read(roots: Vec<Root>) -> Protected {
        Protected::read_with(roots, LIMITS)
    }

    fn read_with(roots: Vec<Root>, limits: Limits) -> Protected {
        let mut nodes = BTreeMap::new();
        let mut views = BTreeMap::new();
        let mut over = BTreeSet::new();
        for root in &roots {
            if !walk(&root.path, limits, &mut nodes) {
                nodes.retain(|p, _| !p.starts_with(&root.path));
                nodes.insert(root.path.clone(), Node::Unread);
                over.insert(root.path.clone());
            } else if root.kind == Kind::Config {
                if let Some(Node::File(..)) = nodes.get(&root.path) {
                    if let Some(v) = config_view(&root.path) {
                        views.insert(root.path.clone(), v);
                    }
                }
            }
        }
        Protected {
            roots,
            limits,
            nodes,
            views,
            over,
        }
    }

    /// Read the paths again and put back each one that changed. `None` when none did.
    pub fn check(&self) -> Option<Tampered> {
        let now = Protected::read_with(self.roots.clone(), self.limits);
        let grown: Vec<&PathBuf> = now.over.difference(&self.over).collect();
        let mut paths: Vec<PathBuf> = self
            .changed(&now)
            .into_iter()
            .filter(|p| !grown.iter().any(|g| p.starts_with(g)))
            .collect();
        self.drop_committed(&mut paths);
        let mut restore_error = None;
        if !paths.is_empty() {
            restore_error = self.put_back(&now, &paths).err().map(|e| e.to_string());
        }
        for g in &grown {
            paths.push((*g).clone());
            restore_error.get_or_insert_with(|| {
                format!(
                    "{} grew to more than ns reads ({} entries, {} bytes, {} levels deep), so it was not put back",
                    g.display(),
                    self.limits.entries,
                    self.limits.bytes,
                    self.limits.depth
                )
            });
        }
        if paths.is_empty() {
            return None;
        }
        paths.sort();
        Some(Tampered {
            paths,
            restore_error,
        })
    }

    fn changed(&self, now: &Protected) -> Vec<PathBuf> {
        let all: BTreeSet<&PathBuf> = self.nodes.keys().chain(now.nodes.keys()).collect();
        all.into_iter()
            .filter(|p| !self.unread_at(p))
            .filter(|p| match (self.views.get(*p), now.views.get(*p)) {
                (Some(a), Some(b)) => a != b,
                _ => self.nodes.get(*p) != now.nodes.get(*p),
            })
            .cloned()
            .collect()
    }

    /// Forget the changes under a tracked root that HEAD holds: what differs from HEAD stays. When git can't say, every change stays.
    fn drop_committed(&self, paths: &mut Vec<PathBuf>) {
        for root in self.roots.iter().filter(|r| r.kind == Kind::Tracked) {
            if !paths.iter().any(|p| p.starts_with(&root.path)) {
                continue;
            }
            let Some(dirty) = dirty_under(&root.path) else {
                continue;
            };
            paths.retain(|p| {
                !p.starts_with(&root.path) || dirty.iter().any(|d| d.starts_with(p) || p == d)
            });
        }
    }

    /// `path`, or a directory above it, could not be read.
    fn unread_at(&self, path: &Path) -> bool {
        path.ancestors()
            .any(|p| self.nodes.get(p) == Some(&Node::Unread))
    }

    /// Whether `ns` may write at `path`: some root holding it allows it.
    fn restorable(&self, path: &Path) -> bool {
        let mut held = self.roots.iter().filter(|r| path.starts_with(&r.path));
        held.clone().next().is_none() || held.any(|r| r.restore)
    }

    /// Put each of `paths` back as `self` holds it: first clear what `now` has there, deepest
    /// first, then write back what was, shallowest first. A directory whose mode alone changed
    /// keeps what is under it. A path outside the unit is left, after the rest are put back, and the
    /// error says so.
    fn put_back(&self, now: &Protected, paths: &[PathBuf]) -> std::io::Result<()> {
        let outside = paths.iter().find(|p| !self.restorable(p));
        let paths: Vec<PathBuf> = paths
            .iter()
            .filter(|p| self.restorable(p))
            .cloned()
            .collect();
        let paths = &paths;
        let mut by_key = BTreeSet::new();
        for p in paths.iter().rev() {
            if self.put_back_config(now, p)? {
                by_key.insert(p);
                continue;
            }
            match (self.nodes.get(p), now.nodes.get(p)) {
                (Some(Node::Dir(_)), Some(Node::Dir(_))) | (_, None) => {}
                (Some(Node::File(..)), Some(Node::File(..)))
                | (Some(Node::Link(_)), Some(Node::Link(_))) => {}
                (_, Some(n)) => remove(p, n)?,
            }
        }
        for p in paths {
            if by_key.contains(p) {
                continue;
            }
            match self.nodes.get(p) {
                Some(Node::Dir(mode)) => {
                    fs::create_dir_all(p)?;
                    fs::set_permissions(p, fs::Permissions::from_mode(*mode))?;
                }
                Some(Node::File(bytes, mode)) => replace(p, |t| {
                    fs::write(t, bytes)?;
                    fs::set_permissions(t, fs::Permissions::from_mode(*mode))
                })?,
                Some(Node::Link(target)) => {
                    replace(p, |t| std::os::unix::fs::symlink(target, t))?;
                }
                Some(Node::Large(len, _)) => {
                    return Err(std::io::Error::other(format!(
                        "{} held {len} bytes, more than ns keeps to put back",
                        p.display()
                    )))
                }
                Some(Node::Unread) | None => {}
            }
        }
        match outside {
            Some(p) => Err(std::io::Error::other(format!(
                "{} is outside the unit's own paths, so ns left it as it is",
                p.display()
            ))),
            None => Ok(()),
        }
    }

    /// Put a config file's changed keys back with `git config`, so a `branch.*` key written
    /// meanwhile stays. False when the file isn't one git read both times, or a changed entry
    /// has no value to set again: those go back whole.
    fn put_back_config(&self, now: &Protected, path: &Path) -> std::io::Result<bool> {
        let (Some(was), Some(is)) = (self.views.get(path), now.views.get(path)) else {
            return Ok(false);
        };
        let key = |e: &String| e.split('\n').next().unwrap_or_default().to_string();
        let keys: BTreeSet<String> = was
            .iter()
            .chain(is)
            .filter(|e| !was.contains(e) || !is.contains(e))
            .map(key)
            .collect();
        let entries: Vec<(&String, &String)> = keys
            .iter()
            .flat_map(|k| {
                was.iter()
                    .filter(move |e| &key(e) == k)
                    .map(move |e| (k, e))
            })
            .collect();
        if entries.iter().any(|(_, e)| !e.contains('\n')) {
            return Ok(false);
        }
        let (Some(dir), Some(file)) = (path.parent(), path.to_str()) else {
            return Ok(false);
        };
        let git = |args: &[&str]| {
            git::run(dir, args)
                .map(drop)
                .map_err(|e| std::io::Error::other(e.to_string()))
        };
        for k in &keys {
            let _ = git(&["config", "--file", file, "--unset-all", k]);
        }
        for (k, e) in entries {
            let value = e.split_once('\n').map_or("", |(_, v)| v);
            git(&["config", "--file", file, "--add", k, value])?;
        }
        Ok(true)
    }
}

/// Write `path` through a temporary file beside it and a rename, so another process never sees
/// it missing or half written.
fn replace(path: &Path, make: impl FnOnce(&Path) -> std::io::Result<()>) -> std::io::Result<()> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = path.with_file_name(format!(".{name}.ns-restore-{}", std::process::id()));
    let _ = fs::remove_file(&tmp);
    let done = make(&tmp).and_then(|()| fs::rename(&tmp, path));
    if done.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    done
}

/// Remove `path`, which `now` read as `node`. A directory that could not be read is made
/// readable first.
fn remove(path: &Path, node: &Node) -> std::io::Result<()> {
    let is_dir = match node {
        Node::Dir(_) => true,
        Node::Unread => fs::symlink_metadata(path)?.is_dir(),
        _ => false,
    };
    if !is_dir {
        return fs::remove_file(path);
    }
    if *node == Node::Unread {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    fs::remove_dir_all(path)
}

/// Read `path` and everything under it into `nodes`, without recursion. A path that does not
/// exist adds nothing. False when the read went past one of `limits`, which stops it.
fn walk(path: &Path, limits: Limits, nodes: &mut BTreeMap<PathBuf, Node>) -> bool {
    let (mut entries, mut bytes) = (0usize, 0u64);
    let mut todo = vec![(path.to_path_buf(), 0usize)];
    while let Some((path, depth)) = todo.pop() {
        entries += 1;
        if entries > limits.entries || depth > limits.depth {
            return false;
        }
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                nodes.insert(path, Node::Unread);
                continue;
            }
        };
        let mode = meta.permissions().mode() & 0o7777;
        if meta.is_file() && meta.len() <= MAX_FILE {
            bytes += meta.len();
            if bytes > limits.bytes {
                return false;
            }
        }
        let node = if meta.is_symlink() {
            fs::read_link(&path).map_or(Node::Unread, Node::Link)
        } else if meta.is_file() && meta.len() > MAX_FILE {
            Node::Large(meta.len(), mode)
        } else if meta.is_file() {
            fs::read(&path).map_or(Node::Unread, |b| Node::File(b, mode))
        } else if meta.is_dir() {
            Node::Dir(mode)
        } else {
            Node::Unread
        };
        let is_dir = node == Node::Dir(mode);
        nodes.insert(path.clone(), node);
        if !is_dir {
            continue;
        }
        match fs::read_dir(&path) {
            Ok(read) => todo.extend(read.flatten().map(|e| (e.path(), depth + 1))),
            Err(_) => {
                nodes.insert(path, Node::Unread);
            }
        }
    }
    true
}

/// The paths under `dir`, a tracked directory of a worktree, that differ from HEAD, or are not in
/// it. Decided from HEAD's tree and the files' own bytes and modes, so no config, attribute,
/// exclude rule or index flag of the repo can hide a change or make git run a command. `None`
/// when git can't say.
fn dirty_under(dir: &Path) -> Option<Vec<PathBuf>> {
    let top = dir.parent()?;
    let name = dir.file_name()?.to_str()?;
    let listing = git::run(top, &["ls-tree", "-r", "-z", "HEAD", "--", name]).ok()?;
    let mut head = BTreeMap::new();
    for entry in listing.split('\0').filter(|e| !e.is_empty()) {
        let (meta, rel) = entry.split_once('\t')?;
        let mut meta = meta.split(' ');
        let mode = meta.next()?;
        let sha = meta.nth(1)?;
        head.insert(top.join(rel), (mode.to_string(), sha.to_string()));
    }
    let mut nodes = BTreeMap::new();
    if !walk(dir, LIMITS, &mut nodes) {
        return None;
    }
    let mut dirty = Vec::new();
    let mut files = Vec::new();
    for (path, node) in &nodes {
        let mode = match node {
            Node::Dir(_) => continue,
            Node::Link(_) => "120000",
            Node::File(_, m) | Node::Large(_, m) if m & 0o100 != 0 => "100755",
            Node::File(..) | Node::Large(..) => "100644",
            Node::Unread => {
                dirty.push(path.clone());
                continue;
            }
        };
        match head.get(path) {
            Some((m, _)) if m == mode => files.push(path),
            _ => dirty.push(path.clone()),
        }
    }
    for chunk in files.chunks(256) {
        let mut args = vec!["hash-object", "--no-filters", "--"];
        args.extend(
            chunk
                .iter()
                .map(|p| p.to_str())
                .collect::<Option<Vec<_>>>()?,
        );
        let out = git::run(top, &args).ok()?;
        let shas: Vec<&str> = out.lines().collect();
        if shas.len() != chunk.len() {
            return None;
        }
        for (path, sha) in chunk.iter().zip(shas) {
            if head.get(*path).is_none_or(|(_, h)| h != sha) {
                dirty.push((*path).clone());
            }
        }
    }
    dirty.extend(head.keys().filter(|p| !nodes.contains_key(*p)).cloned());
    Some(dirty)
}

/// The entries of the git config file `path` without the `branch.*` ones, or `None` when git
/// cannot read it, which then compares by its bytes.
fn config_view(path: &Path) -> Option<Vec<String>> {
    let dir = path.parent()?;
    let file = path.to_str()?;
    let out = git::run(dir, &["config", "--file", file, "--list", "-z"]).ok()?;
    Some(
        out.split('\0')
            .filter(|entry| !entry.starts_with(FREE_CONFIG_PREFIX))
            .map(str::to_string)
            .collect(),
    )
}

#[cfg(test)]
mod tests;
