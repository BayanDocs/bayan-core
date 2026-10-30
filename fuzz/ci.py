#!/usr/bin/env python3
"""Builds the seed corpora of the fuzz targets, runs the targets and minimizes their corpora, as .github/workflows/fuzz.yml does in CI (bayan-core#21, follow-up 1 of CORE-005). README.md in this folder explains the targets, their settings and how to reproduce a failure.

Usage, from anywhere in the repository, after `cargo +nightly-2026-10-02 fuzz build --target x86_64-unknown-linux-gnu`:

  python3 -I fuzz/ci.py seeds SEEDS
      Writes the seeds of every target into SEEDS/<target>/, from public files only: the five python-docx documents that bayan-opc's tests keep, the metadata parts and the XML parts inside them, and bayan-xml's two sample parts. Never add documents of the private corpus: CI saves the corpora that grow from the seeds in the Actions cache, which the workflows of pull requests can read.
  python3 -I fuzz/ci.py run --seconds N --seeds SEEDS [--work WORK] [--jobs J] (--all | TARGET ...)
      Runs each target for N seconds times its share (TARGETS below), J at a time (default 4), from its seeds and its corpus in WORK/corpus/<target>/, where it adds what it finds. WORK is this folder unless given, so that the corpora and artifacts are where cargo-fuzz keeps them. Inputs that crash, time out, run out of memory or leak go to WORK/artifacts/<target>/, each target's output to WORK/logs/<target>.log. Exits with 1 if any target failed.
  python3 -I fuzz/ci.py minimize [--work WORK] [--jobs J] (--all | TARGET ...)
      Keeps only the inputs of each corpus that its coverage needs (libFuzzer's -merge=1), and reports that coverage.

Both run and minimize print a Markdown table, and add it to the job summary when GITHUB_STEP_SUMMARY is set. Neither prints anything of a target's output, in which libFuzzer writes the input that failed: in GitHub Actions the job log and the summary are public, and such an input can disclose a vulnerability (README.md, "When a run fails"). Of a target that failed, they show only the kind of failure (crash, timeout, out of memory or leak), in GitHub Actions also as an annotation, and elsewhere also where its log and inputs are. Needs Python 3.11 or later (for tomllib) and nothing outside its standard library. It runs the targets that cargo-fuzz built for x86_64-unknown-linux-gnu, so Linux on x86-64 only. The tests are in test_ci.py next to this file.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import dataclasses
import hashlib
import os
import re
import shutil
import subprocess
import sys
import time
import tomllib
import zipfile
from pathlib import Path
from typing import Dict, List, Optional, Sequence, Set, Tuple

FUZZ = Path(__file__).resolve().parent
REPOSITORY = FUZZ.parent
DOCUMENTS = REPOSITORY / "crates" / "bayan-opc" / "tests" / "fixtures" / "python-docx"
XML_SAMPLES = REPOSITORY / "crates" / "bayan-xml" / "tests" / "fixtures"
BINARIES = FUZZ / "target" / "x86_64-unknown-linux-gnu" / "release"

# The memory a target may take, AddressSanitizer's share included, before libFuzzer reports it out of memory: libFuzzer's default, which CORE-005 and CORE-006 used too. For the default-limits targets it was checked against the bound that bayan-opc documents (about 256 MiB to open a package and read its core properties): under AddressSanitizer, opening the worst packages of that bound's derivation takes 2.5 to 4.5 times the memory it takes natively, and package_default_limits, which opens a package and then its written copy, peaked at 1,082 MiB on the worst of them. 2,048 MiB leaves room for that, libFuzzer's corpus and AddressSanitizer's quarantine of freed memory, while an input that needs about three times the bound to be opened once, or one and a half times for each of the target's two openings, exceeds it. README.md gives the measurements.
RSS_LIMIT_MB = 2048

# Options for AddressSanitizer, as cargo-fuzz sets them when it runs a target.
ASAN_OPTIONS = "detect_odr_violation=0"

# The configurations that the seeds of markup_compatibility start with. Their first byte chooses which namespaces are understood (bits 0 to 5: urn:a, urn:b, urn:c, WordprocessingML, Word 2010's w14 and wps), whether names without a namespace are (bit 6), and whether extension lists count (bit 7); see fuzz_targets/markup_compatibility.rs.
MC_CONFIGURATIONS = (0x00, 0x08, 0x38, 0x48, 0x78, 0xFF)

# The longest XML part that becomes a seed of the XML targets: their inputs are at most this long (max_len below), and libFuzzer would cut a longer one short.
XML_SEED_LIMIT = 64 << 10


@dataclasses.dataclass(frozen=True)
class Target:
    """How CI fuzzes one target."""

    # The libFuzzer dictionary in dictionaries/, if any.
    dictionary: Optional[str]
    # The longest input libFuzzer makes (-max_len).
    max_len: int
    # The seconds an input may take before libFuzzer reports a timeout (-timeout).
    timeout: int
    # How long the target runs, in multiples of --seconds.
    share: int
    # The kind of seeds it starts from (see seeds()), if any.
    seeds: Optional[str]
    # Whether libFuzzer may make inputs as long as max_len straight away (-len_control=0), instead of lengthening them slowly from the longest input of the corpus.
    long_inputs: bool = False


# Every fuzz target of Cargo.toml, with how CI runs it; `run --all` fails if the two lists differ, so that no target is left out. The default-limits targets get inputs of up to 1 MiB straight away and twice the time of the others, since each of their inputs takes longer; their timeout is longer too, because an input that fills bayan-opc's default metadata budgets takes seconds under AddressSanitizer (12 s for the worst metadata of the memory bound's derivation). The other targets keep the 10 s of CORE-005 and CORE-006, with inputs of up to 64 KiB.
TARGETS: Dict[str, Target] = {
    "zip_archive": Target("zip.dict", 64 << 10, 10, 1, "documents"),
    "package": Target("zip.dict", 64 << 10, 10, 1, "documents"),
    "content_types": Target("xml.dict", 64 << 10, 10, 1, "content_types"),
    "relationships": Target("xml.dict", 64 << 10, 10, 1, "relationships"),
    "core_properties": Target("xml.dict", 64 << 10, 10, 1, "core_properties"),
    "compound_file": Target(None, 64 << 10, 10, 1, None),
    "zip_archive_default_limits": Target("zip.dict", 1 << 20, 60, 2, "documents", long_inputs=True),
    "package_default_limits": Target("zip.dict", 1 << 20, 60, 2, "documents", long_inputs=True),
    "xml_reader": Target("ooxml.dict", XML_SEED_LIMIT, 10, 1, "xml_parts"),
    "markup_compatibility": Target("ooxml.dict", XML_SEED_LIMIT, 10, 1, "configured_xml_parts"),
}

# How libFuzzer names the inputs it keeps, by what went wrong, and the kind of failure that ci.py reports for each: the only thing it shows about a failure. It also keeps inputs that ran slowly (slow-unit-), which are not failures.
FAILURE_PREFIXES = {"crash-": "crash", "timeout-": "timeout", "oom-": "out of memory", "leak-": "leak"}

# libFuzzer writes the corpus size in bytes, KiB or MiB (b, Kb or Mb), so nothing else of a log can reach the summary through it.
STATUS_LINE = re.compile(r"^#(\d+)\s+\S+\s+cov: (\d+) ft: (\d+) corp: (\d+)/(\d+(?:b|Kb|Mb))\b", re.MULTILINE)
FINAL_STAT = re.compile(r"^stat::(\w+):\s+(\d+)$", re.MULTILINE)
MERGE_RESULT = re.compile(r"MERGE-OUTER: (\d+) new files with (\d+) new features added; (\d+) new coverage edges")


def fuzz_targets_in_manifest(manifest: Path = FUZZ / "Cargo.toml") -> List[str]:
    """The names of the fuzz targets ([[bin]]) that Cargo.toml declares."""
    with manifest.open("rb") as file:
        return [binary["name"] for binary in tomllib.load(file).get("bin", [])]


def check_targets(declared: Sequence[str]) -> List[str]:
    """What keeps TARGETS from matching the targets that Cargo.toml declares, if anything."""
    problems = [f"{name} is a fuzz target in fuzz/Cargo.toml but has no settings in TARGETS of fuzz/ci.py" for name in declared if name not in TARGETS]
    problems += [f"{name} has settings in TARGETS of fuzz/ci.py but is no fuzz target in fuzz/Cargo.toml" for name in TARGETS if name not in declared]
    return problems


def seeds(output: Path) -> Dict[str, int]:
    """Writes the seeds of every target into output/<target>/, each named by the SHA-1 of its content, as libFuzzer names its inputs, and returns how many each target has."""
    kinds: Dict[str, List[bytes]] = {"documents": [], "content_types": [], "relationships": [], "core_properties": [], "xml_parts": []}
    for document in sorted(DOCUMENTS.glob("*.docx")):
        kinds["documents"].append(document.read_bytes())
        # Test files of this repository, not hostile input.
        with zipfile.ZipFile(document) as archive:
            for info in sorted(archive.infolist(), key=lambda info: info.filename):
                name = info.filename
                part = archive.read(info)
                if name == "[Content_Types].xml":
                    kinds["content_types"].append(part)
                elif name.endswith(".rels"):
                    kinds["relationships"].append(part)
                elif name == "docProps/core.xml":
                    kinds["core_properties"].append(part)
                if name.endswith((".xml", ".rels")) and len(part) <= XML_SEED_LIMIT:
                    kinds["xml_parts"].append(part)
    kinds["xml_parts"] += [sample.read_bytes() for sample in sorted(XML_SAMPLES.glob("*.xml"))]
    kinds["configured_xml_parts"] = [bytes([configuration]) + part for part in kinds["xml_parts"] for configuration in MC_CONFIGURATIONS]
    counts = {}
    for name, target in TARGETS.items():
        folder = output / name
        folder.mkdir(parents=True, exist_ok=True)
        for data in kinds[target.seeds] if target.seeds else []:
            (folder / hashlib.sha1(data).hexdigest()).write_bytes(data)
        counts[name] = sum(1 for _ in folder.iterdir())
    return counts


@dataclasses.dataclass
class Result:
    """What fuzzing or minimizing one target gave."""

    target: str
    # How long the target ran, measured: one that fails stops before its time is up.
    seconds: int = 0
    status: str = "passed"
    problems: List[str] = dataclasses.field(default_factory=list)
    stats: Dict[str, int] = dataclasses.field(default_factory=dict)
    corpus: str = ""
    log: Optional[Path] = None
    # The folder where libFuzzer keeps the inputs that fail.
    artifacts: Optional[Path] = None


def parse_log(text: str) -> Dict[str, int]:
    """The figures of a libFuzzer log: the executions, coverage (edges and features) and corpus size of its last status line, and the final statistics (-print_final_stats=1)."""
    stats: Dict[str, int] = {}
    lines = STATUS_LINE.findall(text)
    if lines:
        runs, edges, features, inputs, _ = lines[-1]
        stats.update(runs=int(runs), edges=int(edges), features=int(features), inputs=int(inputs))
    for name, value in FINAL_STAT.findall(text):
        stats[name] = int(value)
    return stats


def last_corpus_size(text: str) -> str:
    """The corpus size of the last status line, as libFuzzer writes it (for example 469/8922Kb)."""
    lines = STATUS_LINE.findall(text)
    return f"{lines[-1][3]}/{lines[-1][4]}" if lines else ""


def failures(artifacts: Path) -> List[str]:
    """The inputs that libFuzzer kept in `artifacts` because they crashed, timed out, ran out of memory or leaked."""
    if not artifacts.is_dir():
        return []
    return sorted(path.name for path in artifacts.iterdir() if path.name.startswith(tuple(FAILURE_PREFIXES)))


def kinds(inputs: Sequence[str]) -> List[str]:
    """The kinds of failure that the names of the inputs libFuzzer kept give, in the order of FAILURE_PREFIXES. libFuzzer chooses the start of a name by what went wrong; the rest is the input's SHA-1, which is not shown."""
    return [kind for prefix, kind in FAILURE_PREFIXES.items() if any(name.startswith(prefix) for name in inputs)]


