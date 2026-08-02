#!/usr/bin/env python3
"""Compare interactive CherubSH behavior with the pinned Bash oracle."""

from __future__ import annotations

import argparse
import errno
import fcntl
import json
import os
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import termios
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path


PROMPT = "__PTY_PROMPT__ "
READY_MARKER = b"__PTY_READY__"
ANSI_PATTERN = re.compile(r"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07]*(?:\x07|\x1b\\))")
MARKER_PATTERN = re.compile(r"__PTY_[A-Z_]+(?::[^\n]*)?")


@dataclass(frozen=True)
class SendAction:
    data: bytes


@dataclass(frozen=True)
class PauseAction:
    seconds: float


@dataclass(frozen=True)
class WaitAction:
    marker: bytes
    after: bytes | None = None


@dataclass(frozen=True)
class ResizeAction:
    rows: int
    columns: int


@dataclass(frozen=True)
class SignalAction:
    signal_number: int


@dataclass(frozen=True)
class AbsentAction:
    marker: bytes
    seconds: float
    observation: str


Action = SendAction | PauseAction | WaitAction | ResizeAction | SignalAction | AbsentAction


@dataclass(frozen=True)
class Scenario:
    name: str
    actions: tuple[Action, ...]
    expected_markers: tuple[str, ...]
    observation_patterns: tuple[tuple[str, str], ...] = ()
    expected_exit: int = 0


@dataclass
class ShellResult:
    executable: str
    exit_status: int
    markers: list[str]
    observations: dict[str, object]
    raw_transcript: bytes
    transcript: str


class PtyTimeout(RuntimeError):
    """A bounded PTY operation exceeded its deadline."""


class ShellRunError(RuntimeError):
    def __init__(
        self,
        message: str,
        executable: Path,
        raw_transcript: bytes,
        transcript: str,
        timed_out: bool,
    ) -> None:
        super().__init__(message)
        self.executable = str(executable)
        self.raw_transcript = raw_transcript
        self.transcript = transcript
        self.timed_out = timed_out


def send(data: bytes) -> Action:
    return SendAction(data)


def pause(seconds: float) -> Action:
    return PauseAction(seconds)


def wait(marker: bytes, *, after: bytes | None = None) -> Action:
    return WaitAction(marker, after)


def resize(rows: int, columns: int) -> Action:
    return ResizeAction(rows, columns)


def signal_foreground(signal_number: int) -> Action:
    return SignalAction(signal_number)


def expect_absent(marker: bytes, seconds: float, observation: str) -> Action:
    return AbsentAction(marker, seconds, observation)


