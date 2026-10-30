#!/usr/bin/env python3
"""Tests of ci.py, the fuzzing helper of .github/workflows/fuzz.yml. Run them with `python3 -I -m unittest discover --start-directory fuzz --pattern 'test_*.py'`; CI runs them before it fuzzes. They need no fuzz build: fake targets stand in for libFuzzer."""

from __future__ import annotations

import base64
import contextlib
import hashlib
import io
import os
import shutil
import stat
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import ci

# The input that the fake targets below fail on, or run slowly on. It holds a workflow command, which must never reach the job log as one, and like every input that fails it must not reach the job log or summary at all.
INPUT = "::error::quoted from an input"

# A stand-in for a fuzz target built with libFuzzer: it prints a status line and final statistics as libFuzzer does, and the folder that holds it says how it behaves. Named crash, timeout, oom or leak, it fails as libFuzzer does: it prints libFuzzer's report and the input (in hex, as text and in Base64), keeps the input in a file named by what went wrong, and exits with libFuzzer's code; named broken, it exits with an error without keeping an input. Otherwise it passes, after keeping an input that ran slowly, which libFuzzer also prints in Base64. In merge mode it copies the corpus into the output folder and prints libFuzzer's summary; named sanitizer or signal, a merge prints a sanitizer's or libFuzzer's report and keeps the input that caused it, named report-only it prints a report without keeping an input, and named silent it prints no summary, and these four copy nothing, so that a test sees whether the corpus was replaced by what a failed merge left.
FAKE_TARGET = """#!/bin/sh
mode="$(basename "$(dirname "$0")")"
prefix=
merge=
for argument in "$@"; do
  case "$argument" in
    -artifact_prefix=*) prefix="${argument#-artifact_prefix=}" ;;
    -merge=1) merge=yes ;;
  esac
done
keep() {
  printf '%s' '@INPUT@' > "${prefix}$1"
  echo "artifact_prefix='$prefix'; Test unit written to ${prefix}$1"
  echo "Base64: @BASE64@"
}
dump() {
  echo "MS: 1 ChangeByte-; base unit: adc83b19e793491b1c6ea0fd8b46cd9f32e592fc"
  echo "@HEX@"
  echo '@INPUT@'
  keep "$1"
}
if [ -n "$merge" ]; then
  output="$(eval echo "\\${$(($# - 1))}")"
  input="$(eval echo "\\${$#}")"
  case "$mode" in
    sanitizer) echo "==1==ERROR: AddressSanitizer: heap-use-after-free"; dump crash-89ab ;;
    signal) echo "==1== ERROR: libFuzzer: deadly signal"; dump crash-89ab ;;
    report-only) echo "==1== ERROR: libFuzzer: deadly signal" ;;
    silent) ;;
    *) cp "$input"/* "$output"/ ;;
  esac
  case "$mode" in
    silent) ;;
    *) echo "MERGE-OUTER: $(ls "$output" | wc -l) new files with 34 new features added; 12 new coverage edges" ;;
  esac
  exit 0
fi
echo "INFO: Seed: 1"
echo "#2	INITED cov: 10 ft: 30 corp: 1/10b exec/s: 0 rss: 30Mb"
case "$mode" in
  crash)
    echo "thread '<unnamed>' panicked at fuzz_targets/zip_archive.rs:9:5:"
    echo 'a message that quotes the input: @INPUT@'
    echo "==1== ERROR: libFuzzer: deadly signal"
    echo "    #0 0x55555555 in __sanitizer_print_stack_trace"
    dump crash-0123
    exit 77
    ;;
  timeout)
    echo "ALARM: working on the last Unit for 10 seconds"
    dump timeout-0123
    echo "==1== ERROR: libFuzzer: timeout after 10 seconds"
    exit 70
    ;;
  oom)
    echo "==1== ERROR: libFuzzer: out-of-memory (used: 2100Mb; limit: 2048Mb)"
    dump oom-0123
    exit 71
    ;;
  leak)
    echo "==1==ERROR: LeakSanitizer: detected memory leaks"
    dump leak-0123
    exit 77
    ;;
  broken)
    echo "ParseDictionaryFile: file does not exist or is empty"
    exit 1
    ;;
esac
echo "Slowest unit: 12 s:"
keep slow-unit-0123
echo "#100	DONE   cov: 12 ft: 34 corp: 5/1Kb lim: 4 exec/s: 50 rss: 31Mb"
echo "stat::number_of_executed_units: 100"
echo "stat::average_exec_per_sec:     50"
echo "stat::peak_rss_mb:              31"
""".replace("@INPUT@", INPUT).replace("@HEX@", ",".join(f"0x{byte:02x}" for byte in INPUT.encode())).replace("@BASE64@", base64.b64encode(INPUT.encode()).decode())

