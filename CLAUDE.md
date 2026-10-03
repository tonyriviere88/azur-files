# CLAUDE.md

Operating notes for an agent working in this repository. Facts and rules only — the
reasoning behind almost all of it is in the `//!` module headers, which are unusually
complete. Read the header of a module before changing it.

`azur-files` ("Azur Files") — a Windows file manager in Rust on egui 0.35, ~109k lines across
~170 files in `src/`. Windows-only (`src/windows/` is the platform half of almost every
module). Sibling path dependency: `../azur-egui-theme`, the shared Azur design system.

---

## Build

**Always build into `target/claude`, via the aliases in `.cargo/config.toml`:**

```powershell
cargo claude-build     # -> target/claude/debug/azur-files.exe
cargo claude-clippy    # all targets; reports rustc errors too
cargo claude-test      # shares the compiled dependencies
cargo claude-run
```

- `target/debug` and `target/release` belong to the user. They keep an instance of the app
  running most of the time, Windows locks the running image, so a build there fails **and
  leaves the old exe behind** — a screenshot will then happily show stale code.
- There is deliberately **no `claude-check`**. `clippy` and `check` disagree about the
  rustc wrapper, so alternating them in one target dir re-checks the world each time.
  Use clippy and never `cargo check`.
- Do not add a linker or a profile override to `.cargo/config.toml`. Both are part of
  cargo's fingerprint and would mark every unit in the user's warm `target/debug` dirty.
  `lld-link` was measured and is not faster here (2136 ms vs 2137 ms on the real link).
- **Launch cargo from PowerShell, not Git Bash.** Git Bash bakes a lower-case `d:` into
  `CARGO_MANIFEST_DIR`, which reddens the `.lnk` tooltip test, and it does not self-heal
  on rebuild.
- Timings, kept warm: 0.8 s no-op, 4.7 s one-file edit. From empty: 2 m 35 s and 2.8 GB.
- Under memory pressure (rustc dying at codegen, not at check): build with
  `CARGO_PROFILE_DEV_DEBUG=0` into its own target dir.
- `LNK2019` on `anon.*.llvm.*` symbols is a poisoned rustc incremental session, not a code
  error. Delete that one crate's incremental dir and its rlib. Never `cargo clean`.

`cargo run --release` is the build to judge anything by. `cargo deploy` builds release and
copies to `D:\Programs` (or `$AZUR_DEPLOY_DIR`); it can replace a copy that is running.

**Never run `cargo fmt`.** This repo is hand-formatted; `fmt` churns dozens of untouched
files and `--check` is not a gate.

---

## Tests

858 unit tests, in `#[cfg(test)]` modules and `*/tests.rs` beside the code they cover.
About 43 are `#[ignore]` — measurements, and anything that touches the real shell,
clipboard, network or GPU.

```powershell
cargo claude-test <filter>
cargo claude-test <filter> -- --ignored --nocapture --test-threads=1
```

**Rules, each of which exists because it was broken once:**

- **Tests act only in `target/sandbox`.** `crate::sandbox` is the policy, not a
  convention — `sandbox::dir("name")` hands back an empty directory inside it and panics
  on anything outside. Every test path, env-var supplied or hardcoded, stays under it.
  Do not call `sandbox::allow_outside`; if a test cannot be written against the sandbox,
  that is a conversation with the user.
- **`shell::ops::FOR_REAL` gates every `IFileOperation` job.** Under `cfg!(test)` a job
  reports back done without the shell being asked, unless the `ops::for_real()` scope
  guard is held. Three tests hold it, all inside the sandbox. A green suite once asked
  Windows to permanently delete `.cargo` from the repo while the confirmation dialog sat
  on screen and nothing in the output said a word.
- **`Config::save` returns early under `cfg!(test)`.** A test process has no business
  writing a user's settings. For a real launch, set `YAFE_PROFILE=<dir>` to move the
  settings directory — otherwise the run overwrites the user's window size, tabs and
  bookmarks. Every save keeps `config.ini.bak`.
