#!/usr/bin/env python3
"""Compare pinned real-world Bash sources without checking them out."""

from __future__ import annotations

import argparse
import os
from dataclasses import dataclass
from pathlib import Path
import re
import shlex
import subprocess
import tempfile
from urllib.parse import urlparse


COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")
REGULAR_MODES = {"100644", "100755"}
POLICIES = {"required", "optional-safety"}


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
        if not name or name in names:
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
        and parsed.path.endswith(".git")
        and len([part for part in parsed.path.split("/") if part]) == 2
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

    cache.mkdir(parents=True, exist_ok=True)
    repo = cache / f"{project.name}.git"
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


def bash_shebang(data: bytes) -> bool:
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
    if interpreter == "bash":
        return True
    if interpreter != "env":
        return False
    arguments = words[1:]
    if arguments[:1] == ["-S"]:
        arguments = arguments[1:]
    while arguments and (arguments[0].startswith("-") or "=" in arguments[0]):
        arguments = arguments[1:]
    return bool(arguments) and os.path.basename(arguments[0]) == "bash"


def is_shell_blob(path: bytes, data: bytes) -> bool:
    return path.endswith((b".sh", b".bash")) or bash_shebang(data)


def selected_shell_blobs(repo: Path, entries: list[TreeEntry]) -> list[tuple[TreeEntry, bytes]]:
    selected: list[tuple[TreeEntry, bytes]] = []
    for entry in entries:
        if entry.kind != "blob" or entry.mode not in REGULAR_MODES:
            continue
        data = read_blob(repo, entry.object_id)
        if is_shell_blob(entry.path, data):
            selected.append((entry, data))
    return selected


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

        assert b"link.sh" not in selected_paths and b"submodule" not in selected_paths
        checks += 1

        escape = root.parent / "escape.sh"
        assert is_shell_blob(b"../../escape.sh", b":\n")
        assert not escape.exists()
        checks += 1

    assert checks == 7
    print(f"large-script parity self-test: {checks} checks passed")


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def main() -> int:
    arguments = parse_arguments()
    if arguments.self_test:
        run_self_test()
        return 0
    raise CorpusError("no action selected; pass --self-test")


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except CorpusError as error:
        print(f"error: {error}", file=os.sys.stderr)
        raise SystemExit(2) from error
