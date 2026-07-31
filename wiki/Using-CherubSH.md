# Using CherubSH

CherubSH accepts command strings, script files, and interactive input. Its command-line shape follows Bash closely enough that the `--help` output is the quickest way to check an option on your installed revision.

```sh
cherubsh --help
cherubsh --version
```

## Command strings

Use `-c` when the program should execute a command string. Arguments after the command string are available to the shell in the normal positional-parameter slots.

```sh
cherubsh -c 'printf "first argument: %s\n" "$1"' cherubsh example
```

The first word after the command string is the shell's `$0`. The example prints `example`.

`--pretty-print`, `--dump-strings`, and `--dump-po-strings` parse input without executing it:

```sh
cherubsh --pretty-print -c 'for name in one two; do echo "$name"; done'
cherubsh --dump-strings messages.sh
cherubsh --dump-po-strings messages.sh
```

## Scripts

Pass a script path directly:

```sh
cherubsh build.sh release
```

The example scripts use `#!/usr/bin/env cherubsh`. Once the binary is on `PATH`, make a script executable and run it as usual:

```sh
chmod +x examples/01-basics.sh
./examples/01-basics.sh
```

The examples show functions, indexed and associative arrays, redirection, here-documents, process substitution, traps, coprocesses, and background jobs. `examples/README.md` gives a short description of each file.

## Shell behavior covered by the project

The test suites cover parsing and execution for functions, pipelines, subshells, coprocesses, background jobs, traps, redirections, here-documents, and here-strings. They also cover parameter, arithmetic, command, process, brace, pathname, and tilde expansion; indexed arrays, associative arrays, and namerefs; quoting and word splitting; shell options and `shopt` settings; programmable completion; and the builtins exercised by the compatibility suites.

Coverage does not mean that an untested script is guaranteed to behave like Bash. Treat the project as a Bash-compatible implementation under active development. If a difference matters to you, reduce it to a script and include the observed Bash 5.3.15 behavior when you report it.

## Install a local binary

After a release build, install the binary somewhere on `PATH`:

```sh
sudo install -m 0755 target/release/cherubsh /usr/local/bin/cherubsh
```

Confirm which binary the shell will use:

```sh
command -v cherubsh
cherubsh --version
```

## Use as a login shell carefully

Test scripts and startup files before listing CherubSH as a login shell. Keep the system Bash package installed. Do not replace `/bin/bash`; distribution scripts may rely on that exact path and on their distribution's Bash build options.

If you decide to add CherubSH to the system shell list:

```sh
command -v cherubsh | sudo tee -a /etc/shells
chsh -s "$(command -v cherubsh)"
```

To switch back:

```sh
chsh -s /bin/bash
```

For startup files, line editing, and history settings, continue with [Interactive shell](Interactive-shell).
