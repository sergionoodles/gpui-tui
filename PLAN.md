# gpui-tui: plan

Status: Phases 0–2 implemented and checked in Ghostty. Phase 3 (Qwerio
locally) implemented; the manual check of Qwerio's main flows is pending.
Phase 4 (SSH) implemented and released as v0.2.0; the hello example runs
over SSH on an Ubuntu 24.04 server without a GPU. See "Progress" below.

## Progress (2026-10-21): Phase 4

**Decisions:**
- You test on a real remote host yourself; the code work is verified here
  with the fake terminal and with software rendering forced locally.
- Qwerio's keychain fallback is a later, separate task.
- Only llvmpipe is targeted; vulkan-swrast (lavapipe) was not installed
  here.
- The SSH tuning keys on the **transport**, not on `SSH_CONNECTION`: when
  the terminal cannot read shared memory, frames go inline, and that is
  what makes bytes and CPU expensive. `ssh localhost` therefore still uses
  shared memory, which is correct. Force `GPUI_TUI_TRANSPORT=direct` to try
  the remote path locally. Links stay keyed on the SSH variables, because
  that is about where `xdg-open` would run.

**Done:**
- **Dirty tiles (inline only):** `gpui-tui-term`'s `tiles` module splits
  the frame into cell-aligned tiles of about 128 frame pixels. Each tile is
  its own kitty image (IDs from 100), placed at its cell with `c=`/`r=`.
  Only tiles whose pixels changed are sent. When the frame size or cell
  area changes, everything is deleted (`a=d,d=A`) and resent in the same
  synchronized update. Separate images were chosen over kitty's frame
  editing (`a=f`) because Ghostty does not implement the animation
  commands. Tile sizes prefer a cell count with a whole number of pixels,
  so the terminal scales every tile by the same factor and no seams show.
- **RGB for opaque tiles:** `f=24` drops a quarter of the bytes before
  compression. Tiles with any transparency keep RGBA.
- **Render scale:** `Options::render_scale` and `GPUI_TUI_RENDER_SCALE`.
  GPUI's scale factor becomes `display scale × render scale` while the
  logical window size and mouse mapping stay the same, and the terminal
  scales frames up. It defaults to `1 / display scale` inline (half size on
  high-density displays) and to `1` over shared memory.
- **Frame cap:** `Options::max_frames_per_second` is now an `Option`.
  `None` means 60 over shared memory and 30 inline. This changes the public
  `Options`, but Qwerio only uses `Options::from_env()`, so it is not
  affected.
- **Stats:** `GPUI_TUI_STATS=1` prints fps, render and encode times,
  KiB per frame and per second, and tiles sent out of the total, once per
  second, to the log.
- **Hang-up:** a read of 0 bytes from the tty is now treated as a closed
  terminal, and the app quits. Before, it counted as "no input", so a
  process that survived SIGHUP would spin on a dead terminal.
- **No renderer:** the error now says to install Mesa for llvmpipe.
- **Fake terminal:** it tracks the cursor, composites each image at its
  cell scaled to its `c`/`r` box, and counts a frame when a synchronized
  update ends.
- **Verified:**
  - Unit tests for the tile layout, tile diffing, RGB extraction, presenter
    tile output, options, and stats.
  - A new e2e test: inline frames on a high-density terminal (16×32 cells),
    rendered at half resolution, with click, typing, and resize.
  - All e2e tests pass with the GPU hidden and no display
    (`VK_DRIVER_FILES` pointing nowhere, Mesa's EGL vendor only,
    `LIBGL_ALWAYS_SOFTWARE=1`, no `DISPLAY`/`WAYLAND_DISPLAY`), so
    llvmpipe renders headless.
  - Debug-build stats for the e2e scene: 15 tiles per frame at half
    resolution versus 28 at full, and about 7–11 KiB per frame.

- **Real server:** the `hello` example works over SSH on an Ubuntu 24.04.3
  server without a GPU, rendering with llvmpipe. It was built in a
  `rust:1.98-bookworm` container, because a glibc older than the server's
  is needed. The server needed Mesa (`libegl1`, `libegl-mesa0`,
  `libgl1-mesa-dri`) and fonts: without any, GPUI panics with "failed to
  resolve font '.SystemUIFont'", since gpui-tui (like GPUI's Linux backend)
  maps that alias to IBM Plex Sans and nothing bundles fonts.
  `fonts-ibm-plex` and `fonts-dejavu-core` fixed it.

**Not done yet:**
- Measured numbers from the server (fps, bytes per frame, server CPU with
  `GPUI_TUI_STATS=1`), and Qwerio over SSH. Qwerio needs to move to
  v0.2.0.
- Bundling fonts in gpui-tui, so apps also run on servers without any
  fonts. Not needed while servers get fonts installed.
- Restoring the terminal on SIGTERM. A signal handler needs a new
  dependency (`signal-hook`, or `libc` directly), which is waiting for your
  OK. SIGHUP needs nothing: over SSH it means the terminal is already gone.
