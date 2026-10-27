#!/usr/bin/env python3
"""Builds the seed corpora of the fuzz targets, runs the targets and minimizes their corpora, as .github/workflows/fuzz.yml does in CI (bayan-core#21, follow-up 1 of CORE-005). README.md in this folder explains the targets, their settings and how to reproduce a failure.

Usage, from anywhere in the repository, after `cargo +nightly-2026-10-02 fuzz build --target x86_64-unknown-linux-gnu`:

  python3 -I fuzz/ci.py seeds SEEDS
      Writes the seeds of every target into SEEDS/<target>/, from public files only: the five python-docx documents that bayan-opc's tests keep, the metadata parts and the XML parts inside them, and bayan-xml's two sample parts. Never add documents of the private corpus: CI uploads what fuzzing finds where everyone who can read the repository's Actions can download it.
  python3 -I fuzz/ci.py run --seconds N --seeds SEEDS [--work WORK] [--jobs J] (--all | TARGET ...)
      Runs each target for N seconds times its share (TARGETS below), J at a time (default 4), from its seeds and its corpus in WORK/corpus/<target>/, where it adds what it finds. WORK is this folder unless given, so that the corpora and artifacts are where cargo-fuzz keeps them. Inputs that crash, time out, run out of memory or leak go to WORK/artifacts/<target>/, each target's output to WORK/logs/<target>.log. Exits with 1 if any target failed.
  python3 -I fuzz/ci.py minimize [--work WORK] [--jobs J] (--all | TARGET ...)
      Keeps only the inputs of each corpus that its coverage needs (libFuzzer's -merge=1), and reports that coverage.

Both run and minimize print a Markdown table, and add it to the job summary when GITHUB_STEP_SUMMARY is set. Needs Python 3.11 or later (for tomllib) and nothing outside its standard library. It runs the targets that cargo-fuzz built for x86_64-unknown-linux-gnu, so Linux on x86-64 only. The tests are in test_ci.py next to this file.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import dataclasses
import hashlib
import os
import re
import secrets
import shutil
import subprocess
import sys
import tomllib
import zipfile
from pathlib import Path
from typing import Dict, List, Optional, Sequence

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

# How libFuzzer names the inputs it keeps, by what went wrong. It also keeps inputs that ran slowly (slow-unit-), which are not failures.
FAILURE_PREFIXES = ("crash-", "timeout-", "oom-", "leak-")

STATUS_LINE = re.compile(r"^#(\d+)\s+\S+\s+cov: (\d+) ft: (\d+) corp: (\d+)/(\S+)", re.MULTILINE)
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
    seconds: int = 0
    status: str = "passed"
    problems: List[str] = dataclasses.field(default_factory=list)
    stats: Dict[str, int] = dataclasses.field(default_factory=dict)
    corpus: str = ""
    log: Optional[Path] = None


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
    return sorted(path.name for path in artifacts.iterdir() if path.name.startswith(FAILURE_PREFIXES))


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


def run_one(command: List[str], log: Path) -> int:
    with log.open("wb") as output:
        return subprocess.run(command, stdout=output, stderr=subprocess.STDOUT, env=environment(), check=False).returncode


def in_github_actions() -> bool:
    return os.environ.get("GITHUB_ACTIONS") == "true"


def show(result: Result, line: str) -> None:
    """Prints a target's result and its log, folded in GitHub Actions; a failure's last lines are also printed unfolded, so that its report is in view."""
    log = result.log.read_text(errors="replace") if result.log and result.log.is_file() else ""
    if not in_github_actions():
        print(line, flush=True)
        if result.status not in ("passed", "empty"):
            print(log, flush=True)
        return
    # A target's output can quote parts of its input. Workflow commands are switched off while it is printed, so that it cannot add annotations or change the job.
    token = secrets.token_hex(16)
    print(f"::group::{line}", flush=True)
    print(f"::stop-commands::{token}\n{log}\n::{token}::", flush=True)
    print("::endgroup::", flush=True)
    if result.status not in ("passed", "empty"):
        tail = "\n".join(log.splitlines()[-80:])
        print(f"::error title=Fuzzing {result.target}::{'; '.join(result.problems)}", flush=True)
        print(f"::stop-commands::{token}\n{tail}\n::{token}::", flush=True)


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
            result = Result(name, seconds * TARGETS[name].share, log=work / "logs" / f"{name}.log")
            text = result.log.read_text(errors="replace")
            result.stats = parse_log(text)
            result.corpus = last_corpus_size(text)
            kept = failures(work / "artifacts" / name)
            code = future.result()
            if code != 0 or kept:
                result.status = "failed"
                result.problems = [f"libFuzzer exited with {code}"] if code != 0 else []
                result.problems += [f"kept artifacts/{name}/{input_name}" for input_name in kept]
            results[name] = result
            show(result, f"{name}: {result.status} after {result.seconds} s, {result.stats.get('runs', 0):,} runs, coverage {result.stats.get('edges', 0):,} edges and {result.stats.get('features', 0):,} features")
    return [results[name] for name in TARGETS if name in results]


def minimize(names: Sequence[str], jobs: int, work: Path, binaries: Path) -> List[Result]:
    """Minimizes the corpus of each target in place, `jobs` at a time, and returns the results in the order of TARGETS."""
    (work / "logs").mkdir(parents=True, exist_ok=True)
    results = {}
    pending = {}
    for name in names:
        corpus = work / "corpus" / name
        result = Result(name, log=work / "logs" / f"{name}-minimize.log")
        if not corpus.is_dir() or not any(corpus.iterdir()):
            result.status = "empty"
            results[name] = result
            continue
        result.stats["before"] = sum(1 for _ in corpus.iterdir())
        minimized = work / "minimized" / name
        shutil.rmtree(minimized, ignore_errors=True)
        minimized.mkdir(parents=True)
        (work / "artifacts" / name).mkdir(parents=True, exist_ok=True)
        pending[name] = result
    with concurrent.futures.ThreadPoolExecutor(max_workers=jobs) as pool:
        running = {pool.submit(run_one, merge_command(name, work, binaries), result.log): name for name, result in pending.items()}
        for future in concurrent.futures.as_completed(running):
            name = running[future]
            result = pending[name]
            text = result.log.read_text(errors="replace")
            merged = MERGE_RESULT.findall(text)
            code = future.result()
            # Merging runs the inputs in a process that it restarts after a crash, so a report in its log is a failure too.
            reports = [line for line in text.splitlines() if "ERROR: libFuzzer" in line or "==ERROR:" in line]
            if code != 0 or not merged or reports:
                result.status = "failed"
                result.problems = [f"merging exited with {code}"] + reports[:3]
            else:
                inputs, features, edges = (int(value) for value in merged[-1])
                result.stats.update(after=inputs, features=features, edges=edges)
                corpus = work / "corpus" / name
                shutil.rmtree(corpus)
                (work / "minimized" / name).rename(corpus)
            results[name] = result
            show(result, f"{name}: corpus {result.status}, {result.stats.get('before', 0):,} inputs before and {result.stats.get('after', 0):,} after, coverage {result.stats.get('edges', 0):,} edges and {result.stats.get('features', 0):,} features")
    return [results[name] for name in TARGETS if name in results]


def status_of(result: Result) -> str:
    return result.status if not result.problems else f"{result.status}: {'; '.join(result.problems)}"


def run_table(results: Sequence[Result]) -> str:
    rows = [
        "| Target | Result | Fuzzing time | Executions | Per second | Coverage (edges / features) | Corpus at the end | Peak memory | Inputs up to |",
        "|---|---|---|---|---|---|---|---|---|",
    ]
    for result in results:
        stats = result.stats
        rows.append(
            f"| `{result.target}` | {status_of(result)} | {result.seconds} s | {stats.get('number_of_executed_units', stats.get('runs', 0)):,} | {stats.get('average_exec_per_sec', 0):,} "
            f"| {stats.get('edges', 0):,} / {stats.get('features', 0):,} | {result.corpus} | {stats.get('peak_rss_mb', 0):,} MB | {TARGETS[result.target].max_len >> 10} KiB |"
        )
    return "\n".join(rows)


def minimize_table(results: Sequence[Result]) -> str:
    rows = ["| Target | Result | Inputs before | Inputs after | Coverage (edges / features) |", "|---|---|---|---|---|"]
    for result in results:
        stats = result.stats
        rows.append(f"| `{result.target}` | {status_of(result)} | {stats.get('before', 0):,} | {stats.get('after', 0):,} | {stats.get('edges', 0):,} / {stats.get('features', 0):,} |")
    return "\n".join(rows)


def summarize(title: str, table: str) -> None:
    """Prints the table, and adds it to the job summary in GitHub Actions."""
    text = f"### {title}\n\n{table}\n"
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
        summarize("Fuzzing", run_table(results))
    else:
        results = minimize(names, arguments.jobs, arguments.work, arguments.binaries)
        summarize("Corpora, minimized", minimize_table(results))
    return 0 if all(result.status in ("passed", "empty") for result in results) else 1


if __name__ == "__main__":
    sys.exit(main())
