use super::*;

fn tree(path: &Path) -> Root {
    Root {
        path: path.to_path_buf(),
        kind: Kind::Tree,
    }
}

fn config(path: &Path) -> Root {
    Root {
        path: path.to_path_buf(),
        kind: Kind::Config,
    }
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

fn chmod(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

/// A tree `hooks/` holding `pre-commit` (0755) and `sub/x`.
fn hooks(dir: &Path) -> PathBuf {
    let hooks = dir.join("hooks");
    fs::create_dir_all(hooks.join("sub")).unwrap();
    fs::write(hooks.join("pre-commit"), "exit 0\n").unwrap();
    chmod(&hooks.join("pre-commit"), 0o750);
    fs::write(hooks.join("sub/x"), "x\n").unwrap();
    hooks
}

#[test]
fn nothing_changed_is_no_tampering_even_with_absent_roots() {
    let d = tempfile::tempdir().unwrap();
    let h = hooks(d.path());
    let p = Protected::read(vec![tree(&h), tree(&d.path().join("absent"))]);
    assert!(p.check().is_none());
}

#[test]
fn a_file_edited_added_removed_or_chmodded_is_put_back() {
    let d = tempfile::tempdir().unwrap();
    let h = hooks(d.path());
    let p = Protected::read(vec![tree(&h)]);
    fs::write(h.join("pre-commit"), "exit 1\n").unwrap();
    fs::write(h.join("pre-push"), "exit 0\n").unwrap();
    fs::remove_file(h.join("sub/x")).unwrap();
    let t = p.check().unwrap();
    assert_eq!(
        t.paths,
        [h.join("pre-commit"), h.join("pre-push"), h.join("sub/x")]
    );
    assert!(t.restore_error.is_none(), "{t:?}");
    assert_eq!(
        fs::read_to_string(h.join("pre-commit")).unwrap(),
        "exit 0\n"
    );
    assert!(!h.join("pre-push").exists());
    assert_eq!(fs::read_to_string(h.join("sub/x")).unwrap(), "x\n");
    assert!(p.check().is_none());

    chmod(&h.join("pre-commit"), 0o644);
    assert_eq!(p.check().unwrap().paths, [h.join("pre-commit")]);
    assert_eq!(mode(&h.join("pre-commit")), 0o750);
}

#[test]
fn a_directory_whose_mode_alone_changed_keeps_what_is_under_it() {
    let d = tempfile::tempdir().unwrap();
    let h = hooks(d.path());
    let before = mode(&h.join("sub"));
    let p = Protected::read(vec![tree(&h)]);
    chmod(&h.join("sub"), 0o700);
    assert_eq!(p.check().unwrap().paths, [h.join("sub")]);
    assert_eq!(mode(&h.join("sub")), before);
    assert_eq!(fs::read_to_string(h.join("sub/x")).unwrap(), "x\n");
}

#[test]
fn a_tree_replaced_by_a_file_or_a_link_is_put_back_whole() {
    let d = tempfile::tempdir().unwrap();
    let h = hooks(d.path());
    let p = Protected::read(vec![tree(&h)]);
    fs::remove_dir_all(&h).unwrap();
    fs::write(&h, "not a dir\n").unwrap();
    assert!(p.check().unwrap().restore_error.is_none());
    assert_eq!(fs::read_to_string(h.join("sub/x")).unwrap(), "x\n");

    let elsewhere = d.path().join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    fs::remove_dir_all(&h).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &h).unwrap();
    assert!(p.check().unwrap().restore_error.is_none());
    assert!(!fs::symlink_metadata(&h).unwrap().is_symlink());
    assert_eq!(
        fs::read_to_string(h.join("pre-commit")).unwrap(),
        "exit 0\n"
    );
    assert!(elsewhere.is_dir(), "the link's target is left alone");
}

#[test]
fn a_link_root_is_compared_by_its_target_and_not_followed() {
    let d = tempfile::tempdir().unwrap();
    let l = d.path().join("skills");
    std::os::unix::fs::symlink("../.ns/u/skills", &l).unwrap();
    let p = Protected::read(vec![tree(&l)]);
    fs::remove_file(&l).unwrap();
    std::os::unix::fs::symlink("/tmp", &l).unwrap();
    assert_eq!(p.check().unwrap().paths, std::slice::from_ref(&l));
    assert_eq!(fs::read_link(&l).unwrap(), Path::new("../.ns/u/skills"));
}

#[test]
fn a_config_change_outside_branch_keys_is_tampering_and_its_bytes_are_put_back() {
    let d = tempfile::tempdir().unwrap();
    let c = d.path().join("config");
    let text = "[core]\n\tbare = false\n";
    fs::write(&c, text).unwrap();
    let p = Protected::read(vec![config(&c)]);
    fs::write(
        &c,
        format!("{text}[branch \"ns/u\"]\n\tremote = origin\n\tmerge = refs/heads/ns/u\n"),
    )
    .unwrap();
    assert!(p.check().is_none(), "a branch upstream is free");
    fs::write(&c, format!("{text}\thooksPath = /dev/null\n")).unwrap();
    assert_eq!(p.check().unwrap().paths, std::slice::from_ref(&c));
    assert_eq!(fs::read_to_string(&c).unwrap(), text);
}

#[test]
fn a_config_git_cannot_read_is_compared_by_its_bytes() {
    let d = tempfile::tempdir().unwrap();
    let c = d.path().join("config");
    fs::write(&c, "[core\n").unwrap();
    let p = Protected::read(vec![config(&c)]);
    assert!(p.views.is_empty());
    assert!(p.check().is_none());
    fs::write(&c, "[core\n#\n").unwrap();
    assert_eq!(p.check().unwrap().paths, std::slice::from_ref(&c));
    assert_eq!(fs::read_to_string(&c).unwrap(), "[core\n");

    fs::write(&c, "[core]\n\tbare = false\n").unwrap();
    let p = Protected::read(vec![config(&c)]);
    fs::write(&c, "[core\n\tbare = false\n").unwrap();
    assert_eq!(
        p.check().unwrap().paths,
        std::slice::from_ref(&c),
        "a config broken during the phase"
    );
}

#[test]
fn a_config_created_where_there_was_none_is_removed() {
    let d = tempfile::tempdir().unwrap();
    let c = d.path().join("config.worktree");
    let p = Protected::read(vec![config(&c)]);
    fs::write(&c, "[core]\n\thooksPath = /dev/null\n").unwrap();
    assert_eq!(p.check().unwrap().paths, std::slice::from_ref(&c));
    assert!(!c.exists());
}

/// Whether this process reads past a directory's mode (it runs as root).
fn reads_any_mode(dir: &Path) -> bool {
    let probe = dir.join("probe");
    fs::create_dir(&probe).unwrap();
    chmod(&probe, 0);
    let any = fs::read_dir(&probe).is_ok();
    chmod(&probe, 0o750);
    any
}

#[test]
fn an_unreadable_path_is_neither_compared_nor_put_back() {
    let d = tempfile::tempdir().unwrap();
    if reads_any_mode(d.path()) {
        return;
    }
    let h = hooks(d.path());
    chmod(&h.join("sub"), 0);
    let p = Protected::read(vec![tree(&h)]);
    assert_eq!(p.nodes.get(&h.join("sub")), Some(&Node::Unread));
    chmod(&h.join("sub"), 0o750);
    fs::write(h.join("sub/y"), "y\n").unwrap();
    assert!(
        p.check().is_none(),
        "what was under an unread directory is unknown"
    );
    assert!(h.join("sub/y").exists());

    chmod(&h.join("pre-commit"), 0);
    std::fs::create_dir(d.path().join("locked")).unwrap();
    chmod(&d.path().join("locked"), 0);
    let fifo = h.join("fifo");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    // SAFETY: mkfifo(3) reads the NUL-terminated path, which lives across the call.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
    let behind = d.path().join("locked/x");
    let p = Protected::read(vec![tree(&h), tree(&behind)]);
    for unread in [h.join("pre-commit"), fifo.clone(), behind] {
        assert_eq!(p.nodes.get(&unread), Some(&Node::Unread), "{unread:?}");
    }
    chmod(&d.path().join("locked"), 0o750);
    chmod(&h.join("pre-commit"), 0o750);
    fs::write(h.join("pre-commit"), "changed\n").unwrap();
    fs::remove_file(&fifo).unwrap();
    assert!(p.check().is_none());
}

#[test]
fn a_restore_that_fails_says_so() {
    let d = tempfile::tempdir().unwrap();
    if reads_any_mode(d.path()) {
        return;
    }
    let h = hooks(d.path());
    let p = Protected::read(vec![tree(&h.join("sub"))]);
    fs::write(h.join("sub/x"), "changed\n").unwrap();
    chmod(&h.join("sub"), 0o555);
    let t = p.check().unwrap();
    chmod(&h.join("sub"), 0o750);
    let e = t.restore_error.as_deref().unwrap();
    assert!(e.contains("denied"), "{e}");
    let reason = t.reason("build", |p| p.display().to_string());
    assert!(reason.contains("ns could not put it back"), "{reason}");
}

#[test]
fn the_reason_names_the_first_paths_and_counts_the_rest() {
    let t = Tampered {
        paths: ["a", "b", "c", "d", "e"].map(PathBuf::from).to_vec(),
        restore_error: None,
    };
    assert_eq!(
        t.reason("build", |p| p.display().to_string()),
        "build tampered with a protected path (a, b, c, 2 more); ns put it back"
    );
    let t = Tampered {
        paths: vec![PathBuf::from("a")],
        restore_error: None,
    };
    assert_eq!(
        t.reason("verify", |p| format!("<{}>", p.display())),
        "verify tampered with a protected path (<a>); ns put it back"
    );
}

#[test]
fn the_check_of_a_large_tree_takes_under_a_second() {
    let d = tempfile::tempdir().unwrap();
    let skills = d.path().join("skills");
    for s in 0..100 {
        let dir = skills.join(format!("ns-{s}/references"));
        fs::create_dir_all(&dir).unwrap();
        for f in 0..10 {
            fs::write(dir.join(format!("{f}.md")), "x".repeat(4096)).unwrap();
        }
    }
    let c = d.path().join("config");
    fs::write(&c, "[core]\n\tbare = false\n").unwrap();
    let start = std::time::Instant::now();
    let p = Protected::read(vec![tree(&skills), config(&c)]);
    assert!(p.check().is_none());
    let took = start.elapsed();
    assert!(took < std::time::Duration::from_secs(1), "{took:?}");
}

#[test]
fn protected_paths_holds_the_settings_the_snapshot_and_each_skills_link() {
    let d = tempfile::tempdir().unwrap();
    let wt = d.path().join("wt");
    let common = d.path().join("main/.git");
    let artifacts = wt.join(".ns/u");
    fs::create_dir_all(artifacts.join("skills/ns-a")).unwrap();
    git::run(d.path(), &["init", "-q", wt.to_str().unwrap()]).unwrap();
    let own = wt.canonicalize().unwrap().join(".git/config.worktree");
    fs::create_dir_all(wt.join(".claude")).unwrap();
    fs::create_dir_all(wt.join(".agents/skills")).unwrap();
    std::os::unix::fs::symlink("../.ns/u/skills", wt.join(".claude/skills")).unwrap();
    let elsewhere = d.path().join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    let p = protected_paths(&wt, &common, &artifacts, "u");
    let roots: Vec<(PathBuf, Kind)> = p.roots.iter().map(|r| (r.path.clone(), r.kind)).collect();
    assert_eq!(
        roots,
        [
            (common.join("config"), Kind::Config),
            (common.join("config.worktree"), Kind::Config),
            (common.join("hooks"), Kind::Tree),
            (wt.join(".githooks"), Kind::Tree),
            (artifacts.join("skills"), Kind::Tree),
            (pin::settings_path(&common, "u"), Kind::Tree),
            (own, Kind::Config),
            (wt.join(".claude/skills"), Kind::Tree),
        ],
        "a real .agents/skills is the repo's own, and the link's target is the snapshot already"
    );

    fs::remove_file(wt.join(".claude/skills")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, wt.join(".claude/skills")).unwrap();
    let p = protected_paths(&wt, &common, &artifacts, "u");
    let paths: Vec<&PathBuf> = p.roots.iter().map(|r| &r.path).collect();
    assert!(
        paths.contains(&&elsewhere.canonicalize().unwrap()),
        "{paths:?}"
    );

    fs::remove_dir(&elsewhere).unwrap();
    let p = protected_paths(&wt, &common, &artifacts, "u");
    assert_eq!(
        p.roots.last().unwrap().path,
        wt.join(".claude/skills"),
        "a dangling link is still held"
    );
}

#[test]
fn the_reason_names_three_paths_without_a_count() {
    let t = Tampered {
        paths: ["a", "b", "c"].map(PathBuf::from).to_vec(),
        restore_error: None,
    };
    assert_eq!(
        t.reason("build", |p| p.display().to_string()),
        "build tampered with a protected path (a, b, c); ns put it back"
    );
}

#[test]
fn an_added_directory_is_removed_with_what_is_in_it() {
    let d = tempfile::tempdir().unwrap();
    let h = hooks(d.path());
    let p = Protected::read(vec![tree(&h)]);
    fs::create_dir_all(h.join("new/deeper")).unwrap();
    fs::write(h.join("new/deeper/x"), "x\n").unwrap();
    let t = p.check().unwrap();
    assert!(t.restore_error.is_none(), "{t:?}");
    assert_eq!(t.paths.len(), 3, "{t:?}");
    assert!(!h.join("new").exists());
}

#[test]
fn a_path_made_unreadable_during_the_phase_is_put_back() {
    let d = tempfile::tempdir().unwrap();
    if reads_any_mode(d.path()) {
        return;
    }
    let h = hooks(d.path());
    let p = Protected::read(vec![tree(&h)]);
    chmod(&h.join("pre-commit"), 0);
    chmod(&h.join("sub"), 0);
    let t = p.check().unwrap();
    assert!(t.restore_error.is_none(), "{t:?}");
    assert_eq!(
        t.paths,
        [h.join("pre-commit"), h.join("sub"), h.join("sub/x")]
    );
    assert_eq!(mode(&h.join("pre-commit")), 0o750);
    assert_eq!(fs::read_to_string(h.join("sub/x")).unwrap(), "x\n");
    assert!(p.check().is_none());
}

#[test]
fn a_file_too_large_to_hold_is_compared_by_size_and_removed_when_added() {
    let d = tempfile::tempdir().unwrap();
    let h = hooks(d.path());
    fs::write(h.join("edge"), vec![b'x'; MAX_FILE as usize]).unwrap();
    let edge = Protected::read(vec![tree(&h)]);
    assert!(matches!(
        edge.nodes.get(&h.join("edge")),
        Some(Node::File(..))
    ));
    fs::remove_file(h.join("edge")).unwrap();
    let big = vec![b'x'; MAX_FILE as usize + 1];
    let p = Protected::read(vec![tree(&h)]);
    fs::write(h.join("big"), &big).unwrap();
    assert_eq!(p.nodes.get(&h.join("big")), None);
    let t = p.check().unwrap();
    assert!(t.restore_error.is_none(), "{t:?}");
    assert!(!h.join("big").exists());

    fs::write(h.join("big"), &big).unwrap();
    let p = Protected::read(vec![tree(&h)]);
    assert_eq!(
        p.nodes.get(&h.join("big")),
        Some(&Node::Large(MAX_FILE + 1, mode(&h.join("big"))))
    );
    assert!(p.check().is_none());
    fs::write(h.join("big"), "small\n").unwrap();
    let e = p.check().unwrap().restore_error.unwrap();
    assert!(e.contains("more than ns keeps to put back"), "{e}");
}