- Qwerio's keychain fallback (separate task).
- Possible later tuning, if the stats show a need: compressing tiles on
  worker threads, and the zlib level (currently the fastest).

## Progress (2026-10-21): Phase 3

**Done:**
- **File picker:** `cx.prompt_for_paths` and `cx.prompt_for_new_path` open
  a picker drawn as terminal text in place of the app (`gpui-tui-term`'s
  `picker` module). It changes from §5's "GPUI picker window" because the
  platform has no `App` to open a window with, and the text picker needs no
  app code. While it is open, frames pause and keys, mouse, and paste go to
  the picker.
  - It covers files, folders (a `./` entry picks the current folder),
    several paths (Tab marks them), and save with overwrite confirmation.
  - Typing filters the list, and typed paths (`/…`, `~/…`, `a/b`) are
    resolved. Ctrl+H shows hidden files. The wheel and clicks work too.
  - Control characters in file names are replaced, so a file name cannot
    inject escape sequences.
- **Qwerio:** an optional `tui` feature with a path dependency on this
  checkout. `platform/application.rs` strips `--tui` before the workspace
  argument is read. Without the feature, `--tui` is an error instead of a
  workspace path.
- **Qwerio decisions:**
  - Qwerio already asks for server-side decorations and has no custom
    window controls, so it needs no further changes.
  - It opens a single window; every dialog is an in-window overlay.
  - The single-instance lock is shared with the desktop app. Both use the
    same state database, so only one may run at a time.
- **Verified:**
  - An e2e test drives the picker inside the fake terminal.
  - A smoke run of the real `qwerio --tui` binary inside the fake
    terminal, with isolated XDG folders, works:
    - the first frame arrives after about 2–3 s (debug build),
    - the first-run dialog responds to a click,
    - Ctrl+O shows "Open Workspace" in the picker,
    - Escape brings Qwerio back,
    - Ctrl+Q exits cleanly with the terminal restored.
  - Qwerio's full checks pass with and without `tui` (404 tests).

**Not done yet:** the manual check in Ghostty: connect to SQLite, browse
tables, run SQL, edit a file.

## Progress (2026-10-20): Phases 0–2

**Done:**
- The workspace and its three crates:
  - `gpui-tui-term` (terminal I/O, no GPUI),
  - `gpui-tui` (the GPUI platform),
  - `gpui-tui-testterm` (a fake terminal for end-to-end tests).
- `bench-support` is used for the headless renderer. No Zed PR (decision 9).
- An end-to-end test runs a real GPUI app inside the fake terminal, over both
  shared memory and direct transmission. It covers rendering, a click,
  typing, a resize, Ctrl+C quitting, and terminal restore. It also checks
  that an unsupported terminal is rejected, and that an idle app sends no
  frames and uses no CPU.

**Measured** (one 1600×1000 frame of a simple scene):

| Path | Render + readback | Encode + send |
|---|---|---|
| NVIDIA GPU, debug build | 5–8 ms | 3 ms (shm); 65 ms (direct, zlib unoptimized in debug) |
| NVIDIA GPU, release build | 5–7 ms | 3 ms (shm); 2 ms (direct) |
| llvmpipe (GL, no GPU device opened), release build | 4–6 ms; about 150 ms for the first frame (shader compile) | about 3 ms |

An idle app used 0 ms of CPU over 2 s.

**`gpui-pre` compatibility:**
- 0.3.9 passes.
- 0.3.8 fails to compile: 0.3.9 added `PlatformDisplay::refresh_interval`,
  `Platform::on_displays_changed`, and `PlatformWindow::on_display_changed`.
  The supported range is therefore exactly 0.3.9, as §7 anticipated. Check
  other versions with `scripts/check-gpui-versions.sh <version>`.

**Not done yet:**
- The gpui-kit component sampler example. It needs gpui-kit as a new
  dev-dependency, which is waiting for your OK.
- Restoring the terminal on SIGTERM or on `std::process::exit`. Normal quit,
  Ctrl+C, and panics already restore it.
- ~~A manual check in a real Ghostty or kitty~~: done in Ghostty with the
  hello example.

## 0. Decisions

| # | Topic | Decision |
|---|---|---|
| 1 | Terminals for v1 | kitty and Ghostty |
| 2 | SSH | **Deferred to a later phase** (§10, Phase 4). When it comes, it uses the server's existing sshd (`ssh -t host app --tui`) and must work without a GPU. A built-in SSH server is out of scope. Phases 0–3 are local only, but the design must not rule SSH out |
| 3 | GPUI | Target explicit, tested `gpui-pre` versions (§7), not whatever Qwerio happens to pin |
| 4 | Opt-in | The `--tui` flag (primary), with `GPUI_TUI=1` as an equivalent |
| 5 | OS | Linux only for now |
| 6 | Links over SSH | `open_url`, `reveal_path`, and `open_with_system` do nothing |
| 7 | Repo | git initialized by you |
| 8 | License | MIT |
| 9 | Headless renderer | `bench-support` (§7.1 option A). No Zed PR for now |

