# Firmware test seams

The seam ladder for code that touches hardware. Each rung catches defects the one below misses, and costs more per run. Pick the lowest rung that can go red on the behavior under test, and agree it with the user like any other seam.

Take every command from `docs/agents/stack.md`, which lists which rungs this repo has and how to run each. A rung missing from `stack.md` doesn't exist yet. Say so in the phase artifact instead of improvising one.

| Seam | Catches | Misses | Cost per run |
|---|---|---|---|
| **Host unit, behind a HAL** | logic, state machines, parsers, protocol encoding, edge cases in arithmetic | real timing, interrupt races, peripheral quirks, compiler and target differences (int width, endianness, alignment) | milliseconds, runs on every change |
| **Simulator / emulator** (QEMU, Renode, vendor sim) | the real compiled image, boot, linker layout, interrupt dispatch, peripheral models | anything the model gets wrong or leaves out, analog behavior, real clock drift | seconds, runs in CI |
| **On-target** (tests run on the board, results over serial or a debug probe) | the real core, memory map, flash/RAM budget, real peripherals | the outside world: the sensor, the other end of the bus, power events | tens of seconds plus a flash cycle, needs a board |
| **Hardware-in-the-loop** (board wired to a rig that drives inputs and captures outputs) | end-to-end behavior against real signals, timing, power, recovery | little, but failures are slow to localize | minutes, needs the bench |

## Tradeoffs

- **Default to host unit tests** for the TDD loop. The loop has to be tight, and a test that takes a flash cycle won't get run after every change.
- **Push logic out of the hardware layer** so it can be tested on the host. A driver that parses a frame and also pokes registers has no host seam. Split it into a pure parser and a thin HAL call.
- **Move up a rung only for the defect class that needs it.** An ISR race needs the target or HIL. A checksum bug never does.
- **A host test passing is not evidence the firmware works.** It proves the logic. Claims about timing, peripherals or the board need a target or HIL run, recorded as evidence by `sf-verify`.
- **When the agent can't reach the rung** (no board, no bench), write the test anyway, mark it as needing that rung, and report the run as `blocked` or `inconclusive`, never as a pass.
