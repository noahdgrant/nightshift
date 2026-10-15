//! How `ns` resolves the forge token and host (`token_env`, `token_command`, the environment)
//! and exports them to every child without printing the token.

#![allow(
    clippy::disallowed_methods,
    reason = "an integration test is a crate of its own and cannot use crate::proc"
)]

#[path = "common/mod.rs"]
mod common;
#[path = "common/factory_env.rs"]
mod factory_env;

use std::fs;
use std::path::{Path, PathBuf};

use factory_env::*;

#[test]
fn a_failing_token_command_stops_before_any_child_and_hides_its_output() {
    let e = Env::new();
    let cmd = e.script("token.sh", &format!("echo {FAKE_TOKEN}; exit 1"));
    e.config(&format!(
        "[forge.github]\ntoken_command = {:?}\n",
        cmd.to_str().unwrap()
    ));
    for args in [&["run", "--issue", "7"][..], &["watch", "--once"][..]] {
        let out = e.ns().args(args).assert().code(2).get_output().clone();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("github"), "{err}");
        assert!(err.contains(cmd.to_str().unwrap()), "{err}");
        assert!(!err.contains(FAKE_TOKEN), "{err}");
        assert!(!String::from_utf8_lossy(&out.stdout).contains(FAKE_TOKEN));
    }
    assert!(e.gh_calls().is_empty());
    assert!(e.calls().is_empty());
}