## 1. Goal

`gpui-tui` is a GPUI platform backend that runs an unmodified GPUI app inside
a terminal emulator: locally first, over SSH in a later phase. The terminal
acts as the window: GPUI renders frames, the terminal displays them, and
terminal input becomes GPUI input events.

The first real user is Qwerio (`/home/sergio/workspace/qwerio-v2`). The crate
must not know anything about Qwerio, React, or Solid. Any GPUI app built on a
supported `gpui-pre` version should be able to opt in with a few lines in
`main()`.

**Non-goals for v1:**
- Converting GPUI's element tree into native terminal widgets the way ratatui
  does.
- Terminals other than kitty and Ghostty, including tmux and zellij.
- macOS and Windows.
- A built-in SSH server.

## 2. What the research found

### 2.1 GPUI (`gpui-pre`)

- Qwerio compiles **`gpui-pre` 0.3.9**, imported as `gpui`, through
  `gpui-kit` 0.7.3, which pins `=0.3.9`. The `gpui-fast` crates in its
  lockfile sit behind a gpui-kit feature that Qwerio does not enable, so they
  are never compiled.
- `gpui-pre` publishes about once a week: 0.3.0 through 0.3.9 came out
  between 2026-09-03 and 2026-10-12. The `gpui-pre-*` crates pin each other
  exactly (for example, `gpui-pre-wgpu` 0.3.9 depends on `gpui = "=0.3.9"`).
- GPUI is split into separate crates: `gpui`, `gpui-platform`, `gpui-linux`,
  `gpui-wgpu`, and `gpui-web`. `gpui-web` shows that a platform can live
  outside the core crate: it implements `Platform`, uses `CosmicTextSystem`
  and `WgpuRenderer`, and plugs itself in through
  `Application::with_platform`.
- All the platform traits are `pub` and none are sealed: `Platform` (about 52
  required methods on Linux), `PlatformWindow` (about 42),
  `PlatformDisplay`, `PlatformDispatcher`, `PlatformTextSystem`, and
  `PlatformAtlas`. Many of these methods are trivial stubs for a terminal.
- `Application::with_platform(Rc<dyn Platform>)` is public
  (`gpui-pre/src/app.rs:179`). `gpui_kit` re-exports `gpui::*`.
- **Offscreen rendering already exists.** `gpui_wgpu::WgpuHeadlessRenderer`
  renders a `Scene` into a texture and reads back RGBA through
  `PlatformHeadlessRenderer::render_scene_to_image`. It is gated behind the
  `test-support` or `bench-support` feature
  (`gpui-pre-wgpu/src/wgpu_renderer.rs:2305`). That gate is a packaging
  choice, not a technical limit, so **no GPUI fork is needed for the MVP**.
  The catch is that `bench-support` also pulls in `criterion` as a normal
  dependency.
- `gpui_wgpu::CosmicTextSystem` (cosmic-text plus swash) is public and can
  be reused as-is.
- Input event structs have public fields and are not `#[non_exhaustive]`, so
  an external crate can build them.
- If `PlatformWindow::prompt` returns `None`, GPUI renders its own in-window
  fallback prompt (`gpui-pre/src/window.rs:6689`).
- wgpu on Linux compiles in both the Vulkan and the GL backends, and accepts
  software adapters.

### 2.2 This machine

- GPUs: NVIDIA and Radeon Vulkan ICDs.
- Mesa 26.2.4 is installed with its EGL vendor (`50_mesa.json`), so GL
  through **llvmpipe** can be tested now.
- Vulkan through **lavapipe** needs `vulkan-swrast`, which is not installed
  (§15).

### 2.3 How gpuix does it (reference only, no code reuse)

- **Rendering:** pixels, not cells. It is a real GPUI `Platform` (about
  3.2k lines) that renders offscreen through a patched Zed fork and draws
  with the kitty graphics protocol.
- **Local transport:** each frame goes into POSIX shared memory. Only about
  100 bytes per frame pass through the pty: an `a=T,t=s` command that
  reuses the same image and placement IDs inside synchronized output
  (mode 2026).
- **SSH transport:**
  - A probe (`a=q,t=s` followed by DA1) detects that shm is not reachable,
    and gpuix then falls back to inline frames: zlib level 1, base64, 4 KiB
    chunks.
  - An `a=q` fence after each frame gives backpressure, with at most 2
    frames in flight.
  - They report about 120 KB per simple 1600×960 frame and about 470 KB per
    dense one. That works out to roughly 5–20 fps at 20 Mbit/s.
  - It has no dirty-rectangle updates.
- **Input:**
  - Mouse modes 1003, 1006, and 1016 (any-motion, SGR, and pixel
    coordinates).
  - Focus events (1004), bracketed paste (2004), in-band resize (2048).
  - The kitty keyboard protocol with flags 31.
