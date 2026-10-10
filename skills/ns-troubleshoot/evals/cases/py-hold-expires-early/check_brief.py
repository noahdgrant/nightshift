import argparse
import glob
import os
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
from datetime import datetime

REPLAY_TIMEOUT = 30
EXPIRED = r"inventory: error: .*expired"
RESERVATION_EXPIRED = re.compile(r"inventory: error: reservation '(R\d+)' has expired\n")
RESERVED_UNTIL = re.compile(r"(R\d+): .* until (\S+)\n")
REPLAYABLE = re.compile(r"(?:\w+=\S*\s+)*python3 -m inventory(?:\s|$)")


def section(text, name):
    body, inside, in_fence, depth = [], False, False, 0
    for line in text.splitlines():
        if line.lstrip().startswith("```"):
            in_fence = not in_fence
        elif not in_fence and (heading := re.match(r"(#+)[ \t]+(.*)", line)):
            level = len(heading.group(1))
            if inside and level <= depth:
                break
            if not inside and re.match(name, heading.group(2), re.I):
                inside, depth = True, level
                continue
        if inside:
            body.append(line)
    return "\n".join(body).strip()


def fence_lines(text):
    lines, in_fence = [], False
    for line in text.splitlines():
        line = line.strip()
        if line.startswith("```"):
            in_fence = not in_fence
        elif in_fence:
            lines.append(line.removeprefix("$ "))
    return lines


def stays_inside(argv):
    for arg in argv:
        for part in arg.split("="):
            norm = os.path.normpath(part) if part else part
            if part.startswith(("/", "~")) or norm == ".." or norm.startswith("../"):
                return False
    return True


class GlobalOptions(argparse.ArgumentParser):
    def error(self, message):
        raise ValueError(message)


def global_options(argv):
    parser = GlobalOptions(add_help=False)
    parser.add_argument("--state", default="inventory.json")
    parser.add_argument("--now")
    return parser.parse_known_args(argv[3:])[0]


def before(now, expiry):
    try:
        return datetime.fromisoformat(now) < datetime.fromisoformat(expiry)
    except (TypeError, ValueError):
        return False


def replays_red(commands, root):
    with tempfile.TemporaryDirectory() as tmp:
        shutil.copytree(os.path.join(root, "src"), os.path.join(tmp, "src"))
        expiries = {}
        env = {"PATH": os.environ.get("PATH", os.defpath), "PYTHONPATH": "src", "HOME": tmp}
        for command in commands:
            if not REPLAYABLE.match(command):
                continue
            try:
                argv = shlex.split(command, comments=True)
            except ValueError:
                continue
            argv = argv[argv.index("python3"):]
            if not stays_inside(argv):
                continue
            try:
                options = global_options(argv)
            except ValueError:
                continue
            state = os.path.normpath(options.state)
            proc = subprocess.Popen(
                argv, cwd=tmp, env=env, text=True,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True,
            )
            try:
                out, err = proc.communicate(timeout=REPLAY_TIMEOUT)
            except subprocess.TimeoutExpired:
                os.killpg(proc.pid, signal.SIGKILL)
                proc.communicate()
                return False
            if reserved := RESERVED_UNTIL.fullmatch(out):
                expiries[state, reserved.group(1)] = reserved.group(2)
            expired = RESERVATION_EXPIRED.fullmatch(err)
            if proc.returncode != 0 and expired and before(options.now, expiries.get((state, expired.group(1)))):
                return True
    return False


def failures(text, root):
    m = re.match(r"---\n(.*?)\n---\n", text, re.S)
    if not m:
        return ["frontmatter"]
    fm = {k: v.strip("'\"") for k, v in re.findall(r"(?m)^(\w+):[ \t]*(.*?)[ \t]*(?:#.*)?$", m.group(1))}
    route = re.search(r"(?im)^\**Route:\**[ \t]*(ns-[a-z-]+)", text)
    repro = section(text, "repro")
    cause = section(text, "root cause")
    checks = {
        "phase": fm.get("phase") == "troubleshoot",
        "status": fm.get("status") == "pass",
        "base": fm.get("base", "") != "",
        "agent brief": "Agent Brief" in text and "- [ ]" in text,
        "repro body": repro != "",
        "repro holds a command": re.search(r"python3? -m inventory", repro) is not None,
        "repro ran red": re.search(r"(?m)^\W*" + EXPIRED, repro) is not None
        and replays_red(fence_lines(repro), root),
        "root cause body": cause != "",
        "root cause names expires_at": "expires_at" in cause,
        "root cause names the truncation site": re.search(r"to_dict|models\.py", cause) is not None
        and re.search(r"(?i)minute|timespec", cause) is not None,
        "routes to ns-build": route is not None and route.group(1) == "ns-build",
    }
    return [k for k, ok in checks.items() if not ok]


def brief_paths():
    roots = {os.getcwd()}
    out = subprocess.run(["git", "worktree", "list", "--porcelain"], capture_output=True, text=True).stdout
    roots.update(l.split(" ", 1)[1] for l in out.splitlines() if l.startswith("worktree "))
    roots.update(glob.glob(os.path.join("..", os.path.basename(os.getcwd()) + ".worktrees", "02-*")))
    return sorted({os.path.realpath(p) for r in roots for p in glob.glob(os.path.join(r, ".ns", "02-*", "brief.md"))})


def main():
    briefs = brief_paths()
    if not briefs:
        sys.exit("no .ns/02-*/brief.md in the main checkout or any worktree")
    for path in briefs:
        with open(path, encoding="utf-8") as f:
            failed = failures(f.read(), os.getcwd())
        print(path, failed or "ok")
        if not failed:
            sys.exit(0)
    sys.exit("no brief.md passes every check")


if __name__ == "__main__":
    main()
