# Readline and History

CherubSH ships separate C-compatible GNU Readline and History libraries in addition to the shell's Rust line editor. The public headers, symbols, callbacks, keymaps, shared-library names, static archives, and pkg-config metadata are checked against the pinned GNU Readline reference.

## Build the staged libraries

From the repository root:

```sh
./tools/build-readline.sh
```

The script stages files below `target/readline`:

```text
target/readline/include/readline/  readline.h, history.h, keymaps.h, tilde.h, ...
target/readline/lib/               libreadline.so.8.3, libhistory.so.8.3, static archives
target/readline/lib/pkgconfig/     readline.pc, history.pc
```

`target/` is generated output. Build it again after cleaning the workspace or changing the library code.

## Link a C program

Use the staged headers and library directory. The runtime search path in this example points at the local staged library:

```sh
cc example.c \
  -Itarget/readline/include \
  -Ltarget/readline/lib \
  -Wl,-rpath,"$PWD/target/readline/lib" \
  -lreadline
```

For History, link `-lhistory` as appropriate for the program. The staged pkg-config files provide the same include and link information:

```sh
export PKG_CONFIG_PATH="$PWD/target/readline/lib/pkgconfig"
pkg-config --cflags --libs readline
pkg-config --cflags --libs history
```

`examples/readline-client.c` is a small client that reads a name and records it in History. Build it against the staged libraries with:

```sh
cc examples/readline-client.c \
  -Itarget/readline/include \
  -Ltarget/readline/lib \
  -Wl,-rpath,"$PWD/target/readline/lib" \
  -lreadline -lhistory \
  -o target/readline-client
target/readline-client
```

## Install a development archive

Each Linux release has a `cherubsh-readline-dev` archive for x86-64 and AArch64. It contains the public headers, shared-library links, static archives, pkg-config files, license, C example, component manifests, and installer.

After extracting the archive, install both libraries under `/usr/local` with:

```sh
sudo ./tools/install-readline-dev.sh install --component all --prefix /usr/local
```

For a staged package build, set `DESTDIR` and choose the prefix that the finished package will use:

```sh
DESTDIR="$PWD/package-root" \
  ./tools/install-readline-dev.sh install --component all --prefix /usr
```

Readline and History have separate ownership records. This lets the uninstaller remove one component without deleting files that belong to the other component or to another package:

```sh
sudo ./tools/install-readline-dev.sh uninstall --component readline --prefix /usr/local
sudo ./tools/install-readline-dev.sh uninstall --component history --prefix /usr/local
```

The pkg-config files derive their prefix from their installed location. Moving the staged prefix does not leave the build path embedded in their include or library flags.

## Run the library parity gate

```sh
./tools/run-readline-parity.sh
```

The gate builds GNU Readline 8.3 patch 3, checks public symbol coverage and library names, compiles the same C fixtures against GNU Readline and CherubSH, and compares deterministic example output byte for byte. The fixtures include a pseudo-terminal loop, completion display policy, user-defined C callbacks, macros, bare keymaps, and History behavior. Reports are kept in `target/parity/readline`.

## Include paths in the repository

The public compatibility headers live under `include/readline`. The Rust FFI implementation is in `crates/readline-ffi`; `crates/history-ffi` builds the History library from the shared implementation. The arrangement is intentional: library consumers depend on the exported C interface, while the shell uses the Rust line editor directly.

## When a C program fails

First confirm that the program is using the staged headers and not the system headers:

```sh
pkg-config --modversion readline
pkg-config --cflags readline
```

Then check the dynamic loader search path and the library selected at runtime. A successful compile against one set of headers and a run against another shared library often looks like an ABI defect.
