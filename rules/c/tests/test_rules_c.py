"""The C pack's own tests: every rule has a failing and a passing case under cases/<rule>/.

Run from anywhere: python3 -m unittest discover -s rules/c/tests
"""

import importlib.machinery
import json
import os
import shutil
import subprocess
import sys
import tempfile
import types
import unittest
from pathlib import Path

PACK = Path(__file__).resolve().parent.parent
CASES = Path(__file__).resolve().parent / "cases"
RULES_C = str(PACK / "rules-c")


def run(args, cwd, env=None):
    return subprocess.run(
        [sys.executable, RULES_C] + args, cwd=cwd, capture_output=True, text=True, env=env
    )


def findings(proc):
    return sorted(
        (f["rule"], f["file"], f["line"]) for f in map(json.loads, proc.stdout.splitlines())
    )


def case(rule, side, subcommand, *extra):
    cwd = CASES / rule / side
    files = sorted(p.name for p in cwd.iterdir())
    return run([subcommand, *extra, *files], cwd)


ALLOWLIST = ["--allowlist", str(CASES / "c-include-not-allowed" / "allowed-includes.txt")]


class Cases(unittest.TestCase):
    def assert_fails(self, proc, expected):
        self.assertEqual(proc.returncode, 1, proc.stderr)
        self.assertEqual(findings(proc), sorted(expected))

    def assert_passes(self, proc):
        self.assertEqual((proc.returncode, proc.stdout, proc.stderr), (0, "", ""))

    def test_suppression_without_reason(self):
        rule = "c-suppression-without-reason"
        self.assert_fails(
            case(rule, "fail", "grep", *ALLOWLIST),
            [(rule, "suppress.c", n) for n in (1, 3, 8, 11, 14, 18, 19)],
        )
        self.assert_passes(case(rule, "pass", "grep", *ALLOWLIST))

    def test_reinvented_helper(self):
        rule = "c-reinvented-helper"
        self.assert_fails(
            case(rule, "fail", "grep", *ALLOWLIST),
            [(rule, "helper.c", n) for n in (1, 5, 12, 17, 20, 25, 27, 35, 36, 40, 45, 47, 52, 57, 62, 67, 72, 77)],
        )
        self.assert_passes(case(rule, "pass", "grep", *ALLOWLIST))

    def test_reinvented_helper_message_does_not_assume_zephyr(self):
        proc = case("c-reinvented-helper", "fail", "grep", *ALLOWLIST)
        message = json.loads(proc.stdout.splitlines()[0])["message"]
        self.assertEqual(
            message,
            "this re-implements a helper the project or platform already has; use the existing one"
            " (Zephyr: MIN, MAX, CLAMP, ARRAY_SIZE, CONTAINER_OF in <zephyr/sys/util.h>)",
        )

    def test_duplicate_function(self):
        rule = "c-duplicate-function"
        proc = case(rule, "fail", "grep", *ALLOWLIST)
        self.assert_fails(
            proc,
            [(rule, "a.c", 1), (rule, "b.c", 1), (rule, "c.c", 1), (rule, "p.c", 1), (rule, "q.c", 1),
             (rule, "r.c", 1), (rule, "s.c", 1), (rule, "t.c", 1), (rule, "u.c", 1)],
        )
        self.assertEqual(
            [json.loads(line)["message"] for line in proc.stdout.splitlines()],
            [
                "`advance` is also defined in b.c:1, c.c:1; keep one helper and reuse it",
                "`advance` is also defined in a.c:1, c.c:1; keep one helper and reuse it",
                "`advance` is also defined in a.c:1, b.c:1; keep one helper and reuse it",
                "`dup_name` is also defined in q.c:1; keep one helper and reuse it",
                "`dup_name` is also defined in p.c:1; keep one helper and reuse it",
                "`dup1` is also defined in s.c:1; keep one helper and reuse it",
                "`dup1` is also defined in r.c:1; keep one helper and reuse it",
                "`dup2` is also defined in u.c:1; keep one helper and reuse it",
                "`dup2` is also defined in t.c:1; keep one helper and reuse it",
            ],
        )
        self.assert_passes(case(rule, "pass", "grep", *ALLOWLIST))

    def test_include_not_allowed(self):
        rule = "c-include-not-allowed"
        proc = case(rule, "fail", "grep", *ALLOWLIST)
        self.assert_fails(proc, [(rule, "include.c", n) for n in (3, 4, 6)])
        self.assertIn("<cJSON.h> is not in", proc.stdout)
        self.assert_passes(case(rule, "pass", "grep", *ALLOWLIST))

    def test_bugprone(self):
        self.assert_fails(
            case("bugprone", "fail", "tidy"),
            [("bugprone-not-null-terminated-result", "bugprone.c", 5)],
        )
        self.assert_passes(case("bugprone", "pass", "tidy"))

    def test_cert(self):
        self.assert_fails(case("cert", "fail", "tidy"), [("cert-err34-c", "cert.c", 5)])
        self.assert_passes(case("cert", "pass", "tidy"))

    def test_readability(self):
        self.assert_fails(
            case("readability", "fail", "tidy"),
            [("readability-else-after-return", "readability.c", 5)],
        )
        self.assert_passes(case("readability", "pass", "tidy"))

    def test_tidy_exclusions_stay_off(self):
        self.assert_passes(case("exclusions", "pass", "tidy"))

    def test_wall_wextra(self):
        self.assert_fails(
            case("warnings", "fail", "tidy"),
            [
                ("clang-diagnostic-unused-parameter", "warnings.c", 1),
                ("clang-diagnostic-unused-variable", "warnings.c", 3),
            ],
        )
        self.assert_passes(case("warnings", "pass", "tidy"))