#[test]
fn an_empty_token_is_an_error() {
    let e = Env::new();
    let cmd = e.script("token.sh", "echo '   '");
    e.config(&format!(
        "[forge.github]\ntoken_command = {:?}\n",
        cmd.to_str().unwrap()
    ));
    let out = e
        .ns()
        .args(["run", "--issue", "7"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("printed no token"), "{err}");
    e.config("[forge.github]\ntoken_env = \"MY_GH_TOKEN\"\n");
    let out = e
        .ns()
        .env("MY_GH_TOKEN", "")
        .args(["run", "--issue", "7"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("MY_GH_TOKEN"), "{err}");
    assert!(e.calls().is_empty());
}

#[test]
fn forge_host_is_exported_unless_already_set() {
    let e = Env::new();
    e.config("[forge.github]\ntoken_env = \"MY_GH_TOKEN\"\nhost = \"ghe.example.com\"\n");
    let hosts = || fs::read_to_string(e.ghd.join("hosts")).unwrap_or_default();
    e.ns()
        .env("MY_GH_TOKEN", FAKE_TOKEN)
        .env_remove("GH_HOST")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert!(!hosts().is_empty());
    assert!(
        hosts().lines().all(|h| h == "ghe.example.com"),
        "{}",
        hosts()
    );
    fs::remove_file(e.ghd.join("hosts")).unwrap();
    e.ns()
        .env("MY_GH_TOKEN", FAKE_TOKEN)
        .env("GH_HOST", "mine.example.com")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert!(!hosts().is_empty());
    assert!(
        hosts().lines().all(|h| h == "mine.example.com"),
        "{}",
        hosts()
    );
}

#[test]
fn a_configured_gitlab_forge_exports_its_token_and_host() {
    let e = Env::new();
    e.config("[forge.gitlab]\ntoken_env = \"MY_GL_TOKEN\"\nhost = \"gitlab.example.com\"\n");
    let seen = || fs::read_to_string(e.ctrl.join("gitlab")).unwrap_or_default();
    e.ns()
        .env("MY_GL_TOKEN", FAKE_TOKEN)
        .env_remove("GITLAB_TOKEN")
        .env_remove("GITLAB_HOST")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert!(!seen().is_empty());
    assert!(
        seen()
            .lines()
            .all(|l| l == format!("{FAKE_TOKEN} gitlab.example.com")),
        "{}",
        seen()
    );
}

#[test]
fn a_set_gitlab_token_wins_and_no_command_runs() {
    let e = Env::new();
    let marker = e.base.join("ran");
    let cmd = e.script("token.sh", &format!("touch {}; exit 1", marker.display()));
    e.config(&format!(
        "[forge.gitlab]\ntoken_command = {:?}\n",
        cmd.to_str().unwrap()
    ));
    e.ns()
        .env("GITLAB_TOKEN", "from-env")
        .env_remove("GITLAB_HOST")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert!(
        !marker.exists(),
        "token_command ran though GITLAB_TOKEN was set"
    );
    let seen = fs::read_to_string(e.ctrl.join("gitlab")).unwrap();
    assert!(seen.lines().all(|l| l == "from-env "), "{seen}");
}

#[test]
fn an_empty_gh_token_does_not_count_as_set() {
    let e = Env::new();
    let cmd = e.script("token.sh", &format!("echo {FAKE_TOKEN}"));
    e.config(&format!(
        "[forge.github]\ntoken_command = {:?}\n",
        cmd.to_str().unwrap()
    ));
    e.ns()
        .env("GH_TOKEN", "")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert_eq!(e.tokens_seen(), [FAKE_TOKEN]);
}

#[test]
fn a_token_env_value_is_exported_trimmed() {
    let e = Env::new();
    e.config("[forge.github]\ntoken_env = \"MY_GH_TOKEN\"\n");
    e.ns()
        .env("MY_GH_TOKEN", format!("  {FAKE_TOKEN}\n"))
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert_eq!(e.tokens_seen(), [FAKE_TOKEN]);
}

#[test]
fn a_token_with_a_nul_byte_is_an_error_that_hides_it() {
    let e = Env::new();
    let cmd = e.script("token.sh", "printf 'secretA\\000secretB'");
    e.config(&format!(
        "[forge.github]\ntoken_command = {:?}\n",
        cmd.to_str().unwrap()
    ));
    let out = e
        .ns()
        .args(["run", "--issue", "7"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("forge github"), "{err}");
    assert!(!err.contains("secret"), "{err}");
    assert!(!err.contains("panicked"), "{err}");
    assert!(e.calls().is_empty());
}

#[test]
fn token_command_stderr_is_discarded() {
    let e = Env::new();
    let cmd = e.script(
        "token.sh",
        &format!("echo {FAKE_TOKEN} >&2; echo {FAKE_TOKEN}"),
    );
    e.config(&format!(
        "[forge.github]\ntoken_command = {:?}\n",
        cmd.to_str().unwrap()
    ));
    let out = e
        .ns()
        .args(["run", "--issue", "7"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    assert!(!String::from_utf8_lossy(&out.stderr).contains(FAKE_TOKEN));
    assert!(!String::from_utf8_lossy(&out.stdout).contains(FAKE_TOKEN));
}

#[test]
fn an_unsettable_host_is_an_error_that_hides_the_token() {
    let e = Env::new();
    for host in ["a\\u0000b", ""] {
        e.config(&format!(
            "[forge.github]\ntoken_env = \"MY_GH_TOKEN\"\nhost = \"{host}\"\n"
        ));
        let out = e
            .ns()
            .env("MY_GH_TOKEN", FAKE_TOKEN)
            .env_remove("GH_HOST")
            .args(["run", "--issue", "7"])
            .assert()
            .code(2)
            .get_output()
            .clone();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("forge github"), "{err}");
        assert!(err.contains("host"), "{err}");
        assert!(!err.contains(FAKE_TOKEN), "{err}");
        assert!(!err.contains("panicked"), "{err}");
        assert!(e.calls().is_empty());
    }
}

#[test]
fn a_set_gitlab_host_wins_over_the_configured_one() {
    let e = Env::new();
    e.config("[forge.gitlab]\ntoken_env = \"MY_GL_TOKEN\"\nhost = \"gitlab.example.com\"\n");
    e.ns()
        .env("MY_GL_TOKEN", FAKE_TOKEN)
        .env_remove("GITLAB_TOKEN")
        .env("GITLAB_HOST", "mine.example.com")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    let seen = fs::read_to_string(e.ctrl.join("gitlab")).unwrap();
    assert!(!seen.is_empty());
    assert!(
        seen.lines()
            .all(|l| l == format!("{FAKE_TOKEN} mine.example.com")),
        "{seen}"
    );
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(files_under(&p));
        } else {
            out.push(p);
        }
    }
    out
}

#[test]
fn forge_token_reaches_every_child_and_never_the_logs() {
    let e = Env::new();
    let cmd = e.script("token.sh", &format!("echo '  {FAKE_TOKEN}  '"));
    e.config(&format!(
        "[forge.github]\ntoken_command = {:?}\n",
        cmd.to_str().unwrap()
    ));
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("build", &["pass:commit"]);
    let out = e
        .ns()
        .args(["watch", "--once"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    assert_eq!(e.tokens_seen(), [FAKE_TOKEN]);
    assert_eq!(e.calls(), ["triage", "build", "verify", "review", "ship"]);
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!printed.contains(FAKE_TOKEN), "{printed}");
    let ns_dir = e.root.join(".git/ns");
    let logged = files_under(&ns_dir);
    assert!(logged.iter().any(|p| p.ends_with("runs.jsonl")));
    assert!(logged
        .iter()
        .any(|p| p.to_string_lossy().contains("transcripts")));
    for p in logged {
        let text = fs::read_to_string(&p).unwrap_or_default();
        assert!(!text.contains(FAKE_TOKEN), "token in {}", p.display());
    }
}

#[test]
fn forge_token_env_is_read_and_exported() {
    let e = Env::new();
    e.config("[forge.github]\ntoken_env = \"MY_GH_TOKEN\"\n");
    let out = e
        .ns()
        .env("MY_GH_TOKEN", FAKE_TOKEN)
        .args(["run", "--issue", "7"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    assert_eq!(e.tokens_seen(), [FAKE_TOKEN]);
    assert!(!String::from_utf8_lossy(&out.stdout).contains(FAKE_TOKEN));
}

#[test]
fn a_set_gh_token_wins_and_no_command_runs() {
    let e = Env::new();
    let marker = e.base.join("ran");
    let cmd = e.script("token.sh", &format!("touch {}; exit 1", marker.display()));
    e.config(&format!(
        "[forge.github]\ntoken_command = {:?}\n",
        cmd.to_str().unwrap()
    ));
    e.ns()
        .env("GH_TOKEN", "from-env")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert!(
        !marker.exists(),
        "token_command ran though GH_TOKEN was set"
    );
    assert_eq!(e.tokens_seen(), ["from-env"]);
}
