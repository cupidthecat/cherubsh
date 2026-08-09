#!/usr/bin/env python3
"""Summarize the checked Oils compatibility ratchet."""

from __future__ import annotations

import argparse
import collections
import io
import pathlib
import tempfile
from collections.abc import Iterable

ROOT = pathlib.Path(__file__).resolve().parent.parent
DEFAULT_RATCHET = ROOT / "crates/test-harness/oils-known-mismatches.tsv"
FIELD_ORDER = ("timeout", "status", "stdout", "stderr")
FIELD_WEIGHT = {"timeout": 8, "status": 4, "stdout": 2, "stderr": 1}


def normalized_fields(value: str) -> set[str]:
    fields: set[str] = set()
    for variant in value.split("|"):
        for field in variant.split(","):
            if field.endswith("-timeout"):
                fields.add("timeout")
            elif field in FIELD_ORDER:
                fields.add(field)
    return fields


def read_gaps(lines: Iterable[str]) -> dict[str, set[str]]:
    gaps: dict[str, set[str]] = {}
    for line_number, raw_line in enumerate(lines, 1):
        line = raw_line.rstrip("\n")
        if not line or line.startswith("#") or line.startswith("case\t"):
            continue
        columns = line.split("\t")
        if len(columns) != 5:
            raise ValueError(f"ratchet line {line_number} must have five columns")
        case, _, fields, _, _ = columns
        gaps.setdefault(case, set()).update(normalized_fields(fields))
    return gaps


def read_verdicts(lines: Iterable[str]) -> collections.Counter[str]:
    verdicts: collections.Counter[str] = collections.Counter()
    for line_number, raw_line in enumerate(lines, 1):
        line = raw_line.rstrip("\n")
        if not line or line.startswith("verdict\t"):
            continue
        columns = line.split("\t")
        if len(columns) != 6:
            raise ValueError(f"report line {line_number} must have six columns")
        verdicts[columns[0]] += 1
    return verdicts


def render_summary(
    gaps: dict[str, set[str]], verdicts: collections.Counter[str], limit: int
) -> str:
    by_spec: dict[str, list[set[str]]] = collections.defaultdict(list)
    combinations: collections.Counter[str] = collections.Counter()
    totals: collections.Counter[str] = collections.Counter()
    for case, fields in gaps.items():
        spec = case.split("::", 1)[0]
        by_spec[spec].append(fields)
        for field in fields:
            totals[field] += 1
        combination = ",".join(field for field in FIELD_ORDER if field in fields)
        combinations[combination or "other"] += 1

    rows = []
    for spec, cases in by_spec.items():
        counts = {field: sum(field in case for case in cases) for field in FIELD_ORDER}
        score = sum(counts[field] * FIELD_WEIGHT[field] for field in FIELD_ORDER)
        rows.append((score, len(cases), spec, counts))
    rows.sort(key=lambda row: (-row[0], -row[1], row[2]))

    output = ["# Oils compatibility gaps", ""]
    output.append(f"The checked ratchet contains {len(gaps)} unique known cases.")
    output.append("")
    output.append("| Type | Cases |")
    output.append("| --- | ---: |")
    for field in FIELD_ORDER:
        output.append(f"| {field} | {totals[field]} |")

    if verdicts:
        output.extend(["", "## Current run", "", "| Verdict | Cases |", "| --- | ---: |"])
        for verdict in ("PASS", "KNOWN", "FAIL", "DRIFT", "XPASS", "STALE"):
            output.append(f"| {verdict} | {verdicts[verdict]} |")

    output.extend(
        [
            "",
            f"## Highest-impact specs (top {min(limit, len(rows))})",
            "",
            "Timeouts and status differences rank ahead of output-only differences.",
            "",
            "| Spec | Cases | Timeout | Status | Stdout | Stderr |",
            "| --- | ---: | ---: | ---: | ---: | ---: |",
        ]
    )
    for _, case_count, spec, counts in rows[:limit]:
        output.append(
            f"| {spec} | {case_count} | {counts['timeout']} | {counts['status']} | "
            f"{counts['stdout']} | {counts['stderr']} |"
        )

    output.extend(["", "## Mismatch combinations", "", "| Fields | Cases |", "| --- | ---: |"])
    for fields, count in sorted(combinations.items(), key=lambda item: (-item[1], item[0])):
        output.append(f"| {fields} | {count} |")
    output.append("")
    return "\n".join(output)


def self_test() -> None:
    ratchet = io.StringIO(
        "case\tarch\tfields\toracle_sha256\tcandidate_sha256\n"
        "alpha.test.sh::001::one\t*\tstderr\ta\tb\n"
        "alpha.test.sh::001::one\tx86_64\tstatus,stderr\tc\td\n"
        "beta.test.sh::002::two\t*\tcherub-timeout,status,stdout,stderr\te\tf\n"
    )
    report = io.StringIO(
        "verdict\tcase\tarch\tfields\toracle_sha256\tcandidate_sha256\n"
        "PASS\talpha\tx86_64\t\ta\ta\n"
        "KNOWN\tbeta\tx86_64\tstderr\tb\tc\n"
    )
    rendered = render_summary(read_gaps(ratchet), read_verdicts(report), 10)
    assert "2 unique known cases" in rendered
    assert "| timeout | 1 |" in rendered
    assert "| status | 2 |" in rendered
    assert rendered.index("beta.test.sh") < rendered.index("alpha.test.sh")

    with tempfile.TemporaryDirectory() as directory:
        output = pathlib.Path(directory) / "summary.md"
        output.write_text(rendered, encoding="utf-8")
        assert output.read_text(encoding="utf-8") == rendered
    print("Oils gap summary self-test passed")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ratchet", type=pathlib.Path, default=DEFAULT_RATCHET)
    parser.add_argument("--report", type=pathlib.Path)
    parser.add_argument("--output", type=pathlib.Path)
    parser.add_argument("--limit", type=int, default=20)
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.self_test:
        self_test()
        return 0
    if args.limit < 1:
        raise SystemExit("--limit must be positive")

    with args.ratchet.open(encoding="utf-8") as handle:
        gaps = read_gaps(handle)
    verdicts: collections.Counter[str] = collections.Counter()
    if args.report is not None:
        with args.report.open(encoding="utf-8") as handle:
            verdicts = read_verdicts(handle)
    rendered = render_summary(gaps, verdicts, args.limit)

    if args.output is None:
        print(rendered, end="")
    else:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(rendered, encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
