import contextlib
import io
import os
import pathlib
import shutil
import signal
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


def alive(pid):
    try:
        stat = pathlib.Path(f"/proc/{pid}/stat").read_text()
    except FileNotFoundError:
        return False
    return stat.rsplit(")", 1)[1].split()[0] != "Z"


def kill_leftovers(pidfile):
    if not pidfile.exists():
        return
    for pid in map(int, pidfile.read_text().split()):
        try:
            if alive(pid) and b"sleep" in pathlib.Path(f"/proc/{pid}/cmdline").read_bytes():
                os.kill(pid, signal.SIGKILL)
        except (FileNotFoundError, ProcessLookupError):
            pass


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

    def test_whole_minute_clock_fails(self):
        self.assertIn("repro ran red", check(with_repro(WHOLE_MINUTE_REPRO)))

    def test_whole_minute_clock_with_round_trip_names_in_an_inventory_command_fails(self):
        extra = "python3 -m inventory --state inv.json --now 2026-03-02T09:00:00 add-item to_dict from_dict\n"
        repro = WHOLE_MINUTE_REPRO.replace("```bash\n", "```bash\n" + extra)
        self.assertIn("repro ran red", check(with_repro(repro)))

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
            "import sys\n"
            "if 'reserve' in sys.argv:\n"
            "    print('R0001: 4 x BOLT-M6 until 2026-03-02T09:05:30')\n"
            "else:\n"
            "    print(\"inventory: error: reservation 'R0001' has expired\", file=sys.stderr)\n",
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

    def test_unbalanced_quote_lines_are_not_replayed_even_when_a_plain_split_would_go_red(self):
        self.assertIn("repro ran red", check(good().replace("--state inv.json", "--state it's.json")))

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
            "python3 -m inventory --state inv.json --now 2026-03-02T09:04:50 status"
            "; echo \"inventory: error: reservation 'R0001' has expired\" >&2; false\n" + ERROR_LINE,
        )
        self.assertIn("repro ran red", check(text))

    def test_other_errors_that_echo_the_expired_text_fail(self):
        echoes = (
            "fulfil R0001 has expired",
            "fulfil R0001 \"reservation 'R0001' has expired\"",
            "fulfil R0001 \"inventory: error: reservation 'R0001' has expired\"",
            "hasexpired",
            "reserve \"x has expired\" 1",
            "reserve \"x 'R0001' has expired\" 1",
        )
        for args in echoes:
            with self.subTest(args=args):
                text = good().replace("09:05:10 fulfil R0001", "09:04:50 fulfil R0001").replace(
                    ERROR_LINE,
                    f"python3 -m inventory --state inv.json --now 2026-03-02T09:04:50 {args}\n" + ERROR_LINE,
                )
                self.assertIn("repro ran red", check(text))

    def test_module_that_only_starts_with_inventory_is_not_replayed(self):
        root = pathlib.Path(_tmp.name) / "shadow"
        seed(root)
        (root / "src" / "inventory_shadow.py").write_text(
            "import sys\nsys.exit(\"inventory: error: reservation 'R0001' has expired\")\n", encoding="utf-8"
        )
        text = good().replace("09:05:10 fulfil R0001", "09:04:50 fulfil R0001").replace(
            ERROR_LINE, "python3 -m inventory_shadow --state inv.json --now 2026-03-02T09:04:50\n" + ERROR_LINE
        )
        self.assertIn("repro ran red", check(text, root))

    def test_non_inventory_command_is_not_replayed(self):
        text = good().replace("09:05:10 fulfil R0001", "09:04:50 fulfil R0001").replace(
            "export PYTHONPATH=src\n",
            "python3 -c \"import sys; sys.exit('inventory: error: reservation R0001 has expired')\"\n",
        )
        self.assertIn("repro ran red", check(text))

    def test_different_inventory_error_with_nonzero_exit_fails(self):
        text = good().replace(
            "python3 -m inventory --state inv.json --now 2026-03-02T09:00:30 add-item BOLT-M6 Bolt\n", ""
        )
        self.assertIn("repro ran red", check(text))

    def test_fulfil_past_the_printed_expiry_fails(self):
        for now in ("09:06:00", "09:05:30"):
            with self.subTest(now=now):
                self.assertIn("repro ran red", check(good().replace("09:05:10 fulfil", f"{now} fulfil")))

    def test_fulfil_on_the_wall_clock_fails(self):
        self.assertIn("repro ran red", check(good().replace("--now 2026-03-02T09:05:10 fulfil", "fulfil")))

    def test_sub_second_clock_passes(self):
        text = good().replace("09:00:30", "09:00:00.500000").replace("09:05:10", "09:05:00.200000")
        self.assertEqual(check(text), [])

    def test_now_written_with_equals_passes(self):
        self.assertEqual(check(good().replace("--now ", "--now=")), [])

    def test_state_path_echoing_the_expired_text_fails(self):
        for prefix in ("reservation R0001 has expired", "reservation 'R0001' has expired"):
            with self.subTest(prefix=prefix):
                text = good().replace("09:05:10 fulfil R0001", "09:04:50 fulfil R0001").replace(
                    ERROR_LINE,
                    f"python3 -m inventory --state \"{prefix}/s.json\" --now 2026-03-02T09:04:50 add-item A B\n"
                    f"python3 -m inventory --state \"{prefix}/../src/inventory/models.py\" "
                    "--now 2026-03-02T09:04:50 status\n" + ERROR_LINE,
                )
                self.assertIn("repro ran red", check(text))

    def test_fulfil_whose_last_clock_is_past_the_expiry_fails(self):
        for clocks in (
            "--now 2026-03-02T09:05:10 --now 2026-03-02T09:06:00",
            "--now 2026-03-02T09:05:10 --now=2026-03-02T09:06:00",
            "--now 2026-03-02T09:05:10 --no 2026-03-02T09:06:00",
        ):
            with self.subTest(clocks=clocks):
                self.assertIn("repro ran red", check(good().replace("--now 2026-03-02T09:05:10", clocks)))

    def test_abbreviated_clock_option_passes(self):
        self.assertEqual(check(good().replace("--now ", "--no ")), [])

    def test_line_with_a_missing_option_value_is_skipped(self):
        self.assertEqual(check(good().replace("export PYTHONPATH=src\n", "python3 -m inventory --now\n")), [])

    def test_other_spellings_of_the_same_state_file_pass(self):
        fulfil = "--state inv.json --now 2026-03-02T09:05:10 fulfil"
        for setup, at_fulfil in (("", "--state inventory.json"), ("--state inv.json ", "--state ./inv.json")):
            with self.subTest(setup=setup, at_fulfil=at_fulfil):
                text = good().replace(fulfil, "FULFIL").replace("--state inv.json ", setup)
                text = text.replace("FULFIL", fulfil.replace("--state inv.json", at_fulfil))
                self.assertEqual(check(text), [])

    def test_same_reservation_id_in_another_state_file_fails(self):
        other = "".join(
            f"python3 -m inventory --state b.json --now 2026-03-02T09:00:30 {cmd}\n"
            for cmd in ("add-item BOLT-M6 Bolt", "receive BOLT-M6 10", "reserve BOLT-M6 4 --ttl 60")
        )
        fulfil = "python3 -m inventory --state inv.json --now 2026-03-02T09:05:10 fulfil R0001\n"
        text = good().replace(fulfil, other + fulfil.replace("09:05:10", "09:06:00"))
        self.assertIn("repro ran red", check(text))

    def test_expiry_printed_by_a_command_other_than_reserve_fails(self):
        fulfil = "python3 -m inventory --state inv.json --now 2026-03-02T09:05:10 fulfil R0001\n"
        fake = "python3 -m inventory --state inv.json --now 2026-03-02T09:00:30 add-item X 'x) R0001: y until 2099-01-01T00:00:00 z'\n"
        text = good().replace(fulfil, fake + fulfil.replace("09:05:10", "09:06:00"))
        self.assertIn("repro ran red", check(text))

    def test_inventory_named_after_another_command_is_not_replayed(self):
        text = good().replace("09:05:10 fulfil R0001", "09:04:50 fulfil R0001").replace(
            ERROR_LINE,
            "python3 -c \"import sys; sys.exit(\\\"inventory: error: reservation 'R0001' has expired\\\")\" "
            "--state inv.json --now 2026-03-02T09:04:50 python3 -m inventory\n" + ERROR_LINE,
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

    def test_hung_command_and_its_children_are_killed_and_later_commands_are_not_run(self):
        pidfile = pathlib.Path(_tmp.name) / "hung.pids"
        root = pathlib.Path(_tmp.name) / "hung"
        seed(root)
        (root / "src" / "inventory" / "__main__.py").write_text(
            "import os, subprocess, sys, time\n"
            "if 'sleep' in sys.argv:\n"
            "    child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'])\n"
            f"    open({str(pidfile) + '.tmp'!r}, 'w').write(f'{{os.getpid()}} {{child.pid}}')\n"
            f"    os.replace({str(pidfile) + '.tmp'!r}, {str(pidfile)!r})\n"
            "    time.sleep(30)\n"
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
        self.addCleanup(kill_leftovers, pidfile)
        ready = []

        class StartsTheClockOnceBothRun(check_brief.subprocess.Popen):
            def communicate(self, input=None, timeout=None):
                if timeout is not None:
                    deadline = time.monotonic() + 30
                    while not pidfile.exists() and time.monotonic() < deadline:
                        time.sleep(0.01)
                    ready.append(time.monotonic())
                return super().communicate(input, timeout)

        with mock.patch.object(check_brief, "REPLAY_TIMEOUT", 1), mock.patch.object(
            check_brief.subprocess, "Popen", StartsTheClockOnceBothRun
        ):
            self.assertIn("repro ran red", check(with_repro(repro), root))
        self.assertEqual(len(ready), 1)
        self.assertLess(time.monotonic() - ready[0], 10)
        pids = [int(p) for p in pidfile.read_text().split()]
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and any(map(alive, pids)):
            time.sleep(0.05)
        self.assertEqual([p for p in pids if alive(p)], [])

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
