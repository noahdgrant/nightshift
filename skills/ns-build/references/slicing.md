# Slicing strategies

Adapted from addyosmani `incremental-implementation`. Each slice leaves the system building and its tests passing.

## Vertical slices (default)

One complete path through the stack per slice, each one observable at a seam:

```
Slice 1: create a task (storage + service + CLI command)  → user can create a task
Slice 2: list tasks (query + service + CLI)               → user can see their tasks
Slice 3: edit a task                                      → user can modify tasks
```

Firmware: `Slice 1: parse one frame type and act on it` before `Slice 2: the next frame type`, not "all parsing" then "all actions".

## Risk-first

Put the most uncertain piece first, so a dead end shows up before anything is built on it:

```
Slice 1: prove the DMA transfer completes at the target baud (highest risk)
Slice 2: frame decoding on top of the proven transfer
Slice 3: timeout and recovery
```

## Contract-first

When two sides must meet at an interface (driver and HAL, service and client):

```
Slice 0: define the interface (a typing.Protocol, a header, a message schema)
Slice 1a: implement one side against it, with tests
Slice 1b: implement the other side against a fake of it
Slice 2: integrate and test end to end
```

## Rules that keep slices safe

- **Keep it building.** After each slice the build succeeds and existing tests pass.
- **Safe defaults.** New behavior is off or conservative unless the brief says otherwise: `notify: bool = False`.
- **Flags for unfinished features.** If a slice must merge before the feature is complete, hide it behind a flag the brief or `stack.md` names. Firmware: a build-time flag, so dead code doesn't cost flash in production images.
- **Rollback-friendly.** Additive changes revert cleanly. Keep removals and their replacements in separate commits. Migrations get a down path.
- **Re-run only after change.** Re-running a green command on unchanged code adds no information.
