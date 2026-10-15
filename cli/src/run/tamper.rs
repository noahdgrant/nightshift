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
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Tree,
    /// A git config file, compared by its entries without the `branch.*` ones.
    Config,
}

/// The protected paths as a phase found them.
pub(super) struct Protected {
    roots: Vec<Root>,
    nodes: BTreeMap<PathBuf, Node>,
    views: BTreeMap<PathBuf, Vec<u8>>,
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
/// phase settings, and each skills link with the directory it points at.
pub(super) fn protected_paths(
    worktree: &Path,
    common: &Path,
    artifacts: &Path,
    unit: &str,
) -> Protected {
    let tree = |path: PathBuf| Root {
        path,
        kind: Kind::Tree,
    };
    let config = |path: PathBuf| Root {
        path,
        kind: Kind::Config,
    };
    let mut roots = vec![
        config(common.join("config")),
        config(common.join("config.worktree")),
        tree(common.join("hooks")),
        tree(worktree.join(".githooks")),
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
                roots.push(tree(target));
            }
        }
        roots.push(tree(path));
    }
    Protected::read(roots)
}

impl Protected {
    fn read(roots: Vec<Root>) -> Protected {
        let mut nodes = BTreeMap::new();
        let mut views = BTreeMap::new();
        for root in &roots {
            walk(&root.path, &mut nodes);
            if root.kind == Kind::Config {
                if let Some(Node::File(..)) = nodes.get(&root.path) {
                    if let Some(v) = config_view(&root.path) {
                        views.insert(root.path.clone(), v);
                    }
                }
            }
        }
        Protected {
            roots,
            nodes,
            views,
        }
    }

    /// Read the paths again and put back each one that changed. `None` when none did.
    pub fn check(&self) -> Option<Tampered> {
        let now = Protected::read(self.roots.clone());
        let paths = self.changed(&now);
        if paths.is_empty() {
            return None;
        }
        let restore_error = self.put_back(&now, &paths).err().map(|e| e.to_string());
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

    /// `path`, or a directory above it, could not be read.
    fn unread_at(&self, path: &Path) -> bool {
        path.ancestors()
            .any(|p| self.nodes.get(p) == Some(&Node::Unread))
    }

    /// Put each of `paths` back as `self` holds it: first clear what `now` has there, deepest
    /// first, then write back what was, shallowest first. A directory whose mode alone changed
    /// keeps what is under it.
    fn put_back(&self, now: &Protected, paths: &[PathBuf]) -> std::io::Result<()> {
        for p in paths.iter().rev() {
            match (self.nodes.get(p), now.nodes.get(p)) {
                (Some(Node::Dir(_)), Some(Node::Dir(_))) | (_, None) => {}
                (_, Some(n)) => remove(p, n)?,
            }
        }
        for p in paths {
            match self.nodes.get(p) {
                Some(Node::Dir(mode)) => {
                    fs::create_dir_all(p)?;
                    fs::set_permissions(p, fs::Permissions::from_mode(*mode))?;
                }
                Some(Node::File(bytes, mode)) => {
                    fs::write(p, bytes)?;
                    fs::set_permissions(p, fs::Permissions::from_mode(*mode))?;
                }
                Some(Node::Link(target)) => std::os::unix::fs::symlink(target, p)?,
                Some(Node::Large(len, _)) => {
                    return Err(std::io::Error::other(format!(
                        "{} held {len} bytes, more than ns keeps to put back",
                        p.display()
                    )))
                }
                Some(Node::Unread) | None => {}
            }
        }
        Ok(())
    }
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

/// Read `path` and everything under it into `nodes`. A path that does not exist adds nothing.
fn walk(path: &Path, nodes: &mut BTreeMap<PathBuf, Node>) {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(_) => {
            nodes.insert(path.to_path_buf(), Node::Unread);
            return;
        }
    };
    let mode = meta.permissions().mode() & 0o7777;
    let node = if meta.is_symlink() {
        fs::read_link(path).map_or(Node::Unread, Node::Link)
    } else if meta.is_file() && meta.len() > MAX_FILE {
        Node::Large(meta.len(), mode)
    } else if meta.is_file() {
        fs::read(path).map_or(Node::Unread, |b| Node::File(b, mode))
    } else if meta.is_dir() {
        Node::Dir(mode)
    } else {
        Node::Unread
    };
    let is_dir = node == Node::Dir(mode);
    nodes.insert(path.to_path_buf(), node);
    if !is_dir {
        return;
    }
    match fs::read_dir(path) {
        Ok(entries) => {
            for entry in entries.flatten() {
                walk(&entry.path(), nodes);
            }
        }
        Err(_) => {
            nodes.insert(path.to_path_buf(), Node::Unread);
        }
    }
}

/// The entries of the git config file `path` without the `branch.*` ones, or `None` when git
/// cannot read it, which then compares by its bytes.
fn config_view(path: &Path) -> Option<Vec<u8>> {
    let dir = path.parent()?;
    let file = path.to_str()?;
    let out = git::run(dir, &["config", "--file", file, "--list", "-z"]).ok()?;
    let kept: Vec<&str> = out
        .split('\0')
        .filter(|entry| !entry.starts_with(FREE_CONFIG_PREFIX))
        .collect();
    Some(kept.join("\0").into_bytes())
}

#[cfg(test)]
mod tests;