- **Output escapes:** OSC 2 sets the title, OSC 22 sets the pointer shape,
  and OSC 52 writes the clipboard.
- **Parser:** their review found that crossterm and termina misparse APC
  replies, so the parser is hand-written.
- **Damage tracking:** `zenbu-labs/terminal-browser` (the "pixel" project)
  already does tile-based dirty patching over SSH. It is the better
  reference for the bandwidth work.

## 3. Core decision: pixels via the kitty graphics protocol

GPUI draws arbitrary fonts, rounded corners, shadows, and images, so a
character-cell mapping cannot represent it faithfully. kitty and Ghostty
both implement the kitty graphics protocol, including shared memory.

The frame pipeline stays encoder-agnostic: an encoder turns an RGBA frame
plus damage information into bytes. That keeps sixel or cell-blitter
encoders possible later without touching the GPUI side.

## 4. Architecture

```
┌────────────── app process (e.g. qwerio --tui) ──────────────┐
│                                                             │
│  GPUI app code ──► gpui::Application::with_platform(TuiPlatform)
│                         │                                   │
│  ┌──────────── gpui-tui (GPUI adapter) ───────────────────┐ │
│  │ TuiPlatform   TuiWindow   TuiDisplay   TuiDispatcher   │ │
│  │      │            │ draw(&Scene)                       │ │
│  │      │            ▼                                    │ │
│  │      │     WgpuHeadlessRenderer ──► RGBA frame         │ │
│  │      │   (GPU, or llvmpipe/lavapipe)   │               │ │
│  │      ▼                                 ▼               │ │
│  │  terminal Event ──► PlatformInput   FrameSink          │ │
│  └──────▲─────────────────────────────────┼───────────────┘ │
│  ┌──────┴──────── gpui-tui-term (GPUI-agnostic) ──────────┐ │
│  │ tty raw mode + restore guard   capability probe        │ │
│  │ input parser (CSI-u, SGR-pixel, APC, OSC, paste)       │ │
│  │ damage tracker (tile hashing)   kitty encoder          │ │
│  │ transports: shm | file | inline(zlib+b64)              │ │
│  │ writer thread + in-flight fences (backpressure)        │ │
│  └──────────────────────────┬─────────────────────────────┘ │
└─────────────────────────────┼───────────────────────────────┘
                              │ /dev/tty  (local pty or sshd pty)
                     kitty / Ghostty on your machine
```

### 4.1 Workspace layout

```
gpui-tui/
  crates/
    gpui-tui-term/      # GPUI-agnostic terminal I/O; most unit tests live here
    gpui-tui/           # GPUI Platform implementation + public API
    gpui-tui-testterm/  # fake terminal for end-to-end tests (decodes our output)
  examples/             # hello, input-inspector, scrolling list, gpui-kit sampler
  scripts/              # gpui-pre version-matrix check (§7)
```

All the hard, testable logic lives in `gpui-tui-term`, which does not depend
on GPUI. The `gpui-tui` adapter is the only part tied to `gpui-pre`
versions, so each version bump touches as little as possible.

### 4.2 Main loop and dispatcher

- **Main thread:** a `calloop` event loop, the same pattern gpui-linux
  uses. Its event sources are:
  - the tty fd, for input,
  - SIGWINCH,
  - a `Ping` that wakes the loop when work is sent to the main thread,
  - a frame timer.
- **`TuiDispatcher`:** a worker pool for background tasks, a priority queue
  feeding the main thread, and a timer thread for `dispatch_after`. It
  mirrors `LinuxDispatcher`, which is crate-private, so we rewrite it
  (about 150–250 lines).
- **Frame pacing:**
  - The loop calls GPUI's `on_request_frame` callback, which is cheap
    because GPUI only repaints dirty windows.
  - It ticks at up to 60 Hz locally and 30 Hz over SSH while there has been
    recent input or work. Both caps are configurable.
  - When idle, it sleeps until the next input or wake-up, so idle CPU stays
    around 0%. This matters most with software rendering.
- **Writer thread:** all terminal output goes through a bounded channel to a
  writer thread, so slow SSH writes never block the GPUI main thread.

### 4.3 Per-frame flow

1. GPUI calls `TuiWindow::draw(&scene)`.
2. `WgpuHeadlessRenderer::render_scene_to_image(scene, device_size)` returns
   RGBA. The window's `sprite_atlas()` must return this renderer's atlas.
3. The damage tracker hashes fixed-size tiles (for example 128 px) and
   compares them with the previous frame. The result is: no change, a list
   of dirty tiles, or the full frame.
4. The encoder sends the full frame over shm locally. Inline (over SSH), it
   sends only the dirty tiles, or the full frame when too many changed.
5. The writer thread wraps the output in synchronized output (mode 2026)
   and ends it with a fence query.
6. If 2 fences are still unanswered, the frame is not rendered and the
   window is marked dirty again. The newest frame wins; stale frames are
   dropped, not queued.

### 4.4 Geometry and scale

