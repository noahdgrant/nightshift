# Rollout

Read this when the change reaches devices or production: an OTA update, flashing units, a deploy, or a data migration. `sf-ship` writes the plan into the PR's Rollback section. A human runs the rollout. Releasing, deploying, and flashing production units always wait for a human.

Every release is reversible, observable, and incremental, or the PR says why it can't be.

## The plan

Fill each part, and keep it short.

```markdown
## Rollback

**Rollout:** <stages, cohort sizes, and soak time per stage>
**Advance when:** <metrics and thresholds that must hold at each stage>
**Roll back when:** <trigger conditions>
**How to roll back:** <exact steps, and how long they take>
**Not undone by rollback:** <migrations, persisted formats, flash layout, anything one-way>
**Recovery:** <for firmware, the path for a unit that won't boot>
```

## Stages

Grow exposure in steps. Watch each stage for its soak time before advancing.

| Stage | Service | Firmware |
|---|---|---|
| 1. Pre-production | staging deploy, full suite, smoke test | bench units across the hardware matrix, HIL suite |
| 2. Internal | flag on for the team | internal or dogfood fleet |
| 3. Canary | about 1 to 5% of users | about 1 to 5% of the fleet, spread across hardware revisions |
| 4. Gradual | 25%, then 50% | 25%, then 50% |
| 5. Full | 100%, then remove the flag | 100%, then retire the old image from the update server |

The percentages are starting points. Size the canary so a failure shows up in the metrics within the soak time, and so the worst case is a number of units you can recover. Firmware soak times run longer than service ones, because devices check in on their own schedule and some faults show only after a power cycle or days of uptime.

## Advance, hold, or roll back

Compare the cohort to a baseline at the same stage.

| Signal | Advance | Hold and investigate | Roll back |
|---|---|---|---|
| Error or crash rate | within 10% of baseline | 10 to 100% above | over 2x baseline |
| Latency (p95) | within 20% | 20 to 50% above | over 50% above |
| Firmware: failed update or failed boot | none above baseline | any unexplained | rising, or any unit not recovering |
| Firmware: watchdog or brownout resets | within baseline | new reset reasons | over 2x baseline |
| Firmware: devices checking in after update | at baseline | lagging | dropping |
| Firmware: power draw, battery life | within budget | near budget | over budget |
| Data integrity, security issue | none | | any. Roll back now |

## Firmware specifics

### Hardware revision matrix

List every hardware revision in the field, each bootloader version it ships with, and each firmware version a unit may upgrade from. Test the update on one unit per row, or say which rows are untested and why. A revision with no bench unit is a risk to state in Blast radius.

```markdown
| Board rev | Bootloader | Upgrades from | Tested on bench |
|---|---|---|---|
| B2 | 1.4 | 3.1, 3.2 | yes, 3.2 only |
| C1 | 2.0 | 3.2 | yes |
```

### Bootloader and rollback compatibility

- **Image acceptance**: the new image is signed with a key every field bootloader trusts, and fits the slot on every revision.
- **Anti-rollback**: if the image bumps a security or anti-rollback counter, the old image can never boot again. That makes the release a one-way door. Say so.
- **A/B or swap-and-confirm**: the new image confirms itself only after a health check passes (it boots, connects, and passes a self-test). An unconfirmed image reverts on the next reset.
- **Persisted data**: config, calibration, or NVM schema changes migrate forward, and the old image still reads what the new one wrote. If it can't, the rollback is one-way for those units.
- **Bootloader and partition changes**: updating the bootloader or the flash layout in the field is a one-way door with brick risk. It needs its own staged plan and explicit human sign-off.

### Recovery for bricked units

Name the path for a unit that won't boot, from cheapest to most expensive:

1. Automatic revert to the previous slot on failed confirm or watchdog reset.
2. A recovery or DFU mode reachable without a working app image (a button combination, a boot pin, a recovery partition).
3. Reflash over serial, USB, or the debug port by a technician.
4. RMA or field replacement, with the expected count from the canary failure rate.

If a revision has no path beyond step 3, the canary for that revision should be units you can reach by hand.

### Production flashing

For a factory line, roll out to one station or one batch first, keep the previous image ready to load, and verify the first units off the line with the bench suite before the rest of the batch runs.

## Services

Ship behind a feature flag when you can, so turning it off is the rollback (under a minute). Otherwise the rollback is redeploying the previous version. A database migration needs its own tested down-step, or the PR states it is one-way. After each stage, check the health endpoint, error monitoring, latency, and one critical flow by hand.
