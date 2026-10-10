import contextlib
import io
import os
import pathlib
import shutil
import tempfile
import time
import unittest
import uuid
from unittest import mock

import check_brief

HERE = pathlib.Path(__file__).resolve().parent
SAMPLES = HERE / "samples"
FIXTURE = HERE.parents[4] / "evals" / "fixtures" / "py-inventory"
TRUNCATION = (
    '"expires_at": self.expires_at.isoformat(),',
    '"expires_at": self.expires_at.isoformat(timespec="minutes"),',
)
ERROR_LINE = "# inventory: error: reservation 'R0001' has expired"
WHOLE_MINUTE_REPRO = (
    "```bash\n"
    "python3 -m inventory --state inv.json --now 2026-03-02T09:00:00 add-item BOLT-M6 Bolt\n"
    "python3 -m inventory --state inv.json --now 2026-03-02T09:00:00 receive BOLT-M6 10\n"
    "python3 -m inventory --state inv.json --now 2026-03-02T09:00:00 reserve BOLT-M6 4 --ttl 5\n"
    "python3 -m inventory --state inv.json --now 2026-03-02T09:05:00 fulfil R0001\n"
    f"{ERROR_LINE}\n"
    "```\n"
)
CAUSE = "`Reservation.to_dict` in `models.py` writes `expires_at` cut to the minute, so a reload loses up to 59 s."


def setUpModule():
    global SEEDED, PRISTINE, _tmp
    _tmp = tempfile.TemporaryDirectory()
    PRISTINE = pathlib.Path(_tmp.name) / "pristine"
    SEEDED = pathlib.Path(_tmp.name) / "seeded"
    shutil.copytree(FIXTURE, PRISTINE)
    seed(SEEDED)


def seed(dest):
    shutil.copytree(FIXTURE, dest, dirs_exist_ok=True)
    models = pathlib.Path(dest) / "src" / "inventory" / "models.py"
    source = models.read_text(encoding="utf-8")
    assert TRUNCATION[0] in source, "the case's [setup] sed no longer matches models.py"
    models.write_text(source.replace(*TRUNCATION), encoding="utf-8")


def snapshot(root):
    return {str(p.relative_to(root)): p.read_bytes() for p in root.rglob("*") if p.is_file()}


def tearDownModule():
    _tmp.cleanup()


def check(text, root=None):
    return check_brief.failures(text, root or SEEDED)


def good():
    return (SAMPLES / "good.md").read_text(encoding="utf-8")


def failures(name):
    return check((SAMPLES / name).read_text(encoding="utf-8"))


def with_repro(repro):
    text = good()
    start = text.index("## Repro")
    end = text.index("## Root cause")
    return text[:start] + "## Repro\n" + repro + "\n" + text[end:]


def with_cause(cause):
    text = good()
    return text[: text.index("## Root cause")] + "## Root cause\n" + cause + "\n"