- **Window size in device pixels:** `cols × cell_w` by `rows × cell_h`.
- **Cell size:** from `TIOCGWINSZ`, then the `CSI 16 t` reply, then mode
  2048, and finally an 8×16 guess.
- **GPUI scale factor:** a heuristic (2 when cells are at least 24 device px
  tall), with `GPUI_TUI_SCALE` as an override.
- **Render scale:** the app renders at a fraction of the terminal resolution
  (for example 0.5–0.75), and kitty or Ghostty upscales the placement with
  `c=`/`r=`. Over SSH this cuts both the bytes per frame and the server's
  **software rendering cost** by up to 4×, in exchange for some blur. It
  defaults to automatic over SSH and can be overridden.

## 5. Mapping GPUI platform features to the terminal

| GPUI feature | Local | Over SSH |
|---|---|---|
| Window bounds and resize | Terminal grid in pixels; SIGWINCH and mode 2048 call `on_resize` | Same |
| `set_title` | OSC 2 | Same |
| Cursor style | OSC 22 pointer shapes | Same |
| Keyboard | Kitty keyboard protocol: press, release, repeat, alternate keys, associated text | Same |
| Text input / IME | The terminal composes text, which arrives as `KeyDown` with `key_char`; no marked text | Same |
| Mouse | SGR-pixel (1016), with cell-centre coordinates as a fallback; a synthetic `MouseMove` before each press; wheel becomes `ScrollDelta::Lines` | Same |
| Focus | Mode 1004 calls `on_active_status_change` | Same |
| Paste | Bracketed paste goes to the input handler's `replace_text_in_range` | Same |
| Clipboard write | OSC 52 plus a local cache | OSC 52 reaches your local clipboard |
| Clipboard read | Last cached value (own writes plus last paste) | Same |
| Light/dark (`window_appearance`) | OSC 11 background luminance at startup, then live updates through `CSI ?2031h` notifications | Same |
| `open_url`, `reveal_path`, `open_with_system` | `xdg-open` | **No-op** (decision 6). SSH is detected through `SSH_CONNECTION`/`SSH_TTY` |
| `prompt` (alerts) | Return `None`, so GPUI's in-window fallback prompt is used | Same |
| File dialogs | A built-in text file picker drawn with terminal text in place of the app (open files or folders, multiple, save with overwrite confirmation). Any GPUI app gets it without code changes | Same; the picker browses the server's filesystem, which is where the paths are |
| Menus (`set_menus`, dock) | No-op; apps use in-window menus | Same |
| Multiple windows | A stack: the active window fills the terminal and the others are hidden | Same |
| Window decorations and controls | Report server-side decorations and no window controls | Same |
| Credentials | OS secret service when reachable, otherwise an error | Usually unreachable; the app needs a fallback |
| Displays | One synthetic display the size of the terminal | Same |

## 6. Terminal protocol details

- **Enter sequence:**
  - alternate screen and hidden cursor (1049, 25),
  - mouse 1003, 1006, 1016,
  - focus 1004, bracketed paste 2004, in-band resize 2048,
  - kitty keyboard `CSI >31u`,
  - color-scheme notifications 2031.
- **Probes:** batched, terminated by DA1, about 2 s timeout in total. They
  cover:
  - kitty graphics (`a=q`) and shm reachability (`a=q,t=s`),
  - DECRQM for modes 1016, 2026, and 2048,
  - kitty keyboard (`CSI ?u`), OSC 11, and `CSI 16 t`.

  If the terminal does not answer the kitty graphics probe, gpui-tui exits
  with a clear error saying kitty or Ghostty is required.
- **Transports:**
  - `shm` locally (POSIX shm with `mmap` and `posix_fallocate`; unread
    segments are reaped),
  - `file` (`t=t`) as a local fallback,
  - `inline` (zlib plus base64, chunked) over SSH. This comes with the SSH
    phase.
  - `GPUI_TUI_TRANSPORT` overrides the choice.
- **Exit and safety:**
  - A guard restores the terminal on normal exit, on panic (panic hook), and
    on SIGTERM and SIGHUP. Restoring pops the kitty keyboard mode, deletes
    the images (`a=d,d=A`), disables the modes, and leaves the alternate
    screen.
  - Raw mode disables ISIG, so Ctrl-C reaches the app. If the app does not
    handle it, gpui-tui quits (configurable).
  - An SSH disconnect sends SIGHUP and goes through the same path.
- **Logging:** stderr is the terminal, so logs go to `GPUI_TUI_LOG=<path>`
  or are buffered and printed after exit.
- **Input parser:** built on `vte`, or a custom state machine if `vte`
  handles APC poorly. It covers CSI-u keys, SGR(-pixel) mouse, APC graphics
  replies, OSC replies, DECRPM, DA1, and bracketed paste. A lone ESC is
  resolved after about 20 ms. The parser is pure and tested with byte
  fixtures.

## 7. GPUI version strategy: explicit, tested `gpui-pre` versions

