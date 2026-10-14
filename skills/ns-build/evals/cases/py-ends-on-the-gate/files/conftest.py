import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).parent


def pytest_collection_finish(session):
    every = {p.resolve() for p in (ROOT / "tests").glob("test_*.py")}
    ran = {Path(str(item.fspath)).resolve() for item in session.items}
    if not every or ran != every:
        return
    git = subprocess.run(
        ["git", "rev-parse", "--git-common-dir"], cwd=ROOT, capture_output=True, text=True
    )
    if git.returncode != 0:
        return
    via = "ci-local" if os.environ.get("CI_LOCAL") == "1" else "direct"
    with open(ROOT / git.stdout.strip() / "ns-eval-full-runs.log", "a", encoding="utf-8") as log:
        log.write(via + "\n")
