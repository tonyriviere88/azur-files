# Azur Files

Azur Files is an alternative to Windows explorer.

It's a fast file explorer with tabs support, multiple preview panes (text, image, video, binary dependencies), diff viewer for texts and images, folder diff, git and console.

![The window: two panes, tabs, bookmarks and thumbnails](docs/overview.png)

<br>
<p align="center">
  <a href="https://github.com/tonyriviere88/azur-files/releases/latest">
    <img alt="Download for Windows" src="https://img.shields.io/badge/Download-for%20Windows-0078D4?style=for-the-badge&logo=windows&logoColor=white">
  </a>
  <a href="https://github.com/tonyriviere88/azur-files/releases">
    <img alt="Latest release" src="https://img.shields.io/github/v/release/tonyriviere88/azur-files?style=for-the-badge&label=latest&color=1f2937">
  </a>
</p>
<br>

# Features

- **fast** — a folder is read once and never asked about again, so a frame costs what the
  *window* is worth rather than what the folder is. The status line shows the real numbers.
- **[flatten folder](#flatten-a-folder)** — show all folders and files, as one list or as a tree
- **[folder size](#folder-sizes)** — measure the folder you are looking at, with a bar per row
- **[filter search](#filter-as-you-type)** — type, and the listing narrows as you type
- **bookmarks** — the folders you actually use, arrangeable into groups
- **tabs / panels** — tabs per pane, and panes split by dragging a tab to an edge
- **[preview](#preview)** — [text](#text), [pictures](#picture), [video](#video) and a
  [binary's dependencies](#binary), inside the pane
- **[folder diff](#folder-diff)** — two folder trees side by side, only the differences in colour
- **[console](#console)** — a shell in the folder on show, with its output in blocks
- **[git](#git)** — status on every row, a lens for what changed, and a diff in the preview

## Flatten a folder

Everything below the current folder in one listing — as a tree, or as a flat list with each
file's folder beside its name. Nothing is read twice: the flattened view is the same scan the
listing already did.

![A folder flattened into a tree](docs/flatten.png)

## Folder sizes

Explorer will not tell you how big a folder is without opening its properties one folder at a
time. Press the measure button and every folder on show is counted at once, each row carrying
a bar for its share of the total.

![Folder sizes, with a bar for each folder's share](docs/folder-sizes.png)

## Filter as you type

The filter box narrows the listing to what matches. Combined with the flatten above it becomes
a search over the whole tree — here, every `.rs` file under a project: 15 rows out of 231,
picked out in 5.8 ms, with no background indexer anywhere.

![Filtering a flattened tree](docs/filter.png)

## Preview

Put the keyboard on a file and press **Space** — or `Ctrl+P` — and the panel opens beside the
listing, showing the file itself. It lives *inside* the pane, so two panes side by side each get
their own: comparing two builds of the same DLL is a matter of looking left and right.

`Space` closes it again. Which side it takes — right, bottom, or whichever fits the pane's
shape — is a setting for the window; whether it is open is a fact about each folder, so opening
it here does not open it in the tab beside this one.

### Text

- Text file preview, with line numbers
- Syntax coloration for source files — the language from the extension, or from the first
  couple of characters when the name says nothing
- Markdown rendered as a document, or as its source
- Find within the file, and two or more files side by side
- Diff — see [Git](#git) below, which is where "what did I change here" lives

![A source file, coloured](docs/preview-text.png)

Markdown is rendered rather than shown as text, with a button on the bar to see the markup
instead:

![A Markdown file, rendered as a document](docs/preview-markdown.png)

Select more than one file and the panel tiles them, which is how you read two versions of
something side by side:

![Two source files side by side in the panel](docs/preview-two-files.png)

### Picture

- Image preview, zoom and pan
- PNG, JPEG, GIF, BMP, TIFF, WebP, ICO, and SVG rasterised
- Image diff: select two pictures and get the two of them and the difference

A folder that is mostly pictures switches itself to thumbnails; the panel shows the full image
with its dimensions and a zoom control.

![An image preview beside a grid of thumbnails](docs/preview-image.png)

Two pictures selected at once are shown as three: each of them, and a mask of where they
differ, with the share of differing pixels on the bar. The same view answers "what changed in
this image" against the last commit.

![Two revisions of a poster, and the difference between them](docs/preview-image-diff.png)

### Video

- video player — play, seek, mute, fullscreen, inside the panel

Windows' own decoders, so whatever plays elsewhere on the machine plays here. The clock, the
sound and the seek bar are the panel's; nothing launches.

![A video playing in the preview panel](docs/preview-video.png)

### Binary

- dependencies — what an executable imports, from where, and what is missing

The whole dependency graph for a `.exe` or `.dll`: every module it pulls in, the path each one
actually resolved to, its architecture, and a count of the ones Windows could not find. Fold a
module open to see what *it* depends on, search the graph by name, or flatten it to a plain
list.

![The dependency tree of an executable](docs/preview-deps.png)

Click a module and two more panels appear below the tree: the symbols this binary imports from
it, and the symbols it exports.

## Folder diff

- two folder trees side by side, in a tab of their own
- what is on one side only, and what differs in type, size or date
- show everything, only the changes, or only what was added or removed

Open it from the menu at the top left (**Folder diff...**), or right-click two selected folders
and pick **Folder diff**. Both sides are the whole tree, flattened, each with its own path bar —
point either one somewhere else and the comparison runs again. Everything that is the same on
both sides steps back to grey, so only the real differences keep a colour: a name found on one
side only is green, and for a file both sides have, it is the size, date or type that differs
which is highlighted, not the name. The two trees scroll, open and close together.

![Two releases compared, everything shown](docs/folder-diff.png)

The button at the right of each path bar narrows both sides at once, and the choice is kept for
the next diff: every file, only the changes, or only the names that are on one side and not the
other.

![The same two releases, only the changes](docs/folder-diff-changes.png)

## Console

- basic console support (batch, powershell, bash)
- cwd synchronized with the explorer folder
- output navigation

Not a terminal — a shell on plain pipes, kept alive across commands, with its output collected
in blocks: one per command, foldable, each carrying its exit status, and each walkable with the
keyboard so you can pull one line out of a build log without reaching for the mouse. Which is
the right shape for running a build, a `git status` or a test and reading the answer.

The shell starts in the folder the pane is showing, and it works the other way too: `cd`
somewhere in the console and the pane follows. A prompt hook (optional, one line in your shell
profile) extends that to a *real* terminal outside the window — `cd` there, and the focused pane
goes with it.

`git rebase -i`, `ssh` and anything else that wants a real terminal will not work here, by
design: a pipe is not a terminal, and the alternative was writing a terminal emulator.

![The console panel, running in the folder on show](docs/console.png)

## Git

- changes — every modified, added, deleted and untracked file, in one listing
- git diff — what changed in a file, in the preview panel

Every row in every listing carries git's answer for that file as a badge on its icon: a green
tick for nothing left to do, amber for a change on disk, amber with a plus for a change already
staged, a grey ring for untracked, red for gone or conflicted. A folder wears the most urgent
thing under it.

The filter funnel's **git lens** then throws away everything else and lists only what has
changed, wherever it sits in the tree. The status line carries the branch and the count.

There is no cache: `git` is re-run, because git is the fastest thing that answers this and the
answer is then the same one your own tooling gives.

![The git lens: only what has changed](docs/git-lens.png)

The preview panel's diff view puts the lines a file gained on a green band and the ones it lost
back on a red one, numbered as `HEAD` had them, with long runs of unchanged lines folded away —
and the file's syntax colouring intact.

![A file against HEAD, in the preview panel](docs/git-diff.png)

# Building

You need Windows, a Rust toolchain of 1.85 or later (from [rustup](https://rustup.rs)) with the
MSVC target, and the Visual Studio C++ build tools that come with it. The window renders through
Vulkan only, so the machine that runs it needs a working Vulkan driver.

```sh
git clone https://github.com/tonyriviere88/azur-files.git
cd azur-files
cargo build --release
```

The executable is `target/release/azur-files.exe`. It is self-contained, so you can copy it
anywhere and run it from there.

`cargo deploy` does both steps in one go: it builds the release binary and copies it to
`D:\Programs`, or to `$AZUR_DEPLOY_DIR` if that is set, or to a folder given on the command line
(`cargo deploy E:\Tools`). It can replace a copy that is running.

The first build compiles every dependency from nothing, so expect a few minutes and a few GB
in `target/`. Rebuilds after that are incremental.
