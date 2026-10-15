"""ratchet-memory.sh against fake size tools and the real one, and ratchet.toml under
`ns ratchet check`.

Run from anywhere: python3 -m unittest discover -s rules/c/tests
"""

import json
import os
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

PACK = Path(__file__).resolve().parent.parent
SCRIPT = PACK / "ratchet-memory.sh"
ROOT = PACK.parent.parent
TARGET = Path(os.environ.get("CARGO_TARGET_DIR", "cli/target"))
NS = (TARGET if TARGET.is_absolute() else ROOT / TARGET) / "debug" / "ns"

REPORT = (
    "   text\t   data\t    bss\t    dec\t    hex\tfilename\n"
    "  17800\t   1336\t    152\t  19288\t   4b58\tzephyr.elf\n"
)


class RatchetMemory(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(tmp.name)
        self.elf = self.tmp / "zephyr.elf"
        self.elf.write_bytes(b"\x7fELF")

    def fake_size(self, body):
        """A size tool, named fake-size in a directory of its own, whose script is `body`."""
        bindir = self.tmp / "bin"
        bindir.mkdir(exist_ok=True)
        tool = bindir / "fake-size"
        tool.write_text("#!/bin/sh\n" + body)
        tool.chmod(0o755)
        return tool

    def run_script(self, *args, size=None, path=None):
        env = dict(os.environ)
        env.pop("SIZE", None)
        if size is not None:
            env["SIZE"] = str(size)
        if path is not None:
            env["PATH"] = path
        return subprocess.run(
            [str(SCRIPT), *args], cwd=self.tmp, capture_output=True, text=True, env=env
        )

    def report(self, text):
        """A fake size tool that prints `text` for `-B <elf>` and fails on anything else."""
        return self.fake_size(
            '[ "$1" = -B ] && [ "$2" = zephyr.elf ] || exit 9\ncat <<\'END\'\n' + text + "END\n"
        )

    def assert_error(self, proc, message):
        self.assertNotEqual(proc.returncode, 0)
        self.assertEqual(proc.stdout, "")
        self.assertRegex(proc.stderr, r"(?m)^ratchet-memory\.sh: .*" + re.escape(message))

    def test_flash_is_text_plus_data(self):
        proc = self.run_script("flash", "zephyr.elf", size=self.report(REPORT))
        self.assertEqual((proc.returncode, proc.stdout), (0, "19136\n"), proc.stderr)

    def test_ram_is_data_plus_bss(self):
        proc = self.run_script("ram", "zephyr.elf", size=self.report(REPORT))
        self.assertEqual((proc.returncode, proc.stdout), (0, "1488\n"), proc.stderr)

    def test_a_large_image_prints_an_integer(self):
        report = "text data bss dec hex filename\n4000000000 300000000 0 0 0 zephyr.elf\n"
        proc = self.run_script("flash", "zephyr.elf", size=self.report(report))
        self.assertEqual((proc.returncode, proc.stdout), (0, "4300000000\n"), proc.stderr)

    def test_the_default_tool_is_size_on_path(self):
        tool = self.report(REPORT)
        tool.rename(tool.with_name("size"))
        path = f"{tool.parent}:{os.environ['PATH']}"
        proc = self.run_script("ram", "zephyr.elf", path=path)
        self.assertEqual((proc.returncode, proc.stdout), (0, "1488\n"), proc.stderr)

    def test_usage(self):
        size = self.report(REPORT)
        for args in [(), ("flash",), ("rom", "zephyr.elf"), ("flash", "zephyr.elf", "x")]:
            with self.subTest(args=args):
                proc = self.run_script(*args, size=size)
                self.assertEqual(proc.returncode, 2)
                self.assertEqual(proc.stdout, "")
                self.assertIn("usage:", proc.stderr)

    def test_a_missing_tool_is_an_error(self):
        proc = self.run_script("flash", "zephyr.elf", size=self.tmp / "no-such-size")
        self.assert_error(proc, "no-such-size is not on PATH; set SIZE")

    def test_a_missing_image_is_an_error(self):
        proc = self.run_script("flash", "missing.elf", size=self.report(REPORT))
        self.assert_error(proc, "missing.elf is missing; build it first")

    def test_a_failing_tool_is_an_error(self):
        proc = self.run_script("flash", "zephyr.elf", size=self.fake_size("echo bad >&2\nexit 1\n"))
        self.assert_error(proc, "fake-size failed on zephyr.elf")

    def test_a_report_that_is_not_one_berkeley_line_is_an_error(self):
        reports = {
            "empty": "",
            "header only": REPORT.splitlines(keepends=True)[0],
            "sysv format": "zephyr.elf  :\nsection size addr\n.text 100 0\n",
            "two files": REPORT + "  1\t2\t3\t6\t6\tother.elf\n",
            "not a number": "text data bss dec hex filename\n12k 1 2 3 4 zephyr.elf\n",
            "short line": "text data bss dec hex filename\n1 2\n",
            "no text column": "rom data bss dec hex filename\n1 2 3 6 6 zephyr.elf\n",
            "no bss column": "text data dec bss hex filename\n1 2 3 6 6 zephyr.elf\n",
            "no header": "1 2 3 6 6 a.elf\n4 5 6 15 f zephyr.elf\n",
        }
        for name, report in reports.items():
            with self.subTest(report=name):
                proc = self.run_script("ram", "zephyr.elf", size=self.report(report))
                self.assert_error(proc, "printed no berkeley size report for zephyr.elf")

    @unittest.skipUnless(shutil.which("size"), "no size on PATH")
    def test_the_real_size_on_an_elf(self):
        real = Path(shutil.which("size")).resolve()
        shutil.copy(real, self.elf)
        fields = subprocess.run(
            ["size", "-B", str(self.elf)], capture_output=True, text=True, check=True
        ).stdout.splitlines()[1].split()
        text, data, bss = (int(f) for f in fields[:3])
        for what, want in [("flash", text + data), ("ram", data + bss)]:
            with self.subTest(what=what):
                proc = self.run_script(what, "zephyr.elf")
                self.assertEqual((proc.returncode, proc.stdout), (0, f"{want}\n"), proc.stderr)

    def test_ratchet_toml_runs_under_ns_ratchet_check(self):
        if not NS.is_file():
            if os.environ.get("CI"):
                self.fail(f"{NS} is missing; build it with: cargo build --manifest-path cli/Cargo.toml")
            self.skipTest(f"no {NS}")
        git_env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        subprocess.run(["git", "init", "-q", str(self.tmp)], check=True, env=git_env)
        installed = self.tmp / ".nightshift" / "rules" / "c"
        installed.mkdir(parents=True)
        shutil.copy(SCRIPT, installed)
        toml = (PACK / "ratchet.toml").read_text()
        toml = toml.replace("{build}", "true").replace("{elf}", "zephyr.elf")
        (self.tmp / ".nightshift" / "nightshift.toml").write_text(toml)
        env = {k: v for k, v in os.environ.items() if not k.startswith("NS_RATCHET_")}
        env["SIZE"] = str(self.report(REPORT))
        proc = subprocess.run(
            [str(NS), "ratchet", "check"], cwd=self.tmp, capture_output=True, text=True, env=env, timeout=60
        )
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        got = [
            (r["name"], r["better"], r["status"], r["measured"])
            for r in json.loads(proc.stdout)["ratchets"]
        ]
        self.assertEqual(
            got, [("flash", "lower", "untracked", 19136), ("ram", "lower", "untracked", 1488)]
        )


if __name__ == "__main__":
    unittest.main()