def environment() -> Dict[str, str]:
    """The environment of a target: this one, with cargo-fuzz's options for AddressSanitizer first, so that options the caller set still win."""
    env = dict(os.environ)
    env["ASAN_OPTIONS"] = ":".join(option for option in (ASAN_OPTIONS, env.get("ASAN_OPTIONS")) if option)
    return env


def binary(name: str, binaries: Path) -> Path:
    path = binaries / name
    if not path.is_file():
        raise SystemExit(f"ci.py: {path} does not exist; build the targets first: cargo +nightly-2026-10-02 fuzz build --target x86_64-unknown-linux-gnu")
    return path


def fuzz_command(name: str, seconds: int, seeds_dir: Path, work: Path, binaries: Path) -> List[str]:
    """The command that fuzzes one target with its settings in TARGETS."""
    target = TARGETS[name]
    command = [
        str(binary(name, binaries)),
        f"-max_total_time={seconds * target.share}",
        f"-max_len={target.max_len}",
        f"-timeout={target.timeout}",
        f"-rss_limit_mb={RSS_LIMIT_MB}",
        "-print_final_stats=1",
        f"-artifact_prefix={work / 'artifacts' / name}/",
    ]
    if target.long_inputs:
        command.append("-len_control=0")
    if target.dictionary:
        command.append(f"-dict={FUZZ / 'dictionaries' / target.dictionary}")
    # libFuzzer adds what it finds to the first folder; the seeds are only read.
    command += [str(work / "corpus" / name), str(seeds_dir / name)]
    return command


