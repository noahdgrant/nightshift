//! The running phase's own `TMPDIR`: the one directory under `.git/ns` a phase may write to,
//! while nothing in it is a symlink that could lead a write out of it.

use std::cell::OnceCell;
use std::path::{Path, PathBuf};

use super::files::components;
use crate::pin;

/// How many entries of `TMPDIR` the guard looks at for a symlink before it gives up and exempts
/// nothing.
const MAX_WALK: usize = 20_000;

/// The phase's `TMPDIR` as given and with its parent resolved, and, once asked, why it exempts
/// nothing: a symlink in it, or a walk that could not finish.
#[derive(Default)]
pub(super) struct PhaseTmp {
    dirs: Vec<String>,
    distrust: OnceCell<Option<String>>,
}

impl PhaseTmp {
    /// The phase `TMPDIR` `tmpdir` names. One not of the shape `ns run` gives (`is_phase_tmpdir`)
    /// exempts nothing.
    pub fn new(tmpdir: Option<&str>) -> PhaseTmp {
        let dirs = tmpdir
            .filter(|d| is_phase_tmpdir(d))
            .map(|d| {
                let dir = Path::new(d);
                let real = dir
                    .parent()
                    .and_then(|p| p.canonicalize().ok())
                    .zip(dir.file_name())
                    .map(|(p, name)| p.join(name).to_string_lossy().into_owned())
                    .filter(|r| is_phase_tmpdir(r) && r != d);
                std::iter::once(d.to_string()).chain(real).collect()
            })
            .unwrap_or_default();
        PhaseTmp {
            dirs,
            distrust: OnceCell::new(),
        }
    }

    /// `path` is the phase's `TMPDIR` or in it, by an absolute path with no part below it that
    /// could be `..`, and the directory holds no symlink.
    pub fn exempts(&self, path: &str) -> bool {
        self.holds(path) && self.distrust().is_none()
    }

    fn holds(&self, path: &str) -> bool {
        let parts = components(path);
        path.starts_with('/')
            && self.dirs.iter().any(|d| {
                let root = components(d);
                parts.starts_with(&root) && !parts[root.len()..].iter().any(|p| could_be_up(p))
            })
    }

    /// Why the `TMPDIR` exempts nothing: it holds a symlink, or the guard could not walk it all.
    pub fn distrust(&self) -> Option<&str> {
        self.distrust
            .get_or_init(|| self.dirs.first().and_then(|d| find_link(Path::new(d))))
            .as_deref()
    }

    /// `distrust`, if this call has already looked.
    pub fn found_distrust(&self) -> Option<&str> {
        self.distrust.get().and_then(|d| d.as_deref())
    }

    /// `command` with each `$TMPDIR` and `${TMPDIR}` outside single quotes written out, so the
    /// guard checks a write there by its path. A `TMPDIR` with a character a shell word would
    /// split or expand is left as the variable.
    pub fn substituted(&self, command: &str) -> String {
        let Some(root) = self.dirs.first().filter(|d| {
            d.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/._+-".contains(&b))
        }) else {
            return command.to_string();
        };
        let mut out = String::with_capacity(command.len());
        let (mut single, mut double) = (false, false);
        let mut rest = command;
        while let Some(c) = rest.chars().next() {
            if !single {
                if let Some(after) = variable_at(rest) {
                    out.push_str(root);
                    rest = after;
                    continue;
                }
            }
            match c {
                '\\' if !single => {
                    let n = rest.chars().nth(1).map_or(0, char::len_utf8);
                    out.push_str(&rest[..1 + n]);
                    rest = &rest[1 + n..];
                    continue;
                }
                '\'' if !double => single = !single,
                '"' if !single => double = !double,
                _ => {}
            }
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
        out
    }
}

/// The text after a `$TMPDIR` or `${TMPDIR}` that `s` starts with.
fn variable_at(s: &str) -> Option<&str> {
    if let Some(after) = s.strip_prefix("${TMPDIR}") {
        return Some(after);
    }
    s.strip_prefix("$TMPDIR")
        .filter(|after| !after.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_'))
}

/// A path part a shell could read as `..`: `..` itself, or a glob that may match a dot name.
fn could_be_up(part: &str) -> bool {
    part == ".." || (part.contains(['*', '?', '[']) && part.starts_with(['.', '[']))
}

/// The first symlink under `root`, `root` itself included, without following any; or why the
/// walk could not finish. A `root` that does not exist holds nothing.
fn find_link(root: &Path) -> Option<String> {
    match std::fs::symlink_metadata(root) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => return Some(format!("it could not be read ({e})")),
        Ok(m) if m.is_symlink() => return Some(format!("it is a symlink, {}", root.display())),
        Ok(_) => {}
    }
    let mut dirs: Vec<PathBuf> = vec![root.to_path_buf()];
    let mut seen = 0;
    while let Some(dir) = dirs.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) => return Some(format!("{} could not be read ({e})", dir.display())),
        };
        for entry in entries {
            seen += 1;
            if seen > MAX_WALK {
                return Some(format!(
                    "it has more than {MAX_WALK} entries to check for a symlink"
                ));
            }
            let found = entry.and_then(|e| Ok((e.path(), e.file_type()?)));
            match found {
                Ok((path, kind)) if kind.is_symlink() => {
                    return Some(format!("it holds a symlink, {}", path.display()))
                }
                Ok((path, kind)) if kind.is_dir() => dirs.push(path),
                Ok(_) => {}
                Err(e) => return Some(format!("{} could not be read ({e})", dir.display())),
            }
        }
    }
    None
}

/// `dir` is a plain absolute `.../ns/units/<unit>/tmp/<phase>-<n>`, where `ns run` puts a
/// phase's `TMPDIR`.
fn is_phase_tmpdir(dir: &str) -> bool {
    let parts = components(dir);
    dir.starts_with('/')
        && !parts
            .iter()
            .any(|p| *p == ".." || p.contains(['*', '?', '[']))
        && matches!(
            parts.as_slice(),
            [.., ns, units, _, tmp, name]
                if *ns == pin::COMMON_NS && *units == pin::COMMON_UNITS
                    && *tmp == pin::TMP_DIR && pin::is_phase_tmp_name(name)
        )
}

/// The absolute `dir` joined with `path`, `..` parts taken lexically.
pub(super) fn joined(dir: &str, path: &str) -> String {
    let mut parts = components(dir);
    for part in components(path) {
        if part == ".." {
            parts.pop();
        } else {
            parts.push(part);
        }
    }
    format!("/{}", parts.join("/"))
}