def build_scenarios() -> tuple[Scenario, ...]:
    return (
        Scenario(
            name="interrupt-recovery",
            actions=(
                send(b"sleep 10\n"),
                pause(0.15),
                send(b"\x03"),
                send(b"printf '__PTY_INTERRUPT__:recovered\\n'\n"),
                wait(b"__PTY_INTERRUPT__:recovered"),
                send(b"exit\n"),
            ),
            expected_markers=("__PTY_INTERRUPT__:recovered",),
            observation_patterns=(("recovery", r"^__PTY_INTERRUPT__:(recovered)$"),),
        ),
        Scenario(
            name="resize-sigwinch",
            actions=(
                send(b"printf 'resize-armed\\n'\n"),
                wait(b"resize-armed"),
                wait(PROMPT.encode(), after=b"resize-armed"),
                resize(40, 100),
                pause(0.1),
                send(
                    b"printf '__PTY_RESIZE__:%s:%s\\n' \"$LINES\" \"$COLUMNS\"\n"
                ),
                wait(b"__PTY_RESIZE__:40:100"),
                wait(PROMPT.encode(), after=b"__PTY_RESIZE__:40:100"),
                send(b"exit\n"),
            ),
            expected_markers=("__PTY_RESIZE__:40:100",),
            observation_patterns=(
                ("rows", r"^__PTY_RESIZE__:(40):100$"),
                ("columns", r"^__PTY_RESIZE__:40:(100)$"),
            ),
        ),
        Scenario(
            name="suspend-resume",
            actions=(
                send(b"set -m; printf 'monitor-ready\\n'\n"),
                wait(b"monitor-ready"),
                send(b"sleep 10\n"),
                pause(0.15),
                signal_foreground(signal.SIGTSTP),
                pause(0.15),
                send(
                    b"jobs -s >\"$HOME/jobs\"; cat \"$HOME/jobs\"; case \"$(cat \"$HOME/jobs\")\" in *Stopped*) state=stopped;; *) state=missing;; esac; printf '__PTY_SUSPEND__:%s\\n' \"$state\"\n"
                ),
                wait(b"__PTY_SUSPEND__:stopped"),
                send(b"fg\n"),
                pause(0.15),
                signal_foreground(signal.SIGINT),
                send(b"printf '__PTY_RESUME__:recovered\\n'\n"),
                wait(b"__PTY_RESUME__:recovered"),
                send(b"exit\n"),
            ),
            expected_markers=(
                "__PTY_SUSPEND__:stopped",
                "__PTY_RESUME__:recovered",
            ),
            observation_patterns=(
                (
                    "job-state",
                    r"\[(?:<JOB>|\d+)\][+-]?\s+(Stopped)\s+sleep 10",
                ),
                ("resume", r"^__PTY_RESUME__:(recovered)$"),
            ),
        ),
        Scenario(
            name="foreground-pipeline",
            actions=(
                send(
                    b"printf 'foo\\n' | tr o O | { read line; printf '__PTY_FOREGROUND__:%s\\n' \"$line\"; }\n"
                ),
                wait(b"__PTY_FOREGROUND__:fOO"),
                send(b"exit\n"),
            ),
            expected_markers=("__PTY_FOREGROUND__:fOO",),
            observation_patterns=(("pipeline-output", r"^__PTY_FOREGROUND__:(fOO)$"),),
        ),
        Scenario(
            name="background-pipeline",
            actions=(
                send(
                    b"{ sleep 0.05; printf x; } | { read value; [[ $value == x ]]; } & pipeline=$!; wait \"$pipeline\"; printf '__PTY_BACKGROUND__:%s\\n' \"$?\"\n"
                ),
                wait(b"__PTY_BACKGROUND__:0"),
                send(b"exit\n"),
            ),
            expected_markers=("__PTY_BACKGROUND__:0",),
            observation_patterns=(("pipeline-status", r"^__PTY_BACKGROUND__:(0)$"),),
        ),
        Scenario(
            name="eof",
            actions=(pause(0.05), send(b"\x04")),
            expected_markers=(),
        ),
        Scenario(
            name="unicode-editing",
            actions=(
                send(b"set -o emacs; printf 'unicode-ready\\n'\n"),
                wait(b"unicode-ready"),
                send(b"printf '%s%s:%s\\n' '__PTY_' 'UNICODE__' cafX"),
                pause(0.05),
                send(b"\x1b[D\xc3\xa9\x1b[3~\n"),
                wait("__PTY_UNICODE__:café".encode()),
                send(b"exit\n"),
            ),
            expected_markers=("__PTY_UNICODE__:café",),
            observation_patterns=(("edited-value", r"^__PTY_UNICODE__:(café)$"),),
        ),
        Scenario(
            name="bracketed-paste",
            actions=(
                send(b"bind 'set enable-bracketed-paste on'; printf 'paste-ready\\n'\n"),
                wait(b"paste-ready"),
                send(
                    b"\x1b[200~"
                    b"printf '%s%s\\n' '__PTY_' 'PASTE__:first'\n"
                    b"printf '%s%s\\n' '__PTY_' 'PASTE__:second'"
                    b"\x1b[201~"
                ),
                expect_absent(
                    b"__PTY_PASTE__:first",
                    0.2,
                    "paste-executed-before-submit",
                ),
                send(b"\n"),
                wait(b"__PTY_PASTE__:second"),
                send(b"exit\n"),
            ),
            expected_markers=("__PTY_PASTE__:first", "__PTY_PASTE__:second"),
            observation_patterns=(
                ("first-result", r"^(__PTY_PASTE__:first)$"),
                ("second-result", r"^(__PTY_PASTE__:second)$"),
            ),
        ),
        Scenario(
            name="vi-editing",
            actions=(
                send(b"set -o vi; printf 'vi-ready\\n'\n"),
                wait(b"vi-ready"),
                pause(0.05),
                send(b"printf '__PTY_VI__:%s\\n' wrld"),
                pause(0.05),
                send(b"\x1b"),
                pause(0.05),
                send(b"Fwao\n"),
                wait(b"__PTY_VI__:world"),
                send(b"exit\n"),
            ),
            expected_markers=("__PTY_VI__:world",),
            observation_patterns=(("edited-value", r"^__PTY_VI__:(world)$"),),
        ),
        Scenario(
            name="emacs-editing",
            actions=(
                send(b"set -o emacs; printf 'emacs-ready\\n'\n"),
                wait(b"emacs-ready"),
                pause(0.05),
                send(b"printf '__PTY_EMACS__:%s\\n' wrld"),
                pause(0.05),
                send(b"\x02\x02\x02o\n"),
                wait(b"__PTY_EMACS__:world"),
                send(b"exit\n"),
            ),
            expected_markers=("__PTY_EMACS__:world",),
            observation_patterns=(("edited-value", r"^__PTY_EMACS__:(world)$"),),
        ),
        Scenario(
            name="completion",
            actions=(
                send(
                    b"pick() { printf '__PTY_COMPLETION__:%s\\n' \"$1\"; }; complete -W 'alpha alpine beta' pick; printf 'completion-ready\\n'\n"
                ),
                wait(b"completion-ready"),
                pause(0.05),
                send(b"pick al"),
                pause(0.05),
                send(b"\tha\n"),
                wait(b"__PTY_COMPLETION__:alpha"),
                send(b"exit\n"),
            ),
            expected_markers=("__PTY_COMPLETION__:alpha",),
            observation_patterns=(
                ("submitted-command", r"__PTY_PROMPT__ pick (alpha)\n"),
                ("completed-value", r"^__PTY_COMPLETION__:(alpha)$"),
            ),
        ),
        Scenario(
            name="misc-read-nchars",
            actions=(
                send(
                    b"read -n 3 -p 'enter three chars: ' value; printf '\\n__PTY_MISC_READ_RAW__:%s\\n' \"$value\"\n"
                ),
                pause(0.05),
                send(b"abc"),
                wait(b"__PTY_MISC_READ_RAW__:abc"),
                send(
                    b"read -e -n 3 -p 'enter three chars: ' value; printf '__PTY_MISC_READ_EDIT__:%s\\n' \"$value\"\n"
                ),
                pause(0.05),
                send(b"xyz"),
                wait(b"__PTY_MISC_READ_EDIT__:xyz"),
                send(b"exit\n"),
            ),
            expected_markers=(
                "__PTY_MISC_READ_RAW__:abc",
                "__PTY_MISC_READ_EDIT__:xyz",
            ),
            observation_patterns=(
                ("raw-value", r"^__PTY_MISC_READ_RAW__:(abc)$"),
                ("readline-value", r"^__PTY_MISC_READ_EDIT__:(xyz)$"),
            ),
        ),
        Scenario(
            name="misc-redir-tty",
            actions=(
                send(
                    b"printf 'file-one\\nfile-three\\n' >\"$HOME/input\"; \"$PTY_SHELL\" --noprofile --norc -c 'read line1; exec 4<&0; exec 0</dev/tty; read line2; exec 0<&4; read line3; printf \"__PTY_MISC_TTY__:%s:%s:%s\\n\" \"$line1\" \"$line2\" \"$line3\"' <\"$HOME/input\"\n"
                ),
                pause(0.05),
                send(b"tty-two\n"),
                wait(b"__PTY_MISC_TTY__:file-one:tty-two:file-three"),
                send(b"exit\n"),
            ),
            expected_markers=("__PTY_MISC_TTY__:file-one:tty-two:file-three",),
            observation_patterns=(
                (
                    "redirected-lines",
                    r"^__PTY_MISC_TTY__:(file-one:tty-two:file-three)$",
                ),
            ),
        ),
    )


