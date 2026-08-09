#!/usr/bin/env python3
"""Run pinned shell program entrypoints inside a locked Bubblewrap namespace."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import hashlib
import os
from pathlib import Path, PurePosixPath
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
from urllib.parse import urlparse


COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")
NAME_RE = re.compile(r"^[a-z0-9][a-z0-9-]*$")
ENTRYPOINT_RE = re.compile(r"^[A-Za-z0-9._+/-]+$")
CATEGORIES = {"interactive", "utility", "git", "version", "test", "system"}
MODES = {"command", "interactive", "source"}


class SmokeError(RuntimeError):
    """The smoke runner cannot preserve its execution contract."""


@dataclass(frozen=True)
class Project:
    name: str
    repository: str
    revision: str


@dataclass(frozen=True)
class Scenario:
    name: str
    category: str
    mode: str
    entrypoint: str
    arguments: tuple[str, ...]


@dataclass(frozen=True)
class RunResult:
    state: str
    status: int | None
    stdout: bytes
    stderr: bytes
    files: tuple[str, ...]


def safe_entrypoint(value: str) -> bool:
    path = PurePosixPath(value)
    return (
        bool(value)
        and ENTRYPOINT_RE.fullmatch(value) is not None
        and not path.is_absolute()
        and all(part not in {"", ".", ".."} for part in path.parts)
    )


def valid_repository(value: str) -> bool:
    parsed = urlparse(value)
    return (
        parsed.scheme == "https"
        and parsed.netloc == "github.com"
        and not parsed.params
        and not parsed.query
        and not parsed.fragment
        and parsed.path.endswith(".git")
        and len(PurePosixPath(parsed.path).parts) == 3
    )


def load_projects(path: Path) -> list[Project]:
    projects: list[Project] = []
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not line or line.startswith("#"):
            continue
        fields = line.split("\t")
        if len(fields) != 4:
            raise SmokeError(f"{path}:{number}: expected four tab-separated fields")
        name, repository, revision, _policy = fields
        if not NAME_RE.fullmatch(name) or any(item.name == name for item in projects):
            raise SmokeError(f"{path}:{number}: invalid or duplicate project name")
        if not valid_repository(repository):
            raise SmokeError(f"{path}:{number}: repository must be an HTTPS GitHub URL")
        if not COMMIT_RE.fullmatch(revision):
            raise SmokeError(f"{path}:{number}: invalid revision")
        projects.append(Project(name, repository, revision))
    if not projects:
        raise SmokeError(f"{path}: manifest contains no projects")
    return projects


def load_scenarios(path: Path) -> list[Scenario]:
    scenarios: list[Scenario] = []
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not line or line.startswith("#"):
            continue
        fields = line.split("\t")
        if len(fields) != 5:
            raise SmokeError(f"{path}:{number}: expected five tab-separated fields")
        name, category, mode, entrypoint, raw_arguments = fields
        if not NAME_RE.fullmatch(name) or any(item.name == name for item in scenarios):
            raise SmokeError(f"{path}:{number}: invalid or duplicate scenario name")
        if category not in CATEGORIES:
            raise SmokeError(f"{path}:{number}: unsupported category {category!r}")
        if mode not in MODES:
            raise SmokeError(f"{path}:{number}: unsupported mode {mode!r}")
        if not safe_entrypoint(entrypoint):
            raise SmokeError(f"{path}:{number}: unsafe entrypoint")
        try:
            arguments = () if raw_arguments == "[]" else tuple(shlex.split(raw_arguments))
        except ValueError as error:
            raise SmokeError(f"{path}:{number}: invalid arguments: {error}") from error
        if mode in {"interactive", "source"} and arguments:
            raise SmokeError(f"{path}:{number}: {mode} scenarios do not accept arguments")
        scenarios.append(Scenario(name, category, mode, entrypoint, arguments))
    if not scenarios:
        raise SmokeError(f"{path}: manifest contains no scenarios")
    return scenarios


def align_manifests(projects: list[Project], scenarios: list[Scenario]) -> None:
    project_names = [project.name for project in projects]
    scenario_names = [scenario.name for scenario in scenarios]
    if project_names != scenario_names:
        raise SmokeError("source and smoke manifests must contain the same ordered projects")


def git_run(arguments: list[str], *, capture: bool = True) -> subprocess.CompletedProcess[bytes]:
    completed = subprocess.run(
        ["git", *arguments],
        stdout=subprocess.PIPE if capture else subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0:
        diagnostic = completed.stderr.decode("utf-8", "replace").strip()
        raise SmokeError(f"git {' '.join(arguments)} failed: {diagnostic}")
    return completed


def object_exists(repo: Path, revision: str) -> bool:
    completed = subprocess.run(
        ["git", "-C", str(repo), "cat-file", "-e", f"{revision}^{{commit}}"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    return completed.returncode == 0


def ensure_object_store(project: Project, cache: Path) -> Path:
    if cache.is_symlink() or (cache.exists() and not cache.is_dir()):
        raise SmokeError("cache must be a real directory")
    cache.mkdir(parents=True, exist_ok=True)
    repo = cache / f"{project.name}.git"
    if repo.is_symlink() or (repo.exists() and not repo.is_dir()):
        raise SmokeError(f"{project.name}: invalid object-store path")
    if not repo.exists():
        git_run(["init", "--bare", "--quiet", str(repo)])
    bare = git_run(["-C", str(repo), "rev-parse", "--is-bare-repository"]).stdout.strip()
    if bare != b"true":
        raise SmokeError(f"{project.name}: cache is not a bare repository")
    if not object_exists(repo, project.revision):
        git_run(
            [
                "-C",
                str(repo),
                "fetch",
                "--no-tags",
                "--depth=1",
                project.repository,
                project.revision,
            ],
            capture=False,
        )
        fetched = git_run(
            ["-C", str(repo), "rev-parse", "FETCH_HEAD^{commit}"]
        ).stdout.decode("ascii").strip()
        if fetched != project.revision:
            raise SmokeError(f"{project.name}: fetched revision does not match the manifest")
    return repo


def validate_entrypoint(repo: Path, project: Project, scenario: Scenario) -> None:
    output = git_run(
        ["-C", str(repo), "ls-tree", project.revision, "--", scenario.entrypoint]
    ).stdout
    records = output.decode("utf-8", "replace").splitlines()
    if len(records) != 1:
        raise SmokeError(f"{project.name}: entrypoint does not exist: {scenario.entrypoint}")
    metadata, _, path = records[0].partition("\t")
    fields = metadata.split()
    if len(fields) != 3 or fields[0] not in {"100644", "100755"} or fields[1] != "blob":
        raise SmokeError(f"{project.name}: entrypoint is not a regular file")
    if path != scenario.entrypoint:
        raise SmokeError(f"{project.name}: entrypoint lookup returned an unexpected path")


def executable(path: Path, label: str) -> Path:
    resolved = path.expanduser().resolve()
    if not resolved.is_file() or not os.access(resolved, os.X_OK):
        raise SmokeError(f"{label} is not executable: {resolved}")
    return resolved


def snapshot(root: Path) -> tuple[str, ...]:
    entries: list[str] = []
    for path in sorted(root.rglob("*"), key=lambda item: item.relative_to(root).as_posix()):
        relative = path.relative_to(root).as_posix()
        if path.is_symlink():
            entries.append(f"L\t{relative}\t{os.readlink(path)}")
        elif path.is_dir():
            entries.append(f"D\t{relative}")
        elif path.is_file():
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            entries.append(f"F\t{relative}\t{digest}")
        else:
            entries.append(f"O\t{relative}")
    return tuple(entries)


def run_sandbox(
    *,
    bwrap: Path,
    shell: Path,
    repo: Path,
    project: Project,
    scenario: Scenario,
    root: Path,
    timeout: float,
) -> RunResult:
    workspace = Path(__file__).resolve().parent.parent
    with tempfile.TemporaryDirectory(prefix=f"{project.name}-", dir=root) as raw_work:
        work = Path(raw_work).resolve()
        arguments = [
            str(bwrap),
            "--tmpfs", "/",
            "--dev", "/dev",
            "--proc", "/proc",
            "--ro-bind", "/usr", "/usr",
            "--ro-bind", "/bin", "/bin",
            "--ro-bind", "/sbin", "/sbin",
            "--ro-bind", "/lib", "/lib",
            "--ro-bind-try", "/lib64", "/lib64",
            "--ro-bind", "/etc", "/etc",
            "--ro-bind-try", "/sys", "/sys",
            "--dir", "/home",
            "--tmpfs", "/tmp",
            "--dir", "/tmp/work",
            "--bind", str(work), "/tmp/work",
            "--ro-bind", str(repo.resolve()), "/mnt",
            "--ro-bind", str(workspace), "/srv",
            "--ro-bind", str(shell), "/bin/bash",
            "--ro-bind", str(shell), "/usr/bin/bash",
            "--dir", "/var",
            "--dir", "/var/run",
            "--unshare-all",
            "--die-with-parent",
            "--new-session",
            "--chdir", "/tmp/work",
            "--clearenv",
            "/bin/sh",
            "/srv/tools/program-smoke-sandbox.sh",
            project.revision,
            scenario.mode,
            scenario.entrypoint,
            *scenario.arguments,
        ]
        try:
            completed = subprocess.run(
                arguments,
                input=b"q" if scenario.mode == "interactive" else None,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=timeout,
                check=False,
            )
            return RunResult(
                "exit",
                completed.returncode,
                completed.stdout,
                completed.stderr,
                () if scenario.mode == "interactive" else snapshot(work),
            )
        except subprocess.TimeoutExpired as error:
            return RunResult(
                "timeout",
                None,
                error.stdout or b"",
                error.stderr or b"",
                () if scenario.mode == "interactive" else snapshot(work),
            )


def difference(left: RunResult, right: RunResult) -> str:
    fields = []
    if left.state == "timeout" or right.state == "timeout":
        fields.append("timeout")
    if left.state != right.state:
        fields.append("state")
    if left.status != right.status:
        fields.append("status")
    if left.stdout != right.stdout:
        fields.append("stdout")
    if left.stderr != right.stderr:
        fields.append("stderr")
    if left.files != right.files:
        fields.append("files")
    return ",".join(fields)


def escaped(value: str) -> str:
    return value.replace("\\", "\\\\").replace("\t", "\\t").replace("\n", "\\n")


def write_report(path: Path, rows: list[tuple[Scenario, RunResult, RunResult, str]]) -> None:
    lines = [
        "verdict\tproject\tcategory\tmode\tentrypoint\targuments\tbash\tcherub\tdifference"
    ]
    for scenario, bash, cherub, changed in rows:
        verdict = "PASS" if not changed else "FAIL"
        bash_state = f"{bash.state}:{bash.status if bash.status is not None else '-'}"
        cherub_state = f"{cherub.state}:{cherub.status if cherub.status is not None else '-'}"
        lines.append(
            "\t".join(
                [
                    verdict,
                    scenario.name,
                    scenario.category,
                    scenario.mode,
                    escaped(scenario.entrypoint),
                    escaped(shlex.join(scenario.arguments)),
                    bash_state,
                    cherub_state,
                    changed or "-",
                ]
            )
        )
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def write_failure_artifacts(
    report_dir: Path,
    scenario: Scenario,
    bash: RunResult,
    cherub: RunResult,
) -> None:
    destination = report_dir / "failures" / scenario.name
    destination.mkdir(parents=True, exist_ok=True)
    for label, result in (("bash", bash), ("cherub", cherub)):
        (destination / f"{label}.stdout").write_bytes(result.stdout)
        (destination / f"{label}.stderr").write_bytes(result.stderr)
        (destination / f"{label}.files").write_text(
            "\n".join(result.files) + ("\n" if result.files else ""),
            encoding="utf-8",
        )


def run_self_test() -> None:
    checks = 0
    with tempfile.TemporaryDirectory(prefix="cherubsh-program-smoke-self-test-") as raw:
        root = Path(raw)
        source = root / "source.lock"
        source.write_text(
            "sample\thttps://github.com/example/sample.git\t" + "1" * 40 + "\trequired\n",
            encoding="utf-8",
        )
        projects = load_projects(source)
        assert projects[0].name == "sample"
        checks += 1

        smoke = root / "smoke.lock"
        smoke.write_text("sample\tutility\tcommand\tbin/sample;touch\t\n", encoding="utf-8")
        try:
            load_scenarios(smoke)
        except SmokeError:
            pass
        else:
            raise AssertionError("shell metacharacter in entrypoint was accepted")
        checks += 1

        smoke.write_text("sample\tutility\tinteractive\tbin/sample\t--help\n", encoding="utf-8")
        try:
            load_scenarios(smoke)
        except SmokeError:
            pass
        else:
            raise AssertionError("interactive arguments were accepted")
        checks += 1

        smoke.write_text("sample\tutility\tcommand\tbin/sample\t--help\n", encoding="utf-8")
        scenarios = load_scenarios(smoke)
        assert scenarios[0].arguments == ("--help",)
        checks += 1

        align_manifests(projects, scenarios)
        checks += 1

        smoke.write_text("sample\tutility\tsource\tbin/sample\t[]\n", encoding="utf-8")
        assert load_scenarios(smoke)[0].arguments == ()
        checks += 1

        smoke.write_text("sample\tutility\tcommand\t../escape\t\n", encoding="utf-8")
        try:
            load_scenarios(smoke)
        except SmokeError:
            pass
        else:
            raise AssertionError("unsafe entrypoint was accepted")
        checks += 1

        assert tuple(shlex.split("--flag 'two words'")) == ("--flag", "two words")
        checks += 1

        same = RunResult("exit", 0, b"ok\n", b"", ("F\tx\t123",))
        assert difference(same, same) == ""
        checks += 1

        status = RunResult("exit", 2, b"ok\n", b"", ("F\tx\t123",))
        assert difference(same, status) == "status"
        checks += 1

        output = RunResult("timeout", None, b"no\n", b"", ("F\tx\t456",))
        assert difference(same, output) == "timeout,state,status,stdout,files"
        checks += 1

    assert checks == 11
    print(f"program smoke self-test: {checks} checks passed")


def parse_arguments() -> argparse.Namespace:
    root = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sources", type=Path, default=root / "large-scripts.lock")
    parser.add_argument("--scenarios", type=Path, default=root / "program-smoke.lock")
    parser.add_argument("--cache-dir", type=Path, default=root / "target/upstream/large-scripts")
    parser.add_argument("--report-dir", type=Path, default=root / "target/hardening/program-smoke")
    parser.add_argument("--bash", type=Path, default=root / "target/oracle/bash-5.3.15/bash")
    parser.add_argument("--cherub", type=Path, default=root / "target/debug/cherubsh")
    parser.add_argument("--bwrap", type=Path, default=shutil.which("bwrap") or "bwrap")
    parser.add_argument("--timeout", type=float, default=10.0)
    parser.add_argument("--project", action="append", default=[])
    parser.add_argument("--validate-only", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def main() -> int:
    arguments = parse_arguments()
    if arguments.self_test:
        run_self_test()
        return 0
    if arguments.timeout <= 0:
        raise SmokeError("timeout must be positive")

    projects = load_projects(arguments.sources.resolve())
    scenarios = load_scenarios(arguments.scenarios.resolve())
    align_manifests(projects, scenarios)
    if arguments.project:
        requested = set(arguments.project)
        known = {project.name for project in projects}
        missing = sorted(requested - known)
        if missing:
            raise SmokeError(f"unknown project: {', '.join(missing)}")
        selected = [
            (project, scenario)
            for project, scenario in zip(projects, scenarios)
            if project.name in requested
        ]
    else:
        selected = list(zip(projects, scenarios))

    bash = executable(arguments.bash, "Bash oracle")
    cherub = executable(arguments.cherub, "CherubSH")
    bwrap = executable(arguments.bwrap, "Bubblewrap")
    report_dir = arguments.report_dir.resolve()
    report_dir.mkdir(parents=True, exist_ok=True)
    failures = report_dir / "failures"
    if failures.is_symlink():
        raise SmokeError("failure artifact directory must not be a symlink")
    if failures.exists():
        shutil.rmtree(failures)
    work_root = report_dir / "work"
    work_root.mkdir(exist_ok=True)
    prepared = []
    validation_errors = []
    for project, scenario in selected:
        try:
            repo = ensure_object_store(project, arguments.cache_dir.resolve())
            validate_entrypoint(repo, project, scenario)
            prepared.append((project, scenario, repo))
        except SmokeError as error:
            validation_errors.append(str(error))
    if validation_errors:
        raise SmokeError("invalid scenarios:\n  " + "\n  ".join(validation_errors))
    if arguments.validate_only:
        print(f"program smoke: {len(prepared)} scenarios valid")
        return 0

    rows = []
    failed = False
    for project, scenario, repo in prepared:
        bash_result = run_sandbox(
            bwrap=bwrap,
            shell=bash,
            repo=repo,
            project=project,
            scenario=scenario,
            root=work_root,
            timeout=arguments.timeout,
        )
        cherub_result = run_sandbox(
            bwrap=bwrap,
            shell=cherub,
            repo=repo,
            project=project,
            scenario=scenario,
            root=work_root,
            timeout=arguments.timeout,
        )
        changed = difference(bash_result, cherub_result)
        rows.append((scenario, bash_result, cherub_result, changed))
        if changed:
            write_failure_artifacts(
                report_dir, scenario, bash_result, cherub_result
            )
        verdict = "PASS" if not changed else "FAIL"
        print(
            f"{project.name}: {verdict} "
            f"bash={bash_result.state}:{bash_result.status} "
            f"cherub={cherub_result.state}:{cherub_result.status} "
            f"difference={changed or '-'}"
        )
        failed |= bool(changed)

    report_path = report_dir / "report.tsv"
    write_report(report_path, rows)
    passed = sum(not changed for _, _, _, changed in rows)
    print(f"program smoke: pass={passed} fail={len(rows) - passed} report={report_path}")
    return 1 if failed else 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except SmokeError as error:
        print(f"program smoke: ERROR: {error}", file=sys.stderr)
        raise SystemExit(2)
