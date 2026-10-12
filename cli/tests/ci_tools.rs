//! `scripts/ci-tools.sh`, run in a temp copy against its own tool list and stub tools.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

const LIST: &str = r#"
[widget]
version = "1.2.3"
install = "pip"
check = "widget --version"

[gadget]
version = "18"
install = "apt"
package = "gadget-18"
check = "gadget --version"

[gizmo]
version = "0.20.2"
install = "install-action"
check = "gizmo --version"
"#;

fn write_exe(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn real(tool: &str) -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|d| d.join(tool))
        .find(|p| p.is_file())
        .unwrap_or_else(|| panic!("{tool} is required to run ci-tools.sh"))
}

struct Repo {
    dir: tempfile::TempDir,
}

impl Repo {
    /// The script with `list` as its tool list, and only `bash`, `dirname` and a `python3`
    /// that logs `-m pip` runs on PATH.
    fn new(list: &str) -> Repo {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/ci-tools.sh");
        write_exe(
            &root.join("scripts/ci-tools.sh"),
            &fs::read_to_string(script).unwrap(),
        );
        fs::write(root.join("scripts/ci-tools.toml"), list).unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        for tool in ["bash", "dirname"] {
            std::os::unix::fs::symlink(real(tool), root.join("bin").join(tool)).unwrap();
        }
        let python = format!(
            "#!/bin/sh\ncase \"$1 $2\" in\n  \"-m pip\") echo \"python3 $*\" >> {log:?} ;;\n  *) exec {real:?} \"$@\" ;;\nesac\n",
            log = root.join("log"),
            real = real("python3"),
        );
        write_exe(&root.join("bin/python3"), &python);
        let sudo = format!("#!/bin/sh\necho \"sudo $*\" >> {:?}\n", root.join("log"));
        write_exe(&root.join("bin/sudo"), &sudo);
        Repo { dir }
    }

    /// Puts a stub `tool` on PATH whose `--version` prints `version`, and exits `code`.
    fn tool(self, tool: &str, version: &str, code: i32) -> Repo {
        let body = format!("#!/bin/sh\necho '{tool} {version}'\nexit {code}\n");
        write_exe(&self.dir.path().join("bin").join(tool), &body);
        self
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let root = self.dir.path();
        let mut cmd = common::command(root.join("bin/bash"));
        cmd.arg(root.join("scripts/ci-tools.sh"))
            .args(args)
            .env("PATH", root.join("bin"))
            .env_remove("GITHUB_OUTPUT");
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.output()
    }