SCENARIOS = {scenario.name: scenario for scenario in build_scenarios()}
SCENARIO_NAMES = tuple(SCENARIOS)


def write_all(file_descriptor: int, data: bytes) -> None:
    offset = 0
    while offset < len(data):
        offset += os.write(file_descriptor, data[offset:])


def read_available(file_descriptor: int, output: bytearray, timeout: float) -> bool:
    ready, _, _ = select.select([file_descriptor], [], [], timeout)
    if not ready:
        return True
    try:
        chunk = os.read(file_descriptor, 4096)
    except OSError as error:
        if error.errno == errno.EIO:
            return False
        raise
    if not chunk:
        return False
    output.extend(chunk)
    return True


def read_until(
    file_descriptor: int,
    marker: bytes,
    output: bytearray,
    timeout: float,
    start: int = 0,
) -> None:
    deadline = time.monotonic() + timeout
    while marker not in output[start:]:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            tail = bytes(output[-2000:]).decode("utf-8", errors="replace")
            raise PtyTimeout(
                f"timed out waiting for {marker.decode(errors='replace')}; transcript tail: {tail!r}"
            )
        if not read_available(file_descriptor, output, remaining):
            raise RuntimeError("terminal closed before the expected marker")


def wait_for_exit(
    process_id: int,
    terminal: int,
    output: bytearray,
    timeout: float,
) -> int:
    deadline = time.monotonic() + timeout
    while True:
        exited_process, status = os.waitpid(process_id, os.WNOHANG)
        if exited_process == process_id:
            while read_available(terminal, output, 0):
                pass
            return os.waitstatus_to_exitcode(status)
        if time.monotonic() >= deadline:
            tail = bytes(output[-2000:]).decode("utf-8", errors="replace")
            raise PtyTimeout(f"interactive shell did not exit; transcript tail: {tail!r}")
        read_available(terminal, output, min(0.02, deadline - time.monotonic()))


