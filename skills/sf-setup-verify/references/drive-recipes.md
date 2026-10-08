# Drive recipes

Generic ways to drive a surface when the repo has no harness of its own. Pick the recipe for the primary surface, then wrap each step as a control-CLI verb so agents call `control-<project> <noun> <verb>` instead of re-typing the recipe. Read real commands, ports, chips and baud rates from `docs/agents/stack.md` and the repo, never from these examples.

Contents: [CLI or TUI](#cli-or-tui) · [HTTP service](#http-service) · [Firmware bench](#firmware-bench) · [Emulator](#emulator)

## CLI or TUI

A one-shot CLI needs only `subprocess.run` with captured stdout, stderr and exit code (the template's `exec`). An interactive CLI or TUI needs a PTY, because many programs change behaviour when stdout is not a terminal.

Loop: start in an isolated session with deterministic env (`TERM`, `COLUMNS`, `LINES`, `NO_COLOR`, a temp `HOME`), capture the screen, send one action, wait for a concrete pattern, repeat, save the transcript, kill the session.

PTY, for deterministic waits with no extra tools:

```python
import os, pty, re, select, subprocess, time

def spawn(argv, env=None):
    master, slave = pty.openpty()
    proc = subprocess.Popen(argv, stdin=slave, stdout=slave, stderr=slave,
                            env=env, close_fds=True, start_new_session=True)
    os.close(slave)
    return proc, master

def expect(master, pattern, timeout=10.0, buf=b""):
    deadline = time.monotonic() + timeout
    rx = re.compile(pattern.encode())
    while time.monotonic() < deadline:
        ready, _, _ = select.select([master], [], [], 0.1)
        if ready:
            buf += os.read(master, 4096)
            if rx.search(buf):
                return buf
    raise TimeoutError(f"{pattern!r} not seen; last output: {buf[-300:]!r}")

proc, fd = spawn(["myapp", "shell"])
transcript = expect(fd, r"> $")
os.write(fd, b"status\n")
transcript = expect(fd, r"state: idle", buf=transcript)
proc.terminate(); os.close(fd)
```

tmux, when a human may want to attach, or for resize and key-chord tests:

```bash
S="verify-$RUN_ID"
tmux new-session -d -s "$S" -x 120 -y 40 -- myapp shell
tmux capture-pane -pt "$S"            # read the screen
tmux send-keys -t "$S" "status" Enter
tmux capture-pane -pt "$S" -S - > "$OUT/status.txt"
tmux kill-session -t "$S"             # cleanup kills this session only
```

Control-CLI verbs: `tui start`, `tui send <text>`, `tui keys <key>`, `tui expect <regex> --timeout S`, `tui screen --path F`. Record the session name in the state dir so `cleanup` kills only that.

Gotchas: wait for a pattern, not a sleep; if you must sleep, say why in a comment. The PTY echoes what you send, so a pattern that appears in your own input matches the echo; match on output only the program prints. Strip ANSI escapes before asserting. Never send credentials into a controlled session.

## HTTP service

Launch with a disposable data dir and a free port, wait for readiness, then drive with plain requests.

```python
import json, time, urllib.error, urllib.request

def wait_ready(url, timeout=30.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=2) as r:
                if r.status == 200:
                    return
        except (urllib.error.URLError, ConnectionError):
            time.sleep(0.2)
    raise TimeoutError(f"{url} not ready after {timeout}s")

def call(method, url, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=10) as r:
            return r.status, json.loads(r.read() or b"null")
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode(errors="replace")
```

Control-CLI verbs: `launch` (port and data dir in the state dir), `http get|post <path> [--json F]`, `logs tail --lines N`, `db query <sql>` if a read-back view of stored state exists.

Evidence: request, status, response body, the relevant log lines, and a second read-back view of any write (a `GET` after the `POST`, or a DB query). Doctor checks the port answers, the build revision endpoint matches `git rev-parse HEAD`, and the process on the port is the one in our pidfile.

## Firmware bench

The control CLI wraps the bench so one command does each step. Read the toolchain, flash tool, chip name, probe, serial port and baud from `stack.md`. The bench is usually a single shared resource: the generated skill says so, and `doctor` refuses to run when another run holds the lock (a lockfile in the state dir).

| Verb | What it does | Typical tools |
|---|---|---|
| `build [--profile release]` | Run the build from `stack.md`. Print the image path, size, sha256, and embedded version string. | `make`, `cmake --build`, `west build`, `cargo build` |
| `flash --image F [--dry-run]` | Program and verify the image, then reset. Idempotent: skip if the board already reports the same version and sha. | `probe-rs download --chip <chip> F`, `openocd -f <cfg> -c "program F verify reset exit"`, vendor tools (`nrfjprog`, `STM32_Programmer_CLI`, `esptool.py`) |
| `reset [--halt]` | Hardware reset through the probe, or a reset GPIO. | `probe-rs reset --chip <chip>`, `openocd ... -c "reset run"` |
| `serial send <text>` / `serial expect <regex>` | Talk to the device's shell or log port. | pyserial |
| `capture la --seconds S --channels ...` | Logic analyzer capture to a file in the evidence dir. | `sigrok-cli -d <driver> --time Sms -C D0,D1 -o F.sr`, vendor CLIs |
| `capture power --seconds S` | Current and energy over a window. Report mean, peak and energy, with the raw trace path. | PPK2, Joulescope, or a bench meter over SCPI |
| `hil run <case>` | Run one hardware-in-the-loop case: flash if needed, reset, drive stimuli, assert on serial or capture, save everything. | pytest with fixtures that call the verbs above |

Serial send/expect with pyserial:

```python
import re, time, serial  # pyserial

def serial_expect(port, baud, send, pattern, timeout=5.0, log_path=None):
    rx = re.compile(pattern.encode())
    with serial.Serial(port, baud, timeout=0.1) as s:
        s.reset_input_buffer()
        if send:
            s.write(send.encode() + b"\r\n")
        buf, deadline = b"", time.monotonic() + timeout
        while time.monotonic() < deadline:
            buf += s.read(s.in_waiting or 1)
            if rx.search(buf):
                break
        else:
            raise TimeoutError(f"{pattern!r} not seen on {port}; tail: {buf[-200:]!r}")
    if log_path:
        open(log_path, "ab").write(buf)
    return buf.decode(errors="replace")

serial_expect("/dev/ttyACM0", 115200, "version", r"fw v\d+\.\d+\.\d+")
```

HIL case runner: one pytest file per feature under the repo's HIL dir, marked so host runs skip it (`@pytest.mark.hil`). Fixtures own the bench lock, flash once per session, reset per test, and write the serial log and captures under `--out`. `hil run <case>` is a thin wrapper over `pytest -m hil -k <case> --junitxml <out>/junit.xml` that prints the result as JSON.

Doctor for a bench, each check with a fix:

- Probe attached: `probe-rs list` (or the vendor's list command) shows the expected probe serial.
- Board responds: the probe reads the chip ID, and `serial expect` gets the shell prompt after a reset.
- Firmware version matches: the version string on serial equals the one embedded in the image `build` just produced (typically `git describe --dirty`).
- Serial port is free: no other process holds it (`fuser /dev/ttyACM0` on Linux).
- Capture tools present if the feature map needs them.

A firmware feature file has the same four H2s. Its drive bullets pair a user action with a bench command and an observable result:

```markdown
- **Set sample rate.** Send the shell command. Run `control-sensor serial send "rate 100"` and
  `control-sensor serial expect "rate=100Hz"`. The shell confirms the new rate.
- **Prove it on the wire.** Run `control-sensor capture la --seconds 1 --channels D0 --save rate-100`.
  The capture shows 100 +/- 1 SPI bursts on D0 in one second.
```

Gotchas: USB serial ports re-enumerate after reset; wait for the device node, then for the banner. Logs during boot can be lost if you open the port after reset; open first, then reset. A debug build changes timing; capture timing-sensitive evidence on the build `stack.md` says ships. Flashing production units is a human-only action; the control CLI targets bench boards only, and says so in `flash --help`.

## Emulator

Use an emulator when no board is attached, or to run many cases in parallel. Its evidence is weaker than hardware for anything timing, peripheral or power related. The generated skill says which features the emulator can prove.

QEMU, with the UART on stdio or a PTY and semihosting so the guest can exit with a status:

```bash
qemu-system-arm -M mps2-an385 -cpu cortex-m3 -nographic \
  -kernel build/app.elf -semihosting-config enable=on,target=native \
  -serial mon:stdio
```

Drive it like a CLI: spawn under a PTY (see above) and `expect` on the UART output, or use `-serial pty` and point `serial expect` at the printed `/dev/pts/N`. Add `-S -gdb tcp::<port>` to attach a debugger.

Renode, for boards QEMU doesn't model and for scripted multi-node setups:

```bash
renode --disable-xwt --console -e "include @platforms/boards/<board>.resc; \
  sysbus LoadELF @build/app.elf; start"
renode-test tests/renode/<feature>.robot      # Robot Framework cases, results as XML
```

Control-CLI verbs: `emu start [--machine M]`, `emu serial expect <regex>`, `emu stop`, `emu run <case>`. Keep the same verb shape as the bench (`serial expect`, `hil run`) so feature files can say "on bench or emulator" with one command. Doctor checks the emulator binary and version, and that the machine or platform file exists.
