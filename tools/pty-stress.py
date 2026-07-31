#!/usr/bin/env python3
"""Check that an interactive CherubSH session recovers after Ctrl-C."""

from __future__ import annotations

import argparse
import errno
import os
import pty
import select
import signal
import sys
import tempfile
import time
from pathlib import Path


def write_all(file_descriptor: int, data: bytes) -> None:
    offset = 0
    while offset < len(data):
        offset += os.write(file_descriptor, data[offset:])


def read_until(
    file_descriptor: int,
    marker: bytes,
    output: bytearray,
    timeout: float,
) -> None:
    deadline = time.monotonic() + timeout
    while marker not in output:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise RuntimeError(f"timed out waiting for {marker.decode()}")
        ready, _, _ = select.select([file_descriptor], [], [], remaining)
        if not ready:
            continue
        try:
            chunk = os.read(file_descriptor, 4096)
        except OSError as error:
            if error.errno == errno.EIO:
                raise RuntimeError("terminal closed before the expected marker") from error
            raise
        if not chunk:
            raise RuntimeError("terminal closed before the expected marker")
        output.extend(chunk)


def terminate_process_group(process_id: int) -> None:
    try:
        os.killpg(process_id, signal.SIGKILL)
    except ProcessLookupError:
        return


def wait_for_exit(process_id: int, timeout: float) -> int:
    deadline = time.monotonic() + timeout
    while True:
        exited_process, status = os.waitpid(process_id, os.WNOHANG)
        if exited_process == process_id:
            return os.waitstatus_to_exitcode(status)
        if time.monotonic() >= deadline:
            raise RuntimeError("interactive shell did not exit")
        time.sleep(0.02)


def run_round(cherub: Path, timeout: float) -> None:
    with tempfile.TemporaryDirectory(prefix="cherubsh-pty-") as temporary:
        environment = {
            "HOME": temporary,
            "LANG": "C",
            "LC_ALL": "C",
            "PATH": "/usr/bin:/bin",
            "TERM": "xterm-256color",
        }
        for name in ("ASAN_OPTIONS", "LSAN_OPTIONS", "UBSAN_OPTIONS"):
            if value := os.environ.get(name):
                environment[name] = value
        process_id, terminal = pty.fork()
        if process_id == 0:
            os.execvpe(str(cherub), [str(cherub), "--norc", "-i"], environment)

        output = bytearray()
        exited = False
        try:
            time.sleep(0.15)
            write_all(
                terminal,
                b"printf '%s%s\\n' '__CHERUB_PTY_' 'READY__'\n",
            )
            read_until(terminal, b"__CHERUB_PTY_READY__", output, timeout)

            write_all(terminal, b"sleep 10\n")
            time.sleep(0.15)
            write_all(terminal, b"\x03")
            write_all(
                terminal,
                b"printf '%s%s\\n' '__CHERUB_PTY_' 'RECOVERED__'\nexit\n",
            )
            read_until(terminal, b"__CHERUB_PTY_RECOVERED__", output, timeout)
            if wait_for_exit(process_id, timeout) != 0:
                raise RuntimeError("interactive shell exited with a nonzero status")
            exited = True
        finally:
            if not exited:
                terminate_process_group(process_id)
                try:
                    os.waitpid(process_id, 0)
                except ChildProcessError:
                    pass
            os.close(terminal)


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cherub", type=Path, required=True)
    parser.add_argument("--rounds", type=int, default=5)
    parser.add_argument("--timeout", type=float, default=5.0)
    arguments = parser.parse_args()
    if arguments.rounds < 1:
        parser.error("--rounds must be positive")
    if arguments.timeout <= 0:
        parser.error("--timeout must be positive")
    return arguments


def main() -> int:
    arguments = parse_arguments()
    if not arguments.cherub.is_file() or not os.access(arguments.cherub, os.X_OK):
        print(f"error: executable not found: {arguments.cherub}", file=sys.stderr)
        return 2
    try:
        for _ in range(arguments.rounds):
            run_round(arguments.cherub, arguments.timeout)
    except RuntimeError as error:
        print(f"PTY stress failed: {error}", file=sys.stderr)
        return 1
    print(f"PTY stress: {arguments.rounds} rounds passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
