# Running a gpui-tui app over SSH

A runbook for AI coding agents that are asked to run a GPUI app on a remote
Linux server and use it from a local kitty or Ghostty window. Follow the
steps in order. Every step says what to run, what output to expect, and what
to do if it differs.

Placeholders used below:

| Placeholder | Meaning |
|---|---|
| `<user>@<host>` | The SSH login for the server |
| `<app>` | The app's binary name (for example `qwerio`) |
| `<package>` | The Cargo package that builds `<app>` (for example `qwerio-desktop`) |
| `<feature>` | The Cargo feature that enables gpui-tui in the app (usually `tui`) |
| `<image>` | The Docker image used to build for the server (step 3) |

## Rules for agents

- **Ask the user before running any command on the server.** That includes
  read-only ones. The user may prefer to run them and paste the output.
  Installing packages needs `sudo`, so the user usually runs those steps.
- **Ask before release builds** if the user's instructions require it.
  Debug builds work over SSH but encode frames much more slowly (zlib is
  unoptimized), so use release builds for anything that measures speed.
- **Don't put server addresses, user names, or credentials** in committed
  files.
- **The local terminal must be kitty or Ghostty, run directly.** Not tmux,
  screen, or zellij, which don't pass the kitty graphics protocol through.
  The agent usually cannot see the terminal; the user runs the final
  `ssh -t` command themselves.

## What happens, in short

- The app runs on the server. Frames (kitty graphics images) and input
  travel over the SSH pty. The server needs no GPU and no display server.
- gpui-tui probes whether the terminal can read its shared memory. Over SSH
  it can't, so frames go **inline**: split into tiles, with only changed
  tiles sent, zlib-compressed. Inline frames also default to a 30 fps cap
  and to rendering at `1 / GPUI_TUI_SCALE` of the terminal's resolution
  (half size on high-density displays), which the terminal scales up.
  None of this needs configuration.
- `ssh localhost` still uses shared memory, because it really is the same
  machine. To exercise the inline path locally, see
  [Testing without a server](#testing-without-a-server).

## Step 1: inspect the server

Ask the user to run this, or ask permission to run it:

```sh
ssh <user>@<host> 'cat /etc/os-release | head -4; ldd --version | head -1; uname -m; nproc; \
  ls /usr/lib/*/libEGL_mesa* 2>&1; ls /usr/share/fonts 2>&1'
```

Record these:
- **Distro and version.** These pick the package names in step 2.
- **glibc version** (from `ldd --version`). This picks the build image in
  step 3.
- **Architecture.** This runbook assumes `x86_64`. For `aarch64`, build on
  an arm64 machine or in an arm64 container.
- **Whether Mesa's EGL is installed** (`libEGL_mesa*` found).
- **Whether any fonts are installed** (`/usr/share/fonts` exists and isn't
  empty).

## Step 2: install the server packages

Three groups of packages are needed.

### 2a. Mesa, for software rendering

wgpu renders with Mesa's llvmpipe through EGL when there is no GPU.

| Distro | Packages |
|---|---|
| Debian, Ubuntu | `libegl1 libegl-mesa0 libgl1-mesa-dri` |
| Fedora | `mesa-libEGL mesa-dri-drivers` |
| Arch | `mesa` |

Without Mesa, the app exits with: `no GPU or software renderer found; …
install Mesa`.

### 2b. Fonts

GPUI resolves `.SystemUIFont` to IBM Plex Sans, then tries a fixed list:
`.ZedMono, .ZedSans, Helvetica, Segoe UI, Ubuntu, Adwaita Sans, Cantarell,
Noto Sans, DejaVu Sans, Arial`. gpui-tui, GPUI and gpui-kit bundle no
fonts. Minimal server images often have none, and then GPUI panics with
`failed to resolve font '.SystemUIFont' or any of the fallbacks`. Fonts are
read straight from `/usr/share/fonts`, `/usr/local/share/fonts`, and
`~/.local/share/fonts`, so fontconfig is not required.

| Distro | Packages |
|---|---|
| Debian, Ubuntu | `fonts-ibm-plex fonts-dejavu-core` (`fonts-ibm-plex` is in Ubuntu's universe repository; enable it with `sudo add-apt-repository universe` if apt can't find it) |
| Fedora | `ibm-plex-sans-fonts dejavu-sans-fonts dejavu-sans-mono-fonts` |
| Arch | `ttf-ibm-plex ttf-dejavu` |

`fonts-dejavu-core` (or its equivalents) alone is enough to stop the panic.
IBM Plex Sans makes the UI look as designed, and DejaVu Sans Mono gives
monospace text a real monospace font.

### 2c. Libraries the app binary links

These depend on the app, so find them in step 4 with `ldd`. Apps that also
compile GPUI's desktop backend (for example through `gpui-kit`) link X
keyboard libraries even when they run with `--tui`. On Ubuntu 24.04 that is:

```sh
sudo apt install -y libxkbcommon0 libxkbcommon-x11-0 libxcb1 libxcb-xkb1
```

### Combined command for Debian and Ubuntu

```sh
ssh -t <user>@<host> 'sudo apt update && sudo apt install -y \
  libegl1 libegl-mesa0 libgl1-mesa-dri \
  fonts-ibm-plex fonts-dejavu-core \
  libxkbcommon0 libxkbcommon-x11-0 libxcb1 libxcb-xkb1'
```

## Step 3: build a binary the server can run

A binary built against a newer glibc than the server's fails with
`version 'GLIBC_2.xx' not found`. Build in a container whose glibc is **the
same as or older than** the server's:

| Image | glibc | Runs on (for example) |
|---|---|---|
| `rust:<version>-bullseye` | 2.31 | Debian 11+, Ubuntu 20.04+ |
| `rust:<version>-bookworm` | 2.36 | Debian 12+, Ubuntu 24.04+, Fedora 37+ |
| `rust:<version>-trixie` | 2.41 | Debian 13+, Ubuntu 25.04+ |

`<version>` should match the app's `rust-toolchain.toml` (for example
`1.98`). The official images ship without `rustfmt` and `clippy`, so if the
toolchain file lists them, rustup downloads them on the first build.

