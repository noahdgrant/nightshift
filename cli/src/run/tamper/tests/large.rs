use super::*;

fn large_hooks(d: &Path) -> (PathBuf, Vec<u8>) {
    let h = hooks(d);
    (h, vec![b'x'; MAX_FILE as usize + 1])
}

#[test]
fn a_changed_large_file_does_not_stop_the_restore_of_later_paths() {
    let d = tempfile::tempdir().unwrap();
    let (h, big) = large_hooks(d.path());
    fs::write(h.join("aaa"), &big).unwrap();
    let p = Protected::read(vec![tree(&h)]);
    fs::write(h.join("aaa"), "small\n").unwrap();
    fs::write(h.join("zzz-planted"), "x").unwrap();
    fs::write(h.join("pre-commit"), "changed\n").unwrap();
    let t = p.check().unwrap();
    assert!(t.restore_error.unwrap().contains("aaa"));
    assert!(!h.join("zzz-planted").exists());
    assert_ne!(
        fs::read_to_string(h.join("pre-commit")).unwrap(),
        "changed\n"
    );
}

#[test]
fn a_large_file_that_is_deleted_or_replaced_is_tampering_and_not_restored() {
    for replace_with_dir in [false, true] {
        let d = tempfile::tempdir().unwrap();
        let (h, big) = large_hooks(d.path());
        fs::write(h.join("big"), &big).unwrap();
        let p = Protected::read(vec![tree(&h)]);
        fs::remove_file(h.join("big")).unwrap();
        if replace_with_dir {
            fs::create_dir(h.join("big")).unwrap();
        }
        let t = p.check().unwrap();
        assert_eq!(t.paths, [h.join("big")]);
        assert!(t.restore_error.is_some(), "{t:?}");
    }
}

#[test]
fn a_small_file_grown_past_the_hold_size_is_tampering() {
    let d = tempfile::tempdir().unwrap();
    let (h, big) = large_hooks(d.path());
    fs::write(h.join("grow"), "small\n").unwrap();
    let p = Protected::read(vec![tree(&h)]);
    fs::write(h.join("grow"), &big).unwrap();
    let t = p.check().unwrap();
    assert_eq!(t.paths, [h.join("grow")]);
    assert!(t.restore_error.is_none(), "{t:?}");
    assert_eq!(fs::read_to_string(h.join("grow")).unwrap(), "small\n");
}

#[test]
fn a_mode_only_change_to_a_large_file_is_tampering() {
    let d = tempfile::tempdir().unwrap();
    let (h, big) = large_hooks(d.path());
    fs::write(h.join("big"), &big).unwrap();
    chmod(&h.join("big"), 0o640);
    let p = Protected::read(vec![tree(&h)]);
    chmod(&h.join("big"), 0o750);
    let t = p.check().unwrap();
    assert_eq!(t.paths, [h.join("big")]);
}