class Size(unittest.TestCase):
    def test_file_past_1000_lines_fails_and_at_1000_passes(self):
        with tempfile.TemporaryDirectory() as tmp:
            Path(tmp, "long.c").write_text("int x;\n" * 1001)
            Path(tmp, "edge.c").write_text("int y;\n" * 1000)
            proc = run(["size", "long.c", "edge.c"], tmp)
            self.assertEqual(proc.returncode, 1, proc.stderr)
            self.assertEqual(
                [json.loads(line) for line in proc.stdout.splitlines()],
                [
                    {
                        "tool": "rules-c",
                        "rule": "c-file-too-long",
                        "file": "long.c",
                        "line": 1,
                        "message": "1001 lines, past the 1000-line limit; split it along a seam",
                        "snippet": "file longer than 1000 lines",
                    }
                ],
            )
            self.assertEqual(run(["size", "edge.c"], tmp).returncode, 0)

    def test_unreadable_file_is_an_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            proc = run(["size", "missing.c"], tmp)
            self.assertEqual((proc.returncode, proc.stdout), (2, ""))
            self.assertIn("rules-c error: cannot read missing.c", proc.stderr)


class Wrapper(unittest.TestCase):
    def git_repo(self, tmp):
        env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        subprocess.run(["git", "init", "-q", tmp], check=True, env=env)
        shutil.copytree(PACK, Path(tmp, "rules", "c"), ignore=shutil.ignore_patterns("__pycache__"))
        Path(tmp, "src").mkdir()
        Path(tmp, "src", "long.c").write_text("int x;\n" * 1001)
        Path(tmp, "src", "long.h").write_text("int y;\n" * 1001)
        Path(tmp, "src", "hdr.h").write_text("#include <extra.h>\n")
        Path(tmp, "src", "uses.c").write_text("#include <cJSON.h>\n#include <zephyr/kernel.h>\n")
        subprocess.run(["git", "add", "."], cwd=tmp, check=True, env=env)
        return env

    def test_without_paths_checks_tracked_files_outside_the_pack(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.git_repo(tmp)
            pack = str(Path(tmp, "rules", "c", "rules-c"))
            proc = subprocess.run(
                [sys.executable, pack, "size", "--max", "5"], cwd=tmp, capture_output=True, text=True, env=env
            )
            self.assertEqual(
                findings(proc), [("c-file-too-long", "src/long.c", 1), ("c-file-too-long", "src/long.h", 1)]
            )
            grep = subprocess.run(
                [sys.executable, pack, "grep", "--allowlist", "/dev/null"],
                cwd=tmp,
                capture_output=True,
                text=True,
                env=env,
            )
            self.assertEqual(
                findings(grep),
                [
                    ("c-include-not-allowed", "src/hdr.h", 1),
                    ("c-include-not-allowed", "src/uses.c", 1),
                    ("c-include-not-allowed", "src/uses.c", 2),
                ],
            )
            seed = subprocess.run(
                [sys.executable, pack, "allowlist"], cwd=tmp, capture_output=True, text=True, env=env
            )
            self.assertEqual((seed.returncode, seed.stdout), (0, "cJSON.h\nextra.h\nzephyr/\n"))

    def test_outside_a_git_checkout_without_paths_is_an_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
            env["GIT_CEILING_DIRECTORIES"] = str(Path(tmp).parent)
            proc = run(["size"], tmp, env)
            self.assertEqual((proc.returncode, proc.stdout), (2, ""))
            self.assertIn("rules-c error: cannot list tracked files", proc.stderr)

    def test_from_a_subdirectory_paths_follow_the_working_directory(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.git_repo(tmp)
            pack = str(Path(tmp, "rules", "c", "rules-c"))
            proc = subprocess.run(
                [sys.executable, pack, "size", "--max", "5"],
                cwd=Path(tmp, "src"),
                capture_output=True,
                text=True,
                env=env,
            )
            self.assertEqual(
                findings(proc), [("c-file-too-long", "long.c", 1), ("c-file-too-long", "long.h", 1)]
            )

    def test_a_tracked_file_deleted_from_the_working_tree_is_skipped(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.git_repo(tmp)
            pack = str(Path(tmp, "rules", "c", "rules-c"))
            Path(tmp, "src", "uses.c").unlink()
            for sub in (["size", "--max", "5"], ["grep", "--allowlist", "/dev/null"], ["tidy"]):
                with self.subTest(sub[0]):
                    proc = subprocess.run(
                        [sys.executable, pack, *sub], cwd=tmp, capture_output=True, text=True, env=env
                    )
                    self.assertNotEqual(proc.returncode, 2, proc.stderr)

    def test_a_file_named_like_an_option_is_a_file(self):
        name = "--checks=-*.c"
        with tempfile.TemporaryDirectory() as tmp:
            env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
            subprocess.run(["git", "init", "-q", tmp], check=True, env=env)
            Path(tmp, name).write_text("int scaled(int v, int unused) { return v; }\nint x; // NOLINT\n")
            subprocess.run(["git", "add", "--", name], cwd=tmp, check=True, env=env)
            expected = {
                "tidy": [("clang-diagnostic-unused-parameter", name, 1)],
                "grep": [("c-suppression-without-reason", name, 2)],
                "size": [("c-file-too-long", name, 1)],
            }
            for sub, args in (("tidy", []), ("grep", ["--allowlist", "/dev/null"]), ("size", ["--max", "0"])):
                for paths in ([], [name]):
                    with self.subTest(sub=sub, paths=paths):
                        proc = run([sub, *args, *(["--", *paths] if paths else [])], tmp, env)
                        self.assertEqual(findings(proc), expected[sub], proc.stderr)
                        self.assertEqual(proc.returncode, 1, proc.stderr)
            build = Path(tmp, "build")
            build.mkdir()
            entry = {"directory": tmp, "file": name, "command": f"clang -c ./{name}"}
            (build / "compile_commands.json").write_text(json.dumps([entry]))
            proc = run(["tidy", "-p", "build"], tmp, env)
            self.assertEqual(findings(proc), expected["tidy"], proc.stderr)

    def test_a_file_outside_the_checkout_is_skipped(self):
        with tempfile.TemporaryDirectory() as tmp, tempfile.TemporaryDirectory() as outside:
            outer = Path(outside, "outside.c")
            outer.write_text("int scaled(int v, int unused) { return v; } // NOLINT\n")
            for sub in (["grep", "--allowlist", "/dev/null"], ["size", "--max", "0"], ["tidy"]):
                with self.subTest(sub[0]):
                    proc = run([*sub, str(outer)], tmp)
                    self.assertEqual((proc.returncode, proc.stdout, proc.stderr), (0, "", ""))

    def test_a_clang_tidy_killed_by_a_signal_is_an_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            Path(tmp, "bin").mkdir()
            body = "echo 'a.c:1:1: warning: x [bugprone-foo]'; kill -9 $$"
            Path(tmp, "bin", "clang-tidy").write_text(f"#!/bin/sh\n{body}\n")
            Path(tmp, "bin", "clang-tidy").chmod(0o755)
            Path(tmp, "a.c").write_text("int x;\n")
            env = dict(os.environ, PATH=f"{tmp}/bin:{os.environ['PATH']}")
            proc = run(["tidy", "a.c"], tmp, env)
            self.assertEqual((proc.returncode, proc.stdout), (2, ""))
            self.assertIn("rules-c error: clang-tidy exited -9", proc.stderr)

    def test_a_malformed_compile_database_is_an_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            Path(tmp, "build").mkdir()
            for text in ('[{"file": "a.c"}]', '["a.c"]', '{"a": 1}'):
                with self.subTest(text):
                    Path(tmp, "build", "compile_commands.json").write_text(text)
                    proc = run(["tidy", "-p", "build"], tmp)
                    self.assertEqual((proc.returncode, proc.stdout), (2, ""))
                    self.assertIn("rules-c error: cannot read build/compile_commands.json", proc.stderr)

    def test_without_allowlist_grep_reads_the_default_under_nightshift(self):
        with tempfile.TemporaryDirectory() as tmp:
            Path(tmp, ".nightshift").mkdir()
            Path(tmp, ".nightshift", "rules-c-includes.txt").write_text("errno.h\n")
            Path(tmp, "include.c").write_text("#include <errno.h>\n#include <cJSON.h>\n")
            proc = run(["grep", "include.c"], tmp)
            self.assertEqual(proc.returncode, 1, proc.stderr)
            self.assertEqual(
                [json.loads(line)["message"] for line in proc.stdout.splitlines()],
                [
                    "<cJSON.h> is not in .nightshift/rules-c-includes.txt; reuse an allowed dependency,"
                    " or add it there with the reason it is needed"
                ],
            )

    def test_missing_allowlist_is_an_error(self):
        proc = case("c-include-not-allowed", "pass", "grep", "--allowlist", "nope.txt")
        self.assertEqual((proc.returncode, proc.stdout), (2, ""))
        self.assertIn("rules-c error: cannot read the include allowlist nope.txt", proc.stderr)

    def test_missing_tool_is_an_error(self):
        env = dict(os.environ, PATH="")
        for sub in (["grep", *ALLOWLIST], ["tidy"]):
            with self.subTest(sub[0]):
                proc = run([*sub, "include.c"], CASES / "c-include-not-allowed" / "pass", env)
                self.assertEqual((proc.returncode, proc.stdout), (2, ""))
                self.assertIn("is not on PATH", proc.stderr)

    def test_a_compile_error_under_a_warning_flag_is_an_error_not_a_finding(self):
        with tempfile.TemporaryDirectory() as tmp:
            Path(tmp, "implicit.c").write_text("int f(void) { return g(); }\nint h(void) { int dead; return 0; }\n")
            proc = run(["tidy", "implicit.c"], tmp)
            self.assertEqual((proc.returncode, proc.stdout), (2, ""))
            self.assertIn("rules-c error: clang could not compile", proc.stderr)
            self.assertIn("[clang-diagnostic-implicit-function-declaration]", proc.stderr)

    def test_code_clang_cannot_compile_is_an_error_not_a_finding(self):
        with tempfile.TemporaryDirectory() as tmp:
            Path(tmp, "broken.c").write_text("#include <no_such_header.h>\nint x\n")
            proc = run(["tidy", "broken.c"], tmp)
            self.assertEqual((proc.returncode, proc.stdout), (2, ""))
            self.assertIn("rules-c error: clang could not compile", proc.stderr)
            self.assertEqual(len(proc.stderr.splitlines()), 1)

    def test_compile_database_files_inside_the_checkout_are_checked_once(self):
        with tempfile.TemporaryDirectory() as tmp, tempfile.TemporaryDirectory() as outside:
            Path(outside, "theirs.c").write_text("#include <no_such_header.h>\n")
            Path(outside, "ext.h").write_text("static inline int ext(int unused) { return 0; }\n")
            Path(tmp, "shared.h").write_text("static inline int twice(int v, int unused) { return v * 2; }\n")
            for name in ("one.c", "two.c"):
                Path(tmp, name).write_text(f'#include "shared.h"\n#include "{outside}/ext.h"\n')
            build = Path(tmp, "build")
            build.mkdir()
            entries = [
                {"directory": tmp, "file": name, "command": f"clang -c {name}"} for name in ("one.c", "two.c")
            ] + [{"directory": outside, "file": "theirs.c", "command": "clang -c theirs.c"}]
            entries.append(dict(entries[0]))
            (build / "compile_commands.json").write_text(json.dumps(entries))
            proc = run(["tidy", "-p", "build"], tmp)
            self.assertEqual(findings(proc), [("clang-diagnostic-unused-parameter", "shared.h", 1)])
            Path(tmp, "two.c").write_text("int two(void) { int dead; return 0; }\n")
            proc = run(["tidy", "-p", "build", "one.c"], tmp)
            self.assertEqual(findings(proc), [("clang-diagnostic-unused-parameter", "shared.h", 1)])

    def test_compile_database_lists_each_file_once(self):
        loader = importlib.machinery.SourceFileLoader("rules_c", RULES_C)
        module = types.ModuleType("rules_c")
        module.__file__ = RULES_C
        loader.exec_module(module)
        with tempfile.TemporaryDirectory() as tmp:
            Path(tmp, "build").mkdir()
            entries = [{"directory": tmp, "file": "a.c", "command": "clang -c a.c"}] * 2
            Path(tmp, "build", "compile_commands.json").write_text(json.dumps(entries))
            cwd = os.getcwd()
            os.chdir(tmp)
            try:
                self.assertEqual(module.compile_db_files("build"), ["a.c"])
            finally:
                os.chdir(cwd)

    def test_werror_in_the_build_still_reports_warnings_as_findings(self):
        with tempfile.TemporaryDirectory() as tmp:
            Path(tmp, "dead.c").write_text("int two(void) { int dead; return 0; }\n")
            Path(tmp, "build").mkdir()
            entry = {"directory": tmp, "file": "dead.c", "command": "clang -Werror -c dead.c"}
            Path(tmp, "build", "compile_commands.json").write_text(json.dumps([entry]))
            proc = run(["tidy", "-p", "build"], tmp)
            self.assertEqual(proc.returncode, 1, proc.stderr)
            self.assertEqual(findings(proc), [("clang-diagnostic-unused-variable", "dead.c", 1)])

    def test_aliased_checks_are_one_finding(self):
        with tempfile.TemporaryDirectory() as tmp:
            Path(tmp, "reserved.c").write_text("int _Reserved;\n")
            proc = run(["tidy", "reserved.c"], tmp)
            self.assertEqual(
                findings(proc),
                [("bugprone-reserved-identifier,cert-dcl37-c,cert-dcl51-cpp", "reserved.c", 1)],
            )

    def test_a_file_clang_tidy_cannot_open_is_an_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            Path(tmp, "ok.c").write_text("int scaled(int v, int unused) { return v; }\n")
            proc = run(["tidy", "ok.c", "missing.c"], tmp)
            self.assertEqual((proc.returncode, proc.stdout), (2, ""))
            self.assertIn("no such file or directory", proc.stderr)

    def test_a_tool_that_fails_without_findings_is_an_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            Path(tmp, "bin").mkdir()
            for tool, body in (("ast-grep", "exit 3"), ("clang-tidy", "echo 'Error: bad config' >&2; exit 1")):
                Path(tmp, "bin", tool).write_text(f"#!/bin/sh\n{body}\n")
                Path(tmp, "bin", tool).chmod(0o755)
            env = dict(os.environ, PATH=f"{tmp}/bin:{os.environ['PATH']}")
            for sub, exited in ((["grep", *ALLOWLIST], "ast-grep exited 3"), (["tidy"], "clang-tidy exited 1")):
                with self.subTest(sub[0]):
                    proc = run([*sub, "include.c"], CASES / "c-include-not-allowed" / "pass", env)
                    self.assertEqual((proc.returncode, proc.stdout), (2, ""))
                    self.assertIn(f"rules-c error: {exited}", proc.stderr)
                    self.assertEqual(len(proc.stderr.splitlines()), 1)

    def test_ast_grep_exiting_zero_with_stderr_output_is_an_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            Path(tmp, "bin").mkdir()
            Path(tmp, "bin", "ast-grep").write_text("#!/bin/sh\necho 'partial failure' >&2\nexit 0\n")
            Path(tmp, "bin", "ast-grep").chmod(0o755)
            env = dict(os.environ, PATH=f"{tmp}/bin:{os.environ['PATH']}")
            proc = run(["grep", *ALLOWLIST, "include.c"], CASES / "c-include-not-allowed" / "pass", env)
            self.assertEqual((proc.returncode, proc.stdout), (2, ""))
            self.assertIn("rules-c error: ast-grep exited 0: partial failure", proc.stderr)

    def test_a_path_ast_grep_cannot_read_is_an_error(self):
        proc = case("c-include-not-allowed", "pass", "grep", *ALLOWLIST, "missing.c")
        self.assertEqual((proc.returncode, proc.stdout), (2, ""))
        self.assertIn("missing.c: No such file or directory", proc.stderr)

    def test_a_checkout_with_no_c_files_has_no_findings(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
            subprocess.run(["git", "init", "-q", tmp], check=True, env=env)
            Path(tmp, "untracked.c").write_text("int scaled(int v, int unused) { return v; } // NOLINT\n")
            for sub in (["grep", "--allowlist", "/dev/null"], ["tidy"], ["size", "--max", "0"]):
                with self.subTest(sub[0]):
                    proc = run(sub, tmp, env)
                    self.assertEqual((proc.returncode, proc.stdout, proc.stderr), (0, "", ""))

    def test_missing_compile_database_is_an_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            proc = run(["tidy", "-p", "build"], tmp)
            self.assertEqual((proc.returncode, proc.stdout), (2, ""))
            self.assertIn("rules-c error: cannot read build/compile_commands.json", proc.stderr)


if __name__ == "__main__":
    unittest.main()
