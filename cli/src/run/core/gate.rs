//! Gate decisions over plain data: which gate a phase triggers, and which phase run a red gate
//! blames, over the changed files the shell gathers.

use std::path::Path;

use crate::factory::LocalGate;
use crate::gate::{same_file, Trigger};

/// The gate to run after `phase`, if any. `artifact` is the status of the artifact the phase
/// wrote, `None` when it wrote none. Under `build-only` none runs after ship: the PR's GitHub
/// checks decide in the merge step.
pub fn gate_trigger(
    phase: &str,
    artifact: Option<&str>,
    head_moved: bool,
    local: LocalGate,
) -> Option<Trigger> {
    match (phase, artifact) {
        ("build", Some("pass")) => Some(Trigger::BuildPassed),
        ("build", None) => Some(Trigger::BuildWroteNothing),
        ("build", _) => None,
        ("ship", _) if local == LocalGate::BuildOnly => None,
        _ if head_moved => Some(Trigger::PhaseMovedHead),
        _ => None,
    }
}

/// A phase run a red gate may be charged to: its index in `Ledger::suspects`, its phase, and the
/// files its move changed, `None` when they could not be read. Ship's move may merge the base in:
/// for ship, only the files whose own lines it changed (the interdiff) count.
#[derive(Debug, Clone, PartialEq)]
pub struct Suspect {
    pub index: usize,
    pub phase: String,
    pub changed: Option<Vec<String>>,
}

/// The phase run a red gate's `failing` paths point at, with the paths it matched: the first of
/// `suspects` (newest first) that changed a file the output names. `None` when no phase changed
/// one (the base, or the environment). `root` is the worktree a tool printed absolute paths in,
/// and `tracked` the files git tracks there, which make a short relative path ambiguous.
pub fn culprit(
    failing: &[String],
    suspects: &[Suspect],
    root: &Path,
    tracked: &[String],
) -> Option<(usize, String, Vec<String>)> {
    suspects.iter().find_map(|s| {
        let changed = s.changed.as_ref()?;
        let hit: Vec<String> = failing
            .iter()
            .filter(|f| changed.iter().any(|c| same_file(c, f, root, tracked)))
            .cloned()
            .collect();
        (!hit.is_empty()).then(|| (s.index, s.phase.clone(), hit))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suspect(index: usize, phase: &str, changed: Option<&[&str]>) -> Suspect {
        Suspect {
            index,
            phase: phase.into(),
            changed: changed.map(|c| c.iter().map(|s| s.to_string()).collect()),
        }
    }

    /// The newest suspect that changed a failing file is blamed; one whose files can't be read
    /// is passed over, and a path two crates share names neither.
    #[test]
    fn a_red_gate_blames_the_newest_phase_that_changed_a_failing_file() {
        let root = Path::new("/w");
        let failing = vec!["src/a.rs".to_string(), "b.rs".to_string()];
        let suspects = [
            suspect(2, "review", None),
            suspect(1, "verify", Some(&["docs/x.md"])),
            suspect(0, "build", Some(&["src/a.rs", "c.rs"])),
        ];
        assert_eq!(
            culprit(&failing, &suspects, root, &[]),
            Some((0, "build".into(), vec!["src/a.rs".into()]))
        );
        assert_eq!(culprit(&failing, &suspects[..2], root, &[]), None);
        let tracked = ["a/src/main.rs".to_string(), "b/src/main.rs".to_string()];
        let both = [suspect(0, "build", Some(&["a/src/main.rs"]))];
        let short = ["src/main.rs".to_string()];
        assert_eq!(culprit(&short, &both, root, &tracked), None);
        let full = ["a/src/main.rs".to_string()];
        assert_eq!(
            culprit(&full, &both, root, &tracked).map(|c| c.1),
            Some("build".into())
        );
    }

    /// Build triggers the gate on a pass or no artifact, and never otherwise; a later phase
    /// triggers it by moving HEAD, except ship under `build-only`.
    #[test]
    fn gate_trigger_maps_each_phase_outcome() {
        use Trigger::*;
        let (pass, fail) = (Some("pass"), Some("fail"));
        let every = LocalGate::EveryStep;
        assert_eq!(gate_trigger("build", pass, false, every), Some(BuildPassed));
        assert_eq!(gate_trigger("build", fail, false, every), None);
        assert_eq!(gate_trigger("build", fail, true, every), None);
        assert_eq!(
            gate_trigger("build", None, false, every),
            Some(BuildWroteNothing)
        );
        assert_eq!(
            gate_trigger("verify", pass, true, every),
            Some(PhaseMovedHead)
        );
        assert_eq!(gate_trigger("verify", pass, false, every), None);
        assert_eq!(
            gate_trigger("ship", pass, true, every),
            Some(PhaseMovedHead)
        );
        assert_eq!(gate_trigger("ship", pass, true, LocalGate::BuildOnly), None);
    }
}
