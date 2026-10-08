#!/usr/bin/env python3
"""control-PROJECT: drive PROJECT for verification.

Skeleton copied by the ns-setup-verify skill. Replace every `FILL:` marker,
add the project's verbs (see drive-recipes.md), and delete this paragraph.
Rules: skills/ns-setup-verify/references/cli-for-agents.md in nightshift.

Output contract: one JSON object on stdout per invocation. Diagnostics go to
stderr. Exit 0 on success, 1 when an operation or check fails, 2 on usage error.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

PROJECT = "PROJECT"  # FILL: project name, used in paths and messages.
PROG = f"control-{PROJECT}"

# FILL: the command that starts the thing under test, and the text in its
# output that means it is ready. This default is a stand-in so the skeleton runs.
LAUNCH_CMD = [sys.executable, "-u", "-c", "import time; print('ready'); time.sleep(3600)"]
READY_TEXT = "ready"
READY_TIMEOUT_S = 30

# FILL: tools doctor requires on PATH (e.g. "probe-rs", "sigrok-cli", "tmux").
REQUIRED_TOOLS: list[str] = []

DEFAULT_STATE_DIR = Path(".ns/verify/state")
RUN_ID = time.strftime("%Y%m%dT%H%M%S")
DEFAULT_OUT = Path(f".ns/verify/{RUN_ID}")


class UsageError(Exception):
    def __init__(self, message: str, example: str):
        super().__init__(message)
        self.example = example


class Parser(argparse.ArgumentParser):
    """Usage errors as JSON with a correct example, instead of argparse's text."""

    def error(self, message: str) -> None:  # type: ignore[override]
        emit({"ok": False, "error": message, "usage": self.format_usage().strip(),
              "help": f"{self.prog} --help"}, stream=sys.stderr)
        sys.exit(2)


def emit(obj: dict, stream=sys.stdout) -> None:
    print(json.dumps(obj, indent=None, sort_keys=False), file=stream)


def pidfile(state: Path) -> Path:
    return state / "launch.pid"


def alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def read_pid(state: Path) -> int | None:
    try:
        pid = int(pidfile(state).read_text().strip())
    except (FileNotFoundError, ValueError):
        return None
    return pid if alive(pid) else None


# --- commands -------------------------------------------------------------


def cmd_doctor(args: argparse.Namespace) -> int:
    """Read-only. Answers: is this instance worth driving?"""
    checks = []
    for tool in REQUIRED_TOOLS:
        path = shutil.which(tool)
        checks.append({"name": f"tool:{tool}", "ok": path is not None, "detail": path or "not on PATH",
                       "fix": None if path else f"install {tool} (see docs/agents/stack.md)"})
    pid = read_pid(args.state_dir)
    checks.append({"name": "instance", "ok": pid is not None,
                   "detail": f"pid {pid}" if pid else "no instance started by this CLI",
                   "fix": None if pid else f"{PROG} launch"})
    # FILL: project checks. Service: port answers, build revision matches.
    # Firmware: probe attached, board answers on serial, firmware version == build.
    ok = all(c["ok"] for c in checks)
    emit({"ok": ok, "command": "doctor", "checks": checks})
    return 0 if ok else 1


def cmd_launch(args: argparse.Namespace) -> int:
    state: Path = args.state_dir
    pid = read_pid(state)
    if pid:
        emit({"ok": True, "command": "launch", "status": "already-running", "pid": pid})
        return 0
    log = state / "launch.log"
    if args.dry_run:
        emit({"ok": True, "command": "launch", "dry_run": True, "would_run": LAUNCH_CMD, "log": str(log)})
        return 0
    state.mkdir(parents=True, exist_ok=True)
    with log.open("wb") as fh:
        proc = subprocess.Popen(LAUNCH_CMD, stdout=fh, stderr=subprocess.STDOUT,
                                stdin=subprocess.DEVNULL, start_new_session=True)
    pidfile(state).write_text(str(proc.pid))
    deadline = time.monotonic() + args.timeout
    while time.monotonic() < deadline:
        if READY_TEXT in log.read_text(errors="replace"):
            emit({"ok": True, "command": "launch", "status": "started", "pid": proc.pid, "log": str(log)})
            return 0
        if proc.poll() is not None:
            break
        time.sleep(0.1)
    emit({"ok": False, "command": "launch", "error": f"not ready: {READY_TEXT!r} not seen in {log}",
          "pid": proc.pid, "next": f"{PROG} cleanup && {PROG} launch --timeout {args.timeout * 2}"},
         stream=sys.stderr)
    return 1