class CheckBriefTest(unittest.TestCase):
    def test_good_brief_passes(self):
        self.assertEqual(check(good()), [])

    def test_repro_that_never_went_red_fails(self):
        self.assertIn("repro ran red", failures("bad-repro-not-red.md"))

    def test_vague_root_cause_fails(self):
        self.assertIn("root cause names the truncation site", failures("bad-root-cause.md"))

    def test_whole_minute_clock_without_round_trip_command_fails(self):
        self.assertIn("repro ran red", check(with_repro(WHOLE_MINUTE_REPRO)))

    def test_round_trip_named_only_in_prose_fails(self):
        repro = WHOLE_MINUTE_REPRO + "The reload calls `to_dict` and `from_dict`: a save/load round trip.\n"
        self.assertIn("repro ran red", check(with_repro(repro)))

    def test_round_trip_command_that_is_not_an_inventory_run_fails(self):
        repro = (
            "```bash\n"
            "python3 -c 'import sys; from datetime import datetime as D; from inventory.models import Reservation as R; "
            "r = R(\"R0001\", \"BOLT-M6\", 4, D(2026, 3, 2, 9, 0, 30), D(2026, 3, 2, 9, 5, 30)); "
            "R.from_dict(r.to_dict()).is_expired(D(2026, 3, 2, 9, 5, 10)) and sys.exit(\"inventory: error: R0001 has expired\")'\n"
            f"{ERROR_LINE}\n"
            "```\n"
        )
        self.assertIn("repro ran red", check(with_repro(repro)))

    def test_typed_error_line_after_commands_that_pass_fails(self):
        text = good().replace("09:05:10 fulfil", "09:04:50 fulfil")
        self.assertIn("repro ran red", check(text))

    def test_shell_prompts_are_stripped_before_replay(self):
        text = good().replace("python3 -m inventory --state", "$ python3 -m inventory --state")
        self.assertEqual(check(text), [])

    def test_commands_outside_a_fence_are_not_replayed(self):
        text = good().replace("```bash\nexport PYTHONPATH=src\n", "").replace(
            f"{ERROR_LINE}\n```", f"```bash\n{ERROR_LINE}\n```"
        )
        self.assertIn("repro ran red", check(text))

    def test_commands_after_the_fence_closes_are_not_replayed(self):
        fulfil = "python3 -m inventory --state inv.json --now 2026-03-02T09:05:10 fulfil R0001\n"
        text = good().replace(fulfil, "").replace(f"{ERROR_LINE}\n```\n", f"{ERROR_LINE}\n```\n{fulfil}")
        self.assertIn("repro ran red", check(text))

    def test_good_repro_against_unseeded_fixture_fails(self):
        self.assertIn("repro ran red", check(good(), PRISTINE))

    def test_error_only_on_stdout_fails(self):
        text = good().replace(
            "python3 -m inventory --state inv.json --now 2026-03-02T09:05:10 fulfil R0001\n",
            "python3 -m inventory --state inv.json --now 2026-03-02T09:05:10 fulfil R0001 2>&1\n",
        )
        self.assertIn("repro ran red", check(text))

    def test_error_printed_with_zero_exit_fails(self):
        root = pathlib.Path(_tmp.name) / "zero-exit"
        seed(root)
        (root / "src" / "inventory" / "__main__.py").write_text(
            "import sys\nprint(\"inventory: error: reservation 'R0001' has expired\", file=sys.stderr)\n",
            encoding="utf-8",
        )
        self.assertIn("repro ran red", check(good(), root))

    def test_outside_state_path_is_not_replayed(self):
        name = f"outside-{uuid.uuid4().hex}.json"
        out = pathlib.Path(tempfile.mkdtemp(dir=_tmp.name))
        escapes = (
            f"--state {out}/{name}",
            f"--state={out}/{name}",
            f"--state ../{name}",
            f"--state=../{name}",
            f"--state ~/../{name}",
        )
        for flag in escapes:
            with self.subTest(flag=flag):
                self.assertIn("repro ran red", check(good().replace("--state inv.json", flag)))
                self.assertEqual(list(out.iterdir()), [])
                self.assertFalse((pathlib.Path(tempfile.gettempdir()) / name).exists())

    def test_escaping_state_paths_are_never_spawned(self):
        name = f"outside-{uuid.uuid4().hex}.json"
        escapes = (
            "--state ..",
            f"--state sub/../../{name}",
            f"--state ~/{name}",
            "--state ./..",
            f"--state=sub/../../{name}",
        )
        for flag in escapes:
            with self.subTest(flag=flag):
                with mock.patch.object(check_brief.subprocess, "Popen", wraps=check_brief.subprocess.Popen) as popen:
                    self.assertIn("repro ran red", check(good().replace("--state inv.json", flag)))
                self.assertEqual(popen.call_count, 0)
                self.assertFalse((pathlib.Path(tempfile.gettempdir()) / name).exists())

    def test_inside_state_path_with_dotdot_is_still_spawned(self):
        with mock.patch.object(check_brief.subprocess, "Popen", wraps=check_brief.subprocess.Popen) as popen:
            check(good().replace("--state inv.json", "--state sub/../inv.json"))
        self.assertGreater(popen.call_count, 0)

    def test_unbalanced_quote_line_is_skipped(self):
        text = good().replace("export PYTHONPATH=src\n", "python3 -m inventory 'unbalanced\n")
        self.assertEqual(check(text), [])

    def test_replay_leaves_the_root_untouched(self):
        root = pathlib.Path(_tmp.name) / "untouched"
        seed(root)
        before = snapshot(root)
        self.assertEqual(check(good(), root), [])
        self.assertEqual(snapshot(root), before)

    def test_echoed_error_after_commands_that_pass_fails(self):
        text = good().replace("09:05:10 fulfil", "09:04:50 fulfil").replace(
            ERROR_LINE,
            "echo \"inventory: error: reservation 'R0001' has expired\" >&2; false\n" + ERROR_LINE,
        )
        self.assertIn("repro ran red", check(text))

    def test_chained_echo_and_false_on_one_line_fails(self):
        text = good().replace("09:05:10 fulfil R0001", "09:04:50 fulfil R0001").replace(
            ERROR_LINE,
            "python3 -m inventory --state inv.json --now 2026-03-02T09:04:50 fulfil R0001"
            "; echo \"inventory: error: reservation 'R0001' has expired\" >&2; false\n" + ERROR_LINE,
        )
        self.assertIn("repro ran red", check(text))

    def test_non_inventory_command_is_not_executed(self):
        marker = pathlib.Path(_tmp.name) / "marker"
        text = good().replace("export PYTHONPATH=src\n", f"touch {marker}\n")
        check(text)
        self.assertFalse(marker.exists())

    def test_chained_command_is_not_executed(self):
        marker = pathlib.Path(_tmp.name) / "chained"
        text = good().replace("export PYTHONPATH=src\n", f"python3 -m inventory --help; touch {marker}\n")
        check(text)
        self.assertFalse(marker.exists())

    def test_different_inventory_error_with_nonzero_exit_fails(self):
        text = good().replace(
            "python3 -m inventory --state inv.json --now 2026-03-02T09:00:30 add-item BOLT-M6 Bolt\n", ""
        )
        self.assertIn("repro ran red", check(text))

    def test_main_replays_against_the_current_checkout(self):
        trial = pathlib.Path(_tmp.name) / "trial"
        shutil.copytree(SEEDED, trial)
        (trial / ".ns" / "02-holds-expire-early").mkdir(parents=True)
        (trial / ".ns" / "02-holds-expire-early" / "brief.md").write_text(good(), encoding="utf-8")
        cwd = os.getcwd()
        os.chdir(trial)
        try:
            with self.assertRaises(SystemExit) as exit, contextlib.redirect_stdout(io.StringIO()):
                check_brief.main()
        finally:
            os.chdir(cwd)
        self.assertEqual(exit.exception.code, 0)

    def test_command_past_the_timeout_fails_promptly(self):
        start = time.monotonic()
        with mock.patch.object(check_brief, "REPLAY_TIMEOUT", 0.001):
            self.assertIn("repro ran red", check(good()))
        self.assertLess(time.monotonic() - start, 10)

    def test_hung_command_is_killed_and_later_commands_are_not_run(self):
        pidfile = pathlib.Path(_tmp.name) / "hung.pid"
        root = pathlib.Path(_tmp.name) / "hung"
        seed(root)
        (root / "src" / "inventory" / "__main__.py").write_text(
            "import os, sys, time\n"
            "if 'sleep' in sys.argv:\n"
            f"    open({str(pidfile)!r}, 'w').write(str(os.getpid()))\n"
            "    time.sleep(60)\n"
            "sys.exit(\"inventory: error: reservation 'R0001' has expired\")\n",
            encoding="utf-8",
        )
        repro = (
            "```bash\n"
            "python3 -m inventory sleep\n"
            "python3 -m inventory --now 2026-03-02T09:05:10 fulfil R0001\n"
            f"{ERROR_LINE}\n"
            "```\n"
        )
        start = time.monotonic()
        with mock.patch.object(check_brief, "REPLAY_TIMEOUT", 1):
            self.assertIn("repro ran red", check(with_repro(repro), root))
        self.assertLess(time.monotonic() - start, 10)
        pid = int(pidfile.read_text())
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and pathlib.Path(f"/proc/{pid}").exists():
            time.sleep(0.05)
        self.assertFalse(pathlib.Path(f"/proc/{pid}").exists())

    def test_status_fail_fails(self):
        self.assertIn("status", check(good().replace("status: pass", "status: fail")))

    def test_status_blocked_fails(self):
        self.assertIn("status", check(good().replace("status: pass", "status: blocked")))

    def test_triage_phase_fails(self):
        self.assertIn("phase", check(good().replace("phase: troubleshoot", "phase: triage")))

    def test_root_cause_without_expires_at_fails(self):
        cause = "`Reservation.to_dict` in `models.py` cuts the stored time to the minute, so a reload loses up to 59 s."
        self.assertIn("root cause names expires_at", check(with_cause(cause)))

    def test_missing_agent_brief_fails(self):
        text = good().replace("## Agent Brief", "## Summary")
        self.assertIn("agent brief", check(text))

    def test_empty_base_fails(self):
        self.assertIn("base", check(good().replace("base: main", "base:")))

    def test_agent_brief_without_open_criterion_fails(self):
        text = good().replace("- [ ] the minimised repro", "- the minimised repro")
        self.assertIn("agent brief", check(text))

    def test_truncation_site_without_minute_fails(self):
        cause = "`Reservation.to_dict` in `models.py` writes `expires_at` wrong, so a reload loses up to 59 s."
        self.assertIn("root cause names the truncation site", check(with_cause(cause)))

    def test_minute_without_truncation_site_fails(self):
        cause = "The state file stores `expires_at` cut to the minute, so a reload loses up to 59 s."
        self.assertIn("root cause names the truncation site", check(with_cause(cause)))

    def test_error_line_after_prose_fails(self):
        text = good().replace("# inventory: error:", "# see inventory: error:")
        self.assertIn("repro ran red", check(text))

    def test_route_define_fails(self):
        text = good().replace("**Route:** ns-build", "**Route:** ns-define")
        self.assertIn("routes to ns-build", check(text))

    def test_hash_line_in_code_fence_does_not_end_section(self):
        text = "## Repro\n```bash\n# a comment\nrun\n```\n## Next\nother\n"
        self.assertEqual(check_brief.section(text, "repro"), "```bash\n# a comment\nrun\n```")

    def test_deeper_heading_stays_in_section(self):
        text = "## Repro\nbody\n### Seam\nWarehouse.save\n## Root cause\ncause\n"
        self.assertEqual(check_brief.section(text, "repro"), "body\n### Seam\nWarehouse.save")

    def test_same_or_shallower_heading_ends_section(self):
        for heading in ("## Root cause", "# Top"):
            text = f"## Repro\nbody\n{heading}\ncause\n"
            self.assertEqual(check_brief.section(text, "repro"), "body")


if __name__ == "__main__":
    unittest.main()
