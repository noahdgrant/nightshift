---
id: ADR-686
title: "ns watch waits on low disk space and stops the night when it runs out"
issue: 686
date: 2026-10-15
---

# ADR-686: ns watch waits on low disk space and stops the night when it runs out

## Decision

Before a fill starts a unit, `ns watch` reads the free space on the filesystem holding the worktrees and, under WSL (`/proc/version` names a Microsoft kernel), on `/mnt/c`. The one with the least free space decides. Under `[limits] disk_wait_gb` (default 20 GiB) no unit starts and the night waits, checking again at each fill, even with nothing running; it logs `disk_low` once each time space falls under the limit. Under `[limits] disk_stop_gb` (default 10 GiB) it logs `disk_critical` and stops the night as `--until` does, with `stopped: "disk_critical"`. A filesystem whose space can't be read is warned of once a night and holds nothing up. `ns factory validate` rejects a stop limit above the wait limit, and `ns doctor` reports each filesystem's free space against the limits. Spec: `docs/factory/watch.md` ("Disk space").

## Reason

Agreed by the human in #529. A full Windows C: drive crashed WSL twice in one night: every unit worktree builds its own output, and WSL's disk file only grows, so every unit died with SIGBUS and the night lost hours. Waiting first gives finished units' cleanup, or a person, the chance to free space before anything breaks; stopping at the lower limit ends the night cleanly, with a reason in the summary, before the disk fills. An unreadable filesystem, such as a `/mnt/c` that isn't mounted, must not hold up a night that would otherwise run. Freeing space (removing build output, cleaning worktrees) is separate work from #529.
