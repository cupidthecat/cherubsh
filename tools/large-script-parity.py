#!/usr/bin/env python3
"""Compare pinned real-world Bash sources without checking them out."""

from __future__ import annotations

import argparse
from collections import Counter
import os
from dataclasses import dataclass
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
from urllib.parse import urlparse


COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")
PROJECT_RE = re.compile(r"^[a-z0-9][a-z0-9-]*$")
GITHUB_PATH_RE = re.compile(r"^/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+\.git$")
REGULAR_MODES = {"100644", "100755"}
POLICIES = {"required", "optional-safety"}
SHELL_INTERPRETERS = {"bash", "dash", "sh"}


class CorpusError(RuntimeError):
    """A corpus operation could not be completed."""


class SafetyError(CorpusError):
    """A repository cannot be handled within the data-only boundary."""


@dataclass(frozen=True)
class Project:
    name: str
    repository: str
    commit: str
    policy: str


@dataclass(frozen=True)
class TreeEntry:
    mode: str
    kind: str
    object_id: str
    path: bytes


@dataclass(frozen=True)
class ShellResult:
    state: str
    status: int | None
    stderr: bytes


@dataclass(frozen=True)
class ReportRow:
    verdict: str
    project: str
    revision: str
    path: bytes
    bash: ShellResult
    cherub: ShellResult


@dataclass(frozen=True)
class ProjectResult:
    rows: list[ReportRow]
    inventory: Counter[str]


def git_output(
    arguments: list[str],
    *,
    input_bytes: bytes | None = None,
    environment: dict[str, str] | None = None,
) -> bytes:
    completed = subprocess.run(
        ["git", *arguments],
        input=input_bytes,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=environment,
        check=False,
    )
    if completed.returncode != 0:
        command = " ".join(["git", *arguments])
        diagnostic = completed.stderr.decode("utf-8", "replace").strip()
        raise CorpusError(f"{command} failed: {diagnostic}")
    return completed.stdout


def load_manifest(path: Path) -> list[Project]:
    projects: list[Project] = []
    names: set[str] = set()
    for line_number, raw_line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not raw_line or raw_line.startswith("#"):
            continue
        fields = raw_line.split("\t")
        if len(fields) != 4:
            raise CorpusError(f"{path}:{line_number}: expected four tab-separated fields")
        name, repository, commit, policy = fields
        if not PROJECT_RE.fullmatch(name) or name in names:
            raise CorpusError(f"{path}:{line_number}: invalid or duplicate project name")
        if not valid_repository_url(repository):
            raise SafetyError(f"{path}:{line_number}: repository must be an HTTPS GitHub URL")
        if not COMMIT_RE.fullmatch(commit):
            raise CorpusError(f"{path}:{line_number}: commit must be 40 lowercase hex digits")
        if policy not in POLICIES:
            raise CorpusError(f"{path}:{line_number}: unsupported policy {policy!r}")
        names.add(name)
        projects.append(Project(name, repository, commit, policy))
    if not projects:
        raise CorpusError(f"{path}: manifest contains no projects")
    return projects


def valid_repository_url(repository: str) -> bool:
    parsed = urlparse(repository)
    return (
        parsed.scheme == "https"
        and parsed.netloc == "github.com"
        and parsed.params == ""
        and parsed.query == ""
        and parsed.fragment == ""
        and GITHUB_PATH_RE.fullmatch(parsed.path) is not None
    )


