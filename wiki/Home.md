# CherubSH

CherubSH is a Rust implementation of Bash 5.3 behavior for Linux and WSL. It has its own parser, expander, executor, line editor, and compatible GNU Readline and History libraries. Bash is a compatibility oracle used by the test suite. CherubSH does not call Bash to parse or run shell input.

The project currently targets the behavior of Bash 5.3.15 and Readline 8.3 patch 3. The workspace version is 0.3.0.

## Start here

- [Getting started](Getting-started) covers a local build and the first commands to run.
- [Using CherubSH](Using-CherubSH) covers scripts, command strings, examples, and installation as a login shell.
- [Interactive shell](Interactive-shell) covers startup files, editing modes, completion, and history.
- [Command-line reference](Command-line-reference) lists the supported invocation options.

## Compatibility work

CherubSH checks behavior at several public boundaries. The ordinary workspace tests cover Rust components. The full parity gate also builds a pinned Bash oracle, runs upstream Bash drivers, compares CherubSH fixtures, exercises Brush cases, and checks the Readline and History C interfaces.

The repository's current compatibility report lists:

| Suite | Result |
| --- | ---: |
| Upstream Bash 5.3 `run-*` drivers | 86 / 86 passing |
| CherubSH differential fixtures | 99 / 99 passing |
| Runnable Brush cases | 2,104 / 2,104 passing |
| Brush cases skipped by metadata or Bash version | 1 |

Those figures describe the v0.3.0 parity gates in this repository. Run the checks yourself before relying on a new revision. [Compatibility](Compatibility) explains what each suite proves, and [Testing](Testing) gives the commands.

The report labels two `read -t 0` pipeline cases as `ported-nondeterministic`. Bash can return either documented readiness result for those cases depending on process scheduling; other comparisons remain exact.

## Project map

| Location | Purpose |
| --- | --- |
| `crates/shell` | The `cherubsh` binary, startup handling, prompts, completion, and job-control setup. |
| `crates/lexer`, `parser`, `expander`, `exec` | The shell language pipeline. |
| `crates/builtins` | Shell builtins and their compatibility behavior. |
| `crates/lineedit` | UTF-8 interactive line editing. |
| `crates/readline-ffi`, `history-ffi` | C-compatible Readline and History libraries. |
| `crates/test-harness` | Oracle, fixture, and Brush test support. |
| `tools` | Build, fetch, parity, hardening, packaging, and wiki maintenance commands. |
| `vendor` | Checked-in upstream sources and test material. |

## License and upstream material

CherubSH is GPL-3.0-or-later. The repository also keeps material from GNU Bash, GNU Readline, Brush, and bash-completion under their own licenses. See `LICENSE`, the vendored license files, and [Compatibility](Compatibility) before redistributing a combined build.

## Keeping this wiki current

The Markdown files in the repository's `wiki/` directory are the source of truth. A push to `main` that changes them validates the pages and then mirrors them to the GitHub Wiki. [Publishing the wiki](Publishing-the-wiki) explains the one-time GitHub setup and the local commit hook.