**Constraints:**
- Cargo allows only one semver-compatible version of a crate in a dependency
  graph, and the lockfile resolves optional dependencies too. So
  per-version Cargo features such as `gpui-pre-0_3_8` and `gpui-pre-0_3_9`
  are not possible within `0.3.x`.
- `gpui-pre-*` crates pin each other exactly, and UI kits such as `gpui-kit`
  pin `gpui-pre` exactly. The app's dependency graph therefore already
  decides the exact version.

**Approach:**
1. gpui-tui declares a **verified range** for the whole family:
   ```toml
   gpui      = { package = "gpui-pre",      version = ">=0.3.9, <=0.3.9" }
   gpui_wgpu = { package = "gpui-pre-wgpu", version = ">=0.3.9, <=0.3.9", features = ["bench-support"] }
   ```
   The app's own pins select the exact version within that range.
2. `scripts/check-gpui-versions.sh` loops over every version in the range:
   it runs `cargo update -p gpui-pre --precise <v>` together with the
   sibling crates, then builds and runs the tests. It can become CI later.
   The README keeps a table mapping each gpui-tui version to the `gpui-pre`
   versions it supports.
3. Widening the range:
   - When a new `gpui-pre` release compiles and passes unchanged, extend
     the upper bound and publish a patch release.
   - When a release breaks the platform traits, publish a new gpui-tui
     minor release with a new range. Old versions stay installable for apps
     still on older `gpui-pre`.
   - I'm avoiding `cfg` shims keyed off the resolved version. Cargo does not
     expose it cleanly, and they would rot.
4. **Initial target: 0.3.9 only.** Once the matrix script exists (end of
   Phase 2), test whether older 0.3.x versions also work and widen the
   range downwards if they do.

### 7.1 Getting pixels out of GPUI: is `bench-support` needed?

We need a renderer that draws a `Scene` into a texture and reads it back.
In `gpui-pre` 0.3.9, everything that does this is behind
`any(test, feature = "bench-support", feature = "test-support")`:
`WgpuHeadlessRenderer`, `WgpuContext::new_headless` (which is also
`pub(crate)`), and the `PlatformHeadlessRenderer` trait.
`WgpuRenderer::new` is public but needs a window surface, which a terminal
does not have.

**gpuix does not use `bench-support`.** Its `gpui_terminal` crate depends on
GPUI by path, from a Zed fork checked out as a git submodule
(`gpui = { path = "../../zed/crates/gpui" }`). The fork adds
`WgpuContext::new_offscreen()`, `WgpuRenderer::new_offscreen(&ctx, size)`,
and `render_to_pixels(scene, size, buf)`. On macOS it adds
`MetalRenderer::new_headless`. In other words, they solved this problem by
forking. That works for a monorepo that ships its own GPUI, but it conflicts
with decision 3: apps would have to `[patch]` their GPUI to our fork.

