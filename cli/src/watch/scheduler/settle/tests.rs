use super::*;

#[test]
fn a_unit_the_hold_paused_resumes_at_once_once_the_reset_came() {
    let night = Night::new();
    let now = night.shared.clock.now();
    let paused = |reset: i64, held: bool| {
        RunResult::from_json(serde_json::json!({
            "unit": "2-a", "outcome": "paused", "reset_at": reset, "held": held,
        }))
        .unwrap()
    };
    // The hold's reset came while the unit ended: no new 30-minute wait.
    assert_eq!(resume_at(&paused(now - 1, true), &night), now);
    assert_eq!(resume_at(&paused(now + 60, true), &night), now + 60);
    // A unit's own limit whose reset has passed, the same reset or not, waits as before:
    // the provider is still refusing it.
    for reset in [now - 1, now - 5] {
        let own = paused(reset, false);
        assert_eq!(resume_at(&own, &night), night.resume_at(&own));
        assert!(resume_at(&own, &night) > now);
    }
}
