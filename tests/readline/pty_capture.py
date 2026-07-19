#!/usr/bin/env python3

import errno
import os
import pty
import re
import select
import signal
import sys
import time


CSI_SEQUENCE = re.compile(rb"\x1b\[[0-9;?]*[A-Za-z~]")


def write_all(fd: int, data: bytes) -> None:
    offset = 0
    while offset < len(data):
        offset += os.write(fd, data[offset:])


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: pty_capture.py PROGRAM [ARG ...]", file=sys.stderr)
        return 2

    payload = sys.stdin.buffer.read()
    pid, master = pty.fork()
    if pid == 0:
        os.execvpe(sys.argv[1], sys.argv[1:], os.environ)

    output = bytearray()
    input_sent = not payload
    send_at = time.monotonic() + 0.1
    timed_out = False
    deadline = time.monotonic() + 10
    try:
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                timed_out = True
                os.kill(pid, signal.SIGKILL)
                break
            wait_for = remaining
            if not input_sent:
                wait_for = min(wait_for, max(0, send_at - time.monotonic()))
            ready, _, _ = select.select([master], [], [], wait_for)
            if not ready:
                if not input_sent and time.monotonic() >= send_at:
                    write_all(master, payload)
                    input_sent = True
                continue
            try:
                chunk = os.read(master, 4096)
            except OSError as error:
                if error.errno == errno.EIO:
                    break
                raise
            if not chunk:
                break
            output.extend(chunk)
            if not input_sent:
                write_all(master, payload)
                input_sent = True
    finally:
        os.close(master)

    _, status = os.waitpid(pid, 0)
    normalized = CSI_SEQUENCE.sub(b"", bytes(output)).replace(b"\r", b"")
    sys.stdout.buffer.write(normalized)
    if timed_out:
        print("PTY command timed out", file=sys.stderr)
        return 124
    return os.waitstatus_to_exitcode(status)


if __name__ == "__main__":
    raise SystemExit(main())
