//! The units' shared memory (docs/factory/watch.md, "Memory"): the machine's memory from
//! `/proc/meminfo`, the systemd user slice `ns.slice` that caps every unit `ns watch` starts,
//! and each unit's memory peak, read from its cgroup and recorded in the run log.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::proc::{Kind, Proc};

/// The memory left outside the slice when `[limits] total_memory_mb` is unset.
pub const RESERVE_MB: u64 = 4096;
/// The slice every unit's own slice sits in.
pub const SLICE: &str = "ns.slice";
/// The unit's own slice, which `ns watch` starts its `ns run` in. The run reads its peak there,
/// and a phase under `[limits] memory_mb` starts its scope there.
pub const SLICE_ENV: &str = "NS_SLICE";
/// Tests set this to a directory holding `proc-cgroup`, read in place of `/proc/self/cgroup`,
/// and `root`, the tree read in place of `/sys/fs/cgroup`.
const CGROUP_ENV: &str = "NS_CGROUP_DIR";
/// Tests set this to the file read in place of `/proc/meminfo`.
const MEMINFO_ENV: &str = "NS_MEMINFO";
/// How long a systemd call may take before it counts as failed.
const SYSTEMD_TIMEOUT: Duration = Duration::from_secs(10);

/// The machine's memory, in MB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemInfo {
    pub total_mb: u64,
    pub available_mb: u64,
}

/// `MemTotal` and `MemAvailable` from the text of `/proc/meminfo`.
fn parse_meminfo(text: &str) -> Option<MemInfo> {
    let kb = |key: &str| {
        text.lines().find_map(|l| {
            let v = l.strip_prefix(key)?.strip_prefix(':')?;
            v.trim().strip_suffix("kB")?.trim().parse::<u64>().ok()
        })
    };
    Some(MemInfo {
        total_mb: kb("MemTotal")? / 1024,
        available_mb: kb("MemAvailable")? / 1024,
    })
}

/// The machine's memory now, or `None` when `/proc/meminfo` can't be read.
pub fn meminfo() -> Option<MemInfo> {
    let path = std::env::var_os(MEMINFO_ENV).map_or_else(|| "/proc/meminfo".into(), PathBuf::from);
    parse_meminfo(&fs::read_to_string(path).ok()?)
}

/// The slice's `MemoryMax`: `configured`, else `total_mb` less [`RESERVE_MB`].
pub fn cap_mb(total_mb: u64, configured: Option<u64>) -> u64 {
    configured.unwrap_or(total_mb.saturating_sub(RESERVE_MB))
}

/// Cap [`SLICE`] for tonight's units at `configured` MB, else at the machine's total less
/// [`RESERVE_MB`]: the cap, or why the units go without one.
pub fn shared_cap(configured: Option<u64>) -> Result<u64, String> {
    let m = meminfo().ok_or("cannot read /proc/meminfo")?;
    let mb = cap_mb(m.total_mb, configured);
    if mb == 0 {
        return Err(format!(
            "{} MB of memory leaves none past the {RESERVE_MB} MB kept back",
            m.total_mb
        ));
    }
    set_up(mb)?;
    Ok(mb)
}

/// The memory peaks, in MB, of the units the run log at `log` records as ended.
pub fn peaks(log: &Path) -> Vec<u64> {
    let text = fs::read_to_string(log).unwrap_or_default();
    text.lines()
        .filter(|l| l.contains("\"memory_peak_mb\""))
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|ev| ev["event"] == "end")
        .filter_map(|ev| ev["memory_peak_mb"].as_u64())
        .collect()
}

