#!/usr/bin/env python3
"""Tests of ci.py, the fuzzing helper of .github/workflows/fuzz.yml. Run them with `python3 -I -m unittest discover --start-directory fuzz --pattern 'test_*.py'`; CI runs them before it fuzzes. They need no fuzz build: fake targets stand in for libFuzzer."""

from __future__ import annotations

import contextlib
import hashlib
import io
import os
import stat
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import ci

# A stand-in for a fuzz target built with libFuzzer: it prints a status line and final statistics as libFuzzer does, keeps a crash input and exits with libFuzzer's error code when its name says so, and in merge mode copies the corpus into the output folder.
FAKE_TARGET = """#!/bin/sh
prefix=
merge=
for argument in "$@"; do
  case "$argument" in
    -artifact_prefix=*) prefix="${argument#-artifact_prefix=}" ;;
    -merge=1) merge=yes ;;
  esac
done
if [ -n "$merge" ]; then
  output="$(eval echo "\\${$(($# - 1))}")"
  input="$(eval echo "\\${$#}")"
  cp "$input"/* "$output"/
  echo "MERGE-OUTER: $(ls "$output" | wc -l) new files with 34 new features added; 12 new coverage edges"
  exit 0
fi
echo "INFO: Seed: 1"
echo "#2	INITED cov: 10 ft: 30 corp: 1/10b exec/s: 0 rss: 30Mb"
case "$0" in
  *crash*)
    printf 'x' > "${prefix}crash-0123"
    echo "==1==ERROR: libFuzzer: deadly signal"
    exit 77
    ;;
esac
echo "#100	DONE   cov: 12 ft: 34 corp: 5/1Kb lim: 4 exec/s: 50 rss: 31Mb"
echo "stat::number_of_executed_units: 100"
echo "stat::average_exec_per_sec:     50"
echo "stat::peak_rss_mb:              31"
"""


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

    def test_failures_are_crashes_timeouts_out_of_memory_and_leaks(self) -> None:
        for name in ("crash-1", "timeout-2", "oom-3", "leak-4", "slow-unit-5", "other"):
            (self.folder / name).write_bytes(b"")
        self.assertEqual(ci.failures(self.folder), ["crash-1", "leak-4", "oom-3", "timeout-2"])
        self.assertEqual(ci.failures(self.folder / "missing"), [])


class Runs(Isolated):
    def test_passes_when_every_target_passes(self) -> None:
        binaries = self.fake_targets(["zip_archive", "package"])
        results = ci.run(["package", "zip_archive"], 1, 2, self.folder / "seeds", self.folder / "work", binaries)
        self.assertEqual([(result.target, result.status, result.problems) for result in results], [("zip_archive", "passed", []), ("package", "passed", [])])
        self.assertEqual(results[0].stats["edges"], 12)
        self.assertEqual(results[0].corpus, "5/1Kb")
        table = ci.run_table(results)
        self.assertIn("| `zip_archive` | passed | 1 s | 100 | 50 | 12 / 34 | 5/1Kb | 31 MB | 64 KiB |", table)

    def test_fails_when_a_target_crashes_and_keeps_its_input(self) -> None:
        binaries = self.fake_targets(["zip_archive"])
        # The fake crashes when its path names a crash; the target itself is still zip_archive.
        crashing = self.folder / "crash"
        crashing.mkdir()
        (crashing / "zip_archive").symlink_to(binaries / "zip_archive")
        results = ci.run(["zip_archive"], 1, 1, self.folder / "seeds", self.folder / "work", crashing)
        self.assertEqual(results[0].status, "failed")
        self.assertEqual(results[0].problems, ["libFuzzer exited with 77", "kept artifacts/zip_archive/crash-0123"])
        self.assertTrue((self.folder / "work" / "artifacts" / "zip_archive" / "crash-0123").is_file())
        self.assertIn("ERROR: libFuzzer: deadly signal", self.output.getvalue())
        with mock.patch.object(ci, "run", return_value=results):
            self.assertEqual(ci.main(["run", "zip_archive", "--seconds", "1", "--seeds", str(self.folder)]), 1)

    def test_minimizes_each_corpus_in_place(self) -> None:
        binaries = self.fake_targets(["zip_archive", "package"])
        corpus = self.folder / "corpus" / "zip_archive"
        corpus.mkdir(parents=True)
        for index in range(3):
            (corpus / f"input-{index}").write_bytes(bytes([index]))
        results = ci.minimize(["zip_archive", "package"], 2, self.folder, binaries)
        self.assertEqual([(result.target, result.status) for result in results], [("zip_archive", "passed"), ("package", "empty")])
        self.assertEqual(results[0].stats, {"before": 3, "after": 3, "features": 34, "edges": 12})
        self.assertEqual(sorted(path.name for path in corpus.iterdir()), ["input-0", "input-1", "input-2"])
        self.assertFalse((self.folder / "minimized" / "zip_archive").exists())
        self.assertIn("| `zip_archive` | passed | 3 | 3 | 12 / 34 |", ci.minimize_table(results))


if __name__ == "__main__":
    unittest.main()