# The kinds of failure of the fakes, by the name of their folder.
KINDS = {"crash": "crash", "timeout": "timeout", "oom": "out of memory", "leak": "leak"}


class Isolated(unittest.TestCase):
    """Runs each test outside GitHub Actions, so that ci.py prints no workflow commands and writes no job summary, and keeps what it prints out of the test output."""

    def setUp(self) -> None:
        environment = mock.patch.dict(os.environ)
        environment.start()
        self.addCleanup(environment.stop)
        os.environ.pop("GITHUB_ACTIONS", None)
        os.environ.pop("GITHUB_STEP_SUMMARY", None)
        self.output = io.StringIO()
        redirect = contextlib.redirect_stdout(self.output)
        redirect.__enter__()
        self.addCleanup(redirect.__exit__, None, None, None)
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.folder = Path(folder.name)

    def fake_targets(self, names: list[str]) -> Path:
        binaries = self.folder / "binaries"
        binaries.mkdir(exist_ok=True)
        for name in names:
            path = binaries / name
            path.write_text(FAKE_TARGET)
            path.chmod(path.stat().st_mode | stat.S_IXUSR)
        return binaries

    def fake_named(self, word: str) -> Path:
        """A folder whose fake zip_archive behaves as `word` says (see FAKE_TARGET)."""
        folder = self.folder / word
        folder.mkdir()
        (folder / "zip_archive").symlink_to(self.fake_targets(["zip_archive"]) / "zip_archive")
        return folder

    def assert_shows_nothing_of(self, logs: list[Path], *shown: str) -> None:
        """Checks that no line of the logs, and nothing of the input that the fakes fail on, is in what ci.py showed."""
        lines = [line for log in logs for line in log.read_text().splitlines() if line.strip()]
        self.assertTrue(lines)
        # The input as text, in Base64 and in hex, and the stand-ins for the SHA-1s in the names of the inputs kept (a temporary folder's name never holds a dash).
        secrets = [INPUT, base64.b64encode(INPUT.encode()).decode(), "0x3a,0x3a,0x65", "-0123", "-89ab"]
        for text in shown:
            for secret in lines + secrets:
                self.assertNotIn(secret, text)


class Targets(Isolated):
    def test_every_fuzz_target_has_settings_and_every_setting_a_target(self) -> None:
        self.assertEqual(ci.check_targets(ci.fuzz_targets_in_manifest()), [])

    def test_names_each_difference_between_the_lists(self) -> None:
        problems = ci.check_targets([*ci.TARGETS, "new_target"][1:])
        self.assertEqual(len(problems), 2)
        self.assertIn("new_target is a fuzz target in fuzz/Cargo.toml but has no settings", problems[0])
        self.assertIn(f"{next(iter(ci.TARGETS))} has settings in TARGETS of fuzz/ci.py but is no fuzz target", problems[1])

    def test_every_dictionary_exists(self) -> None:
        for name, target in ci.TARGETS.items():
            if target.dictionary:
                self.assertTrue((ci.FUZZ / "dictionaries" / target.dictionary).is_file(), name)

    def test_the_default_limits_targets_get_long_inputs_and_more_time(self) -> None:
        for name in ("zip_archive_default_limits", "package_default_limits"):
            target = ci.TARGETS[name]
            self.assertEqual((target.max_len, target.long_inputs, target.share), (1 << 20, True, 2), name)
        self.assertEqual({target.max_len for name, target in ci.TARGETS.items() if not name.endswith("_default_limits")}, {64 << 10})


class Seeds(Isolated):
    def test_writes_the_public_seeds_of_every_target(self) -> None:
        counts = ci.seeds(self.folder)
        documents = len(list(ci.DOCUMENTS.glob("*.docx")))
        self.assertGreater(documents, 0)
        for name in ("zip_archive", "package", "zip_archive_default_limits", "package_default_limits"):
            self.assertEqual(counts[name], documents, name)
        self.assertEqual(counts["content_types"], documents)
        self.assertEqual(counts["core_properties"], documents)
        self.assertGreaterEqual(counts["relationships"], documents)
        self.assertEqual(counts["compound_file"], 0)
        self.assertEqual(counts["markup_compatibility"], counts["xml_reader"] * len(ci.MC_CONFIGURATIONS))
        for name, target in ci.TARGETS.items():
            for seed in (self.folder / name).iterdir():
                data = seed.read_bytes()
                self.assertEqual(seed.name, hashlib.sha1(data).hexdigest())
                self.assertLessEqual(len(data), target.max_len, f"{name}/{seed.name}")
        for seed in (self.folder / "markup_compatibility").iterdir():
            self.assertIn(seed.read_bytes()[0], ci.MC_CONFIGURATIONS)
        for name in ("content_types", "relationships", "core_properties", "xml_reader"):
            for seed in (self.folder / name).iterdir():
                self.assertTrue(seed.read_bytes().lstrip(b"\xef\xbb\xbf").startswith(b"<"), f"{name}/{seed.name}")

    def test_writes_the_same_seeds_again(self) -> None:
        first = ci.seeds(self.folder)
        self.assertEqual(ci.seeds(self.folder), first)


