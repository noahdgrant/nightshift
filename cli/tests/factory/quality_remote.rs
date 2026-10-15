/// A bare `records.git` as the repo's `records` remote.
fn records_remote(e: &Env) -> PathBuf {
    let bare = e.base.join("records.git");
    git(&e.base, &["init", "-q", "--bare", bare.to_str().unwrap()]);
    git(
        &e.root,
        &["remote", "add", "records", bare.to_str().unwrap()],
    );
    bare
}

fn records_on(bare: &Path) -> usize {
    if git(bare, &["for-each-ref", QUALITY_BRANCH]).is_empty() {
        return 0;
    }
    git(bare, &["show", &format!("{QUALITY_BRANCH}:records.jsonl")])
        .lines()
        .count()
}

fn outbox_lines(e: &Env) -> usize {
    fs::read_to_string(e.root.join(".git/ns/quality-outbox.jsonl"))
        .unwrap_or_default()
        .lines()
        .count()
}

#[test]
fn run_publishes_its_record_to_the_quality_remote_and_leaves_origin_alone() {
    let e = Env::new();
    let bare = records_remote(&e);
    e.factory("[quality]\nremote = \"records\"\n");
    e.queue("build", &["pass:commit"]);
    e.ctl("review.body", FINDING);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(records_on(&bare), 1);
    assert!(quality_records(&e).is_empty());
    assert_eq!(outbox_lines(&e), 0);
}

#[test]
fn run_keeps_its_record_in_the_outbox_under_a_quality_remote_of_none() {
    let e = Env::new();
    e.factory("[quality]\nremote = \"none\"\n");
    e.queue("build", &["pass:commit"]);
    e.ctl("review.body", FINDING);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert!(quality_records(&e).is_empty());
    assert_eq!(outbox_lines(&e), 1);
    assert_eq!(quality_events(&e)[0]["status"], "outbox");
}

#[test]
fn run_records_by_the_factory_directory_it_was_given() {
    let e = Env::new();
    let alt = e.base.join("alt-factory");
    fs::create_dir(&alt).unwrap();
    fs::write(alt.join("nightshift.toml"), "[quality]\nremote = \"none\"\n").unwrap();
    e.queue("build", &["pass:commit"]);
    e.ctl("review.body", FINDING);
    let v = e.run(
        &["run", "--issue", "7", "--factory", alt.to_str().unwrap()],
        0,
    );
    assert_eq!(v["outcome"], "done", "{v}");
    assert!(quality_records(&e).is_empty());
    assert_eq!(outbox_lines(&e), 1);
}

#[test]
fn clean_publishes_its_records_to_the_quality_remote_and_leaves_origin_alone() {
    let e = Env::new();
    let bare = records_remote(&e);
    e.factory("[quality]\nremote = \"records\"\n");
    unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    let v = e.run(&["clean"], 0);
    assert_eq!(units_in(&v["removed"]), ["3-done"], "{v}");
    assert_eq!(records_on(&bare), 1);
    assert!(quality_records(&e).is_empty());
}

#[test]
fn clean_keeps_its_records_in_the_outbox_under_a_quality_remote_of_none() {
    let e = Env::new();
    e.factory("[quality]\nremote = \"none\"\n");
    unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    let v = e.run(&["clean"], 0);
    assert_eq!(units_in(&v["removed"]), ["3-done"], "{v}");
    assert!(quality_records(&e).is_empty());
    assert_eq!(outbox_lines(&e), 1);
}

#[test]
fn clean_with_a_factory_file_that_does_not_load_holds_the_records_instead_of_pushing_to_origin() {
    let e = Env::new();
    e.factory("[quality]\nremote = \"none\"\nbroken = [\n");
    unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    e.run(&["clean"], 0);
    assert!(quality_records(&e).is_empty());
    assert_eq!(outbox_lines(&e), 1);
    let reason = quality_events(&e)[0]["reason"].as_str().unwrap().to_string();
    assert!(reason.contains("did not load"), "{reason}");
}