/// The `systemctl` arguments that cap [`SLICE`] at `mb` until the user manager restarts.
fn cap_args(mb: u64) -> Vec<String> {
    [
        "--user",
        "set-property",
        "--runtime",
        SLICE,
        &format!("MemoryMax={mb}M"),
        "MemorySwapMax=0",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// The command a unit's `ns run` starts through: a systemd user scope in `slice`.
pub fn launcher(slice: &str) -> Vec<String> {
    [
        "systemd-run",
        "--user",
        "--scope",
        "--quiet",
        "--collect",
        &format!("--slice={slice}"),
        "--",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// The slice of the unit tagged `tag`, a child of [`SLICE`]. systemd reads each `-` in a slice's
/// name as a level, so the tag keeps only letters and digits.
pub fn unit_slice(tag: &str) -> String {
    let tag: String = tag.chars().filter(char::is_ascii_alphanumeric).collect();
    format!("ns-{tag}.slice")
}

/// Cap [`SLICE`] at `mb` and check a scope starts in it; why not, when systemd user scopes
/// don't work here.
fn set_up(mb: u64) -> Result<(), String> {
    let mut cap = Proc::tool("systemctl");
    cap.args(cap_args(mb));
    run(cap, "systemctl --user set-property")?;
    let probe = Proc::launched(Kind::Tool, &launcher(SLICE), "true");
    run(probe, "systemd-run --user --scope")
}

fn run(mut c: Proc, name: &str) -> Result<(), String> {
    match c.timeout(SYSTEMD_TIMEOUT).output() {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(format!(
            "{name} failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Err(e) => Err(format!("{name}: {e}")),
    }
}

/// This process's unit peak, in MB: the high-water mark of the slice [`SLICE_ENV`] names, when
/// this process runs in it and its cgroup records one.
pub fn peak_mb() -> Option<u64> {
    let slice = std::env::var(SLICE_ENV).ok()?;
    match std::env::var_os(CGROUP_ENV).map(PathBuf::from) {
        Some(dir) => {
            let cgroup = fs::read_to_string(dir.join("proc-cgroup")).ok()?;
            peak_in(&dir.join("root"), &cgroup, &slice)
        }
        None => {
            let cgroup = fs::read_to_string("/proc/self/cgroup").ok()?;
            peak_in(Path::new("/sys/fs/cgroup"), &cgroup, &slice)
        }
    }
}

/// The peak, in MB, of the cgroup named `slice` that the process `cgroup` (the text of
/// `/proc/<pid>/cgroup`) runs in, under the cgroup tree at `root`: cgroup v2's `memory.peak`,
/// else cgroup v1's `memory.max_usage_in_bytes`.
fn peak_in(root: &Path, cgroup: &str, slice: &str) -> Option<u64> {
    let (dir, file) = slice_dir(cgroup, slice)?;
    let text = fs::read_to_string(root.join(dir).join(file)).ok()?;
    Some(text.trim().parse::<u64>().ok()?.div_ceil(1024 * 1024))
}

/// Where under the cgroup root the cgroup `slice` is, from the text of `/proc/<pid>/cgroup`,
/// and the file that holds its peak: v2's unified line, else v1's memory controller.
fn slice_dir(cgroup: &str, slice: &str) -> Option<(PathBuf, &'static str)> {
    let up_to = |path: &str| {
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let at = parts.iter().position(|p| *p == slice)?;
        Some(parts[..=at].iter().collect::<PathBuf>())
    };
    cgroup.lines().find_map(|l| {
        let mut f = l.splitn(3, ':');
        let (_, controllers, path) = (f.next()?, f.next()?, f.next()?);
        if controllers.is_empty() {
            return Some((up_to(path)?, "memory.peak"));
        }
        if controllers.split(',').any(|c| c == "memory") {
            return Some((
                Path::new("memory").join(up_to(path)?),
                "memory.max_usage_in_bytes",
            ));
        }
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo_gives_the_total_and_available_mb() {
        let text = "MemTotal:       24608768 kB\nMemFree:  100 kB\nMemAvailable:   21547008 kB\n";
        assert_eq!(
            parse_meminfo(text),
            Some(MemInfo {
                total_mb: 24032,
                available_mb: 21042
            })
        );
        assert_eq!(parse_meminfo("MemTotal: 1024 kB\n"), None);
        assert_eq!(parse_meminfo("MemAvailable: 1024 kB\n"), None);
        assert_eq!(
            parse_meminfo("MemTotal: x kB\nMemAvailable: 1024 kB\n"),
            None
        );
        // A key that only starts with the one wanted is another key.
        let text = "MemTotalX: 9 kB\nMemTotal: 2048 kB\nMemAvailable: 1024 kB\n";
        assert_eq!(
            parse_meminfo(text),
            Some(MemInfo {
                total_mb: 2,
                available_mb: 1
            })
        );
    }

    #[test]
    fn the_cap_is_the_total_less_4_gb_unless_configured() {
        assert_eq!(cap_mb(24032, None), 19936);
        assert_eq!(cap_mb(24032, Some(8000)), 8000);
        assert_eq!(cap_mb(2048, None), 0);
    }

    #[test]
    fn peaks_come_from_the_end_events_that_carry_one() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("runs.jsonl");
        assert!(peaks(&log).is_empty());
        let lines = [
            r#"{"event":"end","outcome":"done","memory_peak_mb":1200}"#,
            r#"{"event":"end","outcome":"done"}"#,
            r#"{"event":"phase","memory_peak_mb":9999}"#,
            r#"{"event":"end","memory_peak_mb":"big"}"#,
            "not json \"memory_peak_mb\"",
            r#"{"event":"end","outcome":"stuck","memory_peak_mb":800}"#,
        ];
        fs::write(&log, lines.join("\n")).unwrap();
        assert_eq!(peaks(&log), [1200, 800]);
    }

    #[test]
    fn the_slice_is_capped_at_the_given_mb_without_swap() {
        assert_eq!(
            cap_args(19936),
            [
                "--user",
                "set-property",
                "--runtime",
                "ns.slice",
                "MemoryMax=19936M",
                "MemorySwapMax=0"
            ]
        );
    }

    #[test]
    fn a_unit_starts_in_a_scope_in_its_own_slice_under_ns_slice() {
        assert_eq!(unit_slice("492-p1-2"), "ns-492p12.slice");
        assert_eq!(unit_slice("a-b_c.d"), "ns-abcd.slice");
        assert_eq!(
            launcher("ns-x1.slice"),
            [
                "systemd-run",
                "--user",
                "--scope",
                "--quiet",
                "--collect",
                "--slice=ns-x1.slice",
                "--"
            ]
        );
    }

    #[test]
    fn the_peak_is_read_from_the_named_slice_in_cgroup_v2_or_v1() {
        let v2 =
            "0::/user.slice/user-1000.slice/user@1000.service/ns.slice/ns-a1.slice/run-r1.scope\n";
        let (dir, file) = slice_dir(v2, "ns-a1.slice").unwrap();
        assert_eq!(
            dir,
            Path::new("user.slice/user-1000.slice/user@1000.service/ns.slice/ns-a1.slice")
        );
        assert_eq!(file, "memory.peak");
        assert_eq!(slice_dir(v2, "ns-b2.slice"), None);
        let v1 =
            "12:cpu,cpuacct:/x\n4:memory:/ns.slice/ns-a1.slice/run-r1.scope\n1:name=systemd:/y\n";
        let (dir, file) = slice_dir(v1, "ns-a1.slice").unwrap();
        assert_eq!(dir, Path::new("memory/ns.slice/ns-a1.slice"));
        assert_eq!(file, "memory.max_usage_in_bytes");
        assert_eq!(slice_dir("4:cpu:/ns-a1.slice\n", "ns-a1.slice"), None);
        assert_eq!(slice_dir("garbage", "ns-a1.slice"), None);

        let root = tempfile::tempdir().unwrap();
        let at = root.path().join("ns.slice/ns-a1.slice");
        fs::create_dir_all(&at).unwrap();
        let cg = "0::/ns.slice/ns-a1.slice/run-r1.scope\n";
        assert_eq!(peak_in(root.path(), cg, "ns-a1.slice"), None);
        fs::write(at.join("memory.peak"), "1048577\n").unwrap();
        assert_eq!(peak_in(root.path(), cg, "ns-a1.slice"), Some(2));
        fs::write(at.join("memory.peak"), "1048576\n").unwrap();
        assert_eq!(peak_in(root.path(), cg, "ns-a1.slice"), Some(1));
        fs::write(at.join("memory.peak"), "max\n").unwrap();
        assert_eq!(peak_in(root.path(), cg, "ns-a1.slice"), None);
        let v1_at = root.path().join("memory/ns.slice/ns-a1.slice");
        fs::create_dir_all(&v1_at).unwrap();
        fs::write(v1_at.join("memory.max_usage_in_bytes"), "3145728").unwrap();
        let cg = "4:memory:/ns.slice/ns-a1.slice/run-r1.scope\n";
        assert_eq!(peak_in(root.path(), cg, "ns-a1.slice"), Some(3));
    }
}
