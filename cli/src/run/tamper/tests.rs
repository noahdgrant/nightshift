use super::*;

fn root(path: &Path, kind: Kind, restore: bool) -> Root {
    Root {
        path: path.to_path_buf(),
        kind,
        restore,
    }
}

fn tree(path: &Path) -> Root {
    root(path, Kind::Tree, true)
}

fn config(path: &Path) -> Root {
    root(path, Kind::Config, true)
}

fn git_ok(dir: &Path, args: &[&str]) {
    let mut full = vec![
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@example.com",
        "-c",
        "commit.gpgsign=false",
    ];
    full.extend_from_slice(args);
    git::run(dir, &full).unwrap();
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
    fs::write(h.join("sub/new"), "added\n").unwrap();
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
fn a_large_unchanged_tree_is_no_tampering() {
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
    let p = Protected::read(vec![tree(&skills), config(&c)]);
    assert!(p.check().is_none());
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
            (wt.join(".githooks"), Kind::Tracked),
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
    let target = p
        .roots
        .iter()
        .find(|r| r.path == elsewhere.canonicalize().unwrap())
        .expect("the link's target is held");
    assert!(!target.restore, "a target outside the unit is not restored");
    assert!(p.roots.iter().filter(|r| !r.restore).count() == 1);

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

fn repo_with_hook(d: &Path) -> (PathBuf, PathBuf) {
    let wt = d.join("wt");
    git::run(d, &["init", "-q", wt.to_str().unwrap()]).unwrap();
    let hooks = wt.join(".githooks");
    fs::create_dir_all(&hooks).unwrap();
    fs::write(hooks.join("pre-commit"), "exit 0\n").unwrap();
    git_ok(&wt, &["add", "."]);
    git_ok(&wt, &["commit", "-q", "-m", "hooks"]);
    (wt, hooks)
}

fn tracked(path: &Path) -> Root {
    root(path, Kind::Tracked, true)
}

#[test]
fn a_githooks_change_the_phase_committed_is_not_tampering() {
    let d = tempfile::tempdir().unwrap();
    let (wt, hooks) = repo_with_hook(d.path());
    let p = Protected::read(vec![tracked(&hooks)]);
    fs::write(hooks.join("pre-commit"), "exit 0\n# edited\n").unwrap();
    fs::write(hooks.join("pre-push"), "exit 0\n").unwrap();
    git_ok(&wt, &["add", "."]);
    git_ok(&wt, &["commit", "-q", "-m", "edit hooks"]);
    assert!(p.check().is_none());
    assert_eq!(
        fs::read_to_string(hooks.join("pre-commit")).unwrap(),
        "exit 0\n# edited\n"
    );

    let p = Protected::read(vec![tracked(&hooks)]);
    git_ok(&wt, &["rm", "-q", "-r", ".githooks/pre-push"]);
    git_ok(&wt, &["commit", "-q", "-m", "drop a hook"]);
    assert!(p.check().is_none(), "a committed removal");
}

#[test]
fn a_githooks_change_left_uncommitted_is_put_back() {
    let d = tempfile::tempdir().unwrap();
    let (_, hooks) = repo_with_hook(d.path());
    let p = Protected::read(vec![tracked(&hooks)]);
    fs::write(hooks.join("pre-commit"), "exit 1\n").unwrap();
    fs::write(hooks.join("extra"), "exit 1\n").unwrap();
    let t = p.check().unwrap();
    assert_eq!(t.paths, [hooks.join("extra"), hooks.join("pre-commit")]);
    assert!(t.restore_error.is_none(), "{t:?}");
    assert_eq!(
        fs::read_to_string(hooks.join("pre-commit")).unwrap(),
        "exit 0\n"
    );
    assert!(!hooks.join("extra").exists());
}

#[test]
fn a_githooks_change_committed_then_changed_again_is_put_back() {
    let d = tempfile::tempdir().unwrap();
    let (wt, hooks) = repo_with_hook(d.path());
    let p = Protected::read(vec![tracked(&hooks)]);
    fs::write(hooks.join("pre-commit"), "exit 2\n").unwrap();
    git_ok(&wt, &["commit", "-q", "-a", "-m", "edit"]);
    fs::write(hooks.join("pre-commit"), "exit 3\n").unwrap();
    let t = p.check().unwrap();
    assert_eq!(t.paths, [hooks.join("pre-commit")]);
}

#[test]
fn a_path_outside_the_unit_is_reported_and_left_alone() {
    let d = tempfile::tempdir().unwrap();
    let h = hooks(d.path());
    let p = Protected::read(vec![root(&h, Kind::Tree, false)]);
    fs::write(h.join("pre-commit"), "changed\n").unwrap();
    fs::write(h.join("added"), "x\n").unwrap();
    let t = p.check().unwrap();
    assert_eq!(t.paths, [h.join("added"), h.join("pre-commit")]);
    let e = t.restore_error.as_deref().unwrap();
    assert!(e.contains("outside the unit"), "{e}");
    assert_eq!(
        fs::read_to_string(h.join("pre-commit")).unwrap(),
        "changed\n"
    );
    assert!(h.join("added").exists());
}

#[test]
fn a_config_restore_puts_back_the_changed_keys_and_keeps_a_concurrent_upstream() {
    let d = tempfile::tempdir().unwrap();
    let c = d.path().join("config");
    let text = "[core]\n\tbare = false\n\thooksPath = hooks\n[user]\n\tname = a\n";
    fs::write(&c, text).unwrap();
    let p = Protected::read(vec![config(&c)]);
    fs::write(
        &c,
        "[core]\n\tbare = false\n\thooksPath = /dev/null\n\tfsmonitor = x\n[user]\n\tname = a\n\
         [branch \"ns/u\"]\n\tremote = origin\n",
    )
    .unwrap();
    let t = p.check().unwrap();
    assert!(t.restore_error.is_none(), "{t:?}");
    let now = fs::read_to_string(&c).unwrap();
    let hooks_path = git::run(
        d.path(),
        &["config", "--file", c.to_str().unwrap(), "core.hookspath"],
    );
    assert_eq!(hooks_path.unwrap(), "hooks");
    assert!(!now.contains("fsmonitor"), "{now}");
    assert!(
        now.contains("remote = origin"),
        "the upstream survives: {now}"
    );
    assert!(p.check().is_none());
}

#[test]
fn a_restore_leaves_no_temporary_file_behind() {
    let d = tempfile::tempdir().unwrap();
    let h = hooks(d.path());
    let p = Protected::read(vec![tree(&h)]);
    fs::write(h.join("pre-commit"), "changed\n").unwrap();
    chmod(&h.join("pre-commit"), 0o600);
    assert!(p.check().unwrap().restore_error.is_none());
    let mut names: Vec<String> = fs::read_dir(&h)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["pre-commit", "sub"]);
    assert_eq!(mode(&h.join("pre-commit")), 0o750);
}

fn tight(entries: usize, bytes: u64, depth: usize) -> Limits {
    Limits {
        entries,
        bytes,
        depth,
    }
}

#[test]
fn a_tree_over_a_cap_before_the_phase_is_never_compared() {
    let d = tempfile::tempdir().unwrap();
    let h = hooks(d.path());
    for limits in [
        tight(2, u64::MAX, 9),
        tight(99, 3, 9),
        tight(99, u64::MAX, 0),
    ] {
        let p = Protected::read_with(vec![tree(&h)], limits);
        assert_eq!(p.nodes.get(&h), Some(&Node::Unread), "{limits:?}");
        fs::write(h.join("pre-commit"), "changed\n").unwrap();
        assert!(p.check().is_none(), "{limits:?}");
        fs::write(h.join("pre-commit"), "exit 0\n").unwrap();
    }
}

#[test]
fn a_tree_that_grows_past_a_cap_during_the_phase_is_tampering() {
    let d = tempfile::tempdir().unwrap();
    let h = hooks(d.path());
    let cases: [(Limits, &str); 3] = [
        (tight(4, u64::MAX, 9), "entries"),
        (tight(99, 20, 9), "bytes"),
        (tight(99, u64::MAX, 2), "deep"),
    ];
    for (limits, what) in cases {
        let p = Protected::read_with(vec![tree(&h)], limits);
        assert!(p.over.is_empty(), "{what}");
        match what {
            "entries" => fs::write(h.join("more"), "x").unwrap(),
            "bytes" => fs::write(h.join("big"), "x".repeat(64)).unwrap(),
            _ => fs::create_dir_all(h.join("a/b/c")).unwrap(),
        }
        let t = p.check().unwrap();
        assert_eq!(t.paths, std::slice::from_ref(&h), "{what}");
        let e = t.restore_error.as_deref().unwrap();
        assert!(e.contains("more than ns reads"), "{what}: {e}");
        let _ = fs::remove_file(h.join("more"));
        let _ = fs::remove_file(h.join("big"));
        let _ = fs::remove_dir_all(h.join("a"));
    }
}

#[test]
fn a_deep_tree_is_read_without_recursion() {
    let d = tempfile::tempdir().unwrap();
    let mut deep = d.path().join("deep");
    for _ in 0..200 {
        deep.push("d");
    }
    fs::create_dir_all(&deep).unwrap();
    let p = Protected::read_with(
        vec![tree(&d.path().join("deep"))],
        tight(10_000, 1 << 20, 500),
    );
    assert_eq!(p.nodes.len(), 201);
    assert!(p.check().is_none());
}

fn repo_without_hooks(d: &Path) -> (PathBuf, PathBuf) {
    let wt = d.join("wt");
    git::run(d, &["init", "-q", wt.to_str().unwrap()]).unwrap();
    fs::write(wt.join("f"), "f\n").unwrap();
    git_ok(&wt, &["add", "."]);
    git_ok(&wt, &["commit", "-q", "-m", "init"]);
    let hooks = wt.join(".githooks");
    (wt, hooks)
}

#[test]
fn a_value_less_config_key_is_put_back_whole() {
    let d = tempfile::tempdir().unwrap();
    let c = d.path().join("config");
    let text = "[core]\n\tbare\n\thooksPath = hooks\n";
    fs::write(&c, text).unwrap();
    let p = Protected::read(vec![config(&c)]);
    fs::write(&c, "[core]\n\tbare = true\n\thooksPath = /dev/null\n").unwrap();
    let t = p.check().unwrap();
    assert!(t.restore_error.is_none(), "{t:?}");
    assert_eq!(fs::read_to_string(&c).unwrap(), text);
    assert!(p.check().is_none());
}

#[test]
fn a_config_key_removed_a_multi_value_key_and_a_deleted_file_are_put_back() {
    let d = tempfile::tempdir().unwrap();
    let c = d.path().join("config");
    fs::write(
        &c,
        "[core]\n\thooksPath = hooks\n[remote \"o\"]\n\tfetch = a\n\tfetch = b\n",
    )
    .unwrap();
    let p = Protected::read(vec![config(&c)]);
    let key = |k: &str| {
        git::run(
            d.path(),
            &["config", "--file", c.to_str().unwrap(), "--get-all", k],
        )
    };

    fs::write(&c, "[remote \"o\"]\n\tfetch = a\n\tfetch = b\n").unwrap();
    assert!(p.check().unwrap().restore_error.is_none());
    assert_eq!(key("core.hookspath").unwrap(), "hooks");

    fs::write(
        &c,
        "[core]\n\thooksPath = hooks\n[remote \"o\"]\n\tfetch = a\n\tfetch = z\n\tfetch = b\n",
    )
    .unwrap();
    assert!(p.check().unwrap().restore_error.is_none());
    assert_eq!(key("remote.o.fetch").unwrap(), "a\nb");

    fs::remove_file(&c).unwrap();
    let t = p.check().unwrap();
    assert!(t.restore_error.is_none(), "{t:?}");
    assert_eq!(key("core.hookspath").unwrap(), "hooks");
    assert!(p.check().is_none());
}

#[test]
fn a_staged_rename_under_githooks_is_put_back() {
    let d = tempfile::tempdir().unwrap();
    let (wt, hooks) = repo_with_hook(d.path());
    let p = Protected::read(vec![tracked(&hooks)]);
    git_ok(&wt, &["mv", ".githooks/pre-commit", ".githooks/renamed"]);
    let t = p.check().unwrap();
    assert!(t.restore_error.is_none(), "{t:?}");
    assert_eq!(t.paths, [hooks.join("pre-commit"), hooks.join("renamed")]);
    assert!(hooks.join("pre-commit").exists());
    assert!(!hooks.join("renamed").exists());
}

#[test]
fn a_new_directory_under_githooks_is_removed_unless_committed() {
    let d = tempfile::tempdir().unwrap();
    let (wt, hooks) = repo_with_hook(d.path());
    let p = Protected::read(vec![tracked(&hooks)]);
    fs::create_dir_all(hooks.join("sub")).unwrap();
    fs::write(hooks.join("sub/x"), "x\n").unwrap();
    let t = p.check().unwrap();
    assert!(t.restore_error.is_none(), "{t:?}");
    assert!(!hooks.join("sub").exists());

    fs::create_dir_all(hooks.join("sub")).unwrap();
    fs::write(hooks.join("sub/x"), "x\n").unwrap();
    git_ok(&wt, &["add", "."]);
    git_ok(&wt, &["commit", "-q", "-m", "sub"]);
    assert!(p.check().is_none());
    assert!(hooks.join("sub/x").exists());
}

#[test]
fn a_githooks_created_where_there_was_none_is_removed_unless_committed() {
    let d = tempfile::tempdir().unwrap();
    let (wt, hooks) = repo_without_hooks(d.path());
    let p = Protected::read(vec![tracked(&hooks)]);
    fs::create_dir_all(&hooks).unwrap();
    fs::write(hooks.join("pre-commit"), "exit 1\n").unwrap();
    let t = p.check().unwrap();
    assert!(t.restore_error.is_none(), "{t:?}");
    assert!(!hooks.exists());

    fs::create_dir_all(&hooks).unwrap();
    fs::write(hooks.join("pre-commit"), "exit 0\n").unwrap();
    git_ok(&wt, &["add", "."]);
    git_ok(&wt, &["commit", "-q", "-m", "hooks"]);
    assert!(p.check().is_none());
    assert!(hooks.join("pre-commit").exists());
}

#[test]
fn dirty_under_is_none_when_git_cannot_say_and_every_change_stays() {
    let d = tempfile::tempdir().unwrap();
    let h = hooks(d.path());
    assert!(dirty_under(&h).is_none());
    let p = Protected::read(vec![tracked(&h)]);
    fs::write(h.join("pre-commit"), "changed\n").unwrap();
    let t = p.check().unwrap();
    assert_eq!(t.paths, [h.join("pre-commit")]);
    assert_eq!(
        fs::read_to_string(h.join("pre-commit")).unwrap(),
        "exit 0\n"
    );
}

#[test]
fn the_tracked_check_does_not_run_a_planted_fsmonitor_or_hook() {
    let d = tempfile::tempdir().unwrap();
    let (wt, hooks) = repo_with_hook(d.path());
    let marker = d.path().join("ran");
    let p = Protected::read(vec![tracked(&hooks)]);
    let cmd = format!("touch {}", marker.display());
    git_ok(&wt, &["config", "core.fsmonitor", &cmd]);
    fs::write(hooks.join("pre-commit"), "exit 1\n").unwrap();
    assert!(dirty_under(&hooks).is_some());
    assert!(!marker.exists(), "git status ran the planted command");
    drop(p);
}

#[test]
fn the_production_limits_hold_a_representative_tree() {
    let d = tempfile::tempdir().unwrap();
    let h = hooks(d.path());
    let p = Protected::read(vec![tree(&h)]);
    assert!(p.over.is_empty());
    assert!(p.nodes.contains_key(&h.join("sub/x")));
    assert_eq!(
        (LIMITS.entries, LIMITS.bytes, LIMITS.depth),
        (20_000, 64 * 1024 * 1024, 32)
    );
}

#[test]
fn the_byte_cap_is_inclusive() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("t");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("a"), [0u8; 10]).unwrap();
    assert!(Protected::read_with(vec![tree(&dir)], tight(10, 10, 4))
        .over
        .is_empty());
    let over = Protected::read_with(vec![tree(&dir)], tight(10, 9, 4)).over;
    assert!(over.contains(&dir));
}
