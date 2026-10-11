//! The fakes `ns-supervise` eval cases run against (`evals/fakes/systemd/`), checked by their
//! own `test.sh` so CI runs it.

use std::path::Path;
use std::process::Command;

#[test]
fn systemd_fakes_pass_their_checks() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../evals/fakes/systemd/test.sh");
    let out = Command::new("bash").arg(&script).output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{text}");
    assert!(text.contains("fakes: ok"), "{text}");
}
