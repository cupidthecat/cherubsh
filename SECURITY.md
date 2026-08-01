# Security Policy

## Supported Versions

Security fixes are made on the current release line and on `main`. Older release lines do not receive backports.

| Version | Supported |
| --- | --- |
| 0.3.x | Yes |
| Earlier versions | No |

## Reporting a Vulnerability

Please do not open a public issue for a suspected vulnerability. Use GitHub's [Private vulnerability reporting](https://github.com/cupidthecat/cherubsh/security/advisories/new) form instead. Include the affected version, a small reproduction, the expected impact, and any workaround you have found.

You should receive an acknowledgement within seven days. The maintainers will confirm the scope, discuss a disclosure date with you, and prepare a fix before publishing details. If the report does not affect CherubSH, the response will explain why.

## Release Verification

Each tagged release includes SHA-256 checksums, CycloneDX SBOM files, and GitHub artifact attestations. The README contains the verification commands. A valid attestation proves which repository and workflow produced an asset; it does not replace review of the source or the SBOM.
