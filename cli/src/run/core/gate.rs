//! Gate decisions over plain data: which gate a phase triggers. Phase 2 of #562 moves the rest
//! here: reusing a verdict, `moved_since`, and the culprit over the changed files the shell
//! gathers.

use crate::factory::LocalGate;
use crate::gate::Trigger;

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

#[cfg(test)]
mod tests {
    use super::*;

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
