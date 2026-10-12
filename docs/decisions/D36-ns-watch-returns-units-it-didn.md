---
id: D36
title: "ns watch returns units it didn't finish to the queue"
issue: 13
date: 2026-10-10
amended_by: [D43]
---

# D36: ns watch returns units it didn't finish to the queue

## Decision

`ns watch` returns units it didn't finish to the queue. At start it moves every open `in_progress_label` issue that no live `ns run` holds back to `ready_label`, keeping the worktree and artifacts so the unit resumes. On SIGINT or SIGTERM it starts nothing new, sends SIGTERM to the running phase's process group and SIGKILL 5 s later, sets aside what the phase wrote, returns the issue to `ready_label`, prints its summary and exits 128 + the signal. Liveness is an OS `flock`: the run lock (`ns-run.lock`) moves from a pid file to a `flock` that records pid, unit and issue, and a new watch lock (`ns-watch.lock`) allows one `ns watch` per repo. The signal handler only stores the signal in an atomic; phase waits, sleeps and loops poll it.

## Reason

A crash, SIGKILL or a closed terminal left the running unit `status:in-progress` forever, and on 2026-10-10 #13 had to be reset by hand (#209). A pid file can name a pid reused after a reboot, so a dead run could look live and keep its issue out of the queue; the kernel drops a `flock` with its holder, so it can't. Without the watch lock, a second `ns watch` would requeue the issue a first one keeps in progress while it sleeps through a usage limit. The phase runs in its own session (so a `kill 0` inside it can't reach `ns`), so a terminal Ctrl-C never reached it; `ns watch` must end it, by process group as `ns-contract` requires. Waiting for a phase to finish could take its whole timeout, an hour or more, which is not what Ctrl-C asks for. The handler is `sigaction` through `libc`, already a dependency: storing one atomic needs no `signal-hook`, and polling, which `run_process` already does every 50 ms, keeps signal-unsafe work out of the handler. Under a stop, only `merged`, `split` or a clean `done` keeps its outcome, since Ctrl-C also kills `ns`'s own `gh` calls and can turn a unit `stuck`. A cut-short phase or gate sets its artifact aside, so the phase runs again: the gate's state lives in memory and a resumed run would skip it. The queue no longer skips an issue whose open PR comes from its own unit branch, so a unit stopped in the merge step resumes there. Each handler is one-shot, so a second Ctrl-C still ends a hung `ns watch` at once. Known limits: `in_progress_label` is `ns watch`'s own, so an issue a person or another machine marks in progress is returned too; SIGHUP is not handled, so a closed terminal is caught by the next start, not at once; a run from an `ns` older than this lock holds no `flock`, so stop old runs before upgrading. Spec: `docs/FACTORY.md` ("Units a night didn't finish").