| Option | Works with stock crates.io `gpui-pre` | Cost |
|---|---|---|
| **A. `bench-support`** | Yes | Builds `criterion` (with HTML reports), `gpui-pre-bench-metrics`, and `hdrhistogram` into the app. It also turns on GPUI's `profiler` feature, which adds task-timing counters to the executors: a small runtime overhead, to be measured in Phase 0. It logs an error when a software adapter is picked (cosmetic) |
| B. `test-support` | Yes | **Not viable.** It swaps GPUI's real executor for the deterministic test scheduler (`gpui-pre/src/executor.rs:321-336`), so a live app would behave wrongly |
| C. Fork, the gpuix way | No; apps need `[patch]` | We maintain a fork that follows weekly snapshots |
| D. Our own offscreen renderer (copy of gpui-wgpu's pipelines and shaders) | Yes | About 2–3k lines that follow upstream changes every week. The copied code stays Apache-2.0 inside our MIT crate |
| E. Upstream change | Yes, once it lands | `gpui-pre` is a snapshot of `zed-industries/zed` (for example `zed@93d7456`). A small Zed PR adding a `headless-renderer` feature, or loosening the `cfg`, would reach `gpui-pre` in a later snapshot. Acceptance is not guaranteed |

**Recommendation:**
- Use **A** for now: gpui-tui enables `bench-support` on `gpui-pre-wgpu`,
  and apps keep it out of normal builds by depending on gpui-tui only
  through an optional `tui` feature. Cargo feature unification means that any binary which
  links gpui-tui gets `bench-support`.
- Open the **E** PR in parallel. When a `gpui-pre` snapshot includes it, the
  gpui-tui release for that range switches to the new feature and A goes
  away.
- Use C or D only if Phase 0 shows A is unacceptable, for example because of
  measurable profiler overhead.

## 8. Public API sketch

```rust
fn main() {
    // `--tui` (or GPUI_TUI=1) selects the terminal platform.
    let (use_tui, args) = gpui_tui::take_tui_flag(std::env::args_os());

    let app = if use_tui {
        gpui_tui::application()          // Application::with_platform(TuiPlatform)
    } else {
        gpui_kit::application()
    };

    app.with_assets(AppAssets).run(move |cx| { /* unchanged, uses `args` */ });
}
```

- `take_tui_flag` is a pure helper. It returns whether TUI mode was asked
  for, plus the arguments with `--tui` removed, so apps that read positional
  arguments are not confused. Apps that use `clap` can declare `--tui`
  themselves and call `gpui_tui::application()` directly.
- `gpui_tui::application_with(Options { transport, render_scale,
  scale_factor, max_fps, quit_on_unhandled_ctrl_c, log_path })` exposes the
  tuning knobs.

## 9. Qwerio integration

1. Add an optional `tui` feature to `qwerio-desktop` that depends on
   `gpui-tui`, and switch platforms at the single `gpui_kit::application()`
   call site (`crates/qwerio-desktop/src/main.rs:92`).
2. **The arguments need care:** `main.rs:90` treats the first argument as the
   initial workspace path, so `qwerio --tui` would otherwise open a
   workspace called `--tui`. It needs `take_tui_flag` before that line.
3. Gaps to handle in Qwerio itself, not in gpui-tui:
   - **Keychain over SSH (Phase 4):** `keyring` (Secret Service) is usually
     unreachable in a plain SSH session. It needs a fallback: an in-memory
     store, a prompt each session, or an encrypted file.
   - **File dialogs:** used by the connection dialog, the workspace
     switcher, and CSV export. They depend on the gpui-tui picker
     (Phase 3).
   - **Theme:** `theme::init` reads `cx.window_appearance()` before any
     window exists, so gpui-tui must finish the OSC 11 probe before
     `on_finish_launching`.
   - **Single-instance lock:** decide whether TUI mode shares it with the
     desktop app or uses its own.
   - **Version coupling:** Qwerio's `gpui-kit` pins `gpui-pre =0.3.9`, so
     Qwerio can use any gpui-tui release whose range includes 0.3.9.
4. Qwerio's `unsafe_code = "forbid"` lint only covers Qwerio's own crates.
   gpui-tui needs some `unsafe` (mmap, ioctl, raw-window-handle stubs),
   kept to a few audited modules.

## 10. SSH on servers without a GPU (later phase)

This is not built until Phase 4. It is described here so the earlier phases
keep it possible: an encoder-agnostic pipeline, a writer thread, fences, and
no assumptions that frames are cheap to send.

- **Usage:** `ssh -t host qwerio --tui` through the server's normal sshd.
  The app runs on the server, and frames travel over the SSH pty with the
  inline transport.
- **Software rendering is the expected path.** wgpu can render with:
  - GL through Mesa **llvmpipe** (EGL, surfaceless, no display server). This
    is the most likely option on a stock server, because it comes with the
    `mesa` package.
  - Vulkan through Mesa **lavapipe** (`vulkan-swrast` on Arch,
    `mesa-vulkan-drivers` on Debian/Ubuntu).

  Phase 0 has to show that `WgpuHeadlessRenderer` gets one of these
  adapters with no `WAYLAND_DISPLAY`/`DISPLAY` set.
- **CPU cost levers:**
  1. render scale (fewer pixels),
  2. a lower frame cap (30 Hz),
  3. idle sleep (no frames without changes).
- **Bandwidth levers:**
  1. dirty tiles,
  2. render scale,
  3. zlib level,
  4. dropping frames through fences,
  5. RGB (`f=24`) for opaque frames.
- **Contingency:** if a target server cannot have Mesa at all (for example
  a minimal container, or no root access), the fallback is a pure-Rust CPU
  rasterizer for GPUI's `Scene` primitives (quads with rounded corners and
  borders, shadows, paths, glyph sprites), built on `tiny-skia`. This is a
  large piece of work, so it happens only if Phase 0 or Phase 4 shows it is
  needed.

## 11. Roadmap

Every phase ends with `cargo fmt --check`, `cargo clippy`, and `cargo test`
passing, plus its own acceptance criteria.

**Phase 0: bootstrap and spike (de-risking)**
- Create the workspace (Rust 1.98.1, edition 2024) and depend on
  `gpui-pre`/`gpui-pre-wgpu` `=0.3.9` with `bench-support`.
- Spike: build a hello `Scene`, render it with `WgpuHeadlessRenderer`, write
  a PNG, then show it once in kitty with direct transmission.
- Measure render plus readback time at 1600×1000 on the NVIDIA GPU.
- Measure the cost of `bench-support` (§7.1): extra build time, binary size,
  and executor overhead from the `profiler` feature.
- Check that the dependencies resolve alongside `gpui-kit` 0.7.3 in a
  scratch copy of Qwerio's manifest, without touching Qwerio.
- Optional, to inform Phase 4 early: the same timing with llvmpipe
  (`LIBGL_ALWAYS_SOFTWARE=1`, no display variables set).
- **Done when** there is a PNG, an image in kitty, and a table of timings
  and costs.

**Phase 1: `gpui-tui-term` (no GPUI)**
- Raw-mode guard, enter/exit sequences, capability probe, input parser,
  kitty encoder (shm and file transports), writer thread, fences.
