//! Remove a tree `ns` owns, whatever modes a phase left inside it (#730).

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Give the owner read, write and search on `root` and every directory under it, a parent before
/// its children, so the tree can be removed. Symlinks are never followed. A path that cannot be
/// changed or read (vanished, not ours) is skipped: the removal after it reports what stays.
pub fn make_removable(root: &Path) {
    let mut todo = vec![root.to_path_buf()];
    while let Some(dir) = todo.pop() {
        let Ok(meta) = fs::symlink_metadata(&dir) else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        if meta.permissions().mode() & 0o700 != 0o700 {
            let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        todo.extend(entries.flatten().map(|e| e.path()));
    }
}

/// Remove the directory `root` and everything under it, after [`make_removable`]. A root
/// already gone is removed.
pub fn remove(root: &Path) -> io::Result<()> {
    make_removable(root);
    match fs::remove_dir_all(root) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        done => done,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{chmod, reads_any_mode, temp_tree};
    use std::os::unix::fs::symlink;

    fn mode(p: &Path) -> u32 {
        fs::symlink_metadata(p).unwrap().permissions().mode() & 0o7777
    }

    #[test]
    fn a_tree_with_nested_read_only_directories_is_removed() {
        let d = temp_tree();
        let root = d.path().join("t");
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("a/f"), "x").unwrap();
        fs::write(root.join("a/b/g"), "y").unwrap();
        chmod(&root.join("a/b"), 0o500);
        chmod(&root.join("a"), 0o500);
        remove(&root).unwrap();
        assert!(!root.exists());
    }

    #[test]
    fn only_directories_change_mode() {
        let d = temp_tree();
        let root = d.path().join("t");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("sub/f"), "x").unwrap();
        chmod(&root.join("sub/f"), 0o444);
        chmod(&root.join("sub"), 0o500);
        make_removable(&root);
        assert_eq!(
            (mode(&root.join("sub")), mode(&root.join("sub/f"))),
            (0o700, 0o444)
        );
    }

    #[test]
    fn a_root_that_is_itself_unreadable_or_read_only_is_removed() {
        for m in [0o000, 0o500, 0o100] {
            let d = temp_tree();
            let root = d.path().join("t");
            fs::create_dir_all(root.join("sub")).unwrap();
            fs::write(root.join("sub/f"), "x").unwrap();
            chmod(&root, m);
            remove(&root).unwrap();
            assert!(!root.exists(), "{m:o}");
        }
    }

    #[test]
    fn an_empty_tree_and_a_missing_root_are_removed() {
        let d = temp_tree();
        let root = d.path().join("t");
        fs::create_dir(&root).unwrap();
        remove(&root).unwrap();
        assert!(!root.exists());
        remove(&root).unwrap();
    }

    #[test]
    fn a_symlink_out_of_the_tree_is_unlinked_not_followed() {
        let d = temp_tree();
        let outside = d.path().join("outside");
        fs::create_dir_all(outside.join("inner")).unwrap();
        fs::write(outside.join("inner/keep"), "k").unwrap();
        fs::write(outside.join("file"), "f").unwrap();
        chmod(&outside.join("inner"), 0o500);
        chmod(&outside, 0o500);
        let root = d.path().join("t");
        fs::create_dir_all(root.join("locked")).unwrap();
        symlink(&outside, root.join("locked/to-dir")).unwrap();
        symlink(outside.join("file"), root.join("locked/to-file")).unwrap();
        symlink(d.path().join("gone"), root.join("locked/dangling")).unwrap();
        chmod(&root.join("locked"), 0o500);
        remove(&root).unwrap();
        assert!(!root.exists());
        assert_eq!(
            (mode(&outside), mode(&outside.join("inner"))),
            (0o500, 0o500)
        );
        assert_eq!(mode(&outside.join("file")) & 0o200, 0o200);
        chmod(&outside, 0o700);
        chmod(&outside.join("inner"), 0o700);
        assert_eq!(fs::read_to_string(outside.join("inner/keep")).unwrap(), "k");
        assert_eq!(fs::read_to_string(outside.join("file")).unwrap(), "f");
    }

    #[test]
    fn a_root_that_is_a_symlink_is_unlinked_not_followed() {
        let d = temp_tree();
        let outside = d.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep"), "k").unwrap();
        chmod(&outside, 0o500);
        let root = d.path().join("t");
        symlink(&outside, &root).unwrap();
        make_removable(&root);
        assert_eq!(mode(&outside), 0o500);
        chmod(&outside, 0o700);
    }

    #[test]
    fn a_tree_that_still_cannot_go_reports_the_error() {
        let d = temp_tree();
        if reads_any_mode(d.path()) {
            return;
        }
        let parent = d.path().join("p");
        let root = parent.join("t");
        fs::create_dir_all(&root).unwrap();
        chmod(&parent, 0o500);
        let e = remove(&root).unwrap_err();
        chmod(&parent, 0o700);
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied, "{e}");
        assert!(root.is_dir());
    }
}