def git_object_exists(repo: Path, object_name: str) -> bool:
    completed = subprocess.run(
        ["git", "-C", str(repo), "cat-file", "-e", object_name],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    return completed.returncode == 0


def ensure_object_store(project: Project, cache: Path) -> Path:
    if not valid_repository_url(project.repository):
        raise SafetyError(f"{project.name}: repository must be an HTTPS GitHub URL")
    if not COMMIT_RE.fullmatch(project.commit):
        raise CorpusError(f"{project.name}: invalid pinned commit")

    if cache.is_symlink():
        raise SafetyError("cache directory must not be a symlink")
    if cache.exists() and not cache.is_dir():
        raise SafetyError("cache path is not a directory")
    cache.mkdir(parents=True, exist_ok=True)
    repo = cache / f"{project.name}.git"
    if repo.is_symlink():
        raise SafetyError(f"{project.name}: object-store path must not be a symlink")
    if not repo.exists():
        git_output(["init", "--bare", "--quiet", str(repo)])
    elif not repo.is_dir():
        raise SafetyError(f"{project.name}: object-store path is not a directory")

    bare = git_output(["-C", str(repo), "rev-parse", "--is-bare-repository"]).strip()
    if bare != b"true":
        raise SafetyError(f"{project.name}: cache is not a bare Git object store")

    commit_object = f"{project.commit}^{{commit}}"
    if not git_object_exists(repo, commit_object):
        git_output(
            [
                "-c",
                "protocol.file.allow=never",
                "-C",
                str(repo),
                "fetch",
                "--no-tags",
                "--depth=1",
                project.repository,
                project.commit,
            ]
        )
        actual = git_output(["-C", str(repo), "rev-parse", "FETCH_HEAD^{commit}"])
        if actual.decode("ascii").strip() != project.commit:
            raise CorpusError(
                f"{project.name}: fetched commit does not match {project.commit}"
            )
        git_output(
            [
                "-C",
                str(repo),
                "update-ref",
                f"refs/cherubsh/pinned/{project.name}",
                project.commit,
            ]
        )
    return repo


def list_tree(repo: Path, commit: str) -> list[TreeEntry]:
    output = git_output(
        ["-C", str(repo), "ls-tree", "-r", "-z", "--full-tree", commit]
    )
    entries: list[TreeEntry] = []
    for record in output.split(b"\0"):
        if not record:
            continue
        try:
            metadata, path = record.split(b"\t", 1)
            mode, kind, object_id = metadata.split(b" ", 2)
        except ValueError as error:
            raise SafetyError("git ls-tree returned a malformed record") from error
        entries.append(
            TreeEntry(
                mode.decode("ascii"),
                kind.decode("ascii"),
                object_id.decode("ascii"),
                path,
            )
        )
    return sorted(entries, key=lambda entry: entry.path)


def read_blob(repo: Path, object_id: str) -> bytes:
    if not COMMIT_RE.fullmatch(object_id):
        raise SafetyError("tree contains an invalid object ID")
    return git_output(["-C", str(repo), "cat-file", "blob", object_id])


def shell_shebang(data: bytes) -> bool:
    first_line = data.split(b"\n", 1)[0][:512]
    if not first_line.startswith(b"#!"):
        return False
    try:
        words = shlex.split(first_line[2:].decode("utf-8"))
    except (UnicodeDecodeError, ValueError):
        return False
    if not words:
        return False
    interpreter = os.path.basename(words[0])
    if interpreter in SHELL_INTERPRETERS:
        return True
    if interpreter != "env":
        return False
    arguments = words[1:]
    if arguments[:1] == ["-S"]:
        arguments = arguments[1:]
    while arguments and (arguments[0].startswith("-") or "=" in arguments[0]):
        arguments = arguments[1:]
    return bool(arguments) and os.path.basename(arguments[0]) in SHELL_INTERPRETERS


def is_shell_blob(path: bytes, data: bytes) -> bool:
    return path.endswith((b".sh", b".bash")) or shell_shebang(data)


def selected_shell_blobs(repo: Path, entries: list[TreeEntry]) -> list[tuple[TreeEntry, bytes]]:
    selected: list[tuple[TreeEntry, bytes]] = []
    for entry in entries:
        if entry.kind != "blob" or entry.mode not in REGULAR_MODES:
            continue
        data = read_blob(repo, entry.object_id)
        if is_shell_blob(entry.path, data):
            selected.append((entry, data))
    return selected


def run_shell(
    binary: Path,
    kind: str,
    source: bytes,
    timeout: float,
    cwd: Path,
) -> ShellResult:
    if kind == "bash":
        arguments = [
            str(binary),
            "--noprofile",
            "--norc",
            "-O",
            "extglob",
            "-n",
            "-s",
        ]
    elif kind == "cherub":
        arguments = [str(binary), "--norc", "-O", "extglob", "-n", "-s"]
    else:
        raise ValueError(f"unsupported shell kind: {kind}")
    environment = {
        "HOME": str(cwd),
        "PATH": "/usr/bin:/bin",
        "LC_ALL": "C",
        "BASH_ENV": "/dev/null",
        "ENV": "/dev/null",
    }
    try:
        completed = subprocess.run(
            arguments,
            input=source,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            cwd=cwd,
            env=environment,
            timeout=timeout,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        stderr = error.stderr if isinstance(error.stderr, bytes) else b""
        return ShellResult("timeout", None, stderr)
    state = "accept" if completed.returncode == 0 else "reject"
    return ShellResult(state, completed.returncode, completed.stderr)


def compare_results(bash: ShellResult, cherub: ShellResult) -> str:
    if bash.state == "timeout" or cherub.state == "timeout":
        return "TIMEOUT"
    if bash.state == cherub.state:
        return "PASS"
    return "FAIL"


def shell_result_field(result: ShellResult) -> str:
    status = "-" if result.status is None else str(result.status)
    return f"{result.state}:{status}"


def escape_report_path(path: bytes) -> str:
    return (
        path.decode("utf-8", "surrogateescape")
        .encode("unicode_escape")
        .decode("ascii")
        .replace("\\t", "\\x09")
    )


def write_report(path: Path, rows: list[ReportRow]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="\n") as report:
        report.write(
            "verdict\tproject\trevision\tpath\tbash_result\tcherub_result"
            "\tbash_timeout\tcherub_timeout\n"
        )
        for row in sorted(rows, key=lambda item: (item.project, item.path)):
            report.write(
                "\t".join(
                    [
                        row.verdict,
                        row.project,
                        row.revision,
                        escape_report_path(row.path),
                        shell_result_field(row.bash),
                        shell_result_field(row.cherub),
                        str(row.bash.state == "timeout").lower(),
                        str(row.cherub.state == "timeout").lower(),
                    ]
                )
                + "\n"
            )


def inventory(entries: list[TreeEntry]) -> Counter[str]:
    counts: Counter[str] = Counter()
    for entry in entries:
        if entry.kind == "commit" or entry.mode == "160000":
            counts["submodule"] += 1
        elif entry.mode == "120000":
            counts["symlink"] += 1
        elif entry.kind == "blob" and entry.mode in REGULAR_MODES:
            counts["regular"] += 1
        else:
            counts["unsupported"] += 1
    return counts


def assert_empty_working_directory(directory: Path) -> None:
    created = list(directory.iterdir())
    if created:
        names = ", ".join(path.name for path in created[:5])
        raise SafetyError(f"no-execution shell created files: {names}")


def run_project(
    project: Project,
    cache: Path,
    bash: Path,
    cherub: Path,
    timeout: float,
    working_root: Path,
) -> ProjectResult:
    repo = ensure_object_store(project, cache)
    entries = list_tree(repo, project.commit)
    counts = inventory(entries)
    selected = selected_shell_blobs(repo, entries)
    counts["selected"] = len(selected)
    if not selected:
        raise CorpusError(f"{project.name}: pinned tree contains no selected shell files")
    rows: list[ReportRow] = []
    working_directory = working_root / project.name
    working_directory.mkdir()
    for entry, source in selected:
        bash_result = run_shell(bash, "bash", source, timeout, working_directory)
        assert_empty_working_directory(working_directory)
        cherub_result = run_shell(cherub, "cherub", source, timeout, working_directory)
        assert_empty_working_directory(working_directory)
        rows.append(
            ReportRow(
                compare_results(bash_result, cherub_result),
                project.name,
                project.commit,
                entry.path,
                bash_result,
                cherub_result,
            )
        )
    return ProjectResult(rows, counts)


def executable_path(path: Path, label: str) -> Path:
    resolved = path.expanduser().resolve()
    if not resolved.is_file() or not os.access(resolved, os.X_OK):
        raise CorpusError(f"{label} is not an executable file: {path}")
    return resolved


def skipped_row(project: Project, verdict: str, reason: str) -> ReportRow:
    unavailable = ShellResult("skip", None, reason.encode("utf-8", "replace"))
    label = f"reason={reason}".encode("utf-8", "replace")
    return ReportRow(verdict, project.name, project.commit, label, unavailable, unavailable)


def run_corpus(arguments: argparse.Namespace) -> int:
    manifest = Path(arguments.manifest).resolve()
    cache = Path(os.path.abspath(arguments.cache_dir))
    report_directory = Path(arguments.report_dir).resolve()
    bash = executable_path(Path(arguments.bash), "Bash oracle")
    cherub = executable_path(Path(arguments.cherub), "CherubSH")
    if arguments.timeout <= 0:
        raise CorpusError("timeout must be greater than zero")

    projects = load_manifest(manifest)
    if arguments.project:
        requested = set(arguments.project)
        available = {project.name for project in projects}
        unknown = sorted(requested - available)
        if unknown:
            raise CorpusError(f"unknown project selection: {', '.join(unknown)}")
        projects = [project for project in projects if project.name in requested]

    rows: list[ReportRow] = []
    failed = False
    with tempfile.TemporaryDirectory(prefix="cherubsh-large-script-run-") as raw_work:
        working_root = Path(raw_work)
        for project in projects:
            try:
                result = run_project(
                    project,
                    cache,
                    bash,
                    cherub,
                    arguments.timeout,
                    working_root,
                )
            except SafetyError as error:
                if project.policy == "optional-safety":
                    rows.append(skipped_row(project, "SKIP", str(error)))
                    print(f"{project.name}: SKIP ({error})")
                    continue
                rows.append(skipped_row(project, "ERROR", str(error)))
                print(f"{project.name}: ERROR ({error})", file=sys.stderr)
                failed = True
                continue
            except CorpusError as error:
                rows.append(skipped_row(project, "ERROR", str(error)))
                print(f"{project.name}: ERROR ({error})", file=sys.stderr)
                failed = True
                continue

            rows.extend(result.rows)
            verdicts = Counter(row.verdict for row in result.rows)
            print(
                f"{project.name}: selected={result.inventory['selected']} "
                f"pass={verdicts['PASS']} fail={verdicts['FAIL']} "
                f"timeout={verdicts['TIMEOUT']}"
            )
            if verdicts["FAIL"] or verdicts["TIMEOUT"]:
                failed = True
                for row in result.rows:
                    if row.verdict in {"FAIL", "TIMEOUT"}:
                        label = escape_report_path(row.path)
                        print(
                            f"  {row.verdict} {label}: "
                            f"bash={shell_result_field(row.bash)} "
                            f"cherub={shell_result_field(row.cherub)}",
                            file=sys.stderr,
                        )

    report_path = report_directory / "report.tsv"
    write_report(report_path, rows)
    totals = Counter(row.verdict for row in rows)
    print(
        "large-script parity: "
        f"pass={totals['PASS']} fail={totals['FAIL']} "
        f"timeout={totals['TIMEOUT']} skip={totals['SKIP']} "
        f"error={totals['ERROR']} report={report_path}"
    )
    return 1 if failed else 0


def self_test_git_environment() -> dict[str, str]:
    environment = dict(os.environ)
    environment.update(
        {
            "GIT_AUTHOR_NAME": "Corpus Self Test",
            "GIT_AUTHOR_EMAIL": "corpus@example.invalid",
            "GIT_COMMITTER_NAME": "Corpus Self Test",
            "GIT_COMMITTER_EMAIL": "corpus@example.invalid",
            "GIT_AUTHOR_DATE": "2000-01-01T00:00:00Z",
            "GIT_COMMITTER_DATE": "2000-01-01T00:00:00Z",
        }
    )
    return environment


def store_blob(repo: Path, data: bytes) -> str:
    return git_output(
        ["-C", str(repo), "hash-object", "-w", "--stdin"], input_bytes=data
    ).decode("ascii").strip()


def run_self_test() -> None:
    checks = 0
    with tempfile.TemporaryDirectory(prefix="cherubsh-large-script-self-test-") as raw_root:
        root = Path(raw_root)
        valid_manifest = root / "valid.lock"
        valid_manifest.write_text(
            "sample\thttps://github.com/example/sample.git\t"
            + "1" * 40
            + "\trequired\n",
            encoding="utf-8",
        )
        assert load_manifest(valid_manifest)[0].name == "sample"
        checks += 1

        invalid_manifest = root / "invalid.lock"
        invalid_manifest.write_text(
            "sample\thttps://github.com/example/sample.git\tbad\trequired\n",
            encoding="utf-8",
        )
        try:
            load_manifest(invalid_manifest)
        except CorpusError:
            pass
        else:
            raise AssertionError("invalid commit was accepted")
        checks += 1

        cache = root / "cache"
        repo = cache / "sample.git"
        repo.parent.mkdir()
        git_output(["init", "--bare", "--quiet", str(repo)])
        shell_blob = store_blob(repo, b"printf '%s\\n' ok\n")
        bash_blob = store_blob(repo, b"#!/usr/bin/env bash\nprintf ok\n")
        text_blob = store_blob(repo, b"plain text\n")
        link_blob = store_blob(repo, b"z.sh")

        base_tree_input = (
            f"100644 blob {text_blob}\tnotes.txt\0"
            f"100644 blob {bash_blob}\trunner\0"
            f"100644 blob {shell_blob}\tz.sh\0"
            f"100644 blob {shell_blob}\ta.bash\0"
        ).encode("ascii")
        base_tree = git_output(
            ["-C", str(repo), "mktree", "-z"], input_bytes=base_tree_input
        ).decode("ascii").strip()
        base_commit = git_output(
            ["-C", str(repo), "commit-tree", base_tree],
            input_bytes=b"base fixture\n",
            environment=self_test_git_environment(),
        ).decode("ascii").strip()
        final_tree_input = (
            f"100644 blob {text_blob}\tnotes.txt\0"
            f"100644 blob {bash_blob}\trunner\0"
            f"120000 blob {link_blob}\tlink.sh\0"
            f"160000 commit {base_commit}\tsubmodule\0"
            f"100644 blob {shell_blob}\tz.sh\0"
            f"100644 blob {shell_blob}\ta.bash\0"
        ).encode("ascii")
        final_tree = git_output(
            ["-C", str(repo), "mktree", "-z"], input_bytes=final_tree_input
        ).decode("ascii").strip()
        commit = git_output(
            ["-C", str(repo), "commit-tree", final_tree],
            input_bytes=b"final fixture\n",
            environment=self_test_git_environment(),
        ).decode("ascii").strip()

        project = Project(
            "sample", "https://github.com/example/sample.git", commit, "required"
        )
        assert ensure_object_store(project, cache) == repo
        entries = list_tree(repo, commit)
        assert [entry.path for entry in entries] == sorted(entry.path for entry in entries)
        checks += 1

        selected = selected_shell_blobs(repo, entries)
        selected_paths = [entry.path for entry, _ in selected]
        assert selected_paths == [b"a.bash", b"runner", b"z.sh"]
        assert b"a.bash" in selected_paths and b"z.sh" in selected_paths
        checks += 1

        assert b"runner" in selected_paths
        assert is_shell_blob(b"env-runner", b"#!/usr/bin/env -S bash -e\n")
        checks += 1

        assert is_shell_blob(b"posix-runner", b"#!/bin/sh\nprintf ok\n")
        assert is_shell_blob(b"env-posix-runner", b"#!/usr/bin/env sh\nprintf ok\n")
        checks += 1

        assert b"link.sh" not in selected_paths and b"submodule" not in selected_paths
        checks += 1

        escape = root.parent / "escape.sh"
        assert is_shell_blob(b"../../escape.sh", b":\n")
        assert not escape.exists()
        skip = skipped_row(project, "SKIP", "safety boundary")
        assert escape_report_path(skip.path) == "reason=safety boundary"
        checks += 1

        accepted = ShellResult("accept", 0, b"")
        rejected = ShellResult("reject", 2, b"syntax error")
        assert compare_results(accepted, accepted) == "PASS"
        checks += 1

        assert compare_results(rejected, rejected) == "PASS"
        checks += 1

        assert compare_results(accepted, rejected) == "FAIL"
        checks += 1

        timeout_shell = root / "timeout-shell"
        timeout_shell.write_text(
            "#!/usr/bin/python3\nimport time\ntime.sleep(1)\n", encoding="utf-8"
        )
        timeout_shell.chmod(0o755)
        timeout_work = root / "timeout-work"
        timeout_work.mkdir()
        timed_out = run_shell(timeout_shell, "cherub", b":\n", 0.01, timeout_work)
        assert timed_out.state == "timeout"
        assert compare_results(accepted, timed_out) == "TIMEOUT"
        checks += 1

        bash_value = os.environ.get("BASH_ORACLE_PATH") or shutil.which("bash")
        if not bash_value:
            raise AssertionError("self-test requires Bash")
        cherub_value = os.environ.get("CHERUBSH_BIN")
        if not cherub_value:
            cherub_value = str(Path(__file__).resolve().parent.parent / "target/debug/cherubsh")
        bash_binary = executable_path(Path(bash_value), "self-test Bash")
        cherub_binary = executable_path(Path(cherub_value), "self-test CherubSH")
        noexec_work = root / "noexec-work"
        noexec_work.mkdir()
        extglob_source = b"case 123 in +([0-9])) : ;; esac\n"
        bash_extglob = run_shell(
            bash_binary, "bash", extglob_source, 2.0, noexec_work
        )
        cherub_extglob = run_shell(
            cherub_binary, "cherub", extglob_source, 2.0, noexec_work
        )
        assert bash_extglob.state == "accept" and cherub_extglob.state == "accept"
        assert_empty_working_directory(noexec_work)
        checks += 1

        canary = noexec_work / "must-not-exist"
        quoted_canary = shlex.quote(str(canary))
        source = (
            f"touch -- {quoted_canary}\n"
            f"value=$(touch -- {quoted_canary})\n"
            f": > {quoted_canary}\n"
            f"source {quoted_canary}\n"
        ).encode("utf-8")
        bash_result = run_shell(bash_binary, "bash", source, 2.0, noexec_work)
        cherub_result = run_shell(cherub_binary, "cherub", source, 2.0, noexec_work)
        assert bash_result.state == "accept" and cherub_result.state == "accept"
        assert not canary.exists()
        assert_empty_working_directory(noexec_work)
        checks += 1

    assert checks == 14
    print(f"large-script parity self-test: {checks} checks passed")


def parse_arguments() -> argparse.Namespace:
    root = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", default=root / "large-scripts.lock", type=Path)
    parser.add_argument(
        "--cache-dir", default=root / "target/upstream/large-scripts", type=Path
    )
    parser.add_argument(
        "--report-dir", default=root / "target/hardening/large-scripts", type=Path
    )
    parser.add_argument(
        "--bash",
        default=os.environ.get(
            "BASH_ORACLE_PATH", root / "target/oracle/bash-5.3.15/bash"
        ),
        type=Path,
    )
    parser.add_argument(
        "--cherub",
        default=os.environ.get("CHERUBSH_BIN", root / "target/debug/cherubsh"),
        type=Path,
    )
    parser.add_argument("--timeout", default=5.0, type=float)
    parser.add_argument("--project", action="append")
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def main() -> int:
    arguments = parse_arguments()
    if arguments.self_test:
        run_self_test()
        return 0
    return run_corpus(arguments)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except CorpusError as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(2) from error