def cmd_exec(args: argparse.Namespace) -> int:
    """Run one command against the instance and record it as evidence."""
    argv = args.argv[1:] if args.argv[:1] == ["--"] else args.argv
    if not argv:
        raise UsageError("no command given", f"{PROG} exec --name smoke -- {PROJECT} --version")
    out: Path = args.out
    out.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    proc = subprocess.run(argv, capture_output=True, text=True, timeout=args.timeout, check=False)
    record = out / f"{args.name}.json"
    result = {"argv": argv, "exit_code": proc.returncode, "stdout": proc.stdout, "stderr": proc.stderr,
              "duration_s": round(time.monotonic() - started, 3)}
    record.write_text(json.dumps(result, indent=2))
    emit({"ok": proc.returncode == 0, "command": "exec", "exit_code": proc.returncode,
          "stdout_head": proc.stdout[:500], "evidence": str(record)})
    return 0 if proc.returncode == 0 else 1


# FILL: project verbs. Keep the noun-verb shape and JSON output, e.g.
#   http get <path>, tui send/expect, build, flash --image, serial send/expect,
#   reset, capture la|power --seconds, hil run <case>.


def cmd_cleanup(args: argparse.Namespace) -> int:
    """Stop what this CLI started. Keep the evidence dirs."""
    state: Path = args.state_dir
    pid = read_pid(state)
    plan = {"kill_pid": pid, "remove": str(state) if state.exists() else None}
    if args.dry_run:
        emit({"ok": True, "command": "cleanup", "dry_run": True, "plan": plan})
        return 0
    if pid:
        try:
            os.killpg(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        for _ in range(50):
            if not alive(pid):
                break
            time.sleep(0.1)
        else:
            os.killpg(pid, signal.SIGKILL)
    if state.exists():
        shutil.rmtree(state)
    # FILL: firmware: reset the board to a known state, release probe and serial port.
    status = "cleaned" if (pid or plan["remove"]) else "nothing-to-clean"
    emit({"ok": True, "command": "cleanup", "status": status, "plan": plan})
    return 0


# --- parser ---------------------------------------------------------------


def build_parser() -> argparse.ArgumentParser:
    fmt = argparse.RawDescriptionHelpFormatter
    p = Parser(prog=PROG, formatter_class=fmt,
               description=f"Drive {PROJECT} for verification. JSON on stdout.",
               epilog=f"Examples:\n  {PROG} doctor\n  {PROG} launch\n  {PROG} cleanup --dry-run\n\n"
                      f"Run '{PROG} <command> --help' for one command.")
    p.add_argument("--state-dir", type=Path, default=DEFAULT_STATE_DIR,
                   help=f"Where pidfiles and logs live (default: {DEFAULT_STATE_DIR})")
    sub = p.add_subparsers(dest="command", metavar="<command>", parser_class=Parser)

    s = sub.add_parser("doctor", help="Read-only health check of tools and the instance",
                       formatter_class=fmt, epilog=f"Examples:\n  {PROG} doctor")
    s.set_defaults(func=cmd_doctor)

    s = sub.add_parser("launch", help="Start an instance; no-op if one is running",
                       formatter_class=fmt,
                       epilog=f"Examples:\n  {PROG} launch\n  {PROG} launch --timeout 60\n  {PROG} launch --dry-run")
    s.add_argument("--timeout", type=float, default=READY_TIMEOUT_S, help="Seconds to wait for ready")
    s.add_argument("--dry-run", action="store_true", help="Print the plan, start nothing")
    s.set_defaults(func=cmd_launch)

    s = sub.add_parser("exec", help="Run a command and save it as evidence",
                       formatter_class=fmt,
                       epilog=f"Examples:\n  {PROG} exec --name version -- {PROJECT} --version\n"
                              f"  {PROG} exec --out .ns/142-uart-timeout/evidence --name ac1 -- pytest -q tests/e2e")
    s.add_argument("--name", required=True, help="Evidence file stem, e.g. the criterion id")
    s.add_argument("--out", type=Path, default=DEFAULT_OUT, help=f"Evidence dir (default: {DEFAULT_OUT})")
    s.add_argument("--timeout", type=float, default=120, help="Seconds before the command is killed")
    s.add_argument("argv", nargs=argparse.REMAINDER, help="Command to run, after --")
    s.set_defaults(func=cmd_exec)

    s = sub.add_parser("cleanup", help="Stop what this CLI started; keep evidence",
                       formatter_class=fmt, epilog=f"Examples:\n  {PROG} cleanup\n  {PROG} cleanup --dry-run")
    s.add_argument("--dry-run", action="store_true", help="Print the plan, change nothing")
    s.set_defaults(func=cmd_cleanup)
    return p


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    if not args.command:
        parser.print_help()
        return 0
    try:
        return args.func(args)
    except UsageError as e:
        emit({"ok": False, "error": str(e), "example": e.example}, stream=sys.stderr)
        return 2
    except subprocess.TimeoutExpired as e:
        emit({"ok": False, "error": f"timed out after {e.timeout}s", "next": "raise --timeout"}, stream=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
