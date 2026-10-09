# Firmware feedback loops

Loop recipes for firmware bugs, fastest first. Each one ends in **one command** that exits non-zero when the bug shows, so it meets Phase 1's completion criterion. Read real boards, ports, chips, baud rates and paths from `docs/agents/stack.md` and the repo, never from these examples.

Prefer the highest loop on the list that still reaches the bug. A host loop runs in seconds and needs no bench; a HIL loop takes minutes and holds a shared resource.

Contents: [ztest on native_sim](#twister-and-ztest-on-native_sim) · [Emulator](#emulator) · [Serial capture](#serial-capture) · [HIL case runner](#hil-case-runner) · [Logic analyzer](#logic-analyzer-capture) · [git bisect on target](#git-bisect-run-on-target)

## Twister and ztest on native_sim

Use when the bug lives in logic that doesn't touch real peripherals: protocol parsing, state machines, ring buffers, timing math. `native_sim` builds the application as a Linux executable, so a ztest suite runs on the host.

Loop: write a ztest case that drives the bug path and asserts the reported symptom, then run only that suite.

```bash
west twister -p native_sim -T tests/uart_rx -s tests/uart_rx/uart_rx.default \
  --inline-logs -v --outdir "$OUT/twister"
```

Red: twister exits non-zero and the inline log shows the failed `zassert`. For a faster inner loop, build once and run the executable directly: `west build -b native_sim tests/uart_rx -d "$OUT/build" && "$OUT/build/zephyr/zephyr.exe"`.

Tighten: native_sim time is simulated, so timing bugs reproduce deterministically. Fake a peripheral with Zephyr's emulators (`CONFIG_EMUL`, the `zephyr,uart-emul` and `zephyr,i2c-emul-controller` nodes) or an FFF fake for a HAL call. Seed any RNG through Kconfig. A bug that only shows under real interrupt timing won't reproduce here: that's a finding, so move down the list.

## Emulator

Use when the bug needs the target's instruction set, memory map or a peripheral model that native_sim lacks: an alignment fault, a stack overflow, an MPU fault, a startup-order bug.

Loop: boot the image in QEMU or Renode, feed it the stimulus, and assert on the console.

```bash
west build -b qemu_cortex_m3 app -d "$OUT/build"
timeout 30 west build -d "$OUT/build" -t run 2>&1 | tee "$OUT/console.log" \
  | grep -q -E 'FATAL|Fault|ASSERTION FAIL' && exit 1   # red: the fault line appeared
exit 0
```

Renode scripts the same loop with a robot test (`renode-test tests/rx_overflow.robot`), drives UART, GPIO and sensor models, and exits non-zero on a failed `Wait For Line On Uart`. It can also run a board model for the exact target, which QEMU often can't.

Tighten: replace the grep on the console stream with the emulator's own expect step (Renode's `Wait For Line On Uart <pattern> timeout=5`) so the verdict is deterministic. Renode's virtual time removes host-load jitter.

## Serial capture

Use when the bug shows on real hardware and the device logs to a UART or RTT. The loop resets the board, captures the log, and asserts on the symptom line.

```python
import re, sys, serial  # pyserial

PORT, BAUD, PATTERN, TIMEOUT = "/dev/ttyACM0", 115200, rb"rx overrun", 10.0

with serial.Serial(PORT, BAUD, timeout=0.1) as s, open(sys.argv[1], "wb") as log:
    s.reset_input_buffer()
    # reset the board here (probe-rs reset, or a reset GPIO) so every run starts from boot
    buf = b""
    for _ in range(int(TIMEOUT / 0.1)):
        chunk = s.read(s.in_waiting or 1)
        log.write(chunk)
        buf += chunk
        if re.search(PATTERN, buf):
            sys.exit(1)  # red: the reported symptom appeared
sys.exit(0)
```

Red: exit 1 and the symptom line in the saved log. Assert on the reporter's exact line, never on "no output".

Tighten: start every run from reset so state doesn't carry over. Drive the stimulus from the same script (send the command that triggers the bug) instead of waiting for it. For a rare symptom, loop the reset-stimulus-capture cycle 100 times and report the rate. The `ns-setup-verify` skill's [drive recipes](../../ns-setup-verify/references/drive-recipes.md#firmware-bench) give the full send/expect helper; reuse the project's control CLI (`control-<project> serial expect`) when it exists.

## HIL case runner

Use when the bug needs a stimulus from the bench (a GPIO edge, a sensor value, a second device on the bus) as well as the device's response. Write the loop as one HIL case so it runs unattended.

```python
def test_rx_overrun_at_921600(bench):
    bench.flash_if_needed()
    bench.reset()
    bench.peer_uart.baud = 921600
    bench.peer_uart.send(b"x" * 4096)
    assert not bench.serial.seen(rb"rx overrun", timeout=5)
```

```bash
python3 -m pytest -q hil/test_rx_overrun.py --count 20   # pytest-repeat raises the rate for a flaky case
```

Red: the case fails with the symptom in the assertion message. Save the serial log and any capture next to the result.

Tighten: the bench is a shared resource, so take its lock (the project's runner, or the control CLI's lockfile) for the whole loop. Pin firmware version, peer configuration and power supply between runs. A HIL loop is slow; once it's red, look for a host or emulator loop that reproduces the same root cause, and hand that seam to `ns-build`.

## Logic-analyzer capture

Use when the symptom is on the wire: a glitch, a wrong bit order, a timing violation, a missing ACK. The loop captures a fixed window and a decoder asserts on it.

```bash
sigrok-cli -d fx2lafw --config samplerate=24m --time 200ms -C D0,D1 \
  --triggers D0=f -o "$OUT/cap.sr"
sigrok-cli -i "$OUT/cap.sr" -P i2c:scl=D0:sda=D1 -A i2c=address-write:data-write \
  > "$OUT/decoded.txt"
python3 check_i2c.py "$OUT/decoded.txt"   # exits 1 when the decoded frames show the bug
```

Red: the checker exits 1 and names the frame that is wrong (a NACK at address 0x48, a 3 µs SCL high time against a 4 µs minimum).

Tighten: trigger on the event that precedes the bug, so every capture holds it. Assert on decoded frames or measured timings, never on eyeballing a waveform. Save the `.sr` file: a captured trace replays through the decoder with no bench, which turns this into a host loop (Phase 1, "Replay a captured trace").

## git bisect run on target

Use when the bug appeared between two known commits and one of the loops above can tell good from bad. Wrap that loop in a script that builds, flashes, runs, and exits with git bisect's codes.

```bash
#!/usr/bin/env bash
# bisect-check.sh: exit 0 good, 1 bad, 125 skip (can't build this commit)
west build -b <board> app -d "$OUT/build" --pristine >/dev/null 2>&1 || exit 125
west flash -d "$OUT/build" >/dev/null 2>&1 || exit 125
python3 serial_check.py "$OUT/serial-$(git rev-parse --short HEAD).log"
```

```bash
git bisect start <bad-sha> <good-sha>
git bisect run ./bisect-check.sh
git bisect log > "$OUT/bisect.log"; git bisect reset
```

Red: `git bisect run` names the first bad commit. That commit is evidence for a hypothesis, not the root cause: read what it changed, then form hypotheses in Phase 3.

Tighten: exit 125 for every reason other than the bug (a build break, a flash failure), so bisect skips instead of blaming the wrong commit. For a flaky bug, run the check several times per commit and call it bad on any failure. Keep the script outside the tree (under `.ns/<unit-id>/troubleshoot/`) so checkouts during the bisect don't remove it. Bisect submodules or the west manifest together with the app when the bug might be in a module.