- `gpui-tui-testterm`: a fake terminal that answers probes, decodes kitty
  commands into a framebuffer, and injects input bytes.
- **Done when** the parser and encoder have byte-fixture unit tests and a
  demo streams a synthetic animation to kitty and Ghostty.

**Phase 2: GPUI platform MVP**
- `TuiDispatcher` and the calloop loop, `TuiPlatform` with stubs, a single
  window, and the render path.
- Input: keyboard, mouse, scroll, resize, focus, paste.
- Output: title, cursor, OSC 52 write, OSC 11 appearance.
- `take_tui_flag`.
- The version-matrix script.
- Examples: hello, input inspector, a gpui-kit component sampler.
- **Done when:**
  - the examples are interactive in kitty and Ghostty locally,
  - idle CPU is about 0%,
  - resize works,
  - the terminal is restored after a quit or a panic,
  - one end-to-end test drives an example through `testterm`.

**Phase 3: Qwerio locally**
- The `tui` feature and argument handling in Qwerio.
- Window controls and decorations, the multi-window stack, the built-in
  file picker.
- **Done when** Qwerio's main flows (connect to SQLite, browse tables, run
  SQL, edit a file) work in kitty and Ghostty locally.

**Phase 4 (later, separate go-ahead): SSH to a GPU-less server**
- The inline transport (zlib, base64, chunking), the shm-reachability
  probe, dirty tiles, render scale, a 30 Hz cap, and tuned compression.
- Software rendering through llvmpipe or lavapipe, SSH detection (links
  become no-ops), and Qwerio's keychain fallback.
- **Done when** `ssh -t host qwerio --tui` on a server rendering with
  llvmpipe or lavapipe stays interactive on a LAN (target at least 15 fps
  for typing and hover), with server CPU, bytes per frame, and fps
  measured.

**Later (by need):**
- Unicode placeholders for tmux.
- Sixel or cell-blitter encoders for other terminals.
- macOS.
- Upstream `headless-renderer` feature.
- The CPU rasterizer contingency (§10).
- A zero-code `[patch]` shim.

## 12. Testing strategy

- **Pure unit tests in `gpui-tui-term`:**
  - the input parser, with byte fixtures captured from real kitty and
    Ghostty,
  - the kitty encoder (chunking, keys, zlib),
  - the damage tracker,
  - probe-reply handling with timeouts.
- **End-to-end tests through `gpui-tui-testterm`:**
  - Run a small GPUI app on gpui-tui against the fake terminal over a pty.
  - Inject clicks and keys, then assert on the decoded framebuffer (pixel
    regions or perceptual hashes) and on app state.
  - Force llvmpipe (`LIBGL_ALWAYS_SOFTWARE=1`) so results are deterministic
    and need no GPU.
- **Version matrix:** `scripts/check-gpui-versions.sh` (§7).
- **Manual checks per phase:** kitty and Ghostty locally. From Phase 4 on,
  also kitty over SSH to a GPU-less host.

## 13. Risks

| Risk | Mitigation |
|---|---|
| Weekly `gpui-pre` releases change platform traits | Verified version ranges, matrix script, a thin adapter, logic kept in `gpui-tui-term` |
| Software rendering too slow on the server | Phase 0 measurement, render scale, 30 Hz cap, idle sleep; CPU rasterizer contingency |
| Server has no Mesa | Document the requirement; CPU rasterizer contingency |
| `bench-support` build weight or profiler overhead, or upstream drops the headless renderer | Measure in Phase 0; app-side optional feature; upstream PR (§7.1 E); fork or own renderer as a last resort |
| SSH bandwidth makes it laggy | Dirty tiles, render scale, fences |
| Shortcut collisions (the terminal grabs Ctrl+Shift+C/V) | Document them; paste works through bracketed paste |
| Terminal left broken after a crash or disconnect | Restore guard, panic hook, SIGHUP/SIGTERM handlers |

## 14. Proposed stack

- Rust 1.98.1, edition 2024, Cargo workspace (matches Qwerio).
- `gpui-pre` and `gpui-pre-wgpu` at the verified range (initially only
  0.3.9; `-wgpu` with `bench-support`).
- `calloop` for the event loop.
- `rustix` for termios, ioctl, shm/mmap, and signals.
- `vte` as the escape-sequence parsing base.
- `flate2` (zlib) and `base64`.
- `anyhow`, `thiserror`, and `log` (same as GPUI).
- Dev: `image` for PNG test snapshots.
- No crossterm or termina, because of their APC parsing problems and
  because we need full control of the byte stream.
- License: MIT. GPUI itself is Apache-2.0; depending on it is fine. Any code
  ever copied from it (§7.1 D) would keep its Apache-2.0 notice.

## 15. Open questions

Saved for Phase 4 (SSH): the remote server's distro, whether Mesa can be
installed there, and whether to install `vulkan-swrast` here for comparison.