### The hello example (smoke test)

From the gpui-tui checkout:

```sh
docker run --rm -v "$PWD":/src -w /src \
  -v gpui-tui-cargo-registry:/usr/local/cargo/registry \
  -v gpui-tui-cargo-git:/usr/local/cargo/git \
  <image> sh -c '
    cargo build --release -p gpui-tui --example hello --target-dir target/server; status=$?;
    chown -R '"$(id -u):$(id -g)"' target/server; exit $status'
```

Output: `target/server/release/examples/hello`.

### An app

From the app's checkout:

```sh
docker run --rm -v "$PWD":/src -w /src \
  -v app-cargo-registry:/usr/local/cargo/registry \
  -v app-cargo-git:/usr/local/cargo/git \
  <image> sh -c '
    apt-get update -qq &&
    apt-get install -y -qq libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev >/dev/null &&
    cargo build --release -p <package> --features <feature> --target-dir target/server; status=$?;
    chown -R '"$(id -u):$(id -g)"' target/server; exit $status'
```

Output: `target/server/release/<app>`.

Notes:
- **Named volumes** cache crates between runs. Don't mount the host's
  `~/.cargo`, because the container runs as root and would leave root-owned
  files there.
- **`chown`** gives the build output back to the local user.
- **`--target-dir target/server`** keeps the container build apart from
  local builds.
- **Missing `-dev` packages:** the `rust:*` images already have the
  fontconfig, freetype, and X11 headers. Apps with GPUI's desktop backend
  also need `libxkbcommon-dev`, `libxkbcommon-x11-dev`, and
  `libwayland-dev`. If linking fails with `unable to find library -lfoo`,
  add the matching `-dev` package (`apt-cache search libfoo`) and rebuild.
  Only the link step runs again.

## Step 4: check the binary before copying it

```sh
BIN=target/server/release/<app>   # or target/server/release/examples/hello
objdump -T "$BIN" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1   # must be <= the server's glibc
ldd "$BIN" | awk '{print $1}' | sort                               # libraries the server must have
```

`ldd` runs against the local machine's libraries, so it only lists names.
Make sure each listed library, except the glibc ones (`libc`, `libm`,
`libgcc_s`, `ld-linux`, `linux-vdso`), is installed on the server. On
Debian and Ubuntu, `apt-file search <library>.so` finds the package for a
library.

## Step 5: copy and run

```sh
scp "$BIN" <user>@<host>:~/<app>
ssh -t <user>@<host> 'GPUI_TUI_STATS=1 GPUI_TUI_LOG=/tmp/<app>.log ~/<app> --tui'
```

- **`-t` is required.** gpui-tui needs a terminal and exits without one.
- **Apps opt in** with `--tui` or `GPUI_TUI=1`. The `hello` example always
  runs in the terminal and takes no flag.
- **`GPUI_TUI_LOG`** collects the app's stdout and stderr, because they
  would otherwise draw over the image.
- **`GPUI_TUI_STATS=1`** adds one line per second while frames are sent.
- **Quitting:** Ctrl+C quits unless the app handles it.

Read the stats afterwards:

```sh
ssh <user>@<host> cat /tmp/<app>.log
```

A stats line looks like this:

```
gpui-tui: 24.0 fps, render 9.1 ms, encode 2.3 ms, 31.4 KiB/frame, 753 KiB/s, images 41/520
```

- **`fps`:** frames actually sent. The target is at least 15 while typing
  and hovering.
