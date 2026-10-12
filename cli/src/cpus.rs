//! The per-unit CPU share, `ns run --cpus` (docs/FACTORY.md, "CPU share"). Every phase and gate
//! of the run inherits it as `NS_CPUS`, `RUST_TEST_THREADS` and `CARGO_BUILD_JOBS`; a variable
//! the operator already set is left as it is.

use serde_json::{json, Map, Value};

/// The variables `--cpus` sets: one stack-neutral, two that cargo and the Rust test runner read.
pub const VARS: [&str; 3] = ["NS_CPUS", "RUST_TEST_THREADS", "CARGO_BUILD_JOBS"];

/// `--cpus`: a whole number, 1 or more.
pub fn parse(s: &str) -> Result<u32, String> {
    s.parse().ok().filter(|n| *n >= 1).ok_or_else(|| {
        "give a whole number of CPUs, 1 or more, e.g. ns run --issue 42 --cpus 4".into()
    })
}

/// The CPUs each of `parallel` units gets on `cores`: an even split, never below 1. `parallel` is
/// 1 or more (`ns watch` checks it).
pub fn share(cores: usize, parallel: usize) -> usize {
    (cores / parallel).max(1)
}

/// The share for each of `parallel` units on this machine.
pub fn share_here(parallel: usize) -> usize {
    share(
        std::thread::available_parallelism().map_or(1, |n| n.get()),
        parallel,
    )
}

#[derive(Debug, PartialEq, Eq)]
pub struct Setting {
    pub var: &'static str,
    pub value: String,
    /// Already in the environment, so passed through rather than computed from `--cpus`.
    pub inherited: bool,
}

/// Each of [`VARS`] for `--cpus n`: the value `get` returns for it, else `n`.
pub fn settings(n: u32, get: impl Fn(&str) -> Option<String>) -> Vec<Setting> {
    VARS.iter()
        .map(|&var| match get(var) {
            Some(value) => Setting {
                var,
                value,
                inherited: true,
            },
            None => Setting {
                var,
                value: n.to_string(),
                inherited: false,
            },
        })
        .collect()
}

/// Set each computed variable in this process, so every child it starts inherits it, and return
/// the settings. Call it before any thread starts: other threads may read the environment.
pub fn export(n: u32) -> Vec<Setting> {
    let s = settings(n, |v| {
        std::env::var_os(v).map(|x| x.to_string_lossy().into_owned())
    });
    for x in s.iter().filter(|x| !x.inherited) {
        std::env::set_var(x.var, &x.value);
    }
    s
}

/// The settings for the run log: each variable's value and whether it was computed or inherited.
pub fn log_value(s: &[Setting]) -> Value {
    let m: Map<String, Value> = s
        .iter()
        .map(|x| {
            let source = if x.inherited { "inherited" } else { "computed" };
            (
                x.var.to_string(),
                json!({"value": x.value, "source": source}),
            )
        })
        .collect();
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_share_splits_the_cores_evenly_and_bottoms_out_at_one() {
        assert_eq!(share(16, 1), 16);
        assert_eq!(share(16, 3), 5);
        assert_eq!(share(4, 3), 1);
        assert_eq!(share(4, 4), 1);
        assert_eq!(share(4, 9), 1);
        assert_eq!(share(1, 1), 1);
    }

    #[test]
    fn a_variable_already_set_wins_one_at_a_time() {
        let s = settings(4, |v| (v == "RUST_TEST_THREADS").then(|| "16".to_string()));
        let got: Vec<_> = s
            .iter()
            .map(|x| (x.var, x.value.as_str(), x.inherited))
            .collect();
        assert_eq!(
            got,
            [
                ("NS_CPUS", "4", false),
                ("RUST_TEST_THREADS", "16", true),
                ("CARGO_BUILD_JOBS", "4", false),
            ]
        );
        assert_eq!(
            log_value(&s),
            json!({
                "NS_CPUS": {"value": "4", "source": "computed"},
                "RUST_TEST_THREADS": {"value": "16", "source": "inherited"},
                "CARGO_BUILD_JOBS": {"value": "4", "source": "computed"},
            })
        );
    }

    #[test]
    fn with_nothing_set_every_variable_is_computed() {
        let s = settings(2, |_| None);
        assert!(s.iter().all(|x| x.value == "2" && !x.inherited), "{s:?}");
    }
}