- **Never run `probe_invokable` (or `probe_invoking`) against a real path.** They invoke
  real context-menu verbs. Run once with `YAFE_PROBE` pointing at this repository, it
  enumerated eighteen verbs, ran them, and permanently deleted the entire working tree.
  Nothing reached the Recycle Bin.
- **`--test-threads=1` for anything under `shell::`.** Two threads asking the shell for an
  icon or a thumbnail get one answer between them. A `shell::` failure under parallel
  threads is usually the harness, not the code.
- **Run only the tests the change touched.** The suite drives the real clipboard and real
  file operations, so unrelated tests are not a free check.
- **A failure may already be there.** Check it against `HEAD` before assuming you caused
  it. A git worktree needs `../azur-egui-theme` junctioned in as a sibling, and removing
  that junction needs a `\\?\` path.
- `cargo claude-test` stalls when a test fails and a backgrounded run looks hung while it
  waits on the cargo lock. Build with `--no-run` and run the test exe directly, output to
  a file.
- **Click tests paint once per palette.** A test that loops `Palette::ALL` builds a whole
  `Harness` per entry and blows a 10-minute timeout. Assert against one palette unless the
  palette *is* what is under test.

Click targets are the one thing that cannot be checked by reading the code: an interaction
rect covered by another reads perfectly correctly at the call site and simply does not
respond. `src/app/click_tests/` drives synthetic `PointerButton` events through whole
frames of the real interface and asserts on what the application did.

**But a synthesised egui event is not a key press.** It bypasses `egui-winit`, which is
exactly where `Tab` and `Ctrl+C` are decided, so keyboard tests can pass against a build
where the gesture is broken. Drive the real build with `SendInput` for those.

---

## Architecture

| module | responsibility |
| --- | --- |
| [`fs/`](src/fs/) | everything that touches the disk, nothing that touches the screen |
| [`loader.rs`](src/loader.rs) | scans on worker threads, LRU cache in front |
| [`pane/`](src/pane/) | tabs: where they point, sort, filter, selection, history |
| [`dock/`](src/dock/) | the binary tree that arranges panes |
| [`ui/`](src/ui/) | painting, at explicit rects |
| [`app/`](src/app/) | state, keyboard, and the single place anything changes |
| [`shell/`](src/shell/) | the parts that *are* the shell: icons, thumbnails, clipboard, `IFileOperation`, `IContextMenu` content, OLE drag and drop |
| [`windows/`](src/windows/) | the Win32/COM half of each of the above |
| [`theme.rs`](src/theme.rs) | Azur roles, the eight palettes, file-kind and code hues |
| [`icons.rs`](src/icons.rs) | painted glyphs on a 16-unit grid, for the fallbacks |
| [`git/`](src/git/) | status by running `git` and reading porcelain v2 |
| [`pe/`](src/pe/), [`archive/`](src/archive/), [`preview/`](src/preview/), [`syntax/`](src/syntax/), [`markdown/`](src/markdown/), [`console/`](src/console/) | the preview panel's readers |

Four structural invariants. Breaking any of them compiles and then misbehaves at runtime:

1. **Nothing mutates during drawing.** A breadcrumb can close a tab in another pane; the
   sidebar navigates whichever pane has focus; dropping a tab restructures the tree the
   draw loop is walking. Drawing code pushes an [`Action`](src/app/action.rs) and
   `App::apply` performs it after the frame. Never take a `&mut` through the draw path.
2. **The UI paints, it does not lay out.** A row is six painter calls against a computed
   rect, not a `horizontal()` of `Label`s. This is why a folder of 100k files scrolls at
   the refresh rate — a frame costs what the *window* is worth, never what the folder is.
3. **`fs/` enumerates once and never goes back to the disk.** The directory read already
   gave each name, size, timestamp and attribute word. A `Path::is_dir` or `fs::metadata`
   on top of that turns one sequential read into a round trip per file: measured 202×
   slower over 60,000 files. `SHGetFileInfo` for a type name is 1700× slower — the shell
   is never asked what a file is; a static extension table answers in 45 ns.
4. **`src/windows/*.rs` is the platform half of the portable module next door**, and each
   one names its counterpart in its own `//!` header. Types live in the portable half so
   what a thing *is* stays platform-neutral.

Anything allocated because of a folder is freed when you leave it. A per-file answer
carries the *view* that asked and the *row* it asked about; an answer arriving after you
have gone finds no view and is discarded. Do not key a per-file cache on `PathBuf` — that
is the leak this design was overhauled to remove.

---

## Do not "fix" these

Each looks like a bug or an oversight and is a measured decision:

- **The renderer is wgpu over Vulkan only**, via `azur_egui_theme::render::wgpu_options()`
  at [main.rs:498](src/main.rs#L498). Not a preference: DWM vertically resamples a
  composed GL or D3D12 surface by one row, which destroys single-pixel hairlines and the
  faint seams the design system is built on. There used to be `AZUR_GLOW`, `AZUR_BACKEND`
  escape hatches; they are gone, and 1.1 MB of standby backends with them. The cost is
  real and stated: a machine with no working Vulkan driver will not open this window.
  `AZUR_ADAPTER=low` (integrated GPU) is the one remaining hatch.
  **No screenshot can show the blur** — a screen capture forces composition, so every
  capture lands on the broken path, including ones taken while the screen looks sharp.
  Use `--shot`, which reads the frame back off the GPU.
- **`egui_glow` leaks a couple of KB per draw call per frame** (the driver, not this code).
  Hence: one atlas for every shell icon, and icons held back from the row loop and flushed
  after it. Do not draw a texture inside a row loop — it splits the frame at every row.
- **The D3D11 device in the video player is deliberately leaked** (`Box::leak`,
  [windows/video.rs:140](src/windows/video.rs#L140)). Releasing it deadlocks inside the
  NVIDIA driver and would hang the window on exit.
- **There is no git cache, and that is the feature.** `git status` is re-run; git is the
  fastest thing that does it, and the answer is the same one the user's own tooling gives.
- **Text is greyscale, not ClearType.** epaint has no per-channel atlas; the coverage gamma
  is the only reachable lever, and it lives in `../azur-egui-theme`, pinned per palette to
  Explorer's measured weight. Four other levers were measured and are duds.
  `multisampling: 0` and `dithering: false` at [main.rs:361-368](src/main.rs#L361-L368)
  are deliberate — egui already antialiases by feathering, and dithering adds noise to
  anything sampled from the glyph atlas.
- **The Recycle Bin is listed here, by reading every `$I…` file** ([`fs::recycle`](src/fs/recycle.rs)),
  at `shell:RecycleBinFolder`. A row's name is its original path and its target the `$R…` file.
  Never hand a `$R…` path to `IFileOperation`, the clipboard or a drag — moving one orphans its
  `$I…`. Restore, Delete and Empty are the bin's own verbs, from the menu the *bin* gives for its
  items (`shell::ops::bin::held_menu`); `recycle::is_held` and `Job::bin_refusal` are the guards,
  and `Modal::send` refuses any bin verb under `cfg(test)`.
- **Collation is code-point order beyond ASCII.** Digits sort as numbers and case is
  ignored. Doing it properly needs ICU and the sort would stop being free.
- **A reparse point is listed but never followed.**
- **`--shot` regenerates `docs/*.png`, and those are the user's to update.** Capture to
  the scratchpad to check a visual change. (`docs/` is not currently tracked or present,
  so the README's image links are dead until the user regenerates them.)

---

## Theme and design system

The window is `../azur-egui-theme` throughout, with the dense-window preset applied by
`desktop::apply` in [theme.rs](src/theme.rs) and read back through `ui::hover_fill`,
`ui::row_fill`, `ui::seam`, `ui::bar`, `ui::squared`.

- **Set the token, not the call sites.** Doing it at call sites produced two hover greys in
  one window because four buttons reach their fill through `ui::control_fills` instead of
  `ui::row_fill`, and nothing looked wrong at either end.
- **A literal colour in a painting routine is a palette that only works once.** A
  hard-coded `#202329` drew the dark theme's hairline across every pane of the light
  palette. `every_colour_on_screen_belongs_to_the_palette_it_is_set_to` catches that class
  by provenance; contrast tests measure roles and cannot see a painter call that never
  asked the theme anything.
- **Counting tokens is not measuring contrast.** The dark ramp is perceptually finer than
  the light one, so "four steps up" and "two steps up" can be the same amount. Any claim
  about a colour belongs in CIELAB: `azur_egui_theme::contrast` — `ratio` for ink, `apart`
  for two surfaces, `over` first for anything translucent.
- **A palette varies regions or roles, and those are different jobs.** `theme::Surfaces`
  is one field per region the window paints; `Surfaces::from_azur` is the old
  role mapping and is what the dark palette still uses, to the byte. Anything about
  *state* — one hover, one press, one selected row — stays Azur's.
- **Align anything drawn in a row.** Centre a glyph by its ink, not by its box; put text on
  its baseline, not centred on its line box (a line box reserves descender room a file name
  does not use). `components::ink_baseline` for a row holding a glyph, `row_baseline` for a
  row of text alone. Row heights round up to a whole *device* pixel, and text origins snap
  with `round_to_pixels`, not `f32::round` — at 125% scaling a whole point is a quarter
  pixel.
- `ROW_HEIGHT` is 24 and a sidebar row 22 (`tokens::row::DENSE` / `TIGHT`), not the design
  system's 36. Rows are contiguous: no `item_spacing` between them.
- Menu geometry is asked for by name (`menu_item_height()`, `menu_divider_height()`,
  `menu_item_metrics()`), never re-derived. Re-deriving it was wrong in three ways at once.

---

## Environment and conventions

- **Line endings are mixed, file by file.** `src/ui/chrome/mod.rs` is CRLF and
  `src/ui/mod.rs` is LF, so a newline-joined search pattern can silently fail to match.
  `git diff` cannot see a whole-file flip.
- **`HEAD` moves during a session.** The user commits while work is in progress, so
  `git checkout <file>` has reverted live work that looked safe. Undo a temporary edit by
  re-editing it, not by checking it out.
- Settings live at `%APPDATA%\Azur Files\config.ini`, a hand-editable `key=value`
  file. Unparseable lines are skipped, never fatal. `config::file` is the one place they are
  read from — there is deliberately no fallback to a folder named after an older build. Four flags default to *on* and are
  read as "anything but `0`" so that a file written before they existed keeps the default:
  `line_numbers`, `diff`, `regroup`, `auto_tiles`.
- Test/diagnostic env vars: `YAFE_PROFILE` (settings dir), `YAFE_WALK`, `YAFE_THUMBS`,
  `YAFE_FLATTEN_ROOT`, `YAFE_NO_FLUSH`, `YAFE_PROBE`, `YAFE_PROBE_TYPED`.
- The CLI exists largely so that things behind a click can be captured or driven:
  `--open=` (repeatable, one pane each), `--reveal=`, `--filter=`, `--lens=git|images`,
  `--size=WxH`, `--theme=<name>`, `--shot=<file>`, `--menu`, `--rename`, `--preview`,
  `--deps=<module>`, `--compare`, `--console[=a;b;c]`, `--flat[=list|tree]`, `--tiles`,
  `--stack`, `--trace`, `--walk=<dir>`. A `--trace` line every three seconds reports
  private bytes, GDI/USER handles and what the cache holds.
- A path given to `--open=` may use either slash; it is rewritten to backslashes, because
  every file API accepts `D:/x` and `SHParseDisplayName` does not — and a path the shell
  cannot parse is a path with no context menu, silently.
