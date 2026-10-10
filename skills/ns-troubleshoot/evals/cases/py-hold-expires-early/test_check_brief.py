import pathlib
import unittest

import check_brief

SAMPLES = pathlib.Path(__file__).parent / "samples"
ERROR_LINE = "# inventory: error: reservation 'R0001' has expired"
CAUSE = "`Reservation.to_dict` in `models.py` writes `expires_at` cut to the minute, so a reload loses up to 59 s."


def good():
    return (SAMPLES / "good.md").read_text(encoding="utf-8")


def failures(name):
    return check_brief.failures((SAMPLES / name).read_text(encoding="utf-8"))


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
        self.assertEqual(check_brief.failures(good()), [])

    def test_repro_that_never_went_red_fails(self):
        self.assertIn("repro ran red", failures("bad-repro-not-red.md"))

    def test_vague_root_cause_fails(self):
        self.assertIn("root cause names the truncation site", failures("bad-root-cause.md"))

    def test_whole_minute_clock_without_round_trip_command_fails(self):
        repro = (
            "```bash\n"
            "python3 -m inventory --state inv.json --now 2026-03-02T09:00:00 reserve BOLT-M6 4 --ttl 5\n"
            "python3 -m inventory --state inv.json --now 2026-03-02T09:05:00 fulfil R0001\n"
            f"{ERROR_LINE}\n"
            "```\n"
        )
        self.assertIn("repro ran red", check_brief.failures(with_repro(repro)))

    def test_round_trip_named_only_in_prose_fails(self):
        repro = (
            "```bash\n"
            "python3 -m inventory --state inv.json --now 2026-03-02T09:00:00 fulfil R0001\n"
            f"{ERROR_LINE}\n"
            "```\n"
            "The reload calls `to_dict` and `from_dict`: a save/load round trip.\n"
        )
        self.assertIn("repro ran red", check_brief.failures(with_repro(repro)))

    def test_round_trip_command_without_pinned_seconds_passes(self):
        repro = (
            "```bash\n"
            "python3 -m inventory --state inv.json reserve BOLT-M6 4 --ttl 5\n"
            "python3 -c 'from inventory.models import Reservation as R; print(R.from_dict(r.to_dict()))'\n"
            f"{ERROR_LINE}\n"
            "```\n"
        )
        self.assertNotIn("repro ran red", check_brief.failures(with_repro(repro)))

    def test_status_fail_fails(self):
        self.assertIn("status", check_brief.failures(good().replace("status: pass", "status: fail")))

    def test_status_blocked_fails(self):
        self.assertIn("status", check_brief.failures(good().replace("status: pass", "status: blocked")))

    def test_triage_phase_fails(self):
        self.assertIn("phase", check_brief.failures(good().replace("phase: troubleshoot", "phase: triage")))

    def test_root_cause_without_expires_at_fails(self):
        cause = "`Reservation.to_dict` in `models.py` cuts the stored time to the minute, so a reload loses up to 59 s."
        self.assertIn("root cause names expires_at", check_brief.failures(with_cause(cause)))

    def test_missing_agent_brief_fails(self):
        text = good().replace("## Agent Brief", "## Summary")
        self.assertIn("agent brief", check_brief.failures(text))

    def test_empty_base_fails(self):
        self.assertIn("base", check_brief.failures(good().replace("base: main", "base:")))

    def test_agent_brief_without_open_criterion_fails(self):
        text = good().replace("- [ ] the minimised repro", "- the minimised repro")
        self.assertIn("agent brief", check_brief.failures(text))

    def test_truncation_site_without_minute_fails(self):
        cause = "`Reservation.to_dict` in `models.py` writes `expires_at` wrong, so a reload loses up to 59 s."
        self.assertIn("root cause names the truncation site", check_brief.failures(with_cause(cause)))

    def test_minute_without_truncation_site_fails(self):
        cause = "The state file stores `expires_at` cut to the minute, so a reload loses up to 59 s."
        self.assertIn("root cause names the truncation site", check_brief.failures(with_cause(cause)))

    def test_error_line_after_prose_fails(self):
        text = good().replace("# inventory: error:", "# see inventory: error:")
        self.assertIn("repro ran red", check_brief.failures(text))

    def test_route_define_fails(self):
        text = good().replace("**Route:** ns-build", "**Route:** ns-define")
        self.assertIn("routes to ns-build", check_brief.failures(text))

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