class Commands(Isolated):
    def test_fuzzes_with_the_settings_of_the_target(self) -> None:
        binaries = self.fake_targets(["package_default_limits", "compound_file"])
        command = ci.fuzz_command("package_default_limits", 100, self.folder / "seeds", self.folder, binaries)
        self.assertEqual(command[0], str(binaries / "package_default_limits"))
        for option in ("-max_total_time=200", "-max_len=1048576", "-timeout=60", "-rss_limit_mb=2048", "-print_final_stats=1", "-len_control=0", f"-dict={ci.FUZZ / 'dictionaries' / 'zip.dict'}"):
            self.assertIn(option, command)
        self.assertIn(f"-artifact_prefix={self.folder / 'artifacts' / 'package_default_limits'}/", command)
        # New inputs go to the first folder, the corpus; the seeds come second.
        self.assertEqual(command[-2:], [str(self.folder / "corpus" / "package_default_limits"), str(self.folder / "seeds" / "package_default_limits")])
        command = ci.fuzz_command("compound_file", 100, self.folder / "seeds", self.folder, binaries)
        self.assertIn("-max_total_time=100", command)
        self.assertFalse([option for option in command if option.startswith(("-dict=", "-len_control="))])

    def test_refuses_to_run_a_target_that_was_not_built(self) -> None:
        with self.assertRaises(SystemExit):
            ci.fuzz_command("package", 100, self.folder, self.folder, self.folder)

    def test_keeps_the_callers_options_for_address_sanitizer(self) -> None:
        self.assertEqual(ci.environment()["ASAN_OPTIONS"], "detect_odr_violation=0")
        os.environ["ASAN_OPTIONS"] = "detect_leaks=0"
        self.assertEqual(ci.environment()["ASAN_OPTIONS"], "detect_odr_violation=0:detect_leaks=0")


class Logs(Isolated):
    def test_reads_the_last_status_line_and_the_final_statistics(self) -> None:
        log = (
            "INFO: Seed: 1\n#2\tINITED cov: 10 ft: 30 corp: 1/10b exec/s: 0 rss: 30Mb\n"
            "#64\tNEW    cov: 11 ft: 32 corp: 2/20b lim: 4 exec/s: 0 rss: 30Mb L: 10/10 MS: 1 ChangeBit-\n"
            "#100\tDONE   cov: 12 ft: 34 corp: 5/1Kb lim: 4 exec/s: 50 rss: 31Mb\n"
            "stat::number_of_executed_units: 100\nstat::peak_rss_mb:              31\n"
        )
        self.assertEqual(ci.parse_log(log), {"runs": 100, "edges": 12, "features": 34, "inputs": 5, "number_of_executed_units": 100, "peak_rss_mb": 31})
        self.assertEqual(ci.last_corpus_size(log), "5/1Kb")
        self.assertEqual(ci.parse_log("ERROR: libFuzzer: deadly signal\n"), {})
        # Only a size as libFuzzer writes it can reach the summary.
        self.assertEqual(ci.last_corpus_size("#3\tNEW    cov: 11 ft: 32 corp: 2/::error::x lim: 4\n"), "")

    def test_failures_are_crashes_timeouts_out_of_memory_and_leaks(self) -> None:
        for name in ("crash-1", "timeout-2", "oom-3", "leak-4", "slow-unit-5", "other"):
            (self.folder / name).write_bytes(b"")
        self.assertEqual(ci.failures(self.folder), ["crash-1", "leak-4", "oom-3", "timeout-2"])
        self.assertEqual(ci.failures(self.folder / "missing"), [])

    def test_the_kind_of_a_failure_comes_from_the_name_libfuzzer_gave_its_input(self) -> None:
        self.assertEqual(ci.kinds(["timeout-2", "crash-1"]), ["crash", "timeout"])
        self.assertEqual(ci.kinds(["oom-3", "leak-4", "leak-5"]), ["out of memory", "leak"])
        self.assertEqual(ci.kinds([]), [])


