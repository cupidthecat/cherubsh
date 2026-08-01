#!/usr/bin/env python3
"""Validate and stage deterministic CycloneDX release SBOMs."""

from __future__ import annotations

import argparse
import hashlib
import json
import tempfile
import uuid
from pathlib import Path


PROJECT_URL = "https://github.com/cupidthecat/cherubsh"


def normalize_workspace_references(value: object, workspace_prefix: str) -> object:
    if isinstance(value, dict):
        return {
            key: normalize_workspace_references(item, workspace_prefix)
            for key, item in value.items()
        }
    if isinstance(value, list):
        return [normalize_workspace_references(item, workspace_prefix) for item in value]
    if isinstance(value, str) and value.startswith(workspace_prefix):
        return f"path+file://.{value.removeprefix(workspace_prefix)}"
    return value


def prepare_sbom(source: Path, output_directory: Path, release_version: str) -> Path:
    document = json.loads(source.read_text(encoding="utf-8"))
    if document.get("bomFormat") != "CycloneDX" or not document.get("specVersion"):
        raise ValueError(f"{source} is not a CycloneDX document")

    component = document.get("metadata", {}).get("component", {})
    component_name = component.get("name")
    if not component_name:
        raise ValueError(f"{source} has no metadata component name")

    resolved_source = source.resolve()
    if resolved_source.parents[1].name != "crates":
        raise ValueError(f"{source} is not directly below a crates workspace member")
    workspace_root = resolved_source.parents[2]
    workspace_prefix = f"path+{workspace_root.as_uri()}"
    document.pop("serialNumber", None)
    document = normalize_workspace_references(document, workspace_prefix)
    canonical_document = json.dumps(document, sort_keys=True, separators=(",", ":"))
    content_digest = hashlib.sha256(canonical_document.encode("utf-8")).hexdigest()
    identity = f"{PROJECT_URL}/sbom/{release_version}/sha256/{content_digest}"
    document["serialNumber"] = f"urn:uuid:{uuid.uuid5(uuid.NAMESPACE_URL, identity)}"

    output_directory.mkdir(parents=True, exist_ok=True)
    destination = output_directory / source.name
    destination.write_text(
        json.dumps(document, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    return destination


def self_test() -> None:
    with tempfile.TemporaryDirectory(prefix="cherubsh-sbom-") as temporary:
        root = Path(temporary)
        sources = []
        for checkout_name in ["checkout-one", "checkout-two"]:
            checkout = root / checkout_name
            source = checkout / "crates/shell/cherubsh.cdx.json"
            source.parent.mkdir(parents=True)
            shell_ref = f"path+file://{checkout}/crates/shell#cherubsh@0.0.0-test"
            common_ref = f"path+file://{checkout}/crates/common#cherubsh-common@0.0.0-test"
            fixture = {
                "bomFormat": "CycloneDX",
                "specVersion": "1.5",
                "metadata": {
                    "component": {
                        "bom-ref": shell_ref,
                        "name": "cherubsh",
                        "version": "0.0.0-test",
                    }
                },
                "components": [{"bom-ref": common_ref, "name": "cherubsh-common"}],
                "dependencies": [{"ref": shell_ref, "dependsOn": [common_ref]}],
            }
            source.write_text(json.dumps(fixture), encoding="utf-8")
            sources.append(source)

        first = prepare_sbom(sources[0], root / "first", "0.0.0-test")
        second = prepare_sbom(sources[1], root / "second", "0.0.0-test")
        first_document = json.loads(first.read_text(encoding="utf-8"))
        assert first.read_bytes() == second.read_bytes()
        assert first_document["serialNumber"].startswith("urn:uuid:")
        assert uuid.UUID(first_document["serialNumber"].removeprefix("urn:uuid:"))
        assert first_document["metadata"]["component"]["bom-ref"].startswith(
            "path+file://./crates/shell#"
        )
        assert first_document["dependencies"][0]["dependsOn"][0].startswith(
            "path+file://./crates/common#"
        )


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