def terminate_process_group(process_id: int) -> None:
    try:
        os.killpg(process_id, signal.SIGKILL)
    except ProcessLookupError:
        pass
    try:
        os.kill(process_id, signal.SIGKILL)
    except ProcessLookupError:
        pass


def reap_terminated_process(process_id: int, timeout: float = 1.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            exited_process, _ = os.waitpid(process_id, os.WNOHANG)
        except ChildProcessError:
            return
        if exited_process == process_id:
            return
        time.sleep(0.01)


def normalized_transcript(raw: bytes, temporary: str, executable: Path) -> str:
    text = raw.decode("utf-8", errors="replace").replace("\r", "")
    text = ANSI_PATTERN.sub("", text)
    text = text.replace("\x07", "")
    text = text.replace(temporary, "<HOME>")
    text = text.replace(str(executable), "<SHELL>")
    text = re.sub(r"\[(\d+)\]([+-])?\s+\d+", r"[<JOB>]\2 <PID>", text)
    return text


def extract_markers(transcript: str) -> list[str]:
    markers: list[str] = []
    for line in transcript.splitlines():
        candidate = line.strip()
        if candidate.startswith(PROMPT):
            candidate = candidate[len(PROMPT) :].strip()
        if MARKER_PATTERN.fullmatch(candidate) and candidate not in {
            READY_MARKER.decode(),
            PROMPT.strip(),
        }:
            markers.append(candidate)
    return markers


def extract_observations(
    scenario: Scenario,
    transcript: str,
    runtime_observations: dict[str, object],
) -> dict[str, object]:
    observations = dict(runtime_observations)
    for name, pattern in scenario.observation_patterns:
        matches = re.findall(pattern, transcript, flags=re.MULTILINE)
        observations[name] = matches
    return observations


def verify_pinned_bash(executable: Path) -> None:
    try:
        completed = subprocess.run(
            [str(executable), "--version"],
            check=False,
            capture_output=True,
            text=True,
            timeout=5,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise RuntimeError(f"cannot inspect Bash oracle {executable}: {error}") from error
    banner = completed.stdout.splitlines()[0] if completed.stdout else ""
    match = re.fullmatch(r"GNU bash, version ([^\s(]+)\([^)]*\)-release.*", banner)
    if completed.returncode != 0 or match is None or match.group(1) != "5.3.15":
        raise RuntimeError(
            f"Bash oracle must report GNU Bash 5.3.15, got {banner or 'no version banner'}"
        )


def run_shell(executable: Path, scenario: Scenario, timeout: float) -> ShellResult:
    executable = executable.resolve()
    if not executable.is_file() or not os.access(executable, os.X_OK):
        raise RuntimeError(f"executable not found: {executable}")

    with tempfile.TemporaryDirectory(prefix="cherubsh-pty-") as temporary:
        environment = {
            "HOME": temporary,
            "LANG": "C.UTF-8",
            "LC_ALL": "C.UTF-8",
            "PATH": "/usr/bin:/bin",
            "PS1": PROMPT,
            "TERM": "xterm-256color",
            "PTY_SHELL": str(executable),
        }
        for name in ("ASAN_OPTIONS", "LSAN_OPTIONS", "UBSAN_OPTIONS"):
            if value := os.environ.get(name):
                environment[name] = value

        process_id, terminal = pty.fork()
        if process_id == 0:
            os.execvpe(
                str(executable),
                [str(executable), "--noprofile", "--norc", "-i"],
                environment,
            )

        output = bytearray()
        runtime_observations: dict[str, object] = {}
        exited = False
        exit_status: int | None = None
        failure: OSError | RuntimeError | None = None
        try:
            write_all(terminal, b"PS1='__PTY_PROMPT__ '; printf '__PTY_READY__\\n'\n")
            read_until(terminal, READY_MARKER, output, timeout)
            ready_end = output.find(READY_MARKER) + len(READY_MARKER)
            read_until(terminal, PROMPT.encode(), output, timeout, ready_end)
            for action in scenario.actions:
                if isinstance(action, SendAction):
                    write_all(terminal, action.data)
                elif isinstance(action, PauseAction):
                    time.sleep(action.seconds)
                elif isinstance(action, WaitAction):
                    start = 0
                    if action.after is not None:
                        anchor = output.rfind(action.after)
                        if anchor < 0:
                            raise RuntimeError(
                                f"cannot wait after missing marker {action.after!r}"
                            )
                        start = anchor + len(action.after)
                    read_until(terminal, action.marker, output, timeout, start)
                elif isinstance(action, ResizeAction):
                    fcntl.ioctl(
                        terminal,
                        termios.TIOCSWINSZ,
                        struct.pack("HHHH", action.rows, action.columns, 0, 0),
                    )
                elif isinstance(action, SignalAction):
                    foreground_group = os.tcgetpgrp(terminal)
                    os.killpg(foreground_group, action.signal_number)
                elif isinstance(action, AbsentAction):
                    start = len(output)
                    deadline = time.monotonic() + action.seconds
                    while True:
                        remaining = deadline - time.monotonic()
                        if remaining <= 0:
                            break
                        read_available(terminal, output, min(0.02, remaining))
                        if action.marker in output[start:]:
                            raise RuntimeError(
                                f"observed {action.marker.decode(errors='replace')} before submission"
                            )
                    runtime_observations[action.observation] = False
                else:
                    raise RuntimeError(f"unknown PTY action: {action!r}")
            exit_status = wait_for_exit(process_id, terminal, output, timeout)
            exited = True
        except (OSError, RuntimeError) as error:
            failure = error
        finally:
            if not exited:
                terminate_process_group(process_id)
                reap_terminated_process(process_id)
            os.close(terminal)

        transcript = normalized_transcript(output, temporary, executable)
        if failure is not None:
            raise ShellRunError(
                str(failure),
                executable,
                bytes(output),
                transcript,
                isinstance(failure, PtyTimeout),
            ) from failure
        assert exit_status is not None
        return ShellResult(
            executable=str(executable),
            exit_status=exit_status,
            markers=extract_markers(transcript),
            observations=extract_observations(
                scenario,
                transcript,
                runtime_observations,
            ),
            raw_transcript=bytes(output),
            transcript=transcript,
        )


def compare_scenario(
    bash: Path,
    cherub: Path,
    scenario: Scenario,
    timeout: float,
    report_directory: Path,
) -> dict[str, object]:
    scenario_directory = report_directory / scenario.name
    scenario_directory.mkdir(parents=True, exist_ok=True)

    def execute_and_store(
        label: str, executable: Path
    ) -> tuple[ShellResult | None, dict[str, object], list[str]]:
        raw_name = f"{label}.raw"
        text_name = f"{label}.txt"
        try:
            result = run_shell(executable, scenario, timeout)
        except ShellRunError as error:
            (scenario_directory / raw_name).write_bytes(error.raw_transcript)
            (scenario_directory / text_name).write_text(
                error.transcript,
                encoding="utf-8",
            )
            return (
                None,
                {
                    "executable": error.executable,
                    "exit_status": None,
                    "markers": extract_markers(error.transcript),
                    "observations": extract_observations(scenario, error.transcript, {}),
                    "raw_transcript": raw_name,
                    "transcript": text_name,
                    "timed_out": error.timed_out,
                },
                [str(error)],
            )
        (scenario_directory / raw_name).write_bytes(result.raw_transcript)
        (scenario_directory / text_name).write_text(result.transcript, encoding="utf-8")
        return (
            result,
            {
                "executable": result.executable,
                "exit_status": result.exit_status,
                "markers": result.markers,
                "observations": result.observations,
                "raw_transcript": raw_name,
                "transcript": text_name,
                "timed_out": False,
            },
            [],
        )

    bash_result, bash_record, bash_failures = execute_and_store("bash", bash)
    cherub_result, cherub_record, cherub_failures = execute_and_store("cherub", cherub)

    expected = list(scenario.expected_markers)
    errors = [f"Bash: {error}" for error in bash_failures]
    errors.extend(f"CherubSH: {error}" for error in cherub_failures)
    if bash_result is not None and bash_result.markers != expected:
        errors.append(f"Bash markers {bash_result.markers!r} did not match {expected!r}")
    if cherub_result is not None and cherub_result.markers != expected:
        errors.append(f"CherubSH markers {cherub_result.markers!r} did not match {expected!r}")
    if (
        bash_result is not None
        and cherub_result is not None
        and bash_result.markers != cherub_result.markers
    ):
        errors.append("Bash and CherubSH markers differ")
    if bash_result is not None:
        missing = [
            name
            for name, _ in scenario.observation_patterns
            if not bash_result.observations.get(name)
        ]
        if missing:
            errors.append(f"Bash semantic observations missing: {missing!r}")
    if cherub_result is not None:
        missing = [
            name
            for name, _ in scenario.observation_patterns
            if not cherub_result.observations.get(name)
        ]
        if missing:
            errors.append(f"CherubSH semantic observations missing: {missing!r}")
    if (
        bash_result is not None
        and cherub_result is not None
        and bash_result.observations != cherub_result.observations
    ):
        errors.append(
            "Bash and CherubSH semantic observations differ: "
            f"{bash_result.observations!r} != {cherub_result.observations!r}"
        )
    if bash_result is not None and bash_result.exit_status != scenario.expected_exit:
        errors.append(
            f"Bash exited {bash_result.exit_status}, expected {scenario.expected_exit}"
        )
    if cherub_result is not None and cherub_result.exit_status != scenario.expected_exit:
        errors.append(
            f"CherubSH exited {cherub_result.exit_status}, expected {scenario.expected_exit}"
        )

    return {
        "scenario": scenario.name,
        "status": "FAIL" if errors else "PASS",
        "errors": errors,
        "bash": bash_record,
        "cherub": cherub_record,
    }


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bash", type=Path)
    parser.add_argument("--cherub", type=Path)
    parser.add_argument("--scenario", action="append", choices=SCENARIO_NAMES)
    parser.add_argument("--report-dir", type=Path, default=Path("target/pty-differential"))
    parser.add_argument("--timeout", type=float, default=5.0)
    parser.add_argument("--list", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    arguments = parser.parse_args()
    if arguments.timeout <= 0:
        parser.error("--timeout must be positive")
    if not (arguments.list or arguments.self_test):
        if arguments.bash is None:
            parser.error("--bash is required")
        if arguments.cherub is None:
            parser.error("--cherub is required")
    return arguments


def self_test() -> None:
    if not SCENARIOS:
        raise RuntimeError("the PTY scenario catalog must not be empty")
    if len(set(SCENARIO_NAMES)) != len(SCENARIO_NAMES):
        raise RuntimeError("PTY scenario names must be unique")
    if any(not name or name.strip() != name for name in SCENARIO_NAMES):
        raise RuntimeError("PTY scenario names must be nonempty and normalized")
    supported_actions = (
        SendAction,
        PauseAction,
        WaitAction,
        ResizeAction,
        SignalAction,
        AbsentAction,
    )
    for name, scenario in SCENARIOS.items():
        if scenario.name != name or not scenario.actions:
            raise RuntimeError(f"invalid PTY scenario: {name}")
        if any(not isinstance(action, supported_actions) for action in scenario.actions):
            raise RuntimeError(f"unknown action in PTY scenario: {name}")

    resize_marker = b"__PTY_RESIZE__:40:100"
    resize_actions = SCENARIOS["resize-sigwinch"].actions
    marker_index = resize_actions.index(WaitAction(resize_marker))
    prompt_barrier = WaitAction(PROMPT.encode(), after=resize_marker)
    if resize_actions[marker_index + 1] != prompt_barrier:
        raise RuntimeError("resize scenario must wait for its prompt before sending exit")

    unicode_scenario = SCENARIOS["unicode-editing"]
    unicode_marker_prefixes = tuple(
        marker.partition(":")[0].encode() for marker in unicode_scenario.expected_markers
    )
    if any(
        prefix in action.data
        for action in unicode_scenario.actions
        if isinstance(action, SendAction)
        for prefix in unicode_marker_prefixes
    ):
        raise RuntimeError("unicode marker prefix must not appear in echoed input")


def main() -> int:
    arguments = parse_arguments()
    if arguments.list:
        print("\n".join(SCENARIO_NAMES))
        return 0
    if arguments.self_test:
        try:
            self_test()
        except RuntimeError as error:
            print(f"PTY differential self-test failed: {error}", file=sys.stderr)
            return 1
        print(f"PTY differential self-test: {len(SCENARIO_NAMES)} scenarios passed")
        return 0

    assert arguments.bash is not None
    assert arguments.cherub is not None
    try:
        verify_pinned_bash(arguments.bash.resolve())
    except RuntimeError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    selected = arguments.scenario or list(SCENARIO_NAMES)
    report_directory = arguments.report_dir.resolve()
    report_directory.mkdir(parents=True, exist_ok=True)
    results: list[dict[str, object]] = []
    failed = False
    for name in selected:
        try:
            result = compare_scenario(
                arguments.bash,
                arguments.cherub,
                SCENARIOS[name],
                arguments.timeout,
                report_directory,
            )
        except (OSError, RuntimeError) as error:
            result = {"scenario": name, "status": "FAIL", "errors": [str(error)]}
        results.append(result)
        if result["status"] == "PASS":
            print(f"PASS {name}")
        else:
            failed = True
            print(f"FAIL {name}", file=sys.stderr)
            for error in result["errors"]:
                print(f"  {error}", file=sys.stderr)
    (report_directory / "report.json").write_text(
        json.dumps(results, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    if failed:
        print(f"PTY differential failed; report: {report_directory}", file=sys.stderr)
        return 1
    print(f"PTY differential: {len(results)} scenarios passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
