#!/usr/bin/env python3
"""Check selected benchmark ratios against conservative limits."""

from __future__ import annotations

import argparse
import csv
import io
import pathlib
import tempfile
from dataclasses import dataclass
from typing import TextIO

ROOT = pathlib.Path(__file__).resolve().parent.parent
DEFAULT_SUMMARY = ROOT / "target/bench/summary.tsv"
DEFAULT_RATCHET = ROOT / "benchmark-ratchet.tsv"


@dataclass(frozen=True)
class Limit:
    case: str
    ratio_column: str
    max_ratio: float


@dataclass(frozen=True)
class Result:
    verdict: str
    case: str
    ratio: float
    max_ratio: float


def read_limits(handle: TextIO) -> list[Limit]:
    reader = csv.DictReader(handle, delimiter="\t")
    if reader.fieldnames != ["case", "ratio_column", "max_ratio"]:
        raise ValueError("ratchet header must be: case, ratio_column, max_ratio")
    limits = []
    seen = set()
    for line_number, row in enumerate(reader, 2):
        case = row["case"]
        ratio_column = row["ratio_column"]
        if not case or case in seen:
            raise ValueError(f"invalid or duplicate case on ratchet line {line_number}")
        if not ratio_column.startswith("ratio_vs_bash_"):
            raise ValueError(f"invalid ratio column on ratchet line {line_number}")
        max_ratio = float(row["max_ratio"])
        if max_ratio <= 0:
            raise ValueError(f"non-positive limit on ratchet line {line_number}")
        seen.add(case)
        limits.append(Limit(case, ratio_column, max_ratio))
    if not limits:
        raise ValueError("ratchet has no cases")
    return limits


def read_summary(handle: TextIO) -> tuple[list[str], dict[tuple[str, str], dict[str, str]]]:
    reader = csv.DictReader(handle, delimiter="\t")
    if reader.fieldnames is None:
        raise ValueError("benchmark summary has no header")
    required = {"case", "shell", "median_ms", "min_ms", "max_ms"}
    if not required.issubset(reader.fieldnames):
        raise ValueError("benchmark summary is missing required columns")
    rows = {}
    for line_number, row in enumerate(reader, 2):
        key = (row["case"], row["shell"])
        if key in rows:
            raise ValueError(f"duplicate benchmark row on line {line_number}: {key}")
        rows[key] = row
    return reader.fieldnames, rows


def evaluate(
    limits: list[Limit], fieldnames: list[str], rows: dict[tuple[str, str], dict[str, str]]
) -> list[Result]:
    results = []
    for limit in limits:
        if limit.ratio_column not in fieldnames:
            raise ValueError(f"summary is missing {limit.ratio_column}")
        row = rows.get((limit.case, "cherubsh"))
        if row is None:
            raise ValueError(f"summary is missing cherubsh case {limit.case}")
        try:
            ratio = float(row[limit.ratio_column])
        except ValueError as error:
            raise ValueError(f"invalid ratio for {limit.case}") from error
        verdict = "PASS" if ratio <= limit.max_ratio else "FAIL"
        results.append(Result(verdict, limit.case, ratio, limit.max_ratio))
    return results


def render(results: list[Result]) -> str:
    lines = ["verdict\tcase\tratio\tmax_ratio"]
    lines.extend(
        f"{result.verdict}\t{result.case}\t{result.ratio:.2f}\t{result.max_ratio:.2f}"
        for result in results
    )
    return "\n".join(lines) + "\n"


def self_test() -> None:
    limits = read_limits(
        io.StringIO(
            "case\tratio_column\tmax_ratio\n"
            "fast\tratio_vs_bash_5315\t2.00\n"
            "slow\tratio_vs_bash_5315\t1.50\n"
        )
    )
    fieldnames, rows = read_summary(
        io.StringIO(
            "case\tshell\tmedian_ms\tmin_ms\tmax_ms\tratio_vs_bash_5315\n"
            "fast\tcherubsh\t10\t9\t11\t1.25\n"
            "slow\tcherubsh\t20\t19\t21\t1.75\n"
        )
    )
    results = evaluate(limits, fieldnames, rows)
    assert [result.verdict for result in results] == ["PASS", "FAIL"]
    rendered = render(results)
    assert "PASS\tfast\t1.25\t2.00" in rendered
    assert "FAIL\tslow\t1.75\t1.50" in rendered

    with tempfile.TemporaryDirectory() as directory:
        output = pathlib.Path(directory) / "ratchet.tsv"
        output.write_text(rendered, encoding="utf-8")
        assert output.read_text(encoding="utf-8") == rendered
    print("benchmark regression self-test passed")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--summary", type=pathlib.Path, default=DEFAULT_SUMMARY)
    parser.add_argument("--ratchet", type=pathlib.Path, default=DEFAULT_RATCHET)
    parser.add_argument("--output", type=pathlib.Path)
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.self_test:
        self_test()
        return 0

    with args.ratchet.open(encoding="utf-8", newline="") as handle:
        limits = read_limits(handle)
    with args.summary.open(encoding="utf-8", newline="") as handle:
        fieldnames, rows = read_summary(handle)
    results = evaluate(limits, fieldnames, rows)
    report = render(results)
    if args.output is not None:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(report, encoding="utf-8")
    print(report, end="")
    failures = sum(result.verdict == "FAIL" for result in results)
    if failures:
        print(f"benchmark ratchet failed: {failures} case(s) exceeded their limits")
        return 1
    print(f"benchmark ratchet passed: {len(results)} case(s) within their limits")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
