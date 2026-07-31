#!/usr/bin/env python3
"""Repeat the PTY interrupt-recovery differential scenario."""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--bash",
        type=Path,
        default=Path(__file__).resolve().parent.parent
        / "target/oracle/bash-5.3.15/bash",
    )
    parser.add_argument("--cherub", type=Path, required=True)
    parser.add_argument("--rounds", type=int, default=5)
    parser.add_argument("--timeout", type=float, default=5.0)
    parser.add_argument(
        "--report-dir",
        type=Path,
        default=Path("target/pty-differential/stress"),
    )
    arguments = parser.parse_args()
    if arguments.rounds < 1:
        parser.error("--rounds must be positive")
    if arguments.timeout <= 0:
        parser.error("--timeout must be positive")
    return arguments


def main() -> int:
    arguments = parse_arguments()
    differential = Path(__file__).with_name("pty-differential.py")
    for round_number in range(1, arguments.rounds + 1):
        command = [
            sys.executable,
            str(differential),
            "--bash",
            str(arguments.bash),
            "--cherub",
            str(arguments.cherub),
            "--scenario",
            "interrupt-recovery",
            "--timeout",
            str(arguments.timeout),
            "--report-dir",
            str(arguments.report_dir / f"round-{round_number}"),
        ]
        completed = subprocess.run(command, check=False, capture_output=True, text=True)
        if completed.returncode != 0:
            if completed.stdout:
                print(completed.stdout, end="", file=sys.stderr)
            if completed.stderr:
                print(completed.stderr, end="", file=sys.stderr)
            print(f"PTY stress failed in round {round_number}", file=sys.stderr)
            return 1
    print(f"PTY stress: {arguments.rounds} rounds passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