def merge_command(name: str, work: Path, binaries: Path) -> List[str]:
    """The command that copies the inputs of a target's corpus that its coverage needs into WORK/minimized/<target>."""
    target = TARGETS[name]
    return [
        str(binary(name, binaries)),
        "-merge=1",
        f"-max_len={target.max_len}",
        f"-timeout={target.timeout}",
        f"-rss_limit_mb={RSS_LIMIT_MB}",
        f"-artifact_prefix={work / 'artifacts' / name}/",
        str(work / "minimized" / name),
        str(work / "corpus" / name),
    ]


def run_one(command: List[str], log: Path) -> Tuple[int, float]:
    """Runs a command with its output in `log`, and returns its exit code and the seconds it took."""
    start = time.monotonic()
    with log.open("wb") as output:
        code = subprocess.run(command, stdout=output, stderr=subprocess.STDOUT, env=environment(), check=False).returncode
    return code, time.monotonic() - start


def in_github_actions() -> bool:
    return os.environ.get("GITHUB_ACTIONS") == "true"


def show(result: Result, line: str) -> None:
    """Prints a target's result, which for a failure is the kind of failure and nothing else; in GitHub Actions a failure also becomes an annotation of the run, and elsewhere ci.py says where its log and inputs are. It never prints a log, because the job log of GitHub Actions is public: libFuzzer writes the input that failed into the log (in hex, as text and in Base64, if it has at most 256 bytes), and an input that ran slowly too (in Base64)."""
    print(line, flush=True)
    if result.status in ("passed", "empty"):
        return
    if in_github_actions():
        # Nothing in it comes from the target's output, so it needs no protection from workflow commands.
        print(f"::error title=Fuzzing {result.target}::{'; '.join(result.problems)}", flush=True)
    else:
        print(f"  the log is {result.log}, the inputs that failed are in {result.artifacts}/", flush=True)


