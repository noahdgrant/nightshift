---
id: D44
title: "An opt-in memory cap, [limits] memory_mb, off by default"
issue: 208
date: 2026-10-10
---

# D44: An opt-in memory cap, [limits] memory_mb, off by default

## Decision

An opt-in memory cap, `[limits] memory_mb`, off by default. Each phase and gate run starts in a systemd user scope (`systemd-run --user --scope -p MemoryMax=<n>M -p MemorySwapMax=0 -p OOMPolicy=stop`) when a probe at start, a scope with the same properties, shows the scope's `memory.max` really is set, else under `prlimit --as`, else `ns run` and `ns watch` exit 2. The mechanism is printed at start and recorded in each unit's `start` event. A run over the cap fails its attempt with "exceeded the <n> MB memory limit". A phase or gate killed by a signal `ns` didn't send fails with "was killed by <SIGNAL>". A phase cut short either way is treated like a timeout: its artifact moves to `history/<artifact>-killed-<n>.md`, and the stuck reason names the cause.

## Reason

On 2026-10-10 a gate's `cargo test` grew to 14 GB and the OOM kill took `ns watch` with it (#208). A cgroup is the real cap: it counts memory in use by the whole process tree, and the kernel enforces it. The probe reads `memory.max` back from inside a scope, because `systemd-run` succeeds even where the memory controller isn't delegated and the cap would be silently absent. `MemorySwapMax=0` keeps a runaway from paging the machine to a crawl instead of stopping at the cap; `OOMPolicy=stop` ends the whole attempt when one process is killed, so a harness never carries on half-dead. Detection asks systemd for the scope's `Result` (`oom-kill`) after the process exits, waiting up to 3 s for the scope to stop, then clears the failed unit; a fresh unique scope name per run makes that lookup unambiguous. The probe uses every property a real run uses because systemd older than 253 rejects `OOMPolicy` on a scope; a rejected property must mean the fallback, not a night of failed phases. RLIMIT_AS is the fallback the issue chose. It is weaker: it applies to each process on its own, so a parallel build or test run can use many times the cap in total, and it caps address space, not memory, and kills nothing, so detection is a heuristic: a failed run whose output (the gate log's tail, the harness's stderr) holds a line with a specific allocation-failure message (whole messages, not words like "out of memory" that a test name can hold). Runtimes that reserve address space need headroom under it; when this was written, `claude --help` started under a 1 GB cap and this repo's `cargo test --no-run` finished under 4 GB, but a real phase was not tried. Before this change a signal-killed phase that left its artifact counted as written, and a gate killed by a signal said only "was killed by a signal"; naming the signal and failing the attempt keeps an OOM kill from reading as an ordinary test failure. `ns` sends signals of its own only on a timeout or a stop, and both keep their existing reasons. Advice to run `ns watch` in its own unit with `OOMPolicy=continue` belongs in the runbook (#7). Spec: `docs/FACTORY.md` ("Memory cap").
