# Architecture

CherubSH keeps the shell language pipeline separate from the interactive shell and the C compatibility libraries. That makes it possible to test parsing and execution without a terminal, while still testing terminal and ABI behavior at their own boundaries.

```text
shell input
    |
lexer -> parser -> expander -> executor -> builtins and processes
    |
shell state, options, variables, jobs, traps, and startup handling

interactive input -> line editor -> shell reader loop
C callers -> Readline and History FFI -> line editor and shared history support
```

## Workspace crates

| Crate | Responsibility |
| --- | --- |
| `cherubsh-common` | Shared shell data structures, variable behavior, options, jobs, signals, history, and completion types. |
| `cherubsh-lexer` | Shell tokens and lexical rules. |
| `cherubsh-parser` | Syntax trees and pretty-printing. |
| `cherubsh-expander` | Shell word expansion and related semantics. |
| `cherubsh-exec` | Command execution, control flow, redirection, functions, and traps. |
| `cherubsh-builtins` | Builtin command implementations. |
| `cherubsh-lineedit` | UTF-8 editing, input decoding, rendering, keymaps, completion UI, and history search. |
| `cherubsh-readline-ffi` | Readline-compatible C library. |
| `cherubsh-history-ffi` | History-compatible C library built from the shared FFI source. |
| `cherubsh` | Binary entry point, invocation parsing, lifecycle, prompt handling, completion, and interactive reader loop. |
| `cherubsh-test-harness` | Support for upstream, Brush, and Readline oracle tests. |

## Shell execution path

The lexer converts input into tokens. The parser builds command structures. The expander resolves shell words and parameter forms. The executor runs command structures with the current shell state, builtins, child processes, redirections, job control, and traps.

The state belongs to the shell layer. It tracks variables and attributes, positional parameters, shell and shopt options, history, jobs, signal and trap state, startup flags, and interactive behavior. Code below the shell layer receives the state it needs instead of treating a system Bash process as part of the implementation.

## Interactive path

The shell crate decides when to initialize interactive mode, load history, load startup files, and hand control to the reader loop. The line editor handles the byte-level terminal interaction and text editing. Prompt expansion, programmable completion, and shell state remain in the shell layer.

This split matters when diagnosing a redraw or completion failure. A raw pseudo-terminal capture can prove what bytes were written, but it does not by itself prove that the prompt, editor, and shell state agree.

## C library path

The FFI crates expose GNU Readline and History headers and symbols for C programs. The public ABI is checked with C fixtures and upstream examples. Keep external compatibility tests at the header, symbol, and behavior boundary instead of coupling C fixtures to Rust internals.

## Reference material

`vendor/` contains source and test material used by the compatibility workflow. `tools/fetch-upstream.sh` verifies pinned external references before the full gate uses them. [Compatibility](Compatibility) describes those boundaries in more detail.
