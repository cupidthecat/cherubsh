#!/usr/bin/env python3
"""Generate bounded shell programs and compare CherubSH with Bash."""

from __future__ import annotations

import argparse
import hashlib
import os
import random
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path


WORDS = ("amber", "birch", "cedar", "dawn", "ember", "fern", "grove", "harbor")


@dataclass(frozen=True)
class RunResult:
    status: int | None
    stdout: bytes
    stderr: bytes
    timed_out: bool


def generated_script(rng: random.Random, case_number: int) -> str:
    first = rng.choice(WORDS)
    second = rng.choice(WORDS)
    number = rng.randrange(1, 500)
    offset = rng.randrange(0, len(first))
    replacement = rng.choice(WORDS)

    stages = [
        f'word="{first}_{second}_{number}"',
        f'fallback="{replacement}"',
        'printf "case=%s word=%s upper=%s fallback=%s\\n" '
        f'"{case_number}" "$word" "${{word^^}}" "${{unset_value:-$fallback}}"',
        f'printf "slice=%s replace=%s\\n" "${{word:{offset}:3}}" "${{word/{first}/{replacement}}}"',
    ]

    choices = [
        'items=(alpha "two words" gamma)\nprintf "items="\nprintf "<%s>" "${items[@]}"\nprintf "\\n"',
        f'left={number}\nright={offset + 3}\n((total = left * right + 7))\nprintf "math=%s\\n" "$total"',
        'pick() { case "$1" in a*) printf "starts-a\\n" ;; *) printf "other\\n" ;; esac; }\npick "$word"',
        'for value in one two three; do printf "loop=%s\\n" "$value"; done',
        'joined=$(printf "%s:%s" "$fallback" "${word#*_}")\nprintf "joined=%s\\n" "$joined"',
    ]
    rng.shuffle(choices)
    stages.extend(choices[: rng.randrange(1, len(choices) + 1)])
    return "set -u\n" + "\n".join(stages) + "\n"


def run_shell(shell: Path, source: str, cwd: Path, timeout: float) -> RunResult:
    environment = {
        "HOME": str(cwd / "home"),
        "LANG": "C",
        "LC_ALL": "C",
        "PATH": "/usr/bin:/bin",
    }
    for name in ("ASAN_OPTIONS", "LSAN_OPTIONS", "UBSAN_OPTIONS"):
        if value := os.environ.get(name):
            environment[name] = value
    try:
        completed = subprocess.run(
            [str(shell), "--norc", "--noprofile", "-c", source],
            cwd=cwd,
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        return RunResult(None, error.stdout or b"", error.stderr or b"", True)
    return RunResult(completed.returncode, completed.stdout, completed.stderr, False)


def results_match(expected: RunResult, actual: RunResult) -> bool:
    return (
        not expected.timed_out
        and not actual.timed_out
        and expected.status == actual.status
        and expected.stdout == actual.stdout
        and expected.stderr == actual.stderr
    )


def save_failure(
    directory: Path,
    case_number: int,
    source: str,
    expected: RunResult,
    actual: RunResult,
) -> Path:
    digest = hashlib.sha256(source.encode()).hexdigest()[:12]
    prefix = directory / f"case-{case_number:04d}-{digest}"
    directory.mkdir(parents=True, exist_ok=True)
    prefix.with_suffix(".sh").write_text(source)
    prefix.with_suffix(".bash.out").write_bytes(expected.stdout)
    prefix.with_suffix(".bash.err").write_bytes(expected.stderr)
    prefix.with_suffix(".cherubsh.out").write_bytes(actual.stdout)
    prefix.with_suffix(".cherubsh.err").write_bytes(actual.stderr)
    return prefix.with_suffix(".sh")


def print_failure(
    case_number: int,
    source: str,
    expected: RunResult,
    actual: RunResult,
    artifact: Path | None,
) -> None:
    print(f"differential fuzz mismatch in case {case_number}", file=sys.stderr)
    print("--- source ---", file=sys.stderr)
    print(source, end="", file=sys.stderr)
    print("--- Bash ---", file=sys.stderr)
    print(f"status={expected.status} timed_out={expected.timed_out}", file=sys.stderr)
    print(expected.stdout.decode(errors="replace"), end="", file=sys.stderr)
    print(expected.stderr.decode(errors="replace"), end="", file=sys.stderr)
    print("--- CherubSH ---", file=sys.stderr)
    print(f"status={actual.status} timed_out={actual.timed_out}", file=sys.stderr)
    print(actual.stdout.decode(errors="replace"), end="", file=sys.stderr)
    print(actual.stderr.decode(errors="replace"), end="", file=sys.stderr)
    if artifact is not None:
        print(f"saved reproduction: {artifact}", file=sys.stderr)


def self_test() -> int:
    first = [generated_script(random.Random(17 + index), index) for index in range(24)]
    second = [generated_script(random.Random(17 + index), index) for index in range(24)]
    assert first == second
    assert all(source.startswith("set -u\n") for source in first)
    assert all("\x00" not in source and "rm " not in source for source in first)
    print("differential fuzz generator self-test: ok")
    return 0


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cherub", type=Path, default=Path("target/debug/cherubsh"))
    parser.add_argument(
        "--bash",
        type=Path,
        default=Path(os.environ.get("BASH_ORACLE_PATH", "target/oracle/bash-5.3.15/bash")),
    )
    parser.add_argument("--cases", type=int, default=100)
    parser.add_argument("--seed", type=int, default=20260731)
    parser.add_argument("--timeout", type=float, default=3.0)
    parser.add_argument("--artifact-dir", type=Path)
    parser.add_argument("--self-test", action="store_true")
    arguments = parser.parse_args()
    if arguments.cases < 1:
        parser.error("--cases must be positive")
    if arguments.timeout <= 0:
        parser.error("--timeout must be positive")
    return arguments


def main() -> int:
    arguments = parse_arguments()
    if arguments.self_test:
        return self_test()
    arguments.cherub = arguments.cherub.resolve()
    arguments.bash = arguments.bash.resolve()
    for shell in (arguments.cherub, arguments.bash):
        if not shell.is_file() or not os.access(shell, os.X_OK):
            print(f"error: executable not found: {shell}", file=sys.stderr)
            return 2

    rng = random.Random(arguments.seed)
    with tempfile.TemporaryDirectory(prefix="cherubsh-fuzz-") as temporary:
        workdir = Path(temporary)
        (workdir / "home").mkdir()
        for case_number in range(arguments.cases):
            source = generated_script(rng, case_number)
            expected = run_shell(arguments.bash, source, workdir, arguments.timeout)
            actual = run_shell(arguments.cherub, source, workdir, arguments.timeout)
            if not results_match(expected, actual):
                artifact = None
                if arguments.artifact_dir is not None:
                    artifact = save_failure(
                        arguments.artifact_dir, case_number, source, expected, actual
                    )
                print_failure(case_number, source, expected, actual, artifact)
                return 1

    print(f"differential fuzz: {arguments.cases} cases passed (seed {arguments.seed})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
