//! The stack's build output: the paths the `build-output` row of a worktree's
//! `docs/agents/stack.md` names, deleted from a worktree that stays once its unit stopped, so a
//! kept worktree doesn't hold gigabytes until a human cleans it (docs/factory/unit-lifecycle.md,
//! Build output).

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};

use crate::git::Repo;

/// Delete each `build-output` path inside `worktree`, refusing any that resolves outside it.
/// Returns `{paths, bytes, refused, failed}`, or `None` when there was nothing to delete or
/// refuse: no row, none of its paths exists, or `worktree` is the main checkout, which the
/// operator builds in.
pub fn remove(repo: &Repo, worktree: &Path) -> Option<Value> {
    if worktree == repo.root {
        return None;
    }
    let stack = fs::read_to_string(worktree.join("docs/agents/stack.md")).ok()?;
    let listed = crate::stack::spans(&stack, "build-output");
    if listed.is_empty() {
        return None;
    }
    let root = worktree.canonicalize().ok()?;
    let (mut paths, mut bytes, mut refused, mut failed) = (vec![], 0, vec![], vec![]);
    for rel in &listed {
        match inside(&root, rel) {
            Err(reason) => refused.push(json!({"path": rel, "reason": reason})),
            Ok(None) => {}
            Ok(Some(path)) => {
                let size = size(&path);
                match delete(&path) {
                    Ok(()) => {
                        paths.push(json!(rel));
                        bytes += size;
                    }
                    Err(e) => failed.push(json!({"path": rel, "error": e.to_string()})),
                }
            }
        }
    }
    if paths.is_empty() && refused.is_empty() && failed.is_empty() {
        return None;
    }
    Some(json!({"paths": paths, "bytes": bytes, "refused": refused, "failed": failed}))
}

/// Where `rel` resolves under `root` (canonical), `None` when nothing is there, or why it is
/// refused: absolute, climbing out, the worktree itself, `.git` or `.ns` (any case), tracked by
/// git, not ignored by git, or a symlink that leads out.
fn inside(root: &Path, rel: &str) -> Result<Option<PathBuf>, String> {
    let p = Path::new(rel);
    if p.is_absolute() {
        return Err("an absolute path".into());
    }
    if p.components().any(|c| c == Component::ParentDir) {
        return Err("climbs out with ..".into());
    }
    if state_dir(p) {
        return Err("git or nightshift state".into());
    }
    let joined = root.join(p);
    match fs::symlink_metadata(&joined) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read it: {e}")),
        Ok(_) => {}
    }
    let real = joined
        .canonicalize()
        .map_err(|e| format!("cannot resolve it: {e}"))?;
    if real == root {
        return Err("the worktree itself".into());
    }
    if !real.starts_with(root) {
        return Err(format!(
            "resolves outside the worktree, to {}",
            real.display()
        ));
    }
    let inner = real.strip_prefix(root).unwrap_or(&real);
    if state_dir(inner) {
        return Err("git or nightshift state".into());
    }
    for candidate in [p, inner] {
        let spec = format!(":(literal){}", candidate.display());
        match crate::git::run(root, &["ls-files", "--", &spec]) {
            Ok(tracked) if tracked.is_empty() => {}
            Ok(_) => return Err("tracked by git".into()),
            Err(e) => return Err(format!("cannot check whether git tracks it: {e}")),
        }
    }
    let plain = inner.to_string_lossy();
    match crate::git::run(root, &["check-ignore", "-q", "--", &plain]) {
        Ok(_) => Ok(Some(real)),
        Err(_) => Err("not ignored by git".into()),
    }
}

/// Whether the first component of `p` names `.git` or `.ns`, in any case: a case-insensitive
/// filesystem resolves `.GIT` to the same directory.
fn state_dir(p: &Path) -> bool {
    matches!(
        p.components().next(),
        Some(Component::Normal(c))
            if c.to_string_lossy().eq_ignore_ascii_case(".git")
                || c.to_string_lossy().eq_ignore_ascii_case(".ns")
    )
}