def run(names: Sequence[str], seconds: int, jobs: int, seeds_dir: Path, work: Path, binaries: Path) -> List[Result]:
    """Fuzzes each target for its share of `seconds`, `jobs` at a time and the longest first, and returns the results in the order of TARGETS."""
    (work / "logs").mkdir(parents=True, exist_ok=True)
    for name in names:
        (work / "corpus" / name).mkdir(parents=True, exist_ok=True)
        (work / "artifacts" / name).mkdir(parents=True, exist_ok=True)
        (seeds_dir / name).mkdir(parents=True, exist_ok=True)
    commands = {name: fuzz_command(name, seconds, seeds_dir, work, binaries) for name in names}
    order = sorted(names, key=lambda name: -TARGETS[name].share)
    results = {}
    with concurrent.futures.ThreadPoolExecutor(max_workers=jobs) as pool:
        running = {pool.submit(run_one, commands[name], work / "logs" / f"{name}.log"): name for name in order}
        for future in concurrent.futures.as_completed(running):
            name = running[future]
            code, elapsed = future.result()
            result = Result(name, round(elapsed), log=work / "logs" / f"{name}.log", artifacts=work / "artifacts" / name)
            kept = failures(result.artifacts)
            if code != 0 or kept:
                # The log of a target that failed is not even read: libFuzzer wrote the input into it.
                result.status = "failed"
                result.problems = kinds(kept) or [f"libFuzzer exited with {code} and kept no input"]
                show(result, f"{name}: {status_of(result)}")
            else:
                text = result.log.read_text(errors="replace")
                result.stats = parse_log(text)
                result.corpus = last_corpus_size(text)
                show(result, f"{name}: {result.status} after {result.seconds} s, {result.stats.get('runs', 0):,} runs, coverage {result.stats.get('edges', 0):,} edges and {result.stats.get('features', 0):,} features")
            results[name] = result
    return [results[name] for name in TARGETS if name in results]


