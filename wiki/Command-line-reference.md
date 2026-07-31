# Command-line reference

Run `cherubsh --help` on the build you are using. The command reports the supported option surface directly from the shell.

```text
cherubsh [GNU long option] [option] ...
cherubsh [GNU long option] [option] script-file ...
```

## Long options

| Option | Use |
| --- | --- |
| `--debug`, `--debugger` | Enable debugging mode. |
| `--dump-po-strings` | Extract translatable strings in PO form without executing input. |
| `--dump-strings` | Extract translatable strings without executing input. |
| `--help` | Print invocation help and exit. |
| `--init-file path`, `--rcfile path` | Choose an interactive startup file. |
| `--login` | Request login-shell startup behavior. |
| `--noediting` | Disable interactive line editing. |
| `--noprofile` | Skip login startup processing. |
| `--norc` | Skip the interactive RC file. |
| `--posix` | Enable POSIX mode. |
| `--pretty-print` | Parse and print shell input without executing it. |
| `--restricted` | Start in restricted mode. |
| `--verbose` | Enable verbose input reporting. |
| `--version` | Print version and license information, then exit. |

## Invocation-only short options

The invocation parser accepts these forms:

```text
-ilrsD
-c command
-O shopt_option
```

Use `-c` for command strings. Use a script path when the command is stored in a file. The short shell options and `-o option` follow Bash-style option handling:

```text
-abefhkmnptuvxBCEHPT
-o option
```

Option behavior can depend on whether the shell is interactive, a login shell, or executing a command string. When writing a bug report, include the full invocation, the RC-file options, and whether the input was attached to a terminal.

## Source-output modes

The three source-output modes are useful for tools that need to inspect shell input without running it:

```sh
cherubsh --pretty-print script.sh
cherubsh --dump-strings script.sh
cherubsh --dump-po-strings script.sh
```

They also accept `-c` input and standard input. Do not use them as a security boundary for untrusted files. They avoid command execution, but parsing untrusted data is still a different activity from treating it as harmless text.

## Version compatibility setting

The shell uses `CHERUBSH_BASH_COMPAT_VERSION` when it is set to a parseable version string. That value affects the Bash compatibility version exposed by the shell. An empty or invalid value falls back to the build's pinned compatibility version.

```sh
CHERUBSH_BASH_COMPAT_VERSION=5.3.15 cherubsh --version
```

Use the repository's default setting for parity work unless you are deliberately testing version presentation.