/// The bytes of the files under `path`, symlinks not followed.
fn size(path: &Path) -> u64 {
    let Ok(m) = fs::symlink_metadata(path) else {
        return 0;
    };
    if !m.is_dir() {
        return m.len();
    }
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| size(&e.path()))
        .sum()
}

fn delete(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        crate::tree::remove(path)
    } else {
        fs::remove_file(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{chmod, reads_any_mode, temp_tree, TempTree};
    use std::os::unix::fs::symlink;

    /// A worktree whose stack.md has the `build-output` row `row`, and `cli/target` with two
    /// files of 3 and 4 bytes.
    fn worktree(row: &str) -> (TempTree, PathBuf) {
        let tmp = temp_tree();
        let wt = tmp.path().canonicalize().unwrap().join("wt");
        fs::create_dir_all(wt.join("docs/agents")).unwrap();
        fs::create_dir_all(wt.join("cli/target/debug")).unwrap();
        fs::write(wt.join("cli/target/a"), "abc").unwrap();
        fs::write(wt.join("cli/target/debug/b"), "defg").unwrap();
        fs::write(wt.join("cli/keep.rs"), "x").unwrap();
        crate::git::run(&wt, &["init", "-q"]).unwrap();
        fs::write(wt.join(".gitignore"), "target\nout.bin\n").unwrap();
        let stack = format!("| Task | Command | Takes |\n|---|---|---|\n{row}\n");
        fs::write(wt.join("docs/agents/stack.md"), stack).unwrap();
        (tmp, wt)
    }

    /// A main checkout other than the worktree.
    fn main() -> Repo {
        Repo {
            root: PathBuf::from("/nonexistent/main"),
            common_dir: PathBuf::from("/nonexistent/main/.git"),
        }
    }

    #[test]
    fn the_main_checkout_keeps_its_build_output() {
        let (_tmp, wt) = worktree("| build-output | `cli/target` | |");
        let repo = Repo {
            root: wt.clone(),
            common_dir: wt.join(".git"),
        };
        assert_eq!(remove(&repo, &wt), None);
        assert!(wt.join("cli/target/a").exists());
    }

    #[test]
    fn the_listed_paths_go_with_the_bytes_they_held() {
        let (_tmp, wt) = worktree("| build-output | `cli/target`, `out.bin` | |");
        fs::write(wt.join("out.bin"), "12345").unwrap();
        let v = remove(&main(), &wt).unwrap();
        assert_eq!(
            v,
            json!({"paths": ["cli/target", "out.bin"], "bytes": 12, "refused": [], "failed": []})
        );
        assert!(!wt.join("cli/target").exists());
        assert!(!wt.join("out.bin").exists());
        assert!(wt.join("cli/keep.rs").exists());
    }

    #[test]
    fn no_row_or_no_path_deletes_nothing() {
        for row in ["| build | `cargo build` | |", "| build-output | none | |"] {
            let (_tmp, wt) = worktree(row);
            assert_eq!(remove(&main(), &wt), None, "{row}");
            assert!(wt.join("cli/target/a").exists(), "{row}");
        }
        let (_tmp, wt) = worktree("| build-output | `cli/target` | |");
        fs::remove_file(wt.join("docs/agents/stack.md")).unwrap();
        assert_eq!(remove(&main(), &wt), None);
        assert!(wt.join("cli/target/a").exists());
    }

    #[test]
    fn a_listed_path_that_is_not_there_is_skipped() {
        let (_tmp, wt) = worktree("| build-output | `build` | |");
        assert_eq!(remove(&main(), &wt), None);
        let (_tmp, wt) = worktree("| build-output | `build`, `cli/target` | |");
        assert_eq!(
            remove(&main(), &wt).unwrap()["paths"],
            json!(["cli/target"])
        );
    }

    #[test]
    fn a_path_outside_the_worktree_is_refused_and_left() {
        let (tmp, wt) = worktree("");
        let outside = tmp.path().join("x");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("f"), "keep").unwrap();
        symlink(&outside, wt.join("link")).unwrap();
        symlink(&outside, wt.join("cli/target/debug/out")).unwrap();
        let row = format!(
            "| build-output | `../x`, `{}`, `link`, `link/f`, `.`, `cli/..`, `cli/target/debug/out` | |",
            outside.display()
        );
        fs::write(wt.join("docs/agents/stack.md"), row).unwrap();
        let v = remove(&main(), &wt).unwrap();
        let reasons: Vec<(&str, &str)> = v["refused"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (r["path"].as_str().unwrap(), r["reason"].as_str().unwrap()))
            .collect();
        let real = outside.canonicalize().unwrap();
        let out = format!("resolves outside the worktree, to {}", real.display());
        let out_f = format!(
            "resolves outside the worktree, to {}",
            real.join("f").display()
        );
        let abs = outside.to_string_lossy().into_owned();
        assert_eq!(
            reasons,
            [
                ("../x", "climbs out with .."),
                (abs.as_str(), "an absolute path"),
                ("link", out.as_str()),
                ("link/f", out_f.as_str()),
                (".", "the worktree itself"),
                ("cli/..", "climbs out with .."),
                ("cli/target/debug/out", out.as_str()),
            ]
        );
        assert_eq!(v["paths"], json!([]));
        assert_eq!(fs::read_to_string(outside.join("f")).unwrap(), "keep");
        assert!(wt.join("cli/target/a").exists());
    }

    #[test]
    fn a_dangling_symlink_is_refused() {
        let (_tmp, wt) = worktree("| build-output | `gone` | |");
        symlink(wt.join("nowhere"), wt.join("gone")).unwrap();
        let v = remove(&main(), &wt).unwrap();
        let reason = v["refused"][0]["reason"].as_str().unwrap();
        assert!(reason.starts_with("cannot resolve it: "), "{v}");
        assert!(fs::symlink_metadata(wt.join("gone")).is_ok());
    }

    #[test]
    fn a_symlink_that_stays_inside_deletes_what_it_points_to() {
        let (_tmp, wt) = worktree("| build-output | `target` | |");
        symlink(wt.join("cli/target"), wt.join("target")).unwrap();
        let v = remove(&main(), &wt).unwrap();
        assert_eq!(
            (v["paths"].clone(), v["bytes"].clone()),
            (json!(["target"]), json!(7))
        );
        assert!(!wt.join("cli/target").exists());
    }

    #[test]
    fn git_and_ns_state_and_tracked_paths_are_refused_and_left() {
        let (_tmp, wt) = worktree("| build-output | `.git`, `.git/config`, `.ns`, `.ns/u`, `cli/keep.rs`, `cli`, `docs`, `via-git`, `via-tracked` | |");
        fs::create_dir_all(wt.join(".ns/u")).unwrap();
        fs::write(wt.join(".ns/u/brief.md"), "b").unwrap();
        symlink(wt.join(".git"), wt.join("via-git")).unwrap();
        symlink(wt.join("cli/keep.rs"), wt.join("via-tracked")).unwrap();
        crate::git::run(&wt, &["add", "cli/keep.rs", "docs", "via-tracked"]).unwrap();
        let v = remove(&main(), &wt).unwrap();
        let refused: Vec<&str> = v["refused"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["path"].as_str().unwrap())
            .collect();
        assert_eq!(
            refused,
            [
                ".git",
                ".git/config",
                ".ns",
                ".ns/u",
                "cli/keep.rs",
                "cli",
                "docs",
                "via-git",
                "via-tracked"
            ]
        );
        assert_eq!(v["paths"], json!([]));
        assert!(wt.join(".git/config").exists());
        assert!(wt.join(".ns/u/brief.md").exists());
        assert!(wt.join("cli/keep.rs").exists());
        assert!(wt.join("docs/agents/stack.md").exists());
        assert!(fs::symlink_metadata(wt.join("via-tracked")).is_ok());
    }

    #[test]
    fn each_candidate_path_catches_a_tracked_file() {
        let (_tmp, wt) = worktree("| build-output | `untracked-link`, `tracked-link` | |");
        symlink(wt.join("cli/keep.rs"), wt.join("untracked-link")).unwrap();
        symlink(wt.join("cli/target"), wt.join("tracked-link")).unwrap();
        crate::git::run(&wt, &["add", "cli/keep.rs", "tracked-link"]).unwrap();
        let v = remove(&main(), &wt).unwrap();
        assert_eq!(
            v["refused"],
            json!([
                {"path": "untracked-link", "reason": "tracked by git"},
                {"path": "tracked-link", "reason": "tracked by git"}
            ])
        );
        assert_eq!(v["paths"], json!([]));
        assert!(wt.join("cli/target/a").exists());
    }

    #[test]
    fn a_git_failure_refuses_instead_of_deleting() {
        let (_tmp, wt) = worktree("| build-output | `cli/target` | |");
        fs::remove_dir_all(wt.join(".git")).unwrap();
        let v = remove(&main(), &wt).unwrap();
        let reason = v["refused"][0]["reason"].as_str().unwrap();
        assert!(
            reason.starts_with("cannot check whether git tracks it: "),
            "{v}"
        );
        assert!(wt.join("cli/target/a").exists());
    }

    #[test]
    fn untracked_content_git_does_not_ignore_is_refused() {
        let (_tmp, wt) = worktree("| build-output | `nested`, `stray.txt` | |");
        fs::create_dir_all(wt.join("nested/.git")).unwrap();
        fs::write(wt.join("nested/work"), "unpushed").unwrap();
        fs::write(wt.join("stray.txt"), "x").unwrap();
        let v = remove(&main(), &wt).unwrap();
        assert_eq!(
            v["refused"],
            json!([
                {"path": "nested", "reason": "not ignored by git"},
                {"path": "stray.txt", "reason": "not ignored by git"}
            ])
        );
        assert!(wt.join("nested/work").exists());
        assert!(wt.join("stray.txt").exists());
    }

    #[test]
    fn state_directories_are_refused_in_any_case() {
        let (_tmp, wt) = worktree("| build-output | `.GIT`, `.Ns/u` | |");
        let v = remove(&main(), &wt).unwrap();
        assert_eq!(
            v["refused"],
            json!([
                {"path": ".GIT", "reason": "git or nightshift state"},
                {"path": ".Ns/u", "reason": "git or nightshift state"}
            ])
        );
    }

    #[test]
    fn an_untracked_path_beside_tracked_ones_still_goes() {
        let (_tmp, wt) = worktree("| build-output | `cli/target` | |");
        crate::git::run(&wt, &["add", "cli/keep.rs"]).unwrap();
        let v = remove(&main(), &wt).unwrap();
        assert_eq!(v["paths"], json!(["cli/target"]));
        assert_eq!(v["refused"], json!([]));
    }

    #[test]
    fn build_output_holding_read_only_directories_is_deleted() {
        let (_tmp, wt) = worktree("| build-output | `cli/target` | |");
        fs::create_dir(wt.join("cli/target/debug/inner")).unwrap();
        fs::write(wt.join("cli/target/debug/inner/c"), "hi").unwrap();
        chmod(&wt.join("cli/target/debug/inner"), 0o500);
        chmod(&wt.join("cli/target/debug"), 0o500);
        let v = remove(&main(), &wt).unwrap();
        assert_eq!(v["paths"], json!(["cli/target"]), "{v}");
        assert_eq!(v["failed"], json!([]), "{v}");
        assert!(!wt.join("cli/target").exists());
    }

    #[test]
    fn a_path_that_cannot_be_deleted_is_reported_as_failed() {
        let (tmp, wt) = worktree("| build-output | `cli/target` | |");
        if reads_any_mode(tmp.path()) {
            return;
        }
        // The parent is outside the tree, so it stays read-only and the unlink fails.
        chmod(&wt.join("cli"), 0o500);
        let v = remove(&main(), &wt).unwrap();
        chmod(&wt.join("cli"), 0o700);
        assert_eq!(
            (v["paths"].clone(), v["bytes"].clone()),
            (json!([]), json!(0))
        );
        assert_eq!(v["failed"][0]["path"], "cli/target");
        assert!(v["failed"][0]["error"]
            .as_str()
            .unwrap()
            .contains("ermission"));
    }
}
