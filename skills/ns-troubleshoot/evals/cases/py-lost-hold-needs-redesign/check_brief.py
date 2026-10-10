import collections
import glob
import os
import re
import subprocess
import sys


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


def ran_red(repro):
    output = [l for l in repro.splitlines() if not re.match(r"\s*\$|.*\bpython3?\b", l)]
    if any(re.search(r"(R\d{4})\b.*\b\1\b", l) for l in output):
        return True
    reserved = collections.Counter(m.group(1) for l in output if (m := re.search(r"(R\d{4}): \d+ x ", l)))
    return any(n > 1 for n in reserved.values())


def failures(text):
    m = re.match(r"---\n(.*?)\n---\n", text, re.S)
    if not m:
        return ["frontmatter"]
    fm = {k: v.strip("'\"") for k, v in re.findall(r"(?m)^(\w+):[ \t]*(.*?)[ \t]*(?:#.*)?$", m.group(1))}
    first_line = next((l for l in text[m.end() :].splitlines() if l.strip()), "")
    route = re.search(r"(?im)^\**Route:\**[ \t]*(\S+)", text)
    repro = section(text, "repro")
    cause = section(text, "root cause")
    checks = {
        "phase": fm.get("phase") == "troubleshoot",
        "status": fm.get("status") == "blocked",
        "base": fm.get("base", "") != "",
        "first line says needs redesign": re.match(r"(?i)\W*needs redesign", first_line) is not None,
        "agent brief": "Agent Brief" in text and "- [ ]" in text,
        "routes to ns-define": route is not None and route.group(1) == "ns-define",
        "repro holds a command": re.search(r"(?m)^\s*(\$ )?python3?\b", repro) is not None,
        "repro ran red": ran_red(repro),
        "root cause body": cause != "",
        "root cause names load and save": re.search(r"\bload\b", cause) is not None
        and re.search(r"\bsave\b", cause) is not None,
        "root cause names the lost update": re.search(r"(?i)lock|last write|overwrit|race|concurren", cause)
        is not None,
    }
    return [k for k, ok in checks.items() if not ok]


def brief_paths():
    roots = {os.getcwd()}
    out = subprocess.run(["git", "worktree", "list", "--porcelain"], capture_output=True, text=True).stdout
    roots.update(l.split(" ", 1)[1] for l in out.splitlines() if l.startswith("worktree "))
    roots.update(glob.glob(os.path.join("..", os.path.basename(os.getcwd()) + ".worktrees", "06-*")))
    return sorted({os.path.realpath(p) for r in roots for p in glob.glob(os.path.join(r, ".ns", "06-*", "brief.md"))})


def main():
    briefs = brief_paths()
    if not briefs:
        sys.exit("no .ns/06-*/brief.md in the main checkout or any worktree")
    for path in briefs:
        with open(path, encoding="utf-8") as f:
            failed = failures(f.read())
        print(path, failed or "ok")
        if not failed:
            sys.exit(0)
    sys.exit("no brief.md passes every check")


if __name__ == "__main__":
    main()