def minimize(names: Sequence[str], jobs: int, work: Path, binaries: Path) -> List[Result]:
    """Minimizes the corpus of each target in place, `jobs` at a time, and returns the results in the order of TARGETS."""
    (work / "logs").mkdir(parents=True, exist_ok=True)
    results = {}
    pending = {}
    # The inputs that had failed before, such as in this job's fuzzing: only those that merging keeps are its own.
    failed_before: Dict[str, Set[str]] = {}
    for name in names:
        corpus = work / "corpus" / name
        result = Result(name, log=work / "logs" / f"{name}-minimize.log", artifacts=work / "artifacts" / name)
        if not corpus.is_dir() or not any(corpus.iterdir()):
            result.status = "empty"
            results[name] = result
            continue
        result.stats["before"] = sum(1 for _ in corpus.iterdir())
        minimized = work / "minimized" / name
        shutil.rmtree(minimized, ignore_errors=True)
        minimized.mkdir(parents=True)
        result.artifacts.mkdir(parents=True, exist_ok=True)
        failed_before[name] = set(failures(result.artifacts))
        pending[name] = result
    with concurrent.futures.ThreadPoolExecutor(max_workers=jobs) as pool:
        running = {pool.submit(run_one, merge_command(name, work, binaries), result.log): name for name, result in pending.items()}
        for future in concurrent.futures.as_completed(running):
            name = running[future]
            result = pending[name]
            text = result.log.read_text(errors="replace")
            merged = MERGE_RESULT.findall(text)
            code, _ = future.result()
            kept = [input_name for input_name in failures(result.artifacts) if input_name not in failed_before[name]]
            # Merging runs the inputs in a process that it restarts after a crash, so a report in its log is a failure too. The report is never shown: libFuzzer writes the input that caused it next to it.
            reported = any("ERROR: libFuzzer" in line or "==ERROR:" in line for line in text.splitlines())
            if code != 0 or not merged or reported or kept:
                result.status = "failed"
                problems = [f"merging exited with {code}"] if code != 0 else []
                problems += [] if merged else ["merging printed no MERGE-OUTER summary"]
                problems += ["merging reported a failure"] if reported else []
                result.problems = kinds(kept) or problems
            else:
                inputs, features, edges = (int(value) for value in merged[-1])
                result.stats.update(after=inputs, features=features, edges=edges)
                corpus = work / "corpus" / name
                shutil.rmtree(corpus)
                (work / "minimized" / name).rename(corpus)
            results[name] = result
            if result.status == "failed":
                show(result, f"{name}: minimizing {status_of(result)}")
            else:
                show(result, f"{name}: corpus {result.status}, {result.stats.get('before', 0):,} inputs before and {result.stats.get('after', 0):,} after, coverage without the seeds {result.stats.get('edges', 0):,} edges and {result.stats.get('features', 0):,} features")
    return [results[name] for name in TARGETS if name in results]


def status_of(result: Result) -> str:
    return result.status if not result.problems else f"{result.status}: {'; '.join(result.problems)}"


def run_table(results: Sequence[Result]) -> str:
    rows = [
        "| Target | Result | Time | Executions | Per second | Coverage (edges / features) | Corpus at the end | Peak memory | Inputs up to |",
        "|---|---|---|---|---|---|---|---|---|",
    ]
    for result in results:
        if result.status == "failed":
            # Of a failure, only the target and the kind (README.md, "When a run fails").
            rows.append(f"| `{result.target}` | {status_of(result)} |" + " |" * 7)
            continue
        stats = result.stats
        rows.append(
            f"| `{result.target}` | {status_of(result)} | {result.seconds} s | {stats.get('number_of_executed_units', stats.get('runs', 0)):,} | {stats.get('average_exec_per_sec', 0):,} "
            f"| {stats.get('edges', 0):,} / {stats.get('features', 0):,} | {result.corpus} | {stats.get('peak_rss_mb', 0):,} MB | {TARGETS[result.target].max_len >> 10} KiB |"
        )
    return "\n".join(rows)