- **`render`:** CPU rendering time per frame, with llvmpipe.
- **`encode`:** tile diffing and compression per frame.
- **`KiB/frame` and `KiB/s`:** what crosses the network.
- **`images sent/total`:** tiles sent out of all tiles, summed over the
  second. A low ratio means the dirty tiles are working.

## Step 6: tuning, if it's slow

All of these are environment variables, set in front of the app on the
server.

| Symptom | Try |
|---|---|
| High `render` ms (CPU-bound server) | `GPUI_TUI_RENDER_SCALE=0.5` (or lower), `GPUI_TUI_MAX_FPS=20` |
| High `KiB/s` on a slow link | Lower `GPUI_TUI_RENDER_SCALE`; check that `images` is far below the total |
| Text too blurry | `GPUI_TUI_RENDER_SCALE=1` (full resolution, about 4× the pixels on high-density displays) |
| UI too small or too large | `GPUI_TUI_SCALE=1` or `2` (the scale is guessed from the cell height) |

## Troubleshooting

| Error or symptom | Cause | Fix |
|---|---|---|
| `failed to resolve font '.SystemUIFont' or any of the fallbacks` (panic) | No usable fonts on the server | Step 2b |
| `no GPU or software renderer found` | Mesa's EGL or DRI drivers are missing | Step 2a |
| `version 'GLIBC_2.xx' not found` | Built against a newer glibc than the server's | Rebuild with an older image (step 3) |
| `error while loading shared libraries: libfoo.so.N` | A runtime library is missing on the server | Install it (step 2c, step 4) |
| `unable to find library -lfoo` while building | A `-dev` package is missing in the container | Add it to the `apt-get install` in step 3 |
| `this terminal does not support the kitty graphics protocol` | Not kitty or Ghostty, or inside tmux, screen, or zellij | Run `ssh` directly in kitty or Ghostty |
| `cannot open /dev/tty` | `ssh` was run without `-t` | Add `-t` |
| The shell warns about an unknown terminal (Ghostty) | The server lacks the `xterm-ghostty` terminfo | `infocmp -x xterm-ghostty \| ssh <user>@<host> -- tic -x -` (gpui-tui itself doesn't need it) |
| The local terminal behaves oddly after a dropped connection | Nothing could restore the terminal modes | Run `reset` locally |
| Saving a password or credential fails | Secret Service is unreachable over SSH | Expected. The app needs its own fallback |
| Links or files don't open | `open_url` and `reveal_path` do nothing over SSH | Expected |

## Behavior differences from a local session

- The file picker browses the **server's** filesystem.
- Copying reaches the local clipboard through OSC 52, if the terminal
  allows it.
- Reading the clipboard returns only what the app itself copied or what was
  last pasted.
- If the connection drops, the app sees the terminal hang up and quits.

## Testing without a server

To exercise the inline path, software rendering, and stats on the
development machine, with the GPU hidden and no display server:

```sh
env -u DISPLAY -u WAYLAND_DISPLAY \
  VK_DRIVER_FILES=/nonexistent.json VK_ICD_FILENAMES=/nonexistent.json \
  __EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json \
  LIBGL_ALWAYS_SOFTWARE=1 \
  GPUI_TUI_TRANSPORT=direct GPUI_TUI_STATS=1 GPUI_TUI_LOG=/tmp/hello.log \
  cargo run --release -p gpui-tui --example hello
```

Run it in kitty or Ghostty. The same environment works for the end-to-end
tests (`cargo test -p gpui-tui --test terminal`), which run in a fake
terminal and need no real one.

## Worked example: Qwerio on Ubuntu 24.04

This is a verified setup: an Ubuntu 24.04.3 server with glibc 2.39 and no
GPU, with gpui-tui v0.2.0.

1. **Server packages:** the combined Debian and Ubuntu command from step 2.
2. **Build image:** `rust:1.98-bookworm`, whose glibc 2.36 is older than
   the server's 2.39. The resulting binary needs at most `GLIBC_2.35`.
3. **Build,** from the Qwerio checkout: the app command from step 3 with
   `<package>` = `qwerio-desktop` and `<feature>` = `tui`. Output:
   `target/server/release/qwerio`.
4. **`ldd`** lists `libxkbcommon.so.0`, `libxkbcommon-x11.so.0`,
   `libxcb.so.1`, `libxcb-xkb.so.1`, `libXau.so.6`, and `libXdmcp.so.6`,
   plus glibc. Step 2c covers them.
5. **Run:**
   `ssh -t <user>@<host> 'GPUI_TUI_STATS=1 GPUI_TUI_LOG=/tmp/qwerio.log ~/qwerio --tui'`.
   For a test database, copy an SQLite file over with `scp` and open it
   through the file picker. Storing credentials fails over SSH, because
   Qwerio has no keychain fallback yet.