    fn log(&self) -> String {
        fs::read_to_string(self.dir.path().join("log")).unwrap_or_default()
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn all_installed() -> Repo {
    Repo::new(LIST)
        .tool("widget", "1.2.3", 0)
        .tool("gadget", "Ubuntu LLVM version 18.1.3", 0)
        .tool("gizmo", "0.20.2", 0)
}

#[test]
fn check_passes_when_every_tool_is_at_its_version() {
    let out = all_installed().run(&["check"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stderr(&out), "");
}

#[test]
fn check_fails_on_a_missing_tool_and_names_its_install_command() {
    for (tool, install) in [
        ("widget", "uv tool install widget==1.2.3"),
        (
            "gadget",
            "sudo apt-get install gadget-18 && sudo ln -sf /usr/bin/gadget-18 /usr/local/bin/gadget",
        ),
        ("gizmo", "cargo install --locked gizmo@0.20.2"),
    ] {
        let repo = all_installed();
        fs::remove_file(repo.dir.path().join("bin").join(tool)).unwrap();
        let out = repo.run(&["check"], &[]);
        assert_eq!(out.status.code(), Some(1), "{tool}: {}", stderr(&out));
        assert_eq!(
            stderr(&out),
            format!("ci-tools: {tool} is not on PATH; install it with: {install}\n")
        );
    }
}

#[test]
fn an_apt_tool_named_as_its_package_needs_no_link() {
    let list = "[wotsit]\nversion = \"2\"\ninstall = \"apt\"\ncheck = \"wotsit --version\"\n";
    let out = Repo::new(list).run(&["check"], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(
        stderr(&out),
        "ci-tools: wotsit is not on PATH; install it with: sudo apt-get install wotsit\n"
    );
}

#[test]
fn check_fails_on_a_wrong_version() {
    for printed in ["1.2.4", "1.2.30", "11.2.3", "0.1.2.3", "1.2", "1a2b3"] {
        let out = all_installed()
            .tool("widget", printed, 0)
            .run(&["check"], &[]);
        assert_eq!(out.status.code(), Some(1), "{printed}: {}", stderr(&out));
        assert_eq!(
            stderr(&out),
            format!(
                "ci-tools: widget is not version 1.2.3 (`widget --version` printed: widget {printed}); install it with: uv tool install widget==1.2.3\n"
            )
        );
    }
}

#[test]
fn check_fails_when_the_version_command_fails() {
    let out = all_installed()
        .tool("gizmo", "0.20.2\nNo module named gizmo", 3)
        .run(&["check"], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(
        stderr(&out),
        "ci-tools: `gizmo --version` failed: No module named gizmo; install it with: cargo install --locked gizmo@0.20.2\n"
    );
}

#[test]
fn check_reports_every_bad_tool_and_only_the_ones_named() {
    let repo = Repo::new(LIST).tool("gadget", "17.0.1", 0);
    let out = repo.run(&["check"], &[]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert_eq!(err.lines().count(), 3, "{err}");
    for tool in [
        "widget is not on PATH",
        "gadget is not version 18",
        "gizmo is not on PATH",
    ] {
        assert!(err.contains(tool), "{err}");
    }

    let out = repo.run(&["check", "widget"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(!stderr(&out).contains("gadget"), "{}", stderr(&out));

    let out = all_installed()
        .tool("gadget", "17.0.1", 0)
        .run(&["check", "widget", "gizmo"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn install_installs_each_tool_its_way() {
    let repo = Repo::new(LIST);
    let output = repo.dir.path().join("github-output");
    fs::write(&output, "earlier=1\n").unwrap();
    let out = repo.run(&["install"], &[("GITHUB_OUTPUT", output.to_str().unwrap())]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        repo.log(),
        "python3 -m pip install widget==1.2.3\n\
         sudo apt-get update\n\
         sudo apt-get install -y gadget-18\n\
         sudo ln -sf /usr/bin/gadget-18 /usr/local/bin/gadget\n"
    );
    assert_eq!(
        fs::read_to_string(&output).unwrap(),
        "earlier=1\ninstall-action=gizmo@0.20.2\n"
    );
}

#[test]
fn install_runs_only_the_installers_the_named_tools_need() {
    let list = format!(
        "{LIST}\n[wotsit]\nversion = \"2\"\ninstall = \"apt\"\ncheck = \"wotsit --version\"\n\n[doohickey]\nversion = \"4.5\"\ninstall = \"install-action\"\ncheck = \"doohickey -V\"\n"
    );
    let repo = Repo::new(&list);
    let output = repo.dir.path().join("github-output");
    let out = repo.run(
        &["install", "wotsit", "gizmo", "doohickey"],
        &[("GITHUB_OUTPUT", output.to_str().unwrap())],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        repo.log(),
        "sudo apt-get update\nsudo apt-get install -y wotsit\nsudo ln -sf /usr/bin/wotsit /usr/local/bin/wotsit\n"
    );
    assert_eq!(
        fs::read_to_string(&output).unwrap(),
        "install-action=gizmo@0.20.2,doohickey@4.5\n"
    );

    let repo = Repo::new(LIST);
    let out = repo.run(&["install", "widget"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(repo.log(), "python3 -m pip install widget==1.2.3\n");
}

#[test]
fn install_action_tools_need_github_actions() {
    let repo = Repo::new(LIST);
    let out = repo.run(&["install", "gizmo"], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(
        stderr(&out),
        "ci-tools: gizmo installs with taiki-e/install-action, which runs only in GitHub Actions; locally: cargo install --locked gizmo@0.20.2\n"
    );
    assert_eq!(repo.log(), "");
}

#[test]
fn an_unknown_tool_or_method_is_a_usage_error() {
    let out = all_installed().run(&["check", "widget", "nope", "nada"], &[]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        stderr(&out),
        "ci-tools: not in scripts/ci-tools.toml: nope nada\n"
    );

    let list = "[odd]\nversion = \"1\"\ninstall = \"brew\"\ncheck = \"odd --version\"\n";
    for mode in ["check", "install"] {
        let repo = Repo::new(list);
        let out = repo.run(&[mode], &[]);
        assert_eq!(out.status.code(), Some(2), "{mode}");
        assert_eq!(
            stderr(&out),
            "ci-tools: odd: install must be pip, apt or install-action, not brew\n"
        );
        assert_eq!(repo.log(), "");
    }
}

#[test]
fn a_bad_mode_prints_usage_and_exits_2() {
    for args in [&[][..], &["list"], &["--check"]] {
        let out = all_installed().run(args, &[]);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_eq!(
            stderr(&out),
            "usage: scripts/ci-tools.sh install|check [<tool>...]\n"
        );
    }
}

#[test]
fn an_empty_list_has_nothing_to_check_or_install() {
    for mode in ["check", "install"] {
        let repo = Repo::new("# no tools yet\n");
        let out = repo.run(&[mode], &[]);
        assert!(out.status.success(), "{mode}: {}", stderr(&out));
        assert_eq!(stderr(&out), "", "{mode}");
        assert_eq!(repo.log(), "", "{mode}");
    }
}

#[test]
fn a_pip_tool_installs_and_hints_by_its_package_name() {
    let list = "[sprocket]\nversion = \"3.4.5\"\ninstall = \"pip\"\npackage = \"sprocket-cli\"\ncheck = \"sprocket --version\"\n";
    let repo = Repo::new(list);
    let out = repo.run(&["install"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(repo.log(), "python3 -m pip install sprocket-cli==3.4.5\n");

    let out = repo.run(&["check"], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(
        stderr(&out),
        "ci-tools: sprocket is not on PATH; install it with: uv tool install sprocket-cli==3.4.5\n"
    );
}

#[test]
fn a_python_older_than_3_11_fails_with_a_message_naming_the_floor() {
    for mode in ["check", "install"] {
        let repo = all_installed();
        write_exe(
            &repo.dir.path().join("bin/python3"),
            "#!/bin/sh\ncase \"$1\" in\n  -c) exit 1 ;;\n  *) echo 'ModuleNotFoundError: tomllib' >&2; exit 1 ;;\nesac\n",
        );
        let out = repo.run(&[mode], &[]);
        assert!(!out.status.success(), "{mode}");
        assert_eq!(
            stderr(&out),
            "ci-tools: needs Python 3.11 or newer (for tomllib); python3 on PATH is older\n",
            "{mode}"
        );
    }
}
