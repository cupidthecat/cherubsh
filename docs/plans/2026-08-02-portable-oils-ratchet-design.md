# Portable Oils ratchet design

## Problem

The Oils parity gate compares raw Bash and CherubSH results. Most known differences have one stable set of mismatched fields and two stable fingerprints. A few cases depend on the Linux host or scheduler. Hostnames, login shells, process IDs, resource usage, and job timing can change their output. The prompt job-count case can also alternate between a `stderr` difference and a combined `stdout,stderr` difference.

Architecture overrides handle real x86_64 and aarch64 differences, but they cannot represent host-specific values or multiple valid field sets. Pinning GitHub runner fingerprints would make the same gate fail on another Linux machine.

## Decision

Keep the ratchet strict by default. A case in the nondeterministic manifest may list more than one accepted mismatch-field set, separated by `|` in the `fields` column. For example, `stderr|stdout,stderr` accepts exactly those two sets. It does not accept any other combination.

The loader will parse the field sets into structured variants and reject empty, duplicate, or unknown fields. Classification succeeds only when the observed fields match one recorded variant and both fingerprints satisfy their exact or `variable` rules. A variable CherubSH fingerprint continues to allow either a known mismatch or an exact pass.

## Ratchet updates

Environment-sensitive entries will use generic `*` rows instead of runner-specific architecture rows:

- The process-substitution race keeps an exact Bash fingerprint and a variable CherubSH fingerprint.
- The prompt job-count case records its two observed field sets and uses variable fingerprints.
- The configure timing and login-shell cases use variable fingerprints with an exact `stderr` field set.
- The hostname case keeps its exact Bash fingerprint and uses a variable CherubSH fingerprint with an exact `stdout` field set.

The Bash help case retains its aarch64 override because that output is architecture-specific rather than host-specific.

## Verification

Parser and classifier tests will cover field-set alternatives, invalid variants, architecture precedence, and the checked-in environment-sensitive entries. The focused test-harness suite, formatting, clippy, wiki validation, and the full local parity driver must pass before push. GitHub Actions must then pass the native x86_64 and aarch64 parity jobs and their downstream Linux gate.
