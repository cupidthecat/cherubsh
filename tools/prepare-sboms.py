#!/usr/bin/env python3
"""Validate and stage deterministic CycloneDX release SBOMs."""

from __future__ import annotations

import argparse
import json
import tempfile
import uuid
from pathlib import Path


PROJECT_URL = "https://github.com/cupidthecat/cherubsh"


def prepare_sbom(source: Path, output_directory: Path, release_version: str) -> Path:
    document = json.loads(source.read_text(encoding="utf-8"))
    if document.get("bomFormat") != "CycloneDX" or not document.get("specVersion"):
        raise ValueError(f"{source} is not a CycloneDX document")

    component = document.get("metadata", {}).get("component", {})
    component_name = component.get("name")
    if not component_name:
        raise ValueError(f"{source} has no metadata component name")
    component_version = component.get("version", release_version)
    identity = f"{PROJECT_URL}/sbom/{release_version}/{component_name}/{component_version}"
    document["serialNumber"] = f"urn:uuid:{uuid.uuid5(uuid.NAMESPACE_URL, identity)}"

    output_directory.mkdir(parents=True, exist_ok=True)
    destination = output_directory / source.name
    destination.write_text(
        json.dumps(document, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    return destination


def self_test() -> None:
    fixture = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "metadata": {"component": {"name": "cherubsh", "version": "0.0.0-test"}},
    }
    with tempfile.TemporaryDirectory(prefix="cherubsh-sbom-") as temporary:
        root = Path(temporary)
        source = root / "cherubsh.cdx.json"
        source.write_text(json.dumps(fixture), encoding="utf-8")
        first = prepare_sbom(source, root / "first", "0.0.0-test")
        second = prepare_sbom(source, root / "second", "0.0.0-test")
        first_document = json.loads(first.read_text(encoding="utf-8"))
        assert first.read_bytes() == second.read_bytes()
        assert first_document["serialNumber"].startswith("urn:uuid:")
        assert uuid.UUID(first_document["serialNumber"].removeprefix("urn:uuid:"))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--version")
    parser.add_argument("--output", type=Path)
    parser.add_argument("sbom", type=Path, nargs="*")
    arguments = parser.parse_args()

    if arguments.self_test:
        self_test()
        print("SBOM preparation self-test passed")
        return 0
    if not arguments.version or arguments.output is None or not arguments.sbom:
        parser.error("--version, --output, and at least one SBOM are required")

    names: set[str] = set()
    for source in arguments.sbom:
        if source.name in names:
            parser.error(f"duplicate SBOM filename: {source.name}")
        names.add(source.name)
        prepare_sbom(source, arguments.output, arguments.version)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