class Runs(Isolated):
    def test_passes_when_every_target_passes(self) -> None:
        binaries = self.fake_targets(["zip_archive", "package"])
        results = ci.run(["package", "zip_archive"], 600, 2, self.folder / "seeds", self.folder / "work", binaries)
        self.assertEqual([(result.target, result.status, result.problems) for result in results], [("zip_archive", "passed", []), ("package", "passed", [])])
        self.assertEqual(results[0].stats["edges"], 12)
        self.assertEqual(results[0].corpus, "5/1Kb")
        # It reports how long the target ran, not the time it was given.
        self.assertLess(results[0].seconds, 600)
        table = ci.run_table(results)
        self.assertIn(f"| `zip_archive` | passed | {results[0].seconds} s | 100 | 50 | 12 / 34 | 5/1Kb | 31 MB | 64 KiB |", table)
        # libFuzzer also prints an input that ran slowly, so a passing target's log is not shown either.
        self.assertIn("zip_archive: passed after", self.output.getvalue())
        self.assert_shows_nothing_of([self.folder / "work" / "logs" / "zip_archive.log"], self.output.getvalue(), table)

    def test_a_failure_shows_only_its_kind_and_keeps_its_input(self) -> None:
        for word, kind in KINDS.items():
            work = self.folder / f"work-{word}"
            results = ci.run(["zip_archive"], 1, 1, self.folder / "seeds", work, self.fake_named(word))
            self.assertEqual((results[0].status, results[0].problems), ("failed", [kind]), word)
            self.assertEqual(ci.failures(work / "artifacts" / "zip_archive"), [f"{word}-0123"])
            self.assertIn(f"zip_archive: failed: {kind}\n", self.output.getvalue())
            # The row names the target and the kind, and nothing else.
            self.assertEqual(ci.run_table(results).splitlines()[-1], f"| `zip_archive` | failed: {kind} | | | | | | | |")
            self.assert_shows_nothing_of([work / "logs" / "zip_archive.log"], self.output.getvalue(), ci.run_table(results))
        with mock.patch.object(ci, "run", return_value=results):
            self.assertEqual(ci.main(["run", "zip_archive", "--seconds", "1", "--seeds", str(self.folder)]), 1)

    def test_a_target_that_fails_without_keeping_an_input_shows_its_exit_code(self) -> None:
        results = ci.run(["zip_archive"], 1, 1, self.folder / "seeds", self.folder / "work", self.fake_named("broken"))
        self.assertEqual((results[0].status, results[0].problems), ("failed", ["libFuzzer exited with 1 and kept no input"]))
        self.assert_shows_nothing_of([self.folder / "work" / "logs" / "zip_archive.log"], self.output.getvalue(), ci.run_table(results))

    def test_minimizes_each_corpus_in_place(self) -> None:
        binaries = self.fake_targets(["zip_archive", "package"])
        corpus = self.folder / "corpus" / "zip_archive"
        corpus.mkdir(parents=True)
        for index in range(3):
            (corpus / f"input-{index}").write_bytes(bytes([index]))
        # An input that failed in the fuzzing before is not the merge's failure.
        (self.folder / "artifacts" / "zip_archive").mkdir(parents=True)
        (self.folder / "artifacts" / "zip_archive" / "crash-4567").write_bytes(b"x")
        results = ci.minimize(["zip_archive", "package"], 2, self.folder, binaries)
        self.assertEqual([(result.target, result.status) for result in results], [("zip_archive", "passed"), ("package", "empty")])
        self.assertEqual(results[0].stats, {"before": 3, "after": 3, "features": 34, "edges": 12})
        self.assertEqual(sorted(path.name for path in corpus.iterdir()), ["input-0", "input-1", "input-2"])
        self.assertFalse((self.folder / "minimized" / "zip_archive").exists())
        self.assertIn("| `zip_archive` | passed | 3 | 3 | 12 / 34 |", ci.minimize_table(results))

    def test_minimizing_fails_on_a_report_and_keeps_the_corpus(self) -> None:
        corpus = self.folder / "corpus" / "zip_archive"
        corpus.mkdir(parents=True)
        (corpus / "input").write_bytes(b"x")
        # The merge finishes and prints its summary, but a report in its log means that an input crashed the process that merging restarted. Only the kind of failure is shown, or, if the merge kept no input, that it reported one.
        for word, problem in (("sanitizer", "crash"), ("signal", "crash"), ("report-only", "merging reported a failure")):
            results = ci.minimize(["zip_archive"], 1, self.folder, self.fake_named(word))
            self.assertEqual((results[0].status, results[0].problems), ("failed", [problem]), word)
            self.assertEqual([path.name for path in corpus.iterdir()], ["input"], word)
            self.assertEqual(ci.minimize_table(results).splitlines()[-1], f"| `zip_archive` | failed: {problem} | | | |", word)
            self.assertIn(f"zip_archive: minimizing failed: {problem}\n", self.output.getvalue())
            self.assert_shows_nothing_of([self.folder / "logs" / "zip_archive-minimize.log"], self.output.getvalue(), ci.minimize_table(results))
            shutil.rmtree(self.folder / "artifacts")

    def test_minimizing_fails_without_the_merge_summary_and_keeps_the_corpus(self) -> None:
        corpus = self.folder / "corpus" / "zip_archive"
        corpus.mkdir(parents=True)
        (corpus / "input").write_bytes(b"x")
        results = ci.minimize(["zip_archive"], 1, self.folder, self.fake_named("silent"))
        self.assertEqual((results[0].status, results[0].problems), ("failed", ["merging printed no MERGE-OUTER summary"]))
        self.assertEqual([path.name for path in corpus.iterdir()], ["input"])


