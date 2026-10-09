import pathlib
import unittest

import check_brief

SAMPLES = pathlib.Path(__file__).parent / "samples"


def failures(name):
    return check_brief.failures((SAMPLES / name).read_text(encoding="utf-8"))


class CheckBriefTest(unittest.TestCase):
    def test_good_brief_passes(self):
        self.assertEqual(failures("good.md"), [])

    def test_repro_that_never_went_red_fails(self):
        self.assertIn("repro ran red", failures("bad-repro-not-red.md"))

    def test_vague_root_cause_fails(self):
        self.assertIn("root cause names the truncation site", failures("bad-root-cause.md"))

    def test_hash_line_in_code_fence_does_not_end_section(self):
        text = (SAMPLES / "good.md").read_text(encoding="utf-8")
        self.assertIn("# reserve with seconds", check_brief.section(text, "repro"))

    def test_top_level_heading_ends_section(self):
        text = (SAMPLES / "good.md").read_text(encoding="utf-8")
        self.assertNotIn("Warehouse.save", check_brief.section(text, "repro"))

    def test_route_define_fails(self):
        text = (SAMPLES / "good.md").read_text(encoding="utf-8").replace("**Route:** ns-build", "**Route:** ns-define")
        self.assertIn("routes to ns-build", check_brief.failures(text))


if __name__ == "__main__":
    unittest.main()
