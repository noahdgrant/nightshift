import pathlib
import unittest

import check_brief

SAMPLES = pathlib.Path(__file__).parent / "samples"
FIRST_LINE = "needs redesign: route ns-define, because stopping the lost update needs locking or a state file revision, and ADR-0001 rules out locks.\n"


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

    def test_brief_routed_to_build_fails(self):
        failed = failures("bad-routes-build.md")
        for check in ("status", "routes to ns-define", "first line says needs redesign"):
            self.assertIn(check, failed)

    def test_repro_without_red_output_fails(self):
        self.assertIn("repro ran red", failures("bad-repro-not-red.md"))

    def test_missing_frontmatter_fails(self):
        self.assertEqual(check_brief.failures("no frontmatter\n"), ["frontmatter"])

    def test_status_fail_fails(self):
        self.assertIn("status", check_brief.failures(good().replace("status: blocked", "status: fail")))

    def test_triage_phase_fails(self):
        self.assertIn("phase", check_brief.failures(good().replace("phase: troubleshoot", "phase: triage")))

    def test_missing_base_fails(self):
        self.assertIn("base", check_brief.failures(good().replace("base: main\n", "")))

    def test_first_line_below_issue_fails(self):
        text = good().replace(FIRST_LINE, "")
        text = text.replace("## Agent Brief", FIRST_LINE + "## Agent Brief")
        self.assertIn("first line says needs redesign", check_brief.failures(text))

    def test_empty_body_fails_the_first_line(self):
        text = good()[: good().index(FIRST_LINE)]
        self.assertIn("first line says needs redesign", check_brief.failures(text))

    def test_brief_without_acceptance_criteria_fails(self):
        text = good().replace("- [ ]", "-")
        self.assertIn("agent brief", check_brief.failures(text))

    def test_missing_route_fails(self):
        text = good().replace("**Route:** ns-define\n", "")
        self.assertIn("routes to ns-define", check_brief.failures(text))

    def test_missing_agent_brief_fails(self):
        text = good().replace("## Agent Brief", "## Summary")
        self.assertIn("agent brief", check_brief.failures(text))

    def test_route_none_fails(self):
        text = good().replace("**Route:** ns-define", "**Route:** none")
        self.assertIn("routes to ns-define", check_brief.failures(text))

    def test_repro_with_no_command_fails(self):
        repro = "Two stations reserved at once and both got R0001, R0001.\n"
        failed = check_brief.failures(with_repro(repro))
        self.assertIn("repro holds a command", failed)

    def test_duplicate_id_in_a_command_line_is_not_red(self):
        repro = "```bash\npython3 -m inventory fulfil R0001 R0001\n```\n"
        self.assertIn("repro ran red", check_brief.failures(with_repro(repro)))

    def test_duplicate_id_in_a_prompt_line_is_not_red(self):
        repro = "```bash\npython3 -m inventory status\n$ grep R0001 inv.json # R0001\n```\n"
        self.assertIn("repro ran red", check_brief.failures(with_repro(repro)))

    def test_two_reserve_outputs_with_one_id_are_red(self):
        repro = (
            "```bash\n"
            "python3 -m inventory --state inv.json reserve BOLT-M6 1 &\n"
            "python3 -m inventory --state inv.json reserve BOLT-M6 1 & wait\n"
            "R0006: 1 x BOLT-M6 until 2026-03-02T09:30:00\n"
            "R0006: 1 x BOLT-M6 until 2026-03-02T09:30:00\n"
            "```\n"
        )
        self.assertNotIn("repro ran red", check_brief.failures(with_repro(repro)))

    def test_two_reserve_outputs_with_different_ids_are_not_red(self):
        repro = (
            "```bash\n"
            "python3 -m inventory --state inv.json reserve BOLT-M6 1\n"
            "R0006: 1 x BOLT-M6 until 2026-03-02T09:30:00\n"
            "R0007: 1 x BOLT-M6 until 2026-03-02T09:30:00\n"
            "```\n"
        )
        self.assertIn("repro ran red", check_brief.failures(with_repro(repro)))

    def test_root_cause_without_load_and_save_fails(self):
        cause = "Two runs overlap with no lock, and the last write wins."
        self.assertIn("root cause names load and save", check_brief.failures(with_cause(cause)))

    def test_root_cause_naming_only_load_fails(self):
        cause = "Each run calls `storage.load` with no lock, so the last write wins."
        self.assertIn("root cause names load and save", check_brief.failures(with_cause(cause)))

    def test_root_cause_without_the_race_fails(self):
        cause = "`storage.load` and `storage.save` handle the state file."
        self.assertIn("root cause names the lost update", check_brief.failures(with_cause(cause)))

    def test_empty_root_cause_fails(self):
        self.assertIn("root cause body", check_brief.failures(with_cause("")))

    def test_hash_line_in_code_fence_does_not_end_section(self):
        text = "## Repro\n```bash\n# a comment\nrun\n```\n## Next\nother\n"
        self.assertEqual(check_brief.section(text, "repro"), "```bash\n# a comment\nrun\n```")

    def test_same_or_shallower_heading_ends_section(self):
        for heading in ("## Root cause", "# Top"):
            text = f"## Repro\nbody\n{heading}\ncause\n"
            self.assertEqual(check_brief.section(text, "repro"), "body")


if __name__ == "__main__":
    unittest.main()