class Output(Isolated):
    def test_in_github_actions_a_failure_shows_only_the_target_and_the_kind(self) -> None:
        os.environ["GITHUB_ACTIONS"] = "true"
        summary = self.folder / "summary.md"
        os.environ["GITHUB_STEP_SUMMARY"] = str(summary)
        work = self.folder / "work"
        self.assertEqual(ci.main(["run", "zip_archive", "--seconds", "1", "--seeds", str(self.folder / "seeds"), "--work", str(work), "--binaries", str(self.fake_named("crash"))]), 1)
        corpus = work / "corpus" / "zip_archive"
        (corpus / "input").write_bytes(b"x")
        self.assertEqual(ci.main(["minimize", "zip_archive", "--work", str(work), "--binaries", str(self.fake_named("signal"))]), 1)
        log, shown = self.output.getvalue(), summary.read_text()
        self.assert_shows_nothing_of([work / "logs" / "zip_archive.log", work / "logs" / "zip_archive-minimize.log"], log, shown)
        # The only workflow commands are ci.py's annotations, and none comes from a target's output.
        self.assertEqual([line for line in log.splitlines() if line.startswith("::")], ["::error title=Fuzzing zip_archive::crash"] * 2)
        self.assertIn("zip_archive: failed: crash\n", log)
        self.assertIn("zip_archive: minimizing failed: crash\n", log)
        self.assertNotIn(str(work), log)
        self.assertIn("| `zip_archive` | failed: crash | | | | | | | |", shown)
        self.assertIn("| `zip_archive` | failed: crash | | | |", shown)
        self.assertEqual(shown.count(ci.KEPT_PRIVATE), 2)

    def test_in_github_actions_a_passing_run_adds_no_note(self) -> None:
        os.environ["GITHUB_ACTIONS"] = "true"
        summary = self.folder / "summary.md"
        os.environ["GITHUB_STEP_SUMMARY"] = str(summary)
        self.assertEqual(ci.main(["run", "zip_archive", "--seconds", "1", "--seeds", str(self.folder / "seeds"), "--work", str(self.folder / "work"), "--binaries", str(self.fake_targets(["zip_archive"]))]), 0)
        self.assertNotIn("::", self.output.getvalue())
        self.assertNotIn(ci.KEPT_PRIVATE, summary.read_text())
        self.assertIn("| `zip_archive` | passed |", summary.read_text())

    def test_elsewhere_a_failure_also_says_where_its_log_and_inputs_are(self) -> None:
        work = self.folder / "work"
        self.assertEqual(ci.main(["run", "zip_archive", "--seconds", "1", "--seeds", str(self.folder / "seeds"), "--work", str(work), "--binaries", str(self.fake_named("timeout"))]), 1)
        log = self.output.getvalue()
        self.assertIn(f"zip_archive: failed: timeout\n  the log is {work / 'logs' / 'zip_archive.log'}, the inputs that failed are in {work / 'artifacts' / 'zip_archive'}/\n", log)
        self.assertNotIn("::", log)
        self.assertNotIn(ci.KEPT_PRIVATE, log)
        self.assert_shows_nothing_of([work / "logs" / "zip_archive.log"], log)


if __name__ == "__main__":
    unittest.main()
