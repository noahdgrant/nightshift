//! Stamp the commit `ns` is built from into `NS_BUILD_COMMIT`, which `ns --version` shows and
//! `ns watch`'s self-update compares against (docs/FACTORY.md, "When the watched repo is
//! nightshift"). A `NS_BUILD_COMMIT` already set, as the self-update sets it for a source tree
//! with no `.git`, wins; a tree that is neither gets `unknown`.

mod build_stamp;

use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .output()
        .ok()?;
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}

fn main() {
    println!("cargo:rerun-if-env-changed=NS_BUILD_COMMIT");
    let commit = build_stamp::commit(std::env::var("NS_BUILD_COMMIT").ok(), || {
        for p in ["HEAD", "logs/HEAD"] {
            if let Some(path) = git(&["rev-parse", "--path-format=absolute", "--git-path", p]) {
                if Path::new(&path).exists() {
                    println!("cargo:rerun-if-changed={path}");
                }
            }
        }
        git(&["rev-parse", "HEAD"])
    });
    println!("cargo:rustc-env=NS_BUILD_COMMIT={commit}");
}
