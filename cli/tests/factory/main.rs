//! End-to-end tests for `ns run` and `ns watch` with a fake harness (`claude`) and a fake `gh`.

#![allow(
    clippy::disallowed_methods,
    reason = "an integration test is a crate of its own and cannot use crate::proc"
)]

#[path = "../common/mod.rs"]
mod common;
#[path = "../common/factory_env.rs"]
mod factory_env;

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use common::{copy_exe, exits, write_exe, Group, Ns};
use factory_env::{
    another_actor_lands_linked, git, queued, reasons, run_log, unit_with_pr, units_in, Env,
    FAKE_TOKEN, FINDING, NOW, READY, SQUASH_B, UNIT,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

// The tests, by feature. They share this file's namespace, so a helper defined in one part is
// visible in every other.
include!("run_phases_and_skills.rs");
include!("run_rebases_and_gates.rs");
include!("run_ratchets_and_worktrees.rs");
include!("run_locks_and_merge_order.rs");
include!("run_setup_signals.rs");
include!("run_runners_and_github_checks.rs");
include!("merge_conflicts_and_validate.rs");
include!("watch_fix_main.rs");
include!("watch_nights_and_splits.rs");
include!("watch_splits_and_labels.rs");
include!("watch_disk_space.rs");
include!("triage_pass_and_memory_cap.rs");
include!("run_signals_and_cleanup.rs");
include!("cleanup_and_cpus.rs");
include!("cpus_and_self_update.rs");
include!("self_update_and_drains.rs");
include!("skills_sync_and_merge_gate.rs");
include!("merge_gate_and_labels.rs");
include!("merge_update_skip.rs");
include!("run_review_loop.rs");
