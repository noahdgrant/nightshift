import glob
import os
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile

REPLAY_TIMEOUT = 30
EXPIRED = r"inventory: error: .*expired"
RESERVATION_EXPIRED = re.compile(r"inventory: error: reservation \S+ has expired")
REPLAYABLE = re.compile(r"python3 -m inventory(?:\s|$)")


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


def replays_red(commands, root):
    with tempfile.TemporaryDirectory() as tmp:
        shutil.copytree(os.path.join(root, "src"), os.path.join(tmp, "src"))
        env = {"PATH": os.environ.get("PATH", os.defpath), "PYTHONPATH": "src", "HOME": tmp}
        for command in commands:
            if not REPLAYABLE.match(command):
                continue
            try:
                argv = shlex.split(command)
            except ValueError:
                continue
            if not stays_inside(argv):
                continue
            proc = subprocess.Popen(
                argv, cwd=tmp, env=env, text=True,
                stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, start_new_session=True,
            )
            try:
                _, err = proc.communicate(timeout=REPLAY_TIMEOUT)
            except subprocess.TimeoutExpired:
                os.killpg(proc.pid, signal.SIGKILL)
                proc.communicate()
                return False
            if proc.returncode != 0 and RESERVATION_EXPIRED.match(err):
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
    pinned_seconds = re.search(r"--now[ =]\S*T\d\d:\d\d:(?!00)\d\d", repro) is not None
    checks = {
        "phase": fm.get("phase") == "troubleshoot",
        "status": fm.get("status") == "pass",
        "base": fm.get("base", "") != "",
        "agent brief": "Agent Brief" in text and "- [ ]" in text,
        "repro body": repro != "",
        "repro holds a command": re.search(r"python3? -m inventory", repro) is not None,
        "repro ran red": re.search(r"(?m)^\W*" + EXPIRED, repro) is not None
        and pinned_seconds
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
