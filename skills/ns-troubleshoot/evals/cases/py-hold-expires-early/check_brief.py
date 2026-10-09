import glob
import os
import re
import subprocess
import sys


def section(text, name):
    body, inside, in_fence = [], False, False
    for line in text.splitlines():
        if line.lstrip().startswith("```"):
            in_fence = not in_fence
        elif not in_fence and re.match(r"#+[ \t]", line):
            if inside:
                break
            inside = re.match(r"#+[ \t]*" + name, line, re.I) is not None
            continue
        if inside:
            body.append(line)
    return "\n".join(body).strip()


def failures(text):
    m = re.match(r"---\n(.*?)\n---\n", text, re.S)
    if not m:
        return ["frontmatter"]
    fm = {k: v.strip("'\"") for k, v in re.findall(r"(?m)^(\w+):[ \t]*(.*?)[ \t]*(?:#.*)?$", m.group(1))}
    route = re.search(r"(?im)^\**Route:\**[ \t]*(ns-[a-z-]+)", text)
    repro = section(text, "repro")
    cause = section(text, "root cause")
    pinned_seconds = re.search(r"--now[ =]\S*T\d\d:\d\d:(?!00)\d\d", repro) is not None
    round_trip = re.search(r"(?i)round.?trip|reload|save.*load|to_dict|from_dict", repro) is not None
    checks = {
        "phase": fm.get("phase") == "troubleshoot",
        "status": fm.get("status") == "pass",
        "base": fm.get("base", "") != "",
        "agent brief": "Agent Brief" in text and "- [ ]" in text,
        "repro body": repro != "",
        "repro holds a command": re.search(r"python3? -m inventory", repro) is not None,
        "repro ran red": re.search(r"(?m)^\W*inventory: error: .*expired", repro) is not None
        and (pinned_seconds or round_trip),
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
            failed = failures(f.read())
        print(path, failed or "ok")
        if not failed:
            sys.exit(0)
    sys.exit("no brief.md passes every check")


if __name__ == "__main__":
    main()