def minimize_table(results: Sequence[Result]) -> str:
    # libFuzzer never copies the seeds into the corpus, and every run reads them again, so a minimized corpus can reach less than the run that fuzzed with the seeds.
    rows = ["| Target | Result | Inputs before | Inputs after | Coverage without the seeds (edges / features) |", "|---|---|---|---|---|"]
    for result in results:
        if result.status == "failed":
            rows.append(f"| `{result.target}` | {status_of(result)} |" + " |" * 3)
            continue
        stats = result.stats
        rows.append(f"| `{result.target}` | {status_of(result)} | {stats.get('before', 0):,} | {stats.get('after', 0):,} | {stats.get('edges', 0):,} / {stats.get('features', 0):,} |")
    return "\n".join(rows)


# Below the table of a run in GitHub Actions in which a target failed.
KEPT_PRIVATE = (
    "Of a target that failed, only the kind of failure is shown. Its input and its log, which holds the input, can disclose a vulnerability, "
    "so they are neither uploaded nor printed, and GitHub deletes them with the runner after the job. "
    'Reproduce the failure privately and report it as fuzz/README.md explains ("When a run fails").'
)


def summarize(title: str, table: str, failed: bool) -> None:
    """Prints the table, and adds it to the job summary in GitHub Actions, with KEPT_PRIVATE if a target failed."""
    text = f"### {title}\n\n{table}\n"
    if failed and in_github_actions():
        text += f"\n{KEPT_PRIVATE}\n"
    print(text, flush=True)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as file:
            file.write(text + "\n")


def selected(arguments: argparse.Namespace) -> List[str]:
    """The targets that the command line names, in the order of TARGETS; with --all, every target, after checking that TARGETS matches Cargo.toml."""
    if arguments.all:
        problems = check_targets(fuzz_targets_in_manifest())
        if problems:
            raise SystemExit("ci.py: " + "; ".join(problems))
        return list(TARGETS)
    unknown = [name for name in arguments.targets if name not in TARGETS]
    if unknown or not arguments.targets:
        raise SystemExit(f"ci.py: name targets or give --all; unknown: {', '.join(unknown) or 'none'}; known: {', '.join(TARGETS)}")
    return [name for name in TARGETS if name in arguments.targets]


def main(argv: Optional[Sequence[str]] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    seeds_parser = commands.add_parser("seeds", help="write the seed corpora")
    seeds_parser.add_argument("output", type=Path)
    for name in ("run", "minimize"):
        command = commands.add_parser(name, help=f"{name} the targets")
        command.add_argument("targets", nargs="*")
        command.add_argument("--all", action="store_true", help="every target")
        command.add_argument("--work", type=Path, default=FUZZ, help="the folder of corpus/, artifacts/ and logs/ (default: this folder, as cargo-fuzz uses it)")
        command.add_argument("--jobs", type=int, default=4, help="how many targets run at the same time (default 4)")
        command.add_argument("--binaries", type=Path, default=BINARIES, help="where cargo-fuzz built the targets")
        if name == "run":
            command.add_argument("--seconds", type=int, required=True, help="how long a target with a share of 1 runs")
            command.add_argument("--seeds", type=Path, required=True, help="the folder that `seeds` wrote")
    arguments = parser.parse_args(argv)
    if arguments.command == "seeds":
        for name, count in seeds(arguments.output).items():
            print(f"{name}: {count} seeds")
        return 0
    names = selected(arguments)
    if arguments.jobs < 1:
        raise SystemExit("ci.py: --jobs must be at least 1")
    if arguments.command == "run":
        if arguments.seconds < 1:
            raise SystemExit("ci.py: --seconds must be at least 1")
        results = run(names, arguments.seconds, arguments.jobs, arguments.seeds, arguments.work, arguments.binaries)
        table = run_table(results)
    else:
        results = minimize(names, arguments.jobs, arguments.work, arguments.binaries)
        table = minimize_table(results)
    passed = all(result.status in ("passed", "empty") for result in results)
    summarize("Fuzzing" if arguments.command == "run" else "Corpora, minimized", table, not passed)
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
