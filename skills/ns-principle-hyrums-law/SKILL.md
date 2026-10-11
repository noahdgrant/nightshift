---
name: ns-principle-hyrums-law
description: "Apply when a change touches anything another program, script, person or device can observe: output format, field names, ordering, timing, error text, exit codes, wire or register layouts, persisted formats. Treat every observable behaviour as contract, and change it only deliberately."
disable-model-invocation: true
metadata:
  upstream: addyosmani/agent-skills@1401c8b8030e:skills/api-and-interface-design
---

# Hyrum's Law

With enough users, every observable behaviour of an interface is depended on by somebody, whatever the docs promise. Treat what a caller can observe as contract, and change it only on purpose.

**Why:** A field you "just moved", an error message you reworded or a list you re-sorted breaks a script you never saw. No test in the repo catches it, because the consumer lives outside the repo.

**Pattern:**
- **List what is observable** before changing an interface: output format and field names, ordering, timing, error text, exit codes, defaults, wire and register layouts, persisted formats. Each one is contract unless it is documented as unstable.
- **Add, don't move or rename.** A new field goes next to the old ones. A new shape keeps the old fields beside it until a deliberate break.
- **Break only deliberately.** A break the brief asks for, or one that can't be avoided, gets a version (a `version` field, a new flag, a new endpoint) and a line in the artifact saying who it breaks. In `build.md`, a kept field the brief didn't mention goes under Deviations from the brief.
- **Leak less.** Don't expose what callers shouldn't rely on (internal IDs, hash ordering, debug text). Once it is observable, it is contract.
- **Pin it in a test.** A test that asserts the exact output turns an accidental break into a red build.

**Python:** the brief groups `status --json`'s counts into a `stock` object. Add `stock` and keep `on_hand`, `reserved` and `available` at the top level, because a cron job may read `.available`. Record the kept fields in `build.md`.

**Firmware:** treat protocol frames, persisted flash formats and timing other nodes rely on as contract. A CAN frame keeps its ID, byte order and period; a new signal goes in spare bytes or a new frame. A settings struct in flash gets a version byte and a migration on boot before a field moves. A heartbeat another node times out on keeps its 100 ms period, even when sending it less often would save power.
