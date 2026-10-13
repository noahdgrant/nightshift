//! A shell that starts in the phase's `TMPDIR` writes there, and only there.

use super::*;

const OWN: &str = "/r/.git/ns/units/u1/tmp/build-1";

fn denied_from(cwd: &str, cmd: &str) -> bool {
    let call = json!({"tool_name": "Bash", "cwd": cwd, "tool_input": {"command": cmd}});
    check_in(&call, Some(OWN)).is_some()
}

#[test]
fn relative_writes_from_a_cwd_in_tmpdir_are_allowed() {
    let sub = format!("{OWN}/sub");
    for cwd in [OWN, sub.as_str()] {
        for c in [
            "echo hi > out.txt",
            "cargo test > out.log",
            "touch a b",
            "rm -rf *",
            "mkdir -p x && cd x && echo hi > y",
            "echo x > $TMPDIR/out.txt",
            "cd $TMPDIR && echo hi > out.txt",
        ] {
            assert!(!denied_from(cwd, c), "{cwd}: {c}");
        }
    }
    assert!(!denied_from(&sub, "echo x > ../out.txt"));
    assert!(!denied_from(&sub, "cd .. && touch x"));
}

#[test]
fn relative_writes_that_leave_tmpdir_from_a_cwd_in_it_are_denied() {
    let sub = format!("{OWN}/sub");
    for c in [
        "echo hi > ../../phase-settings.json",
        "echo x > ../build-2/x",
        "rm -rf ../..",
        "rm -rf .*",
        "cd .. && touch x",
        "cd ../.. && echo {} > phase-settings.json",
    ] {
        assert!(denied_from(OWN, c), "{c}");
    }
    for c in [
        "echo hi > ../../../phase-settings.json",
        "echo x > ../../build-2/x",
        "rm -rf ../../..",
    ] {
        assert!(denied_from(&sub, c), "{c}");
    }
}

#[test]
fn a_cwd_in_another_tmpdir_stays_protected() {
    for cwd in [
        "/r/.git/ns/units/u1/tmp/build-2",
        "/r/.git/ns/units/u2/tmp/build-1",
        "/r/.git/ns/units/u1",
    ] {
        assert!(denied_from(cwd, "echo hi > out.txt"), "{cwd}");
    }
}
