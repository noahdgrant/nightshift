import contextlib
import io
import os
import pathlib
import shutil
import tempfile
import time
import unittest
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
    shutil.copytree(FIXTURE, SEEDED)
    models = SEEDED / "src" / "inventory" / "models.py"
    source = models.read_text(encoding="utf-8")
    assert TRUNCATION[0] in source, "the case's [setup] sed no longer matches models.py"
    models.write_text(source.replace(*TRUNCATION), encoding="utf-8")


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

    def test_round_trip_command_without_pinned_seconds_passes(self):
        repro = (
            "```bash\n"
            "python3 -c 'import sys; from datetime import datetime as D; from inventory.models import Reservation as R; "
            "r = R(\"R0001\", \"BOLT-M6\", 4, D(2026, 3, 2, 9, 0, 30), D(2026, 3, 2, 9, 5, 30)); "
            "R.from_dict(r.to_dict()).is_expired(D(2026, 3, 2, 9, 5, 10)) and sys.exit(\"inventory: error: R0001 has expired\")'\n"
            f"{ERROR_LINE}\n"
            "```\n"
        )
        self.assertNotIn("repro ran red", check(with_repro(repro)))

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

    def test_error_with_zero_exit_fails(self):
        text = good().replace("fulfil R0001\n", "fulfil R0001 || true\n")
        self.assertIn("repro ran red", check(text))

    def test_replay_leaves_the_root_untouched(self):
        before = sorted(p.relative_to(SEEDED) for p in SEEDED.rglob("*") if "__pycache__" not in p.parts)
        self.assertEqual(check(good()), [])
        after = sorted(p.relative_to(SEEDED) for p in SEEDED.rglob("*") if "__pycache__" not in p.parts)
        self.assertEqual(before, after)

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
        text = good().replace("export PYTHONPATH=src\n", "sleep 30; true\n")
        start = time.monotonic()
        with mock.patch.object(check_brief, "REPLAY_TIMEOUT", 1):
            self.assertIn("repro ran red", check(text))
        self.assertLess(time.monotonic() - start, 10)

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
