# Azur File Explorer

![the mark](../azur-egui-theme/app-icons/file-explorer/app-64.png)

A file manager that stays out of the way. Tabs in the title bar, above the pane they
belong to, a breadcrumb that behaves like Explorer's, a details view that does not care
how big the folder is, and panes you build by dragging a tab to the edge of one.

Rust, [egui](https://github.com/emilk/egui) 0.35, and the
[Azur design system](../azur-egui-theme) — dark by default.

```
cargo run --release
```

| Dark | Light |
| --- | --- |
| ![The window in dark mode](docs/window.png) | ![The window in light mode](docs/window-light.png) |

The images are made by the program itself rather than captured by hand, so they can be
remade after any change:

```
cargo run --release -- --open=<a> --open=<b> --size=1360x760 --shot=docs/window
```

## The layout

```
┌──────────────────────────────────────────────────────────────────────┐
│ ▣  [ Documents ✕ ][ src ✕ ] +                         ─  ▢  ✕       │  title bar
├───────────────┬──────────────────────────────────────────────────────┤
│ Drives        │ ← → ↑ │ This PC › C: › Users › tony  ★ ↻ [Filter]    │  path bar
│  Windows (C:) ├──────────────────────────────────────────────────────┤
│  ▰▰▰▰▰▱▱▱▱▱▱▱ │ Name               │ Size  │ Type      ▲ │ Modified  │  header
│ Bookmarks     ├──────────────────────────────────────────────────────┤
│  ★ Sources    │ 📁 Desktop         │       │ File folder │ 02/08 03:2│
│ Places        │ 📄 notes.md        │ 4.2 KB│ Markdown    │ 01/08 19:1│  rows
│  This PC …    ├──────────────────────────────────────────────────────┤
│               │ 7 folders, 21 files · 4.1 MB              0.4 ms     │  status
└───────────────┴──────────────────────────────────────────────────────┘
```

Split side by side, and each pane's strip sits in the title bar **directly above that
pane** — at the pane's own x-range, not packed along from the left:

```
┌──────────────────────────────────────────────────────────────────────┐
│ ▣              [ Documents ✕ ][ src ✕ ] + │ [ System32 ✕ ] +  ─ ▢ ✕  │  title bar
├───────────────┬─────────────────────────┬────────────────────────────┤
│ Drives        │ ← → ↑ │ … › tony  ★ ↻   │ ← → ↑ │ … › System32  ★ ↻  │  path bar
│  Windows (C:) ├─────────────────────────┼────────────────────────────┤
│  ▰▰▰▰▰▱▱▱▱▱▱▱ │ Name       │ Size │ Type│ Name       │ Size │ Type   │  header
│ Bookmarks     ├─────────────────────────┼────────────────────────────┤
│  ★ Sources    │ 📁 Desktop │      │ …   │ 📁 0409    │      │ …      │  rows
│ Places        ├─────────────────────────┼────────────────────────────┤
│  This PC …    │ 21 items      0.4 ms    │ 4910 items        2.3 ms   │  status
└───────────────┴─────────────────────────┴────────────────────────────┘
```

Stack them instead and the lower pane has no title bar above it — only another pane. So
its row gets **a band of its own**, painted like the title bar, taking its height off the
top of the panes under it:

```
┌──────────────────────────────────────────────────────────────────────┐
│ ▣              [ src ✕ ] +                            ─  ▢  ✕       │  title bar
├───────────────┬──────────────────────────────────────────────────────┤
│ Drives        │ ← → ↑ │ … › src  ★ ↻                                │  path bar
│  Windows (C:) │ Name               │ Size  │ Type      ▲ │ Modified  │
│  ▰▰▰▰▰▱▱▱▱▱▱▱ │ 📄 app.rs          │ 112 KB│ Rust source │ 02/08 17:0│  rows
│ Bookmarks     │ 8 files · 207 KB                          0.1 ms    │  status
│  ★ Sources    ├──────────────────────────────────────────────────────┤
│ Places        │ [ Fonts ✕ ] +                                        │  band
│  This PC …    ├──────────────────────────────────────────────────────┤
│               │ ← → ↑ │ … › Fonts  ★ ↻                              │  path bar
│               │ 📄 85f874.fon      │ 12 KB │ FON file    │ 01/04 09:2│  rows
│               │ 629 items · 434 MB                        0.7 ms    │  status
└───────────────┴──────────────────────────────────────────────────────┘
```

![Two panes, stacked](docs/stacked.png)

One rule produces both: **a tab goes above the pane it governs.** Everything else follows
from where that place happens to be. The band spans only the panes in its own row, so a
full-height pane beside a split column is not covered by its neighbour's tabs, and the
rightmost strip in the bar stops short of the caption buttons — tabs fill a strip from the
left, and without that reserve a pane with enough tabs leaves nowhere to grab the window by.

**A tab is shaped like a browser's**: it fills the strip's height rather than floating in it
as a chip, sits hard against its neighbour, and is rounded only at the top — the bottom edge
is the join with the pane it governs, and rounding a join that is not there only blurs where the
edge is. Two quiet tabs side by side get a hairline between them, because touching surfaces with
no fill between them read as one long row of labels; where either has a surface of its own, its
own edge does that job and a line there would be noise.

Which pane the keyboard is in is said by its active tab — and *only* there. The active tab is
grey rather than accented, and the focused pane's is one step lighter than an unfocused pane's:
`background-control-active` against `background-control-hover`. It used to be a 2px accent bar
along the tab's bottom edge, which was the loudest thing in a quiet title bar for a fact that
matters only when there is more than one pane. A ring around the focused pane says the same thing
ten times as loudly again, and in a two-pane window it makes every click redraw a frame.

### One surface, divided by lines

The sidebar and the panes used to be **cards**: `radius-medium`, a `stroke-subtle` ring each,
four points of canvas between them, four more around the outside, and a point of padding inside
each one to clear its own ring. Every interior boundary was therefore three lines wide — two
borders and a channel between them — and the window read as a handful of loose panels that
happened to be next to each other rather than as one window divided up.

They are now square, unringed, **one point apart**, and flush: with the window's own border on
three sides and the title bar on the fourth, `background-canvas` is not painted anywhere at all.
What shows in that one point between them is the fill of a selected tab.

That colour does three jobs, which is the whole of the idea: **the selected tab, the path bar
directly under it, and the seams between panels.** The tab and the bar are one surface with the
listing hanging off it; the lines dividing the window are that same surface seen edge-on.
`ui::seam` is the one place it is decided and `ui::SEAM` is the one point, so a tab cannot drift
from the bar it is welded to.

**Nobody draws the seams.** One fill goes down behind the whole block and the panels are painted
over it, so a boundary is simply where the panels do not quite meet. That is worth the indirection
because panes come from a tree of splits: *where the seams are* is the layout's answer and changes
with every divider drag, whereas *the panels do not cover this* is true for free and for any
nesting.

The colour is **`stroke-subtle`**, which is what Azur calls a separator inside a surface — which
is what a seam is. It was `background-control-active`, and came down a step because as a filled
band across the top of every pane that was too loud in the dark theme.

Counting tokens gives the wrong answer about what that did, which is worth recording.
`control-active` is four *names* up the ladder from `background-layer` in the dark theme and two
in the light one, so it looks like the same token was doing very different things on the two
sides. It was not: the dark gray ramp is perceptually finer than the light paper ladder, and
measured in CIELAB the old pairing was nearly symmetric — **ΔL\* 15.4 dark against 13.9 light**.
Nothing was broken. It was simply more contrast than wanted.

| | frame vs panel | | |
| --- | --- | --- | --- |
| dark, `control-active` `#33373f` | ΔL\* 15.4 | | was |
| dark, `stroke-subtle` `#202329` | ΔL\* 6.1 | | is |
| light, `stroke-subtle` `#ccd1d9` | ΔL\* 13.9 | | unchanged — `GRAY_14` is what `control-active` already was there |

So the dark frame is now deliberately the quieter of the two rather than matching it. The floor on
how far it can go is the column header, `background-layer-alt`, which the bar has to read as a
different surface from — ΔL\* 3.9 in the dark theme, and a test holds it.

The strip's whole ladder is pinned to that colour rather than to Azur's control steps, and came
down with it: nothing, then `background-layer-alt` for a hover or an unfocused pane's active tab,
then the seam itself for a press or the focused pane's active tab. A press taking the *selected*
colour is deliberate — it is about to become the selection. Reading those off `control-hover` and
`control-active` was right while the bar was `control-active` too, and became an inversion the
moment it was not: a hovered inactive tab came out brighter than the selected one, which reads as
though the pointer had selected it.

One thing the darker greys cost, and its fix: an unfocused pane's active tab is now *one* step off
the strip rather than three, and one step at the bottom of the dark ramp is `GRAY_2` to `GRAY_3` —
barely a change. So the label carries the distinction too, `text-secondary` against
`text-primary`, which is the more legible of the two channels anyway.

And one thing that was a bug: **a pressed control on the path bar was invisible.** A control
sitting on a surface that is itself one of the control steps has nowhere to step to, and paints
the surface's own colour onto the surface. That is the light theme, where the seam and
`control-active` are the same `GRAY_14`; in the dark theme they are two apart and the ordinary
ladder is right. Nothing was wrong with any of that code — two tokens simply met.
`ui::control_fills` takes the surface a control sits on and steps *away* from it, so hover and
press stay distinct from the surface and from each other and go the same direction on either bar.

Reaching the edges has a bill, and it is the **resize bands**. This window has no platform frame,
so it draws its own resize edges, and the reason they were guaranteed never to steal a click was
that they sat in the four points of canvas no panel wanted. There is no such canvas now, so
`chrome::RESIZE_BAND` is taken out of the panels — which is what every window with chrome of its
own does here: the frame is inside the client area or there is no frame to grab. What it costs,
measured rather than assumed:

| edge | what it overlaps | cost |
| --- | --- | --- |
| top | nothing — the title bar's own controls all start below `TOP_BAND` | none |
| left | a sidebar row's left padding | none worth naming |
| bottom | the pane's two switches and `N changed`, on the status line | their lowest point of 18 |
| right | the outer 4 of a 10-point scrollbar | 6 points left to grab |

The scrollbar is the only real one, so it has a test that drags it from one point inside the band and
checks the listing moves. The bottom row of that table used to read "the status line, which nothing
clicks"; the console's switch is now down there, and 18 points of switch in a 22-point bar has no
slack to dodge a 4-point band with — so its lowest point belongs to the window's edge, and a hover that
does not light up is what says so. It was two points until the whole line moved up one; halving it was
not the reason for that and is what it bought. The rest has a test that finds the switch by sweeping
the bar for something under the pointer, because a control that cannot be reached is a control that
does not work, however right its rect looks. The **view switch** in front of it and `N changed` at the
other end of the group are the second and third controls down there, and each has the same sweep for
the same reason — the view switch's also checks that the two 18-point switches four points apart are
still two targets and still in the order they were asked for, which is not something a rect can be read
to prove. A maximised window skips the bands entirely, since there is nothing to resize.

**The weld is real, not just matching paint.** The title bar draws a `stroke-subtle` hairline
along its bottom to say where it ends — which matters against the sidebar, whose fill is the same
`background-layer` — and that line used to run straight through the join between the active tab
and the path bar it shares a colour with. It was invisible before only because there were four
points of canvas under it. Now the hairline goes down *before* the tabs, and the focused pane's
active tab paints one point past its own rect to cover it. An unfocused pane's tab is a different
grey from the bar below it and keeps the edge, which is right: it is not welded to a surface it
does not match.

Four tests hold all this, three of them against the shapes a frame actually painted rather than
against the source — the harness keeps the frame's `Shape`s now, because a fill is not a click
target and reading the source only proves the source says what it says.

**The whole bar behaves like a title bar**: drag any part of it that is not a tab or a
button to move the window, double-click to maximise. That falls out of registering the
caption *first* and letting everything drawn after it claim its own pixels back — egui
gives a click to the last widget that asked for it, so the mark, the tabs, the `+` and the
caption buttons all win without any of them having to be subtracted from anything.

The window opens at **1024×600**, which is also the size the density is set for: at that
size the sidebar still holds every drive, bookmark and place at once, and the listing
still has four columns and a status line. **Reset window size**, in the app menu under the mark,
puts it back there — the way out of a window dragged to a shape you did not mean, and the
counterpart of double-clicking the sidebar splitter. One constant, `config::WINDOW_SIZE`, is both
the size it opens at and the size it goes back to.

It un-maximises first, and says so rather than relying on it: on Windows an inner size alone is
enough, because `SetWindowPos` restores a maximised window on the way — measured, from a maximised
2560×1392 straight back to 1024×600 with that line taken out — but that is winit's platform
behaviour and not a promise.

## Docking

**Drag a tab onto the right-hand edge of a pane and let go.** The pane splits and the
tab opens beside it. The same gesture works on all four edges, nests as deep as you
like, and the drop preview shows exactly what you are about to get before you commit.

- **Onto a pane's edge** → split, with the tab on that side.
- **Onto a pane's middle** → move the tab into that pane.
- **Onto a tab strip** → reorder, or move between panes, with a caret showing where.
- **Drag a divider** to rebalance; **double-click** it to even the split up.
- Closing a pane's last tab collapses the split. The last tab of the last pane closes
  the window.

Without touching the mouse: `Ctrl+\` splits the current folder to the right, and the
app menu (the mark at the top left) and every folder's context menu both offer
"open in a pane to the right / below".

## The path bar

Explorer's, which is still the best version of this control:

- Every **segment** goes there. Middle-click opens it in a new tab.
- Every **chevron between segments** drops down that folder's subfolders, so you can step
  sideways into a sibling without going up first, and the leading one lists the drives.
  The folder is read on the frame the menu opens, once, rather than on every frame it is
  showing — a popup body runs continuously, and re-reading `C:\Windows\System32` sixty
  times a second to draw the same list would be a self-inflicted stall.
- **Once a dropdown is open, the whole bar is one control.** The chevron and the name in
  front of it are filled as a single shape — they *are* one thing: that folder, and what is
  inside it — and moving the pointer along the bar carries the dropdown with it, no second
  click needed. Hover a name and its own chevron opens; hover a chevron and it opens.
  Clicking an entry, clicking away or `Escape` lets go.
- **A path too long to fit** collapses from the front into a `…` that opens the rest as a
  menu. The current folder is never the part that disappears, and the `…` joins in the same
  tracking as the chevrons.
- **Click the empty space** past the last segment — or `Ctrl+L` — to type a path. Either
  slash works, `%VARS%` and `~` expand, and a drive letter on its own means its root. The path
  arrives **selected**, so typing a new one replaces it rather than appending to it.
- **Right-click the field for `Use / in path`**, which is which slash it writes between the parts
  of a path. Off, because `\` is what Windows shows everywhere else; on, the field fills with
  `D:/Sources/MyTools` and completing walks it with `/` too. A question about where the path is
  going next rather than about the folder in front of you — a shell, a URL and nearly every source
  file want the other slash, and converting one by hand after copying it out of here is a small tax
  paid every time. It is remembered between sessions, and ticking it rewrites the path already in
  the field rather than waiting for the next one: the menu stays open, and what it did is behind it.
  Nothing else changes — the field has always taken either slash, whichever way this is set.
- **What you type completes**, in the same dropdown a chevron opens — the same rows, the same
  shell icons, because it is the same question asked with the keyboard instead of the pointer.
  Matching is by prefix and case-insensitive, as a Windows file name is.

  `Down` and `Up` walk the offers, and either one puts the list up if it is not up. Past the
  last and before the first is **what you typed**, so you can get back to your own text without
  deleting anything. `Right` and `Tab` put the highlighted name in the field *with the separator
  after it*, so what is offered next is what is inside it — `Down Right Down Right` walks a tree
  from the keyboard with no pointer and no `Enter` anywhere in it. `Enter` on an offer goes
  there; on nothing it goes where the text says, exactly as it did before.

  Nothing is offered until you type. `Ctrl+L` fills the field with where you already are, and a
  list of the folder you are standing in is a list in the way — it says something the bar behind
  it already said. `Down` brings it up too, for when you would rather pick than type.

  **Ten rows, and then it scrolls.** A whole number of them, which is the reason the figure is
  stated rather than left to the design system's 320-point ceiling: that is 11.43 rows, so the
  twelfth came out as a two-point sliver along the bottom edge. A list you walk with the arrow
  keys should end where a row ends — a half-drawn one reads as a rendering fault, and saying
  "there is more below" is the scrollbar's job, which it already does. It is a ceiling and not a
  size: five offers make a five-row dropdown.

  **The folders come from the loader**, on a worker, and never from the drawing thread. This is
  the one place in the window where that is not merely tidiness: a completion reads whatever has
  been typed, on the keystroke, and what has been typed can name a share that is not there.
  `Path::is_dir` against a mapped drive whose share has gone away blocks for as long as SMB takes
  to give up — [`fs::drives`](src/fs/drives.rs) measures it at 22 seconds — so the field never
  asks. It hands the path to the scanner and draws whatever came back. See
  [`fs::typed_folder`](src/fs/mod.rs), which is `resolve_input` with that one step left out, and
  exists for exactly that reason.

  Folders only, and hidden ones only once a name is being typed: with nothing after the
  separator this is the chevron menu's question and gets its answer, which keeps
  `$Recycle.Bin` off the top of every drive. With a name half typed it is a different question,
  and a folder that exists and is not offered reads as a bug.
- **A pen at the right-hand end** says so, and it is a drawing rather than a button: its space
  is reserved out of the trail's, so a path deep enough to fill the bar stops short of it. Under
  the pointer the bar takes the border a text field wears — over the whole shape the field is
  about to have — and the pen brightens to the same ink, so the two read as one cue. No fill:
  washing the bar to announce something that is only an announcement was too much.

  The hairline this replaced was `stroke-subtle`, which is the colour [`ui::seam`](src/ui/mod.rs)
  paints the bar — a line drawn in the colour behind it. It had been invisible the whole time.

**Back, Forward, Up, Refresh** at the left, in that order, and at the right the **flatten**
toggle and the filter box. Refresh sits with the other three because it is the same kind of
thing — something you do to the folder you are looking at — and because the right-hand end is
the end that gets given up when a pane is narrow. It is in the group that is never dropped now,
which is the point of moving it.

Flatten and the filter are the same kind of thing as each other — a question asked *of* the
folder rather than somewhere to go — which is why they sit together, in that order, and are
given up together when there is no room. The button survives a little longer than the box does:
24 points is affordable long after 160 is, and a flatten you could turn on and not off would be
a trap. It carries a **context menu**, like the eye beside it, and for the same reason: the choice
between the two flatten modes is a setting of the thing the button opens, and settings belong on the
control that opens it. See [Flattening a folder](#flattening-a-folder).

**Every word in the box narrows, in any order**, and three characters mean something:

| typed | keeps |
| --- | --- |
| `report` | anything containing `report`, in any case |
| `wor he` | anything containing **both**, in **either order** |
| `!tmp` | anything *not* containing `tmp` |
| `^src` | anything **beginning** with `src` |
| `.rs$` | anything **ending** with `.rs` |
| `@git` | only what git says has changed — see below |

One substring is a poor way to find a file you half remember: you know two things about it and
not which comes first. `pane pre` finds `preview.rs` in a pane's worth of source either way
round, and `!target !.lock` typed once over a flattened tree is a listing you can read. The
rule itself is [the design system's](../azur-egui-theme/README.md#a-filter-field-is-its-behaviour-too),
not this program's, because a filter field is one of its components and what typing in one
means is as much a part of that component as its border — the same reason the hover grey is
not decided here either. **The tooltip on the box is that library's own two-line
description**, so the two cannot drift apart, and so the field says the same thing in every
window that has one.

**`@git` is the one word in the box that is not about the name.** It keeps the rows git has
something to say about — changed, staged, untracked, conflicted, the same set the status line counts
as `N changed` — and it is the question a folder cannot otherwise be asked: *what have I touched
here.* Folders are kept when anything under them has changed, however deep, because a folder filtered
out is a folder you cannot open to reach what is inside it. The word names who is being asked rather
than what the answer is, which is why it is not `@changes`: staged, untracked and conflicted are all
in the set, and so is a folder that only has one of them somewhere underneath.

It is a **word rather than a button**, so it composes with the rest of the line: `@git .rs$` is
both tests, and it can be typed anywhere in the box rather than only first. (There *is* a button, and
it types this word: `N changed` in [the status line](#the-status-line). A word that a control can put
in the box loses nothing; a control that could not be composed with `.rs$` would.) `@` because it cannot
begin a name anybody is typing a fragment of, so nothing is taken away from the ordinary case — and
the design system's [`Query`](../azur-egui-theme/README.md#a-filter-field-is-its-behaviour-too) never
sees it. The word is taken off the line first, which keeps a filter field's syntax the library's and
git the application's.

One thing about it is not obvious and is deliberate: git answers a frame or two *after* the listing,
and until then the question cannot be evaluated. An unanswered question **excludes nothing** and the
order is rebuilt when the answer lands. The alternative was a listing that emptied for two frames on
every refresh — on every file operation, every `F5`, every time the watcher noticed something — and
then filled again, which reads as the folder having been wiped. A folder that has been asked about and
*is* not a repository keeps no rows, which is the honest answer and the one case that needs the two
states told apart.

What this program decides is **what the words are matched against: the whole path.** Not the
name on the row, which was the rule while there was only ever one word. A flattened listing is
mostly a question *about folders* — `!node_modules`, `^src`, `!\obj\` — and the folder you are
looking at is part of what you are looking at, so it should be part of what you can say about
it. The cost is one surprise worth knowing: every row shares the folder's own path, so a
fragment that appears in it keeps everything, and typing `s` in `C:\Sources` narrows nothing.
A word or two in is where that stops mattering — and since the box waits for the typing to
stop, the state that matches everything is mostly one you never see.

**`Ctrl+F` or `F3` puts the caret in the box**, and the field selects what it already holds, so
either key lands ready for a query typed over the last one. `F3` because it is one key rather than
a chord and because most Windows programs have meant "find" with it for longer than `Ctrl+F` has
been the convention; nothing else in this window wants it, since `F2` renames and `F5` re-reads.
Both live beside the field rather than in `App::keyboard`, because both need its response to hand
focus to — which also means both are there only while the pane is wide enough to draw the box.

**A changed filter opens the listing at the top.** Row 200 of what one filter left is not row 200
of what the next one leaves, and narrowing five thousand rows to twelve while halfway down them
leaves no offset worth keeping — what a scroll area does with one it cannot honour is show you the
*end* of the twelve, or the empty tail under them. The rows a filter finds are worth reading from
the first one. Only a *changed filter* does this: a column click and an `F5` rebuild the same
order from the same filter, and losing your place in a listing you are still reading is what those
two must not do.

**The filter waits 250ms for the typing to stop**, and every keystroke restarts the wait. A
filter is re-applied from scratch on each change — one pass over every entry, then a sort of
whatever survived — so on a big enough listing it is not the *filter* that is slow, it is
typing. Measured on a flattened `C:\Program Files`, 188,734 entries, release build:

| filter | rows it leaves | one pass |
| --- | --- | --- |
| `""` | 188,690 | 250 ms |
| `"e"` | 188,690 | 237 ms |
| `"ex"` | 62,297 | 129 ms |
| `"exe"` | 3,073 | 27 ms |
| `"micro exe"` | 1,805 | 24 ms |
| `"!exe"` | 185,617 | 261 ms |

Note which passes are the expensive ones: the **intermediate** needles, because they are the ones
that leave enough rows to sort. Typing `exe` cost 416ms of frozen window in three stalls to
arrive at an answer that takes 27ms to compute, and the two stalls in front of it were for orders
nobody was going to read. Now only the last keystroke does any work.

The last two rows say the same thing twice: **matching is not what costs anything, the sort of
what survives is.** `micro exe` walks every path looking for two words instead of one and comes
out a tenth of the price of `e`, which walks it looking for one and keeps everything. `^c:` keeps
all 188,690 in 220ms, which is `e`'s number — so an anchor is free too. Nothing in the syntax can
make a keystroke slow; only the answer it leaves can. (Run to run these vary by about 15%, which
is wider than any gap between the terms. And `"e"` is where the whole-path rule shows: it used to
leave 187,961 rows, and now every row contains an `e` because `C:\Program Files\` does.)

A quarter of a second is longer than the ~150ms at which a pause starts to read as lag, and that
trade is deliberate: the pause is not being hidden, it is being spent on *not* running the two
expensive passes above it. A 250ms gap between keystrokes is slow typing, so a word typed at any
ordinary speed collapses into one pass — and where the answer costs a fifth of a second to
compute, waiting a quarter for the typing to finish is cheaper than computing three answers
nobody reads. On a small folder, where a pass is microseconds, all it costs is that the listing
settles a beat after you stop.

The repaint that notices the deadline has to be *asked for*: this window is idle between events,
so a keystroke's frame is the last one there will be, and without it a filter typed and left
alone would apply whenever the pointer next moved.

There is no bookmark star. It was a permanent fixture that spent most of its life saying
nothing, and a toggle whose two states are a hollow star and a filled one is a poor way to be
told anything. `Ctrl+D` still pins the folder, and so does its context menu, which is where the
rest of what you can do to a folder already lives.

And so does **Windows' own entry for it**. A folder's context menu has `Pin to Quick access`
in it — `Épingler à l'accès rapide` on a French Windows — because the shell put it there, and
what it is *for* is the sidebar of a file manager. The sidebar in front of you is this one, so
that entry pins here, into Bookmarks, exactly as `Ctrl+D` would; `Unpin from Quick access`
removes it again.

Four more of the shell's own entries are answered here for the same reason — what they are *for*
is the file manager in front of you:

| the entry | what Windows would do | what happens instead |
| --- | --- | --- |
| `Open` on a folder | opens it in a new Explorer window | navigates this pane; several folders get a tab each |
| `Cut`, `Copy` | fill the clipboard, and nothing here knows | this program's own, so the cut rows fade |
| `Paste` on a folder | the shell's own copy | this program's, into that folder |

Every one is recognised by verb (`open`, `cut`, `copy`, `paste`, `pintohome`, `unpinfromhome`)
rather than by label, since the labels are translations — `Ouvrir`, `Couper`, `Copier`, `Coller`
on a French Windows. `Open` on a *file* is deliberately left alone: the shell's is the registered
default verb, and a file is not a place this program can show.

Right-clicking **empty space** is the one menu with an entry of this program's own in it, at the
top, greyed when there is nothing to paste. That is not a preference: the background menu is a
different shell object from a selection's, it carries no `Paste` to redirect, and Explorer's own
is synthesised by its view rather than read out of the shell. So either this program puts one
there or empty space has no Paste at all. `Ctrl+V` does the same thing.

Everything else in those menus is still Windows'.

With one difference from Explorer: **going up does not trim the bar.**

```
in  d:\Sources\MyTools\yet-another-file-explorer\src\ui
  › This PC › d: › Sources › MyTools › yet-another-file-explorer › src › ui

after Alt+Up, twice
  › This PC › d: › Sources › MyTools › yet-another-file-explorer › src › ui
                                       ^^^^^^^^^^^^^^^^^^^^^^^^  bold: where you are
```

The folders you came out of stay on the bar, so going back down is a click on a name that is
already on screen. Explorer cuts them off, and then the only way back is to open a chevron
and read a menu to find a name you were looking at a moment ago.

The bold segment is the folder being shown — wherever along the trail it sits — and
everything else on the bar is a link, on both sides of it. Stepping anywhere off the trail
replaces it; going deeper extends it. And the overflow is measured only up to the current
folder, so a long tail can never push the folder you are in off the front of the bar: what
does not fit is the tail, truncated at the right-hand edge.

## The left panel

Drives, bookmarks and places, each group collapsible and remembered.

The icons are **Windows' own**, per place rather than per type: the Downloads arrow, the
Pictures thumbnail, the Recycle Bin, a network volume's plug, a drive with a custom
`autorun.inf` icon. That means asking the shell about *that path* — with the file
attributes flag off, so it is allowed to read the folder's `desktop.ini` — and for the
Recycle Bin and This PC, which are not files at all, parsing a moniker into a PIDL first.
There are a dozen of them, resolved once in the background, so the per-path cost the
listing goes to such lengths to avoid does not apply here. Until an answer arrives the
painted glyph stands in, which is also what a machine with no shell gets.

Those two are also the only lookups here that need COM, which is worth knowing because
getting it wrong is invisible: `SHParseDisplayName` on a thread with no apartment fails
with `CO_E_NOTINITIALIZED`, and a failed lookup is indistinguishable from "no icon for
this" — so This PC and the Recycle Bin quietly drew their painted glyphs while every real
folder beside them had the shell's. They go through one long-lived thread that enters an
apartment once and never leaves it, because entering one *per lookup* is worse than the
bug: the last `CoUninitialize` in a process frees shell state other threads are still
using, and that showed up immediately as `SHGetFileInfoW` elsewhere returning nothing.

The same applies inside the listing: under This PC, a row stands for a volume rather than
for a child of the folder, so its icon is asked about by path too — otherwise every drive
draws the generic folder, which is what they did.

Each drive shows its name and a bar for how full it is. **The numbers are a tooltip** —
`Data (D:) — 713 GB free of 931 GB` — rather than a caption on the row. Inline, that caption
charged every drive row for a second piece of text that had to fit beside the name, which in
a 200-point panel it often did not: it came and went as the panel was dragged, and took
width from the name whenever it stayed. The bar already answers "how full" at a glance; the
digits are what you go looking for, and going looking is what a hover is.

The panel opens at **200 points**, which fits the longest place name and a labelled drive
letter with room to spare. Drag the splitter to change it — and **double-click the splitter to put
it back**, which is the same gesture the column edges in the listing already answer to, and the
way out of a panel dragged somewhere silly. One constant, `config::SIDEBAR_WIDTH`, is both the
width it opens at and the width it goes back to.

Bookmarks are a list you arrange:

- **Drag a folder from a listing onto the group** to pin it. It reports itself to the
  pointer as a *link*, which is the truthful answer — nothing is copied and nothing moved —
  and is what Explorer does when you drag a folder onto Quick Access.
- **Drag a bookmark up or down** to reorder, with a caret showing where it will land.

## Sorting

Click a header to sort by it; click again to reverse. Folders stay above files whatever
the column and whichever direction, because a folder is a place and a file is a thing.

A listing opens **sorted by type**, which puts a folder of mixed work into groups you can
scan — the sources together, the images together — with names still in order inside each
group. Explorer defaults to Name; this does not, because the case for Name is that it is
what you would sort by if you had to pick one thing, and the case for Type is that it
gives you Name *and* a grouping for free.

The Type column sorts on **the label it shows**, not on the extension behind it. That
sounds like a detail and is the difference between a column that is in order and one that
is not: by extension, `.cpp` precedes `.exe` and the column reads "C++ source" above
"Application". Resolving a label per comparison would be `n log n` lookups, so each
*distinct* extension is resolved once, the labels are ranked among themselves, and the
sort compares integers — which also means `.jpg` and `.jpeg` rank equal, since both say
"JPEG image", and interleave into one group by name rather than forming two.

The sorted column's triangle sits **two points below the middle of the header cell**, and that
correction lives in the design system rather than here. A header centres its label and its
indicator on the same line, and a line of text does not fill its own box: the ascender and
descender room that `Modified` and `Type` never reach is counted into the height being centred, so
the ink lands lower than a shape that fills its box does. The label is the thing there is a column
of, so the shape is what moves — inside `icons::sort_asc`/`sort_desc`, where every consumer gets
it, including an application drawing its own table header, which is the case it was found in.
Verified by photographing the header strip either side of the change: the only pixels that differ
anywhere in it are the triangle's, and they moved from rows 8–10 to rows 10–12.

## Flattening a folder

The button before the filter — or `Ctrl+E` — replaces the folder's own children with
**everything underneath it**, in one read. Not a search: the same details view, the same columns,
the same sort and the same selection, over every file in the subtree at once.

**Two ways of showing it**, chosen from the button's own context menu and remembered in the
settings. `list` is the default and the view the button has always produced; `tree` is the same
rows arranged as the hierarchy they came out of.

```
d:\Sources\…\src        as a list             as a tree
  fs                      fs                   ⌄ fs
  shell                   fs\dir.rs                dir.rs
  ui                      fs\drives.rs             drives.rs
  app.rs                  fs\fmt.rs                fmt.rs
  main.rs                 shell\clipboard.rs   › shell
  …                       ui\filelist.rs       ⌄ ui
                          app.rs                   filelist.rs
                                                 app.rs
```

**A row's name is its path relative to the folder** — that is what is *stored*, and it is what
the sort orders by (so a name sort groups by folder) and the tail of what the filter matches
against, so `Ctrl+E` and then typing `ui\` in the filter box is one folder's worth of the tree,
`.rs$` is every source file in it, and `!\target\ .rs$` is every source file that is not build
output. `Dir::leaf` is the file's own name back out of that, for the
places that need it: the Name column, a rename that edits `filelist.rs` and not `ui\filelist.rs`,
and a row revealed after a rename or a New folder.

What the row *shows* is the name first and the folders after it, dimmed —
[see below](#a-row-says-where-it-is). Except in the tree, where the indent already says where the
row is and saying it again would be a column of dimmed paths repeating the shape of the listing. The
one place a tree row does show folders in front of its name is a **merged chain**, where they are what
the row is made of — [a chain of folders is one row](#a-chain-of-folders-is-one-row).

### The tree is the same listing, re-ordered

**Switching between the two modes reads nothing.** The walk's answer is one `Dir` and the mode
decides only the display order built over it — `sort::build_order` for the list,
`sort::build_tree_order` for the tree — so a tree that took seconds to walk switches instantly, and
so does opening and closing a folder in it. That is what the tree adds over browsing folder by
folder: the whole subtree is already here, so a branch costs a re-sort rather than a directory read,
and two folders five levels apart can be on screen together.
`switching_flatten_modes_reorders_the_listing_it_already_has` holds the `Arc` either side of the
switch and asserts it is the same one.

The order is **pre-order** — a folder, then everything inside it, then the next folder — with each
folder's own children sorted among themselves by whatever the header says. So a click on `Size`
orders each folder's contents rather than shuffling the tree into a list, and folders still lead
every level. Three rules are not the sort:

- **A row that is out takes its subtree with it.** Hidden, or excluded by the `@git` filter:
  you cannot see inside a folder you cannot see, which is what browsing one does too — the folder
  is what is marked hidden, not each file in it.
- **A shut folder's children are not in the order at all**, which is what makes a collapsed tree
  cheap: those rows are not drawn, not hit-tested and not scrolled past.
- **A filter ignores what is shut, and keeps the folders that lead to a match.** Both halves are the
  same decision. A match three levels down is unreachable without the folders above it, so a tree
  that dropped them would be a list with holes in it; and having found it, hiding it inside a folder
  you closed an hour ago would be a filter that answered and then covered its own answer. What is
  shut is *kept*, not cleared, so clearing the filter puts the tree back as it was.

**The twisty is a click on the chevron and not on the row.** Opening a branch is a way of *looking*
at it, and a click that also moved the selection would make it unusable as one — so the chevron's
box is tested first and returns, and a click anywhere else in the row selects as it always did.
`Left` and `Right` do the same from the keyboard: `Right` opens the folder under the cursor, `Left`
shuts it, and `Left` on anything that is not an open folder steps out to the folder it is in. Those
two keys were free because a details listing has nothing to scroll sideways, and the navigation pair
is `Alt+Left` and `Alt+Right`.

One level steps in by **exactly the twisty's width**, so a child's chevron sits under its parent's
icon and the chevrons down one branch make a single straight column. The indent comes out of the
Name column and nothing else, which is the other reason to keep the step small: eight folders deep at
Explorer's own 19-point step would have spent 152 points before the first letter of a name.

Which folders are shut is held **by path, not by entry index** — an index is only good until the
folder is read again, and an `F5`, a file operation or the watcher replaces the whole listing. It is
the set of what is *closed*, so the empty set is a fully expanded tree: that is what the mode shows
the moment it is turned on, because opening every folder by hand to see what you just asked to see
would be the button undoing itself.

#### A chain of folders is one row

**A folder whose entire content is one folder is not worth a row of its own.** `src\main\java\com` is
four rows, four indents and four twisties saying nothing — there was never a choice to make at any of
them. So they become one row, and it reads as the path it is:

```
  strict hierarchy            regrouped
⌄ src                       ⌄ src > main > java > com
  ⌄ main                        App.java
    ⌄ java                      Other.java
      ⌄ com                 ⌄ lib
          App.java              ⌄ b
          Other.java                c.txt
⌄ lib                             a.txt
  ⌄ b                         deep > deeper > deepest
      c.txt
    a.txt
⌄ deep
  ⌄ deeper
      deepest
```

`lib` is the counter-example and it is in the picture on purpose: it holds two things, so nothing about
it merges — and `b`, which holds exactly one *file*, does not merge either. The rule is **one folder
and nothing else**, and it applies again at every step, which is how four levels become one row.

The row that comes out is the **innermost** folder, and everything else follows from that: its Size,
Type and Modified are that folder's, because that is the folder you would have arrived at; opening the
row goes there; renaming it edits that folder's name, and the field opens over that segment rather than
over the whole chain; and its twisty and its shut-state are that folder's, so what closes is the thing
that has something in it. Only the **indent** belongs to the outermost — the chain stands exactly where
the folder it starts with stood, and its children are one level in from *there* rather than from the
depth their paths happen to have. `sort::TreeRow` is those two figures, worked out once per rebuild and
kept beside the display order.

The folders in front are drawn **dimmed** and the last one is not, for the same reason the flattened
list dims the folders after a name: they are where the row *is* rather than what it is. And when the
Name column is too narrow, the chain is cut **from the front** — `… > java > com` — which is the
opposite end from every other truncation in the window and the same reasoning: what must survive is
the part that says what the row is, and here that part is last.

Three things stop a chain, and the third is the interesting one: anything else in the folder, nothing
at all in it (`deep > deeper > deepest` is a chain that ends in an empty folder), and a folder that is
**shut** — a chain cannot reach through a closed door, so opening one merges the rest of it.

It is asked of **the rows that are on show**, which is the honest reading of "only one folder in it": a
folder holding one subfolder and one hidden file is a chain while hidden files are hidden and two rows
when they are shown, and under a filter a chain forms out of whatever the filter left. That last part
is the one worth having — `c.txt` in the filter box leaves `lib` holding one folder where it held two,
so `lib > b` becomes one row, and a filtered tree reads as its answers instead of as a ladder of
single matches.

**On by default**, and it is the one preference in the settings whose default is not the quieter
option: the rows it takes away are rows that never had anything to say. `Regroup single folders`, in
the flatten button's own context menu under the two modes — it belongs to the `Tree` above it rather
than being a third mode, and like the modes it takes effect at once on every pane already showing one
and never reads a folder again.

#### And the tree's pass is the cheaper one

Building a tree order does strictly more work than building a list one — it groups every row by the
folder it is in before it sorts anything — and it re-runs on every keystroke in the filter box, so
it is worth knowing which way that goes. Measured on a flattened `C:\Program Files`, 189,981
entries, release build:

| filter | rows it leaves | list | **tree** |
| --- | --- | --- | --- |
| `""` | 189,810 | 250 ms | **136 ms** |
| `"e"` | 189,810 | 287 ms | **172 ms** |
| `"ex"` | 65,461 | 124 ms | **130 ms** |
| `"exe"` | 4,103 | 28 ms | **82 ms** |
| `"!exe"` | 186,728 | 271 ms | **173 ms** |

**The tree is faster on every case that keeps the listing**, by nearly half, and the grouping is what
pays for it: sorting twenty thousand small sibling groups is `n log k` where one sort of 190,000 rows
is `n log n`, and that saving is larger than the hash pass costs. Where it loses is a filter narrow
enough that the list has almost nothing left to sort — the grouping is one pass over every row
whatever survives, so `exe` costs 82 ms against 28. Which settles the question the other way round
from how it was asked: the tree's *worst* case is better than the list's, so it moves nothing about
[the 250 ms the filter waits](#flattening-a-folder), and the 82 ms is a third of it.

`Ctrl+E` **works with the caret in the filter box**, which almost nothing else does: a focused
text field owns the keyboard, and rightly, but these two are the same question about the same
folder. Type two letters, look at what came up, want the rest of the tree. The filter survives
the toggle, so the two compose in either order. Not while renaming — that is an edit of one name
that a re-read would throw away — and not under a menu.

[`Ctrl+P`](#the-preview-panel) gets through for the same reason and in the other order: two letters
to find something, then see what is inside it. Those two are the only shortcuts that do.

Three things the walk has to get right, all in
[`fs::scan::scan_deep`](src/fs/scan.rs):

- **A reparse point is listed but never opened.** `C:\Users\All Users` is a junction to
  `C:\ProgramData` and `C:\Documents and Settings` is one to `C:\Users`; a walk that follows
  either never finishes. Every backup tool refuses the same thing for the same reason.
- **It stops somewhere, and says when it did.** 200,000 rows or ten seconds, whichever comes
  first, and then the status line reads `stopped at the limit — not all of the tree is here`. A
  listing quietly missing rows is the one wrong answer a file manager must never give. The row cap
  bounds the directories opened too, since every folder the walk descends into was pushed as a row
  before it was queued. The clock matters separately from the rows: a hundred nearly-empty folders
  on a sleeping share cost a round trip each and no rows at all.
- **Breadth first**, so a truncated answer is a fair picture of the tree rather than one deep path
  — and so the directories at each level are a *batch*, which is what makes the next part possible.

**The batch is read on up to eight threads at once.** A directory read is a syscall waiting on the
file system, not work for the CPU, so a thread that is waiting is a thread another directory could
have been read on. Measured warm, against the serial walk this replaced:

| tree | entries | 1 thread | 4 | **8** | 16 |
| --- | --- | --- | --- | --- | --- |
| this repository's `src` | 35 | 0.2 ms | 0.2 | **0.2** | 0.2 |
| its `target` | 26,917 | 47.9 ms | 25.5 | **22.8** | 24.9 |
| `C:\Program Files` | 188,729 | 1,945 ms | 672 | **561** | 537 |

The case worth doing something about — a tree big enough to wait for — comes back **3.5× sooner**,
and the case that was already instant is untouched: a batch no bigger than the thread count is read
inline, because spawning threads to read three directories costs more than reading them. Sixteen is
a wash against eight, so eight is where it stops.

Two things the parallel read is not allowed to cost. The answers come back **in batch order**,
because the order is what makes a truncated listing a fair picture and it is the first thing a
parallel read would lose — `reading_in_parallel_does_not_change_the_answer` holds the whole listing
and the truncated one identical at 1, 2, 4 and 8 threads. And the deadline is checked **between
directories on every thread**, so a batch of a thousand folders on a share that has gone away
cannot take a thousand timeouts to get through whatever the patience said.

It is **not cached**, where every other listing is. A flattened tree is the largest thing this
program can hold and the most quickly wrong — anything created anywhere under the root invalidates
it — and turning the button off and on again is a request to look afresh. The folder's own listing
*is* still cached, so turning it off is instant. It also runs on a thread of its own rather than
on the loader's two-to-four workers: it is the one read here that can take seconds, and a tree
walk holding a worker would leave the rest of the window queued behind it.

Two things it does not do. **The watcher stays shallow**: a change in a subfolder does not re-walk
the tree, because `ReadDirectoryChangesW` over a subtree fires continuously on anything a build
touches, and each answer here is a walk of seconds rather than a scan of milliseconds. So a
flattened listing is close to a snapshot — a change to the root folder's own files re-walks it, and
that is the only thing that does; `F5` is how you ask for the rest. And **This PC cannot be
flattened** — its rows are volumes, each of which is a place to flatten of its own — so the button
is drawn disabled there rather than latching over a listing that would not have changed.

### The flatten that switched itself off

A re-read has to know which of the two reads it is. `Loader::request` answers with the folder's own
children and `request_deep` with the tree, and `Tab::apply` takes either without complaint — so a
flattened tab asked the shallow way goes back to one folder deep with `Tab::flat` still set and the
button still lit. Nothing looks broken. The view has simply switched itself off.

Which is what the *watcher* did. `App::folder_changed` asked the shallow way whatever view the tab
was in, so saving a file into a flattened folder undid the flatten from underneath — and the reason
it was hard to pin on anything is that nothing about it is visible: the watch is not recursive, so
only the root folder's own files could trigger it and a file saved three folders down did nothing,
and a burst settles for 150ms first. It fired on some saves and not others, minutes apart, with no
gesture of yours anywhere near it.

`F5` had been fixed for this once already — `Tab::refresh` drops the listing and lets
`App::start_scans` re-ask, and that one does branch on `flat` — which is what makes it worth a
named function rather than a second branch. `ask_for(tab, loader)` is now the only place the
choice is made, by both callers.

The test for it is worth its own note, because the one that existed **could not have caught this**.
It asserted `tab.flat` — the flag, not the listing — and the flag was never the half that went. It
also flattened `src\ui`, a folder with no subfolders, whose flattened listing and shallow listing
are the same rows: there was nothing about the listing that fixture could have told you. So the
assertion that matters compares the rows before and after, over a folder that has a tree, and
`deep_reads_survive_a_folder_changing_underneath_them` puts both re-read paths through it.

**It does not follow you into the next folder**, for the same reason the filter does not: both are
a question about the folder you were looking at. That is also what makes opening a row the way
*out* of the view — without it, clicking a folder three levels down would start a second tree walk
on arrival and there would be no gesture that ended one. `F5` keeps it, because a re-read is the
same question again, and so does a duplicated tab, which is the same place as it currently looks.

## A row says where it is

The Name column is two things in one cell: **the name, and then — after a `>`, in secondary ink —
where the row is or what it points at.**

```
dir.rs > fs                                     in a flattened listing: which folder
Remote Desktop Connection.lnk > %windir%\system32\mstsc.exe    a shortcut: its target
Windows Media Player Legacy.lnk > …\Windows Media Player\wmplayer.exe
All Users > C:\ProgramData                      a junction: where it leads
```

One galley with two sections rather than two draws, so the pair share a baseline and a single
truncation, and **the name is first because the context is what should give way**: everything
after the separator is there to identify the row, not to be read.

When it does give way, **the context loses its leading components, not its trailing ones** —
`> …\Resources\Lang` rather than `> PluginGeosystem\Resour…`. The folder immediately holding a
file is what identifies it, and the last component of a shortcut's target is the program it runs.
A component at a time, never mid-name, since a path cut mid-component reads as a different path.
End-elision is right for every other cell in the listing and exactly wrong here.

**A shortcut's target has to be asked for**, and that is the one thing on a row the enumeration
cannot say. Two kinds of row have one, and they are unrelated: a `.lnk` file, whose target lives
in the file's contents and takes `IShellLink` to read, and a reparse point — a symlink or a
junction — whose target is metadata and takes one syscall. [`shell::links`](src/shell/links.rs)
answers both, and it is built the way the per-file icons are, for the same reason: asked once per
row per view, answered on a worker, delivered to the tab that asked, dropped if that tab has moved
on. A `.lnk` pointing at a share that is not currently reachable is the classic Explorer hang, and
`IPersistFile::Load` is where it happens — nothing here is allowed to make the window wait.

`SLGP_RAWPATH` is the other half of that: it hands back the path the shortcut *stores* instead of
asking the shell to go and find where the target moved to. So a stale shortcut costs a local file
read rather than a network timeout, and `%windir%\system32\mstsc.exe` is shown as it is written —
which is also what `Properties` shows. A shortcut pointing at something with no path at all — the
Recycle Bin, Control Panel, a printer — keeps its name alone.

When a row is both — a shortcut in a subfolder of a flattened listing — the **target wins**,
because that is what the row *is*. In an ordinary listing every row shares the same parent anyway,
so the target is the only context there is to give.

### And what it is, when the pointer rests on it

```
Name      LgsxItemBuilders.cpp
In        Inspect\Model                       ← only in a flattened listing
Type      C++ source
Size      9.38 KB (9,605 bytes)               ← only on a file
Modified  07/08/2026 18:24
Git       Changed on disk                     ← only where git has something to say
```

**Nothing in it is only in the tooltip**, and that is the point of it: it is the four columns of the row
it is over, plus the two things a column cannot hold, plus what git says. The Name column is whatever
the other three leave, and on a narrow pane that is narrower than plenty of names — so the first line is
the one thing the row might not have been able to show. The exact byte count is the second: `9.38 KB` is
for comparing two files at a glance and `9,605 bytes` is for the times when only the number will do. And
git has a badge on the row and nowhere to put a sentence, so the last line is the badge in words —
"something under it has changed" for a folder, because a folder's mark is the strongest thing anywhere
beneath it.

**Key and value, in two columns**, and the keys are the column headers because that is what four of the
six already are two inches up the same window. It was four unlabelled lines with the type and the size
run together by an interpunct, which had to be *read* rather than glanced at — `C++ source · 9.38 KB
(9,605 bytes)` is two facts and a piece of punctuation doing a column's job. The keys are
`text-secondary` against the values' `text-primary`, the same two colours the dimmed half of a Name cell
uses to say which half of the line is the answer.

It is drawn rather than written into a string, and the reason is the alignment: a value has to start at
the same `x` on every line, and a proportional font cannot be padded into a column with spaces. That is
also what fixed the **measure**. A tooltip's width in the design system is a paragraph's — 280 points,
because a tooltip is read in one glance and a long line is not — and a table laid out in it wrapped every
value under its own key. `components::tooltip_at_pointer_ui` is the same frame, anchor and delay with no
measure of its own, so the frame is the size of the table; the values wrap at 560 points, which clears
everything the columns can hold and still keeps a two-hundred-character name from making a tooltip as
wide as the window.

It is held back while the pointer is **doing** something: a tooltip over a rubber band is a tooltip in
the way of the gesture, and one over a row being renamed covers the field. Everything else about when it
appears is egui's — half a second of stillness, and no delay at all moving from one row to the next.

### A folder shortcut opens here

Handing a `.lnk` to the shell is what one gets by default, and for a shortcut pointing at a folder
that means **Explorer opening over the top of this program** — a file manager whose rows open a
different file manager. So opening one navigates instead: double-click, `Enter`, a middle click
into a tab of its own, or a path typed into the bar. A shortcut to a *file* still goes to the
shell, which is the only thing that knows what to do with it.

The decision is made in one place — `Action::Open` — so there is no route that gets it wrong, and
it is the *only* thing in [`shell::links`](src/shell/links.rs) that runs on the UI thread. That is
deliberate: opening has to do the same thing every time, and reaching for the column the display
fills in would make the gesture depend on whether the row had been on screen long enough. It costs
one small local read of a file in the folder already being listed, and only for a row whose name
ends in `.lnk`; the alternative on that same thread was `ShellExecute`, which does rather more.

**Nothing is asked of the target.** Whether it is a folder comes from the attributes the shortcut
itself stores, so a shortcut to a share that is not currently reachable costs nothing to classify
— where a `Path::is_dir` would have been the network timeout this program is otherwise careful to
avoid. A shortcut whose stored attributes have gone stale falls through to the shell, which is
what used to happen to every one of them.

A **junction or a directory symlink** never reaches any of this: the enumeration reports it as a
directory, so it is an ordinary navigation. And the shell context menu's own `Open` is still the
shell's — those entries are its, invoked by verb, and it is not this program's place to intercept
them.

## The preview panel

The eye on the path bar — or `Ctrl+P` — opens a panel **inside the pane**, showing what is in the
file the keyboard is on. Three views: a picture, some text, or for a binary the dependency tree
below.

```
┌───────────────┬──────────────────────────────────────────────────────┐
│ Drives        │ ← → ↑ │ … › pics        👁 ▤ [Filter]                │  the eye, latched
│  Windows (C:) ├──────────────────────────┬───────────────────────────┤
│  ▰▰▰▰▰▱▱▱▱▱▱▱ │ Name          │ Size     │ 🖼 alpha.png 300×200 100% │  the panel's bar
│ Bookmarks     ├──────────────────────────┤ − ⊡ + ✕                   │
│  ★ Sources    │ 🖼 alpha.png   │  1.55 KB │ ▨▨▨▨▨▨▨▨▨▨▨▨▨▨▨▨▨▨▨▨▨▨▨▨ │
│ Places        │ 🖼 mark.svg    │    222 B │ ▨▨▨▨▨▨▨▨▨●●●●▨▨▨▨▨▨▨▨▨▨▨ │  a checkerboard
│  This PC …    │ 📄 notes.txt   │  1.80 KB │ ▨▨▨▨▨▨▨▨●●●●●●▨▨▨▨▨▨▨▨▨▨ │  behind the alpha
│               │               │          │ ▨▨▨▨▨▨▨▨▨●●●●▨▨▨▨▨▨▨▨▨▨▨ │
└───────────────┴──────────────────────────┴───────────────────────────┘
```

**Inside the pane, not across the window**, and that is the whole design. A preview belongs to the
folder it is beside: two panes each showing a build of the same DLL get their own, and comparing
them is a matter of looking left and right rather than clicking back and forth. So each *folder*
has one — shut by default, and switching tabs puts back the one that tab had open.

Where it goes is the window's preference and lives on the eye's **context menu**: `Show preview`,
then `Right`, `Bottom` or `Auto`. One setting rather than one per folder, because "where the
preview goes" is a habit and "is it showing for this folder" is not — and it is on the button that
opens the thing, which is where people look for the settings of a thing and keeps three radio
buttons off a path bar that has no room for them. The menu is **sticky**: ticking a position leaves
it up, since three radio buttons you have to reopen the menu between are three menus.

`Auto` reads the pane's own shape, every frame: **a pane wider than it is tall gets a panel down the
right, and anything squarer or taller gets one along the bottom.** Split the window in two and each
pane's preview moves to its bottom by itself, which is the case the setting exists for — a preview
taking 40% of a 500-point pane leaves no listing at all.

Whichever side it is on, the panel stops above [the status line](#the-status-line): the line is the
pane's floor rather than the listing's, and the shape `Auto` reads is what is left above it.

The threshold is 1.25 rather than 1.0, and it leans that way deliberately: **width is the scarcer
thing in a listing.** Four columns and a path bar need it. Height costs a listing nothing but rows,
and rows scroll.

### It follows the keyboard

Open it once, then arrow down the folder and look at each file in turn. Which is only bearable
because it *waits* — a quarter of a second of the selection sitting still, restarted on every move,
exactly as [the filter](#the-path-bar) does and for a sharper reason: holding the down arrow through
thirty photographs would otherwise decode all thirty, twenty-nine of which nobody is going to look
at.

Moving the keyboard onto a folder, an archive or anything else with no preview **clears the panel**
rather than leaving the last answer up. That is the opposite of what you would want from a panel
across the window, and it is right for this one for exactly the reason it is inside the pane: it
sits beside the row it is about, so a stale picture next to a different selection would be a lie.
Closing it lets go of everything — a decoded picture is up to 16 MB of texture and a dependency
graph a megabyte of names, and re-reading is a quarter of a second.

### What is on the bar, and what goes first

Left to right after the name: a **comment**, the **size**, then the controls — the zoom field, Fit,
zoom out, zoom in, the view's own toggles, and the way out.

```
one.png <-> two.png      300 x 200  (!) 8.89% differs   100% v   [/]  -  +  |||  X
```

The slot before the close button belongs to whichever view is on the canvas, because the toggles in it
are about the view rather than about the panel: showing both sources of a comparison, numbering the
lines of a text file, or — for a Markdown file — going back to the markup. Numbering and markup do
appear together, since numbering the source of a document is a reasonable thing to want; what does not
appear is the gutter's toggle over the *rendered* document, which has no lines of the file's in it.

Two of them **nest**, and the rule is the same both times: a toggle that only means something while
another one is on is drawn inside it. `±` is what git changed, and inside it the folded rows for a text
file or the both-images switch for a picture — neither of which is a question you can ask about a file
that is not being compared with anything.

When the bar will not hold all of that, things go in this order: **the comment, then the size, then
the name starts to crop.** The controls never go. Which has one consequence worth knowing about
before you take it for a bug: a *long name* takes the details away even in a wide panel. That is the
right way round — the name is what identifies the file and `300 x 200` is a nicety — and it does mean
the details come and go as you arrow down a folder of mixed names.

The two slots are priorities and not captions, which matters once for the comparison below: there,
the share that differs is the answer somebody opened it for, so it takes the slot that survives and
the dimensions take the one that goes first.

The zoom is an **editable combo box** with a subtle border — no fill and no frame until the pointer
is over it, which is the right treatment on a bar this crowded. It offers Fit and 25% to 400%, and
you can type `137` into it, because a zoom reachable only through `+` and `-` is a zoom you cannot
ask for: 100% from 874% is eight clicks. It also **gives the keyboard back** when you are done with
it — on `Enter`, on `Escape`, and on a press anywhere else — because `App::keyboard` stands down
whenever anything has focus, so a field that holds on takes `Ctrl+P`, `F5`, the arrow keys and the
type-ahead with it.

Four things `azur::ComboBox` needed for that, and all four were design-system gaps rather than
anything specific to a file manager:

- **`Variant::Subtle`.** The variant existed and `trigger_paint` implemented it; the combo box did
  not expose it.
- **`filter(false)`.** An editable combo box is two different controls. Filtering is right for a
  *picker* — a long list of names, where typing narrows it. It is wrong for a *value with presets*,
  where the field already holds the current value: filtering by it narrows ten presets down to the
  one you are already on, and a value nobody offered (`137%`) narrows it to **nothing**, so the list
  stops opening at all.
- **Opening on the release rather than the press.** This is the one that made the list look missing.
  It opened on `gained_focus`, which fires on the button *press*; egui's own guard against a popup
  being shut by the very click that opened it is `was_open_last_frame`, which is false on that frame
  — so the release a frame later counted as a click outside the list and closed it again. The list
  appeared for exactly one frame, which from the outside is a combo box with no list.
- **A width of its own.** A popup's width is its `Area`'s, and `MenuItem` elides its label to
  whatever it is given — so a sixty-point trigger gave a sixty-point list and nine of the ten
  entries came out as `25…`. It is measured from the widest option now, and never narrower than the
  trigger it hangs off.
- **`icons(false)`.** Every entry reserved 24 points down its left edge for an icon column, which is
  right for a menu that may show a tick or a glyph and is an indent with no cause for a list of ten
  percentages. That column belongs to the *menu* rather than to the entry — the whole point of it is
  that labels line up, which a per-row decision cannot promise — so `Menu`, `ContextMenu`, `Select`
  and `ComboBox` each declare it once and every entry inside reads it. The design system's own
  guideline moved with it; see [its README](../azur-egui-theme/README.md). Turning it off on a
  `Select` drops the tick as well, and that is deliberate: a tick with no column would be drawn over
  a label, and for a list of values the trigger already says which one is current in a field you can
  read without opening anything.

That last one is also the sharpest lesson here. I reverted it once, on the evidence of a test that
said the labels were not cropped — and the test was **vacuous**: `Galley::text()` hands back the
string a galley was *asked* to draw, not the glyphs it drew, so a label elided to `25…` still answers
`25%` and `contains('…')` can never be true. `Galley::elided` is the flag that knows.
`Harness::cropped` asks it, and the moment it did, nine cropped labels appeared. Each of these four
fixes was checked by reverting it and watching the test fail; this is the one where that discipline
earned its keep, because the first time round it was the *test* that was broken and the revert looked
justified.

Measuring the width synchronously rather than probing is deliberate, and it is the one place this
diverges from `Menu`. `Menu` cannot know its own content — a caller fills it — so it reports a width
from inside and reads it back a frame later. A combo box knows every option up front, and a galley is
cached, so there is no reason to spend that frame: the frame `Menu` spends is a frame this list would
be showing `Fi…` for, and a dropdown is looked at for about three of them.

### Pictures

Every raster format `image` implements in pure Rust — PNG, JPEG, GIF, BMP, ICO, TIFF, WebP, TGA,
DDS, HDR, QOI, the Netpbm family — plus **SVG**, rasterised by `resvg`. No C library appears
anywhere in that list, which is the whole reason for those two crates rather than bindings.

**Alpha shows as alpha**, on a checkerboard. Which two greys the board uses was picked by
measurement rather than by eye: `background-canvas` against `background-control` is 11.5 ΔL* apart
in the dark theme and **3.6** in the light one, where the board all but disappeared and with it the
point of having one. `layer` against `control-active` is 15.4 and 13.9 — balanced, and about what
Photoshop's white-and-light-grey board measures, which is the value everyone already reads as
"nothing here". `the_checkerboard_reads_as_a_checkerboard` holds both halves of that: visible
enough to see, and *the same* in both themes.

It is drawn as **one tiled quad**, not a grid of rectangles. A 400-point canvas at 8-point squares
is two and a half thousand rects a frame; a 2×2 texture with `TextureWrapMode::Repeat` and a `uv`
of many tiles is one.

The image **fits the canvas by default, and is never enlarged past 1:1** — fitting is an upper
bound, and a 16-pixel icon blown up to fill a 400-point panel is not a preview of it, it is a
mosaic. Vector art has no natural pixel size, so it is rasterised at the cap and does fill the
panel: a 48×48 check-mark SVG comes up at 874%.

The wheel zooms **about the pointer**, dragging pans, a double click goes back to fit. Zooming about
the pointer rather than the middle is the one that matters — it is what makes getting to a corner of
a large image possible without a dozen alternating zooms and drags.

Two bounds, and they are different bounds for different reasons. **`CAP` is 2048 on the long edge**,
16 MB as RGBA, and anything larger is scaled down to it and says `scaled to fit memory` on the bar.
**`DECODE_MAX` is 40 megapixels**, and it has to exist separately because `image` has no streaming
resize: scaling something down to the cap means decoding all of it first, so a 200-megapixel scan
would be 800 MB in flight to produce a 16 MB thumbnail. Past it the panel says how large the thing
is, which is more use than a window that stops for two seconds.

An animated GIF shows its first frame. Text inside an SVG is not drawn — `usvg`'s text support means
a font database, shaping and system font enumeration, which is most of that dependency and none of
the value here — and the bar says `vector, no text` rather than leaving a silent hole.

### Two pictures, compared

Select **two** images and the panel becomes a diff viewer: the two of them and a third view of where
they differ, with a toggle down to the difference alone.

Two selected rows rather than a mode or a menu, because picking a second image is a deliberate act
with an obvious meaning and no other pair of files has one. Three selected is not a comparison of
anything, so the panel goes back to the cursor's file rather than choosing two of them.

**The zoom and the pan are shared.** Three views that scrolled independently would be three views of
nothing in particular. And all three are placed against the *larger* of the two sizes, anchored at
its top-left rather than centred, so the same pixel of two files of different shapes lands at the
same offset in each view — centring them would misalign the comparison by half the difference.

Two files of **different dimensions** are compared at the larger size, with a pixel that exists in
only one of them counted as differing. They are not the same picture, and saying so by lighting up
the region one of them does not reach is more useful than refusing to compare them or quietly
cropping to the overlap and reporting a small difference. The bar reports both sizes in that case.

The difference comes off the worker as a **mask** — white, with the alpha carrying how much — and is
tinted `status-danger` when it is drawn. That split is the rule this program holds everywhere: what
colour "different" is painted in is the theme's decision, so what a worker hands over is a
measurement. The mask is **amplified eightfold**, because the job of that view is to be *findable*: a
one-level difference at alpha 1 is invisible, and past a delta of 32 it is fully opaque. The
percentage on the bar is the honest count, before any of that.

The same three views are what `±` shows for **one** picture git has an older version of — see
[A picture, and the one in the last commit](#a-picture-and-the-one-in-the-last-commit).

### Text

Wrapped, scrolled, and **monospace only where the columns mean something.** The line between the two
lists is a question with an answer rather than a matter of taste: does moving a character sideways
change what the file means? In a log, a table, a diff or any source file it does — a column that no
longer lines up is information lost. Markdown, `.rst` and a `.txt` are paragraphs, and paragraphs
are what the proportional face is for.

Two lists of extensions decide it, and **one list of names that beats them both**. `CMakeLists.txt` is
what that list is for: its extension is `txt`, which is prose and right about nearly every other file
carrying it, and this one is a build script full of indented blocks and aligned arguments. So the name
is asked first — which makes it the mirror image of the prose names (`README`, `LICENSE`), where the
name loses to an extension and only speaks when there is none. `Makefile` and `Dockerfile` need no
entry, since a file with no extension is monospaced already.

The name in that list is the **stem**, without the extension, because that is what the worker passes.
Worth writing down: the first version of the list held `cmakelists.txt`, a string no caller can
produce, so the rule could never have fired — and its unit test passed, because the test handed the
function the string the program does not. The test that catches that goes through `code_of` and gives
it a *path*.

**Which files are text is not only a list of extensions.** There is a list — sixty-odd — but the
useful part is what happens to a file that is not on it: anything with no extension, or a dotfile,
is **sniffed** on the worker. Two tests, and both matter. A **NUL byte** is the oldest and still the
best binary tell: no text encoding puts one in the middle of a document, and every executable,
archive and database is full of them. **Valid UTF-8** over the same 4 KB window catches the rest,
with the last few bytes forgiven — a 4 KB window usually cuts a multi-byte character in half, and
that is not a reason to refuse a file. So `README`, `LICENSE`, `Makefile` and `.gitignore` preview,
and a file with no extension that is really a database does not. A file with nothing to go on is
monospaced, since most of what has no extension in a source folder is a build or configuration file
— with a short list of exceptions for the ones somebody actually sits and reads.

The button on the bar **numbers the lines**, and it is remembered, because it is a way of reading
rather than a fact about a document. The interesting part is that the panel *wraps*: a wrapped
paragraph is several visual rows of one logical line, so the gutter cannot count rows. It walks the
finished galley and numbers the rows that **begin** a line — which is a fact only the layout knows,
and the reason the galley is laid out by hand here and then handed to a `Label` rather than left to
the widget. The numbers are set in the same font as the body, which is what guarantees the two share
a baseline; a caption-sized number beside a body-sized line would sit a point above it, and a gutter
that does not line up is worse than no gutter. Only the rows on screen get a galley of their own — a
15,000-line file is 15,000 numbers, and laying out the ones nobody can see would undo the whole
reason the body is one galley.

A megabyte at most, and it says so. Not squeamishness: egui lays out the whole galley whether or not
it is on screen, so a 200 MB log would be a frozen window rather than a slow one.

`\r` and `\t` are replaced on the way in. Both would otherwise be laid out as glyphs — a hollow box
at the end of every line of every file written on this platform, and no indentation at all in the
other case.

### Source is coloured, and it is a lexer rather than a parser

[`syntax.rs`](src/syntax.rs) is one pass over the body producing a byte range per thing worth a
colour, and [`ui::preview`](src/ui/preview.rs) turns those into layout sections over **the same
galley the plain view uses**. That is the whole architecture, and it is what keeps the find bar, the
line-number gutter, wrapping and selection untouched: colour is a *format* over the body, not a
different way of drawing it. The two layers compose, and the search wins where they meet — a
highlight a keyword's colour showed through would be a highlight you could not see.

There is no grammar and there will not be one. A preview pane has to answer in a millisecond, over a
file it has never seen, in whatever language, and half of it possibly truncated mid-expression. What
it needs is the reading you get from squinting — *that is a comment, that is a string, that is a word
the language owns* — and a table per language gets you there. Seventeen of them: the C family as one
(`class` is a keyword in a `.c` file, and five tables that are 80% the same would be a sixth bug),
Rust, JavaScript and TypeScript together, Python, Go, SQL, shell, PowerShell, batch, CSS, JSON, YAML,
TOML, `.ini`, CMake, and two with tokenisers of their own — XML/HTML, whose structure is in the angle
brackets, and a unified **diff**, which is the only one coloured a whole line at a time, because what
matters in a diff is not what the code says but which side of the change it is on.

Three guesses it makes, all of them deliberate. **A type is a capital that comes back down** — `Rect`
is one, `MAX_SIZE` is not, which is right nearly always and costs nothing where knowing for certain
costs a symbol table. **A call is a word with a `(` welded to it**, which is what every editor with
no language server does. And **a key is a word before its `:` or `=`**, which is the whole of what a
lexer can see of CSS, JSON, YAML, TOML and `.ini` — and it applies to strings too, so a JSON key and
a JSON string value get different colours from the same characters.

The Rust lifetime is the trap worth naming. A `'` treated as a plain delimiter runs to the next one,
so `impl<'a, 'b>` colours `'a, '` as a character literal and inverts the rest of the file. A letter
literal here is *one* character or one escape and nothing else, which is why `'a` is not the start of
anything and `'\u{1F600}'` still is. The other direction matters as much: an unterminated string
stops at the end of its line, so one stray quote in a truncated file does not paint the rest of it
green.

**Punctuation gets no colour**, and that is arithmetic before it is taste: it is about a quarter of
the tokens in a source file, and every span becomes a layout section egui hashes each time it draws
the galley. For the same reason there is a cap — 128 KB, about three thousand lines. Past it the body
is drawn **plain rather than coloured to the cap and then stopping**, which is the important half: a
file that goes monochrome two thirds of the way down reads as a bug, where a large file that is
simply not coloured reads as a large file. `colour_speed` is the measurement — 128 KB in 6 ms over
this repository's own source and 12 ms over dense code, in a debug build, at 34 and 139 spans per
kilobyte respectively.

The seven colours are the application's, in [`theme.rs`](src/theme.rs) beside the file-kind hues,
because that is where the design system says a domain palette goes — and `every_syntax_colour_can_be_read`
is the measurement it asks for in return.

**The dark side is a transcription** of the `editor.tokenColorCustomizations` this program's author
reads code in every day, and there is no better argument for a scheme than that. What it has to be is
legible on this window's two surfaces, and it is: every value clears 4.5:1 twice over, tightest at
4.79:1 for the comment green on the panel.

**Two surfaces, not one**, and the second is what decides things — a fenced code block puts the whole
palette on a recessed fill rather than on the panel. That is what chose the fill: on the neutral
ramp's `GRAY_4` the comment green came to 4.20:1, and on `bg.canvas` it is 5.34:1. The canvas turns
out to be the right answer in both themes, and the same one in both directions: paper's canvas is
darker than its sheet, and the dark canvas is darker than its layer, so a code block reads as
recessed either way — the window's own colour showing through the panel sitting on it. `status.danger`
measured 4.28:1 on that fill, which is why a diff's removals have a colour of their own rather than
borrowing the status hue — those roles are *marks*, a dot and a gauge and a glyph, and a diff is a
screenful of text somebody is reading.

**The light side is a compromise rather than a translation, and worth knowing about.** The scheme
separates by *lightness*: five of its seven hues are in the warm yellow-green family and are told
apart on black by how bright each one is. A light theme has no lightness to spend — every ink has to
be dark to clear 4.5:1 against near-white paper — so those five compress into one narrow band. Green
stays green and blue stays blue; the gold cannot survive at all (`#ffd700` on paper is **1.4:1**, and
a gold dark enough to read is an amber, which is a different colour rather than a darker one). It is
measured, so it is legible; it is not the same scheme.

Four extensions in the text list are deliberately left plain: `.rb`, `.pl`, `.php`, `.lua`. Each
needs its own comment marker and keyword list — Lua's comment is `--`, Perl's variables are sigils
three ways, PHP is two languages in one file — and a language coloured with a nearly-right table is
worse than one coloured with none, because a wrong keyword is a claim about the code.

### Markdown is shown as a document

A `.md` is parsed by [`markdown.rs`](src/markdown.rs) and drawn as blocks: headings on the type ramp
with a rule under the first two levels, paragraphs that close up their line breaks, `>` quotes behind
a bar, bullet and numbered lists that nest, task boxes as the two characters they mean, fenced code
on a recessed fill **coloured by the same pass a `.rs` file goes through**, rules, and tables as rows
in the monospace face. The button beside it goes back to the markup, and which of the two you last
looked at is remembered — it is a way of reading, like the line numbers, and the gutter's own toggle
hides itself over a document because a rendered document has no lines of the file's to number.

**Nothing is one galley here**, and that is the difference from the plain view: a code block has a
fill behind it, a quote has a bar beside it, a list item hangs its text off a marker, and none of
that is expressible as a format over one run of text. What that would ordinarily cost is the find
bar, whose hits are byte offsets into one body — so the parser produces exactly that: **one flat
string of everything that will be on screen**, with each block owning a slice of it. A hit stays one
number, and the search searches what you can see. Looking for `bold` in `**bold**` finds it; looking
for `**` finds nothing, because there are no asterisks on the screen.

**Inline code needed a line height of its own**, and that is the one thing in the block renderer that
is a measurement rather than a decision. epaint places a glyph in a row at
`face_ascent + valign × (row height − line height)`, so two faces in one row share a baseline only
where both of those agree — and at 14 points the proportional face's ascent is 15.09 against the
monospace face's 10.41, its line height 18.59 against 16.41. Inline code came out **three pixels
above** the prose around it, with its tinted fill left behind at the right height, which is what made
it read as an underline rather than as a raised word.

No value of `valign` fixes that: `TOP` aligns the two ascents, `BOTTOM` the two line boxes' bottoms,
`Center` splits the difference, and all three differ. The only per-section dial that moves a baseline
by an arbitrary amount is `line_height`, which `BOTTOM` subtracts — so shortening the code section's
line box by the gap lowers it by exactly that much. The gap is **read back rather than derived**,
because deriving it needs the faces' ascents and epaint's public `Fonts` exposes only `row_height`:
both faces go into one two-character probe job, and the difference between where epaint actually put
them is the correction. Both figures come back already rounded to the pixel grid, which is what makes
this land *on* the baseline rather than near it. `a_markdown_file_is_rendered_and_the_toggle_shows_its_source`
reads the painted galley and insists every glyph of a paragraph shares one `pos.y`; without the
correction it reports `[16.0, 13.0, 16.0]`.

The subset is bounded and the boundaries are in the source. Emphasis inside a link's text is not
parsed — a link is one run — and footnotes, reference definitions and HTML are passed through as the
text they are, on the grounds that a `<br>` shown as `<br>` is a smaller lie than a `<br>` shown as
nothing. Tables are rows rather than a grid: a real grid means measuring every column before drawing
any of it, which is a different shape of code from everything else here, and it is worth doing and
not done.

Two rules earn their place by being the ones that break a README. `---` under a paragraph is a
heading and `---` anywhere else is a rule, which is the format's own ambiguity and settled by asking
the setext case first. And **an underscore inside a word is part of the word** — `snake_case_name`
has two underscores an even distance apart, so the rule that works for asterisks would set `case` in
italics and eat both marks. CommonMark forbids an `_` that follows an alphanumeric from opening
emphasis for exactly this reason, and a README full of identifiers is the document where it shows.

### Selecting text in it

Drag to select, **double click for a word**, triple click for a line, `Ctrl+C` to copy — all of it
egui's, on a selectable `Label`, and none of it code here. What is here is a test, because a gesture
nothing in the source mentions is a gesture nothing in the source would miss: it double clicks over
`alpha beta gamma`, reads the band out of the galley's own row mesh — which is where epaint puts a
selection — and asserts it covers **the word and not the line**, and that it is still there three
frames later.

That last part is the one worth testing. Three things can quietly take the gesture away: something
drawn over the Label winning the pointer, a `Sense` that stops sensing clicks, and the Label's
auto-generated id changing between frames — which makes egui drop the selection on the frame *after* it
was made, so the word flashes and vanishes.

### Finding something in it

```
┌──────────────────────────────────────────────────────────┐
│ 📄 pe.rs                          54.5 KB  🔍  ▤  ✕      │  the magnifier, latched
│ //! What a bina┌──────────────────────────────────────┐  │
│ Dependency Walk│ 🔍 machine      Aa ab .* │ 1 of 47 ∧ ∨ ✕│  the bar, floating
│ //!            └──────────────────────────────────────┘  │
│ //! A Windows executable names the DLLs it needs in its  │
```

The magnifier on the bar reveals a **find bar floating over the top right of the text**, and it is an
editor's find bar because that is the one everybody already knows: the field, then the three toggles
*inside* it — `Aa` match case, `ab` whole word, `.*` regular expression — then the count, the two
arrows, and the close button. Every hit is marked and the current one is marked more strongly.

It stops **short of the scroll bar** rather than covering it, which is the one place it differs from
where an editor puts one: the text under the bar still scrolls, and a close button sitting on the
thumb is a thumb you have to scroll the panel to reach. The inset is the design system's scroll-bar
width read off the installed style plus the margin egui keeps for it, not a number typed here — 22
points as the two are set now.

**`Enter` and `F3` are both "again"**, with `Shift` reversing either, so a search never needs the
pointer twice; `Escape` shuts it. `F3` is what every Windows program has meant by *find next* for
thirty years, and it is also — in this window — the key that puts the caret in the pane's **filter**
box. The path bar is drawn before this panel, so it would take the keystroke first and the find bar
would never see it. What settles that is a rule worth having anyway: **a bare key stands aside while
another field has the keyboard, and a chord does not.** A chord is unambiguous wherever it is pressed;
a bare key belongs to whatever is being typed into. So `Ctrl+F` reaches the filter box from anywhere
and `F3` only reaches it when nothing else is being typed — with the filter box itself excepted from
its own guard, since `F3` with the caret already there should be where it already is rather than
inert.

**Not `Ctrl+F` for the panel**, though. That belongs to the filter box and has since long before this
panel existed; a preview that took it would be taking the keyboard away from the window it lives in,
for a bar that only exists while one kind of file is selected.

**The text is scrolled when the search changes and when you step, and at no other time.** That reads
like a triviality and is not: the bar works out its arrows once a frame from whatever its buttons and
keys came to, which is *nothing* on almost every frame, and asking to reveal the current hit anyway
put a scroll in every frame the panel drew. The text sprang back under the wheel and could not be read
around. A step of nothing does nothing.

**The query outlives the file and the hits do not.** Arrow down a folder with the bar open and you are
asking "where else does this appear", which is the useful shape — so the query and the three toggles
survive, while the hits are cleared, because they are byte offsets into one body. That is not tidiness:
the offsets become sections of the layout job that draws the text, and an offset past the end of it is
a panic rather than a stray highlight.

**Whole word means what `\b` means**, which is that word-ness *changes* — not the tempting "the
characters either side are not letters". The two differ whenever the needle itself begins or ends with
punctuation: `\b-x\b` matches the `-x` in `a-x`, and the tempting rule refuses it because `a` is a
letter. The literal path has to mean the same thing by the toggle as the regex path, or one button
would do two jobs.

**The plain search does not go through the regex engine.** It is the search that runs on every
keystroke of every find, and without `regex`'s `perf` feature group — left out deliberately, see
`Cargo.toml` — the engine walks a megabyte an order of magnitude slower than a folded character
compare does. It is also the path that has to match exactly the characters typed. What the engine is
there for is the third toggle, and it is the only engine safe to hand a pattern somebody is still
typing: `regex` has no backtracking by construction, so `(a+)+b` pasted into a field on the UI thread
cannot hang the window. `size_limit` covers the other end, where a pattern is legal but compiles to
something enormous.

**It stops counting at 4,096 hits** and says `4096+` rather than pretending — a budget like the
flatten's and the walk's, and it is reached often, because the *first keystroke* of any search over a
large file reaches it. Every hit is two sections of the layout job for the body, and one letter typed
over a megabyte of source is on the order of 80,000 of them.

Two things were measured rather than chosen. The **highlight** is `accent.subtle` with `text.primary`
on it and `accent.default` with `text.on_accent` for the current hit — both over 4.5:1, in both
themes, because a highlight you cannot read the text through hides the thing it is pointing at. And
the **bar's own edge**: `bg.layer_alt` is the surface every popover here floats on, but this one floats
over the panel that surface is a step *from*, and the step is **2.2 ΔL\* in the dark theme** against
8.5 in the light one — a bar with an edge in one theme and no edge in the other, which is the
checkerboard's mistake again. So the fill's job is only to be opaque and `stroke.default` draws the
outline. `SAME` is the ruler and not WCAG's `SHAPE`, because what a one-point line is doing here is
dividing two surfaces; held to `SHAPE`, nothing in the dark ramp would pass, since a ratio between two
near-blacks is dominated by the 0.05 in its own denominator.

**And it is not an `Area`.** Floating something over a canvas is what egui's `Area` is for, and with
the bar on `Order::Foreground` — above the pane by every rule egui has — the body text still came out
over the top of it at about a tenth of its own opacity. Faint enough to miss in the dark theme, plain
in the light one, and confirmed off the framebuffer with the bar's fill temporarily set to red: the
glyphs are there, lighter than the fill. Whatever egui does with the shapes of a selectable label
inside a `ScrollArea`, it survives being put under a higher layer. Painting into the panel's own
painter after the text cannot lose that race, because within one layer the order is the order things
were added — and the input the `Area` was buying is bought instead by one widget over the whole bar,
added after the label, since egui gives the pointer to the last widget added over a point.

`--find=<text>` opens it for a screenshot, the same way `--preview` and `--compare` exist: a capture
run has no keyboard.


## What a binary needs

For a `.exe`, a `.dll`, a driver or a control panel applet the preview is **everything that binary
needs in order to load, and where each one came from** — the Dependency Walker question, answered
without leaving the folder.

```
🖵 azur-file-explorer.exe                                    ⊗ 5 missing  ✕
  ⌄ azur-file-explorer.exe        D:\Sources\MyTools\…\target\debug      x64
    › ole32.dll                   C:\windows\system32                    x64
    › shell32.dll                 C:\windows\system32                    x64
      api-ms-win-core-synch-l1-2-0.dll   resolved by the API set schema
    › VCRUNTIME140.dll            C:\Program Files\AdoptOpenJDK\…\bin    x64
```

That last row is the whole point of the thing. Nothing about this program has anything to do with a
JDK; a `VCRUNTIME140.dll` from one is simply what is furthest forward in this machine's `PATH`, and
there is no way to find that out by looking at the folder the program is in.

### Three things a name can turn out to be

A module is a **file**, an **API set**, or **missing**, and telling the second from the third is
what separates a useful list from a wall of red.

`api-ms-win-core-file-l1-2-0.dll` and its several hundred siblings are not DLLs. They are names in
a schema the loader carries, mapped at load time onto whichever real DLL implements that contract
on this build of Windows — usually `kernelbase` or `ntdll`. Some builds ship stub files of the same
name and some do not, which is exactly why looking on disk is the wrong question to ask about them.
The original Dependency Walker asked it anyway and reported every one as missing, which is how it
became a tool people learned to ignore on anything built after Windows 7. Here they are marked for
what they are, not followed — their own imports are `ntdll`, which every module in the graph already
has — and counted separately: **246 files, 689 API sets** is the honest shape of a modern graph, and
"935 modules" would not be.

### The five that are missing, and why nothing is on fire

Walking this program's own binary finds five DLLs that are not on this machine —
`HvsiFileTrust`, `AzureAttestManager` and three more — every one of them **delay-loaded** from
somewhere deep inside `shell32`, and every one harmless: Windows ships the stubs for features that
are not installed, and a delay-loaded import is not opened until something calls into it.

So the tree opens with the root expanded and then **exactly the branches that lead to something
which would stop the program from starting** — a missing module reachable by imports that are all
loaded up front. Everything else stays folded. For this binary that is nothing, and the initial
view is nineteen rows rather than the several hundred that expanding a path through `shell32` would
have produced. A delay-loaded reference is dimmed and says so in its Location column, which is the
difference between "this will not run" and "one feature will not work, later, somewhere else".

The other thing worth knowing at a glance is on the right-hand end of every row: the processor it
was built for, in the danger colour when it is not the root's. Windows will not load a mismatch,
which makes it the second most useful thing a dependency list can tell you after "it is not there".

### Three inks, and how that number was arrived at

The tree uses `text-primary`, `text-secondary` and `status-danger`, and nothing else. Not for
tidiness — the first version reached for `text-tertiary` for an API set, `text-disabled` for a
location that is not a path, and `status-warning` for a processor mismatch, all of which are
perfectly reasonable-looking choices. Measured against the surfaces they are actually drawn on:

| ink | on a row | on a hovered row |
| --- | --- | --- |
| `text-primary` | 16.6 / 18.1 | 7.3 / 9.7 |
| `text-secondary` | 6.9 / 7.4 | 3.0 / 4.0 |
| `status-danger` | 4.9 / 4.4 | 2.1 / 2.4 |
| ~~`text-tertiary`~~ | 3.8 / 5.8 | **1.7** / 3.1 |
| ~~`text-disabled`~~ | 2.3 / 2.5 | **1.0** / 1.3 |
| ~~`status-warning`~~ | 8.2 / **2.8** | 3.6 / **1.5** |

`text-disabled` at 1.00:1 *is* the hover fill in the dark theme: an API set's location line was
invisible for exactly as long as the pointer was over the row it was on. And `status-warning` is a
legitimate role that simply is not ink at 12 points on a light surface — it was the processor tag on
a mismatched module, which is the one row in a dependency list you most need to be able to read.

The same measurement is why the panel's missing count is `text-primary` with a **danger-coloured
mark beside it** rather than being red itself: on that surface `status-danger` is 3.01:1 in the
light theme, which is the right floor for a shape and well under the one for 12-point text. So the
mark carries the colour, where 3.01 is the number that applies, and the count stays at 12.5:1.
`every_ink_in_the_tree_can_be_read` holds all of that, and also asserts that the three that were
taken out really do fail somewhere — across both themes, since `status-warning` is fine on the dark
side and a rule derived from the dark theme alone is how the two light-theme defects this project
has already fixed got in.

### Where it looked

In this order: **the binary's own folder, then `System32`, then `System`, then the Windows directory,
and only then `PATH`** — the loader's own order, for the parts of it that can be known from outside a
running process.

`PATH` being last is the part that matters, and getting it wrong is not a subtle failure. On an
ordinary machine `System32` is *also* in `PATH`, so a search that only knew about `PATH` still
**found** `kernel32.dll` and every test passed — it simply reported it as coming from wherever
`System32` happened to sit in that user's `PATH`, behind anything installed in front of it. On this
machine that meant `VCRUNTIME140.dll` was reported out of a JDK's `bin` folder: a true statement about
`PATH` and a false one about what the loader would do. It resolves to `System32` now.

The three system directories are asked of the platform — `GetSystemDirectoryW`,
`GetWindowsDirectoryW` — rather than assembled from `%SystemRoot%`, because the environment is
writable and this is not; and because that call gets the *redirected* answer right, which is the
directory a 32-bit process's imports really would come from.

Hovering the panel's title gives the whole answer — what the walk found, how long it took, and the
search order — because none of the three fits on a bar a few hundred points wide, and because they
belong together: a location means nothing without the list of places that were tried, and that list
means nothing without what it does *not* model.

- **`KnownDLLs` wins over everything.** `kernel32`, `ole32`, `user32` and about thirty others are
  resolved from a section the session manager opened at boot, so a copy of one sitting next to your
  program is ignored — where this would report the copy.
- **Side-by-side assemblies** are resolved from a manifest, which is why two programs on one
  machine can load different `MSVCR90.dll`s.
- `SetDllDirectory`, `LOAD_WITH_ALTERED_SEARCH_PATH`, manifest `<file>` redirection and a `.local`
  folder all move the goalposts at run time, and nothing on disk can predict them.
- **The current directory** sits between the Windows directory and `PATH` in the real order and is
  left out: a file manager has no meaningful current directory to offer, and inventing one would put
  an answer in the list that the program being inspected would never see.

The application directory is the **root's** folder at every level of the walk, not each DLL's own,
because that is what the loader does: the application directory belongs to the process, so a DLL in
`C:\lib` loaded by a program in `C:\app` looks for *its* imports in `C:\app`.

`PATH` entries that are not directories are dropped once, up front, rather than being tried for
every module. That is not tidiness: an entry pointing at a share that has gone away costs a
connection timeout every time something is looked for in it, so a walk of two hundred modules
against a dead entry would be two hundred timeouts instead of the one this spends.

### 937 modules in 66 milliseconds

Only the headers and the import descriptors are read, never a whole file. A walk touches a couple
of hundred binaries and `Qt6Core.dll` alone is 6 MB; reading them whole would be most of a gigabyte
off the disk to find a few hundred strings. Each module costs four small reads for its headers plus
two per import, all of which land in the page cache for the system DLLs that every walk visits.

And a level of the graph is resolved on up to eight threads at once, for the reason
[the flatten walk](#flattening-a-folder) is: the work is a syscall waiting on the file system, so a
thread that is waiting is a module another thread could have been reading. Each worker takes the
next name nobody has claimed rather than a fixed share, because one name off a network share would
otherwise leave every other thread idle behind it — and a level no larger than the thread count
goes inline, since spawning a thread costs about what reading a warm binary's headers does.

The whole thing is capped at 4,096 modules and five seconds, and says **`stopped early`** on the
panel's bar if it reaches either. A dependency list that is missing entries and does not admit it is
worse than no list at all.

A graph is a graph and not a tree — `kernel32` is under nearly everything, and real graphs have
cycles: `kernel32` and `kernelbase` refer to each other through forwarders. So each module is held
once, keyed by name, and the *display* is a walk that stops where a module repeats along the branch
it is already on. That row says `(already above)` rather than looking like a leaf, because both
things really do import it.

### The row that stopped answering clicks

The rows share **one hit-test widget** — a click's row is arithmetic from the pointer's `y`, for the
reason the listing gives: a widget per row would be an id, a hit test and an animation slot each for a
highlight that subtraction gives away. What that widget is *called* turned out to matter. Its id was
`ui.id().with("deps-hit")`, and `ui.id()` inside a `ScrollArea` is derived from the **auto-id
sequence** — so it moved whenever anything earlier in the frame changed how many widgets the panel had
created. Adding the find button to the bar above was enough: the rows drew correctly, hovered
correctly, and stopped unfolding when clicked.

It is `Id::new(("deps-hit", x, y))` now, keyed to the panel's own position exactly as the `ScrollArea`
around it already was. Worth knowing because of how it presented: not as a broken panel but as one
test failing, only when it ran beside another, and flipping on an unrelated `println!` — which reads
like flakiness and was a real defect with an address.

### Everything in a row is on one baseline

The tree is the case the design system's [`ink_baseline`](../azur-egui-theme/src/components/mod.rs)
is written for: three texts at two sizes beside two painted glyphs, on a 24-point row. Two things
have to be true at once and neither is what a rect-centred galley gives you. The texts have to be
level with the **glyphs**, which are centred on their own ink — a line box reserves room under the
baseline for descenders a DLL's name does not have, so text centred in the row hangs a point and a
half below a glyph centred in it. And the 12-point location has to be level with the 14-point
**name**, which centring both boxes in the same rect does not do either: the two fonts differ in
line height *and* in ascent, so their baselines end up 1.5 points apart and the eye sees the column
step down as it crosses the row.

So the row commits to one baseline, taken from its *principal* font, and every text in it is drawn
on that baseline. `everything_in_a_dependency_row_sits_on_one_line` asserts it **exactly** rather
than within a tolerance, because the property is exact — and reverting the location column to the
way this is usually written puts it on 473.5 against the name's 475, which is the number that
argument was about.


## Selecting

The whole row is the target — the name, the size, the type, the date and the space
past them all behave the same, and so does a double click anywhere along it.

- **Click** selects; **Ctrl+click** adds or removes; **Shift+click** takes the range.
- **Drag from the empty space below the files** to rubber-band. It auto-scrolls when
  you pull past an edge, and it *lets go* again when you pull the band back — a band is
  not a paintbrush. `Ctrl` or `Shift` while banding adds to what was already selected.
- **Drag from a file** — its icon, its name, or any of its three values — to pick it up.
  **Drag from the space around them** and you get a band instead. A 24-point row across a
  wide pane has ink on maybe a third of it, and treating all of it as a drag handle is what
  makes a band a gesture you can only start by first finding the bottom of the listing.
  Which one it is comes from where the button went *down*, not from where the pointer has
  reached by the time a drag is a drag — that is a row or two along, and using it would
  pick up the file the drag arrived at.
- **Click below the files** to cancel a selection.

A row the keyboard is on but which is *not* selected — what Ctrl+click leaves behind — wears a
**dashed grey rectangle**, square-cornered, one pixel inside the row. It used to be a solid
accent outline, and that was the accent saying the opposite of what it means everywhere else in
this window: the accent is what "selected" looks like, both as the row's fill and as the bar down
its left edge. Grey and dashed is what every list on the platform marked this state with before
any of them had a theme, and it cannot be read as a selection at a glance. Drawn as one closed
polyline rather than four dashed edges, so the dashes stay in step round the corners.

## Rows or tiles

The first switch on [the status line](#the-status-line) turns the pane into Explorer's **Large icons**:
a grid of 96-point tiles with a thumbnail on anything that has one.

```
┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐
│  ▓▓▓▓▓▓  │ │  ▒▒▒▒▒▒  │ │   ⌸      │ │   ⌸      │
│  ▓▓▓▓▓▓  │ │  ▒▒▒▒▒▒  │ │          │ │          │
│ img20.j… │ │ img21.j… │ │ Cargo.t… │ │ README.… │
└──────────┘ └──────────┘ └──────────┘ └──────────┘
```

**Both views are the same listing.** The order, the sort, the filter, the selection, the cursor and the
rubber band are all the tab's, and a tile is identified by its *position in the display order* exactly
as a row is — so every gesture in the program reaches the same code from a different rectangle: a
paste, a delete, a drag out, a shell menu, `Ctrl+A`, type-ahead, F2. Switching costs a frame and never
a re-read, which is the same claim [the two flatten modes](#the-tree-is-the-same-listing-re-ordered)
make.

What genuinely differs is geometry, and it is three things rather than one. Hit-testing is
`(column, row)` arithmetic instead of a division by the row height. The band covers a *rectangle* of
cells rather than a range of rows, which is why `Tab::apply_band_at` exists beside `apply_band` — and
why the gap between two tiles belongs to the folder, so a band can still be started between them.
And the arrow keys go to whichever axis the view has: `Down` is a whole line of tiles, `Left` and
`Right` are one.

**It is per tab, it is not remembered, and going anywhere puts it back.** Which is the one place this
and the flatten mode part company, and it is deliberate: which way you want a *tree* shown is a habit —
so that one is a window preference and lives in the settings file — where whether a folder is worth
looking at as pictures is a fact about *that folder*. A folder of photographs and the folder of source
you open out of it want opposite answers, so an answer about one is not an answer about the other.

So `Tab::go_to` puts it back to the details view beside the flatten and the filter, which are the other
two questions asked of a folder rather than of the window, and there is no `view=` key in the settings
at all: **every folder opens in the details view, every time.** A *refresh* keeps it, because a refresh
is the same folder read again and every file operation ends in one — a paste that dropped you back into
rows would be the view undoing itself under your hands — and so does flattening, which is another
question about the folder you are already looking at. Two panes can therefore be in different views at
once, which is most of the point.

**There is no column header over tiles**, because four draggable dividers over a grid would be four
dividers about nothing. That costs one thing and it is worth saying plainly: the header is where sorting
is done, so in this view the sort is whatever the details view was last set to. It is per tab and it
survives the switch, so setting it once is enough.

### A tree keeps its folders as rows

Flattened as a **list**, the pane is one grid over the whole order and the arithmetic is a division:
nothing is cached, nothing walked, and 300,000 files cost what forty do.

Flattened as a **tree**, the folders stay rows — indented, with a twisty, exactly as in the details view
— and each folder's files become a grid of its own:

```
  ⌸ ⌸ ⌸ ⌸            ← the files of the folder being listed
∨ 📁 a nested folder
    ⌸ ⌸ ⌸ ⌸          ← its files, at its indent
  ∨ 📁 deeper
      ⌸
∨ 📁 another
```

**A folder's files come immediately after its own row and before the folders under it.** Which is the
only arrangement that works: the alternative puts a folder's contents below its entire subtree, so
looking at what is in the folder you just opened means scrolling past everything inside everything
inside it. The files of the folder being *listed* are therefore the first thing on the pane — the same
rule with nothing above it.

That shape has no closed form for "how tall is this" or "which block is at *y*", because a grid's height
depends on how many columns fit at its indent. So both are precomputed into `grid::Layout` — a run of
blocks with their tops, walked once per order and per width, and looked up by binary search — and
invalidated on `Tab::order_gen` rather than on the order's *length*, since a click on a column header
leaves the length alone and moves every row.

### Explorer's thumbnails, not a decoder

`IShellItemImageFactory::GetImage`, which is the call Explorer's own views make, so a tile shows what
Explorer would have shown. Three reasons it is the shell rather than the `image` crate that is already
in this binary:

1. **It has already been done.** Windows keeps a per-user thumbnail cache that Explorer fills; a folder
   you have looked at there comes back instantly, and one you have not is extracted once and cached for
   both programs.
2. **It answers for everything.** Camera RAW, HEIC from a phone, PSD, PDF, `.mp4` — each has a
   thumbnail provider registered by whatever produced it, and not one of them is a codec this program
   would be right to embed.
3. **A file with no thumbnail still needs a picture**, and the same call gives it: the file's *large*
   icon at the size asked for. A tile drawn from the 16-point image list behind the details view would
   be a blur. One call covers both halves of a folder, which is what makes the grid one code path.

What it costs is a shell call **per file** rather than the per-*type* answer
[the row icons](#why-it-is-fast) are built around. It is affordable here
for one reason: a tile is 96 points, so a screenful is a hundred-odd of them rather than forty rows ×
nothing, and only cells actually on screen are ever asked about. Everything else is the shape the icon
service already has — one worker thread with an apartment that lasts, a short queue, questions from a
folder you have left dropped rather than answered, and a painted glyph on the tile until the answer
arrives.

**An atlas of pages, and it is also the cache.** Tiles are drawn from an atlas rather than a texture
each, because egui begins a new draw call whenever the texture changes between primitives and
`egui_glow`'s painter leaks per draw call — a texture per thumbnail would break a frame's primitive
stream at every tile.

It is a *stack* of 144-cell pages rather than one big texture, and that is the second thing this got
wrong. A single fixed atlas makes the cache's size the atlas's size, so a window showing more tiles than
there are cells has tiles that can never hold a picture — and every one of those cells is being drawn,
so there is nothing to evict either. It showed up as a band of plain glyphs that filled in only when you
scrolled. So a page is added when a cell on it is first claimed: one for a laptop window, two for
2560 × 1392, four for a maximised 4K panel, six at the cap. A session that stays in the details view
allocates none of it, and the pages cost one draw call each only because the tiles are painted in a
single batch that is **sorted by texture** first.

**A cell that is on screen is never taken from the tile drawing it**, which is the rule underneath both.
Plain least-recently-used is not enough and the failure is not subtle once you have seen it: with more
tiles than cells, every visible tile is drawn every frame, so every entry is equally recent, so LRU picks
one of *them* — that tile loses its picture, falls back to its glyph, asks again, and its answer takes a
cell off another visible tile. Round it goes, every frame, for ever: a wall of thumbnails flickering as
though each were being replaced by its neighbour, which is exactly what is happening.

### The cell that belonged to nothing

**The bug that stopped the view working at all**, and it took four rounds of guessing to find because
its symptom is nothing like its cause. Its trace line was this:

```
thumbs   71 known /   71 drawn /  0 gaveup /  0 queued /  140 asks / 6 pages /    0 spare
```

864 cells handed out — six pages — and **71 entries left**. So 793 cells belonged to no entry and were
not on the free list either: unreachable and unusable. The atlas exhausted while holding seventy-one
pictures, so nothing new could ever be placed, so a screenful of tiles kept their painted glyphs for
ever. Scrolling made a couple of the drawn cells stale, which freed a couple of cells, which is why
scrolling loaded *some but not all* of what was missing. Every symptom of the report falls out of that
one number.

The leak was one line: an answer for a file that already had a cell replaced its entry, and the cell the
old entry was holding was dropped on the floor. So `remember` now puts a displaced cell back on the free
list — and separately, the *reason* duplicates arrived at all is closed: the worker used to release its
claim on a path when it finished with the file, which left a window one frame wide before the answer was
written down. A tile asking inside that window found no entry and no claim and started a second job for
the same file. With 140 tiles asking every frame, **every answer was exposed to it**. The claim is now
released when the answer is recorded.

`no_cell_is_ever_lost` asserts the invariant rather than the symptom — every cell is held by an entry, on
the free list, or never handed out — across a picture, a duplicate, a refusal landing over a picture, and
an eviction. It fails on the old line, one cell short of 864.

`--trace` grew the numbers that found it: `known` against `drawn`, plus `gaveup`, `queued`, `asks` and
`spare`. Between them they say which end is stuck, and each points at a different fix. `known` sitting
below `pages × 144` is this one.

### The frame it is polled in is part of the rule

The first version asked for a cell to have been quiet for **two** frames, because answers were taken
delivery of at the top of a frame like everything else this program polls — and from there a cell drawn
one frame ago is indistinguishable from one about to be drawn again.

Those two frames were a bug, and the shape of it is worth keeping because it is the shape of every
paint-on-demand bug. **Switch a scrolled grid from a tree to a list**: the tiles are all new, every cell
is held by a file you scrolled past, and none has been quiet for two frames yet — so nothing is asked
for, nothing arrives, *nothing asks for a repaint*, and the window sits on a grid of painted glyphs until
you scroll and force some frames by hand.

So delivery moved to the **end** of the frame, where "no tile drew this cell" is a fact rather than a
guess, and one quiet frame is enough. Belt and braces, a tile that is refused for want of room books a
repaint before giving up, so a view that is genuinely at the cap still fills itself in as cells free up
rather than waiting for the pointer to move.
`shell::thumbs`' `a_visible_cell_is_never_taken_from_the_tile_drawing_it` fills the atlas by hand and
holds both halves; it fails outright on the two-frame rule.

Two smaller things that are easy to get wrong and were:

- **The alpha comes back premultiplied.** `ColorImage::from_rgba_unmultiplied` multiplies it *in*, so
  premultiplied pixels through that constructor go twice: a 50%-opaque edge comes out at a quarter of
  its colour, which is a dark fringe round every icon and a grey halo round every soft-edged thumbnail.
  At sixteen points that is a pixel nobody sees, which is why the small icon path has always been able
  to ignore it; at ninety-six it is the edge of the picture.
- **A failure is not an answer about the file.** `GetImage` fails under contention — for a thumbnail
  Explorer is extracting at that moment, for a cache it is writing, for a file another window of this
  program is asking about — and it does not confine itself to the documented `E_PENDING` when it does.
  It is reproducible, which is how it was found: two threads of one process asking about one file get
  exactly one answer between them, and the tests that reach the shell hold a lock over each other to
  say so.

  Cached as "this file has no picture", a failure is a tile that keeps its painted glyph for as long as
  the folder is open — and it **accumulates**. Every scroll-and-switch asks a few hundred files at once,
  a slice of them fail while the shell is busy with the rest, and each of those is written off for good.
  Round again and another slice goes, until the files asked first have pictures and everything asked
  since is a wall of glyphs. So there is no such answer: a failure is a *not yet*, retried four times
  with the wait growing — a tenth of a second, then half, then a second and a half, then five. The
  growing is the part that matters. Three attempts inside three frames is fifty milliseconds, all of it
  inside the same busy window that caused the first failure, which is one attempt with extra steps. A
  file the shell genuinely cannot draw costs five calls in total and then nothing, and a tile sitting
  out a wait books the frame its next attempt comes due in, because nothing else would ask for one.

A **frame** is drawn round the picture, and only where the file really is one. A photograph with a
white sky or a dark one bleeds into the pane without it, which is why Explorer frames its thumbnails; an
icon is a shape on transparency and a box round one would be a box round nothing. The file's extension
is what tells them apart, since the shell does not say which of the two it handed back. A picture
smaller than the tile is drawn at its own size rather than blown up — a 32-pixel icon stretched to 96 is
a blur, and a blur is a worse answer than a small icon.

## The status line

**It is the floor of the pane**, across its whole width and under everything in it — the listing, the
console, and the preview panel whichever side that is docked to. Which is why the pane takes it off
first and hands the rest out: it is the one piece of furniture that is about the *pane* rather than
about a panel inside it, and a pane whose bottom edge is a status line under one half and a preview
panel under the other has two floors.

It used to be the listing's to place, and that looked right for exactly as long as the preview panel
went along the bottom — where it lands above the line either way. Docked to the **right** it did not:
the line stopped where the panel began and the panel ran on down to the pane's own edge.

Two groups, one bar. On the left the pane's two **controls** and a fact about the repository; on the
right arithmetic about the folder.

```
[▦] [>_]  ⑂ master ↑2 ↓1  13 changed        0.3 ms  ·  1016 KB  ·  4 / 19 (3)
```

| | |
| --- | --- |
| `▦` | rows or tiles — see [Rows or tiles](#rows-or-tiles) |
| `>_` | show or hide this pane's console — the same thing `Ctrl+²` does |
| ⑂ `master` `↑2` `↓1` `13 changed` | see [Git, by asking git](#git-by-asking-git) |
| `13 changed` | and it is a **button**: press it for a listing of exactly those thirteen |
| `0.3 ms` | how long this folder took to read |
| `1016 KB` | the folder's size, or the selection's the moment there is one |
| `4 / 19 (3)` | selected, on show, and not on show |

**Both switches are at this end because the left group is the one that never gives way** — a switch
nobody can see is a switch nobody can find — and they are in front of the figures because that is where
a control belongs on a bar read left to right. The console's is here for a second reason of its own: it
opens the band directly above this bar, and a panel whose only door is a keystroke is a panel most
people never find. The **view switch goes first** of the two, because the order is the order of what
they are about: one changes the listing filling the pane, the other opens a band at the bottom of it.

Each is subtle until the pointer is on it and latched while its thing is showing, which is what every
other toggle in this window does. Four points apart rather than eight: they are one group, and the gap
inside a group has to read as smaller than the gap after it.

**`13 changed` is the one fact on this line that is also a question**, so it is the second control on
it. Pressed, it flattens the folder and puts `@git` in the filter box — the two together are the
listing the number is a count of, and they were three gestures away: flatten, find the filter, know
the word. It **sets** rather than toggles, because a button labelled with a fact about the repository
should not undo itself when you press it twice.

It is *subtle* in the sense the rest of this window's chrome uses the word: no border and no fill at
rest, so the line still reads as a line of figures, and the same quiet hover the switch at the other
end wears. The text does not change — a count that restyled itself for being pressable is a count you
read twice. One thing about it is honest rather than tidy: a flatten **re-asks git**, because the marks
are keyed by path relative to the folder and a flattened listing's rows are longer paths, so the count
is briefly absent while the answer comes back.

**The counts are three colours rather than three words.** `4 / 19 (3)` is the shortest thing that
says all of it, and the colour is what stops it reading as one number: the selection is the accent's,
the total is `text-secondary` because it is what the other two are measured against, and what is
being held back is quieter still. Hovering spells it out, and that tooltip is where the folders-and-files
breakdown this line used to print now lives. What is *not shown* is a hidden file or one the filter
excluded — and the parenthesis is left out entirely when there is nothing behind it, because `(0)` is
a sentence about nothing.

**The left group has priority.** When a pane is too narrow for both, the right gives way a group at a
time — the scan's figure first, then the size, and the counts last, since the counts are the part of
this line a listing cannot be read without. There is always either air between the two groups or one
group missing: at eight points apart `13 changed` and `0.3 ms` read as one long phrase in three
colours, so `GROUP_GAP` is the width of a separator and a run that cannot clear it is dropped instead.

A **copy in progress**, or the last thing that went wrong, displaces everything but the switch — it is
the more urgent fact, and the counts have not changed anyway. So does `Reading…`, in the git summary's
place: a folder that has not been read yet has no answer from git either, so those two are never both
there. The one thing that cannot be pushed off the line is a **flatten that stopped at its limit**,
which wears a warning glyph inside the group that never gives way. A listing quietly missing rows is
the one wrong answer a file manager must not give: everything else here can be checked against the
folder, and that cannot.

### One baseline, and one pixel grid

Everything on this bar sits on a single **ink** baseline, and the bar itself is put onto whole device
pixels before that baseline is taken off it. Both halves are `status_geometry`, both are load-bearing,
and neither is visible at 100% scaling — which is exactly why they have a test that runs at 1×, 1.25×,
1.5× and 2× with the bar's top edge at six different fractions of a point.

- **A line box is not its ink.** It reserves room under the baseline for descenders and above the
  capitals for accents, and a branch name uses neither — so text centred in the box hangs low beside a
  glyph centred on its own ink. At 1.5× that came to **1.5 points**, which on a 22-point bar is
  visible and was. It is the same correction `CELL_LIFT` makes one row up, taken from the design
  system's own `ink_baseline` rather than measured again here.
- **A pane's bottom edge is a fraction of a window divided by splits.** A bar at a fractional `y` is
  feathered across two rows at each edge while the text inside it is snapped to the grid, so the
  band's *visible* middle is not the one anything was centred on. Rounded, not floored, so the band
  stays 22 points tall.

And then **one point up from all of that**, which is where the arithmetic stops and an eye finishes the
argument. The bar's bottom edge is not its bottom edge: the window's own 1px border is drawn over it,
and `background-layer-alt` against `stroke-default` is a boundary the eye reads as the end of the band
while the measurement does not. So `status_geometry` hands back two rects — the **band** that gets
painted and the **line** everything is placed against, one point above it — and the whole line moves
together, switch and glyphs and words, or the level the rest of this bought would be given away a point
at a time. The test asserts the offset rather than trusting it.

**And every ink on it is measured on it.** The bar is `background-layer-alt`, one rung off the surface
every other status mark in this window sits on, and one rung is the difference between a figure you
can read and one you cannot. That test has caught three: Azur's `status.success` is **2.37:1** on the
light bar and its `status.warning` **2.23:1**, both under the floor for a *shape* let alone for
`13 changed`; and the scan's own figure was in `text-disabled` at **2.20:1**, which is no way to show a
number the window invites you to check. `theme::Bar` is the answer to the first two — the dark side is
Azur's unchanged, the light side the same four hues taken down until each clears 4.5:1 on paper, which
is what `theme::Syntax` already does about the same problem. It is not a design-system change:
`status.warning` is right for a message bar's icon, and what was wrong was asking a mark's colour to
be a word's.

## Managing files

Every operation here goes through the shell's own interfaces, so the results are the
same results:

| | |
| --- | --- |
| `Ctrl+X` / `Ctrl+C` / `Ctrl+V` | cut, copy, paste |
| `Delete` / `Shift+Delete` | to the Recycle Bin / permanently |
| `F2` | rename, in place |
| `Ctrl+Shift+N` | new folder |
| drag out | to Explorer, an archiver, an upload field — anywhere |
| drag in | from anywhere, with `Ctrl` to copy and `Shift` to move |

- **The clipboard is the shell's clipboard.** Files cut here paste into Explorer; files
  copied in Explorer paste here; 7-Zip and everything else that speaks `CF_HDROP`
  works. A cut marks its files but moves nothing until something pastes — and cut rows
  are shown faded, the way Explorer shows them.
- **Copy, move, delete and rename are `IFileOperation`.** That is Explorer's engine, so
  they come with its progress dialog, its time estimate, its cancel, its
  "there is already a file with this name" comparison, its elevation prompt, its
  handling of long paths and junctions and cloud placeholders — and the Recycle Bin,
  with `Ctrl+Z` in Explorer taking the operation back. Each one runs on its own thread,
  so a long copy shows the shell's dialog while this window carries on.
- **An unmodified drag moves within a volume and copies across one**, which is
  Explorer's rule and not an arbitrary one: within a volume a move rewrites a directory
  entry, and across one it is a copy followed by a delete. `Ctrl` forces a copy and `Shift`
  a move, and the cursor says which before you let go.
- **A folder on screen is watched, so a listing is never stale.** A listing used to be as
  fresh as the last thing *this* program did to it: a file deleted from a terminal left a row
  behind, a build writing into the folder showed it as it had been, and a file dragged out to
  Explorer stayed on screen because Explorer's move finishes after our drag does. See
  [Never a stale row](#never-a-stale-row).
- **Dragging out runs on a thread of its own, so the window keeps painting.** `DoDragDrop`
  does not return until the drop lands, and on the UI thread that means no frames — no
  highlight of the folder the drop will land in, no sign of the file the drag just selected.
  It takes the mouse capture and the button state along with it, both of which are per input
  queue rather than per process, so the two queues are joined for the length of the drag.
  See [A drag you can see happening](#a-drag-you-can-see-happening).

## The context menu

The menu is **this program's**, drawn with the design system — same palette, same row
states, same focus ring, keyboard-navigable, positioned at the pointer and flipped
rather than clipped near an edge. The *content* is **Explorer's**, so whatever is
installed appears and works: Open With, Send To, Properties, 7-Zip, TortoiseGit, Scan
with Defender.

The trick is that `IContextMenu::QueryContextMenu` does not display anything — it
*populates an `HMENU`*. So one is created, the shell and every extension fill it, and it
is read back out with `GetMenuItemInfoW`: labels, separators, disabled and checked
states, accelerator text, item bitmaps and submenus. `TrackPopupMenuEx` is never called
and no native menu ever appears.

Two details are what make that faithful rather than approximate:

- **Submenus have to be asked for.** An extension fills its submenu lazily, on
  `WM_INITMENUPOPUP` via `IContextMenu2::HandleMenuMsg`. A menu that is never shown never
  gets that message, so it is sent explicitly before a submenu is read. Without it,
  Send To and New come back empty — the usual way a re-drawn shell menu ends up looking
  finished and being broken.
- **Commands are invoked by canonical verb** (`open`, `copy`, `properties`, or a CLSID in
  braces for anything registered as an `IExplorerCommand`) where the shell offers one,
  since a verb is stable and usable from any thread. Where an extension offers none, the
  numeric id is used — and then the whole shape of the menu has to be reproduced: the same
  `CMF_` flags, the same items, and the submenu it came from populated, because that is
  when the ids inside a submenu are handed out. All three of those were wrong at once, and
  a third of the menu did nothing at all: read the note on `shell::menu::win::invoke`
  before changing how a command is resolved.

The one thing that is *not* reproduced is which entry Windows would have run on a double
click. The shell says so with `MFS_DEFAULT`, and the mark for it here was the 2px accent
bar a selected row gets — which put a blue bar down the side of the top row of every menu,
where it read as a selection nobody had made. So it is not read and not drawn.

### None of it happens in a frame

`QueryContextMenu` shows nothing, so for a long time it was called during the frame that
opened the menu. It is also slow, and not only the first time — the shell asks every
installed extension to contribute, and each one goes to the registry and the disk to decide
what to offer. Measured by `what_the_shell_menu_takes_to_build`:

| | `QueryContextMenu` | submenu prefill | reading the `HMENU` | bitmaps | verbs |
| --- | --- | --- | --- | --- | --- |
| a file, first of a session | 690 ms | 116 ms | 0.2 ms | 0.2 ms | 0.0 ms |
| a file, after that | 130–570 ms | 41–51 ms | 0.2 ms | 0.1 ms | 0.0 ms |
| a folder, or empty space | 130–200 ms | 3 ms | 0.2 ms | 0.3 ms | 0.0 ms |

So a right click froze the window for a sixth of a second at best and most of a second at
worst. The spread is wide and none of it is ours — the same call on the same file measured
567 ms in one session and 140 ms in the next — so read those as an order of magnitude and
not as a benchmark. What is *not* wide is everything this program does with the answer:
reading the `HMENU` back and converting the item bitmaps come to under half a millisecond
together, every time. The icons were the first suspect and they are not the problem.

Three things follow:

- **The query runs on a thread of its own,** and the menu is not drawn until the answer is
  in. So the window keeps running frames while the shell takes its time, and the menu appears
  once, whole, at its final size. It does *not* open early and grow: an earlier version showed
  this program's own entries with a greyed `Loading…` row and let the shell's land underneath,
  which kept the window responsive and did not read as a context menu at all. One menu at a
  time, each with a token, so the answer to a menu already dismissed is recognised and dropped.
  A thread of its own turned out not to be enough — see below.
- **Submenus are filled when they are opened.** Prefilling all of them cost 41–116 ms on a
  file — Open With alone was nearly all of it — for menus usually never opened. The
  `IContextMenu` stays alive for as long as the menu is on screen, so the hover that opens a
  submenu is what pays for it. Submenus are identified by an opaque id the shell hands over
  rather than by a path through the entries, which is what a first version used — and it was
  wrong the moment this program's own entries went above the shell's, since the path the menu
  asked with was six entries and a divider further along than the one the submenu was filed
  under. Every lookup missed and every submenu came back empty.
- **Invoking by verb no longer rebuilds the menu.** A verb needs no numbering, so the
  re-query that precedes it passes `CMF_OPTIMIZEFORINVOKE` and the extensions skip building
  a menu nobody will see. Only a command with no verb — a *position* in the menu — still
  needs the identical query it was read from, or the wrong thing runs.

One thing this does not fix. Some submenus populate themselves *after* `WM_INITMENUPOPUP`
returns, so read straight away Send To has only `Desktop (create shortcut)` in it and Include
in library has the shell's own `Retrieving libraries...` placeholder. Read from the next
`IContextMenu` in the same process, both are complete. Waiting does not help — measured at
300 ms and 1 s, and with `WM_INITMENUPOPUP` re-sent — so it is fixed at the moment that
particular `IContextMenu` initialised it, and only the first right click of a session is
affected. Warming the shell at startup would load every installed extension into the process
for 11 MB of private bytes whether or not anybody right-clicks, which is a worse trade than
one short submenu once per run.

### Twenty-four seconds for one menu, and then no menus at all

A right click on a 14 MB executable on a mapped SMB share put up no menu for the best part of
half a minute — and after that, a right click on a *local* file did nothing either. Measured by
`probe_menu_costs`, which times each call in a menu against a path you give it:

| the item right-clicked | `QueryContextMenu` |
| --- | --- |
| a folder on the share | 0.20 s |
| a 41 MB `.lib` on the share | 1.8 s |
| a 6.4 MB `.exe` on the share | 11.7 s |
| a 11.5 MB `.exe` on the share | 19.8 s |
| a 14.7 MB `.exe` on the share | **23.9 s** |

Every time, not just the first — three rounds in a process and three processes all agree.

The shape of that is one extension reading the whole executable: the time is linear in its
size, at about 570 kB/s. It is not bandwidth. A 41 MB file that is *not* an executable comes
back in under two seconds, so the link is doing better than 20 MB/s and the slow read is
small-chunk and latency-bound. And nothing else in a menu costs anything worth naming: on that
same file, parsing the path was 0.26 s, binding to its parent 0.19 s, and reading the whole
`HMENU` back out — item bitmaps and all twenty-six canonical verbs included — came to **0.4 ms**.

Which extension is a fair question with no clean answer from here. The exe's menu is the only
one of the three with `Troubleshoot compatibility` in it, and `acppage.dll` is registered
against `exefile` specifically; the app-compat shim engine matches an executable by reading it.
The other exe-only handler installed is NVIDIA's `nv3dappshext.dll`, and a corporate
privilege-management extension (`PGExtension.dll`) is registered against every file. Naming the
guilty one for certain needs either a stack sampler or unregistering handlers on a managed
machine, and neither is worth it, because **none of it is this program's to make faster.**

What *was* this program's is the second half of the complaint. `QueryContextMenu` had a thread
of its own, but it was one long-lived thread reading a queue — so the local file's menu sat
behind the network one for the rest of those twenty-four seconds. One slow menu meant no menus
at all until it finished, which from the outside is a context menu that has stopped working.

There is no asking a blocking call into somebody else's code to stop, so the only cancellation
available is to **stop waiting for it**. Asking for a menu while the previous one is still being
built now *retires* its worker — the sender is dropped, the abandoned thread finishes whenever
the shell lets go, releases its `HMENU` and its extension interfaces on the thread that made
them, and exits; its answer arrives with a stale token and goes in the bin — and the new menu is
built on a fresh worker straight away. Driven against the real window, right-clicking that same
14 MB executable and then a local `.rs` file:

| | the local file's menu appeared after |
| --- | --- |
| one thread with a queue | **17.0 s** |
| a worker per menu | **0.55 s** |

Two smaller things came with it, because twenty-four seconds of a window with nothing to say is
its own bug. The pointer shows the progress cursor while a menu is being built, and **Escape**
(or a click) gives up on one — which cannot stop `QueryContextMenu` either, but does mean no
menu appears out of nowhere half a minute after the click that asked for it was called off.
Checked by watching the window for thirty-two seconds after an Escape: nothing appeared, and it
answered every `SendMessageTimeout(WM_NULL)` ping throughout.

Closing the window is the third place those twenty-four seconds could turn up, and did. A menu
still open at exit leaves a worker holding an `IContextMenu` and, through it, a live object
inside every extension that contributed — which wants releasing on the thread that made it,
while that thread is still there. So shutdown waits for its workers, but only for 300 ms, and
spends the wait servicing cross-apartment calls rather than asleep. Closing the window with a
worker still inside that same call took **19.9 s** joining unconditionally and **1.9 s** with the
bound, which is the rest of shutting down and not this.

The cost is that two workers can briefly hold two sets of extension interfaces, which the
single thread was carefully avoiding. That is the right way round: a few megabytes for a few
seconds, against a program whose context menu stops working.

### Asking for less, where more is not affordable

Not waiting is not the same as being quick. The next question was whether the slow entries could
simply be left out — and they cannot, not in that order. `QueryContextMenu` is one call that
lets every installed extension contribute, and the whole cost is inside it, *before a single
entry exists*. An entry that took twenty-four seconds to decide on has already taken them by the
time this program can see it. Filtering afterwards saves nothing at all.

What is left is asking for less. `CMF_` flags are the whole of that lever, and `probe_menu_costs`
measures them back to back on a 6.4 MB executable on the share:

| flags | `QueryContextMenu` | what came back |
| --- | --- | --- |
| `CMF_NORMAL \| CMF_EXPLORE` | 19.2 s | 29 entries — everything |
| `CMF_OPTIMIZEFORINVOKE` | 10.3 s | 20, labelled `open`, `runas`, `pintohomefile` |
| `CMF_NORMAL \| CMF_EXPLORE \| CMF_DONOTPICKDEFAULT` | 21.0 s | 29 entries |
| **`CMF_DEFAULTONLY`** | **0.49 s** | Open, Run as administrator, Cut, Copy, Paste, Create shortcut, Delete, Rename, Properties |
| `CMF_NOVERBS` | 1.3 ms | Cut, Copy, Create shortcut, Delete, Properties |

`CMF_OPTIMIZEFORINVOKE` is the flag whose documented job is exactly this — "do not do work only
needed to display the menu" — and it is no use for a menu that will be displayed: the labels come
back as raw verb names, because extensions skip building display strings, and it is *still* ten
seconds, because whoever reads the whole file does not honour it.

`CMF_DEFAULTONLY` does, forty times over, and what it leaves behind is the shell's own verbs —
everything anybody actually does to a file. What goes missing is the third-party extras: 7-Zip,
Send To, Open With, Copy as path, Previous Versions and the several installed "open with"
entries. It is a request for the *default verb* rather than for a short menu, so this leans on it
a little sideways; what it does empirically is skip the extensions that have to look at the file
to decide what to offer, which is exactly the expensive thing.

So the rule is: **an executable image on a network drive gets the short menu.** Narrow, and
narrow on purpose — what is slow is not "the network". A 41 MB `.lib` on that same share builds a
*full* menu in 1.8 s and the folder itself in 0.2 s. A blanket rule for network paths would throw
7-Zip and Send To away on every file on the share to fix something only executables have.
Deciding costs 376 µs the first time a drive is seen and 26 µs after — `GetDriveTypeW` answers
from the mount table and never touches the network, so a share that has gone away says
`DRIVE_REMOTE` rather than hanging.

That list of extensions covers what was measured. It cannot cover what this machine's extensions
decide to read a whole file for next, so there is a second net underneath: **any menu on a
network drive that has not arrived in 2.5 s is abandoned and asked for again with less.** Network
only, because a local menu is 0.13–0.69 s and should never be quietly shortened because the
machine was busy for a moment. And a short menu says so in the status line, since 7-Zip going
missing should not have to look like a broken program.

Right-clicking that 14 MB executable, driven against the real window and timed from the click to
a menu on screen:

| | menu on screen after |
| --- | --- |
| neither | **23.2 s** |
| the 2.5 s deadline alone | **4.3 s** |
| both | **1.6 s** — and 0.35 s of that is the harness settling the pointer |

### Only Windows' entries

There used to be half a dozen of this program's own above the shell's — open in a new tab,
open in a pane to the right or below, bookmark, copy path, new folder, show hidden files. They
are gone. A right click gives Windows' menu and nothing else, which is what "the content is
Explorer's" ought to mean; every command they carried is still on its keyboard shortcut, and
most of them are in the shell's own menu anyway under the name Explorer gives it.

The only entries this program still owns anywhere are the three a **right-button drop** asks
— `Copy here`, `Move here`, `Cancel` — because that is not a question the shell has a menu
for.

One thing to know about the trade: right-clicking the empty space below the files gives the
folder's own shell menu, and `New`, `Refresh` and `Paste` are **not** in it. Those come from
Explorer's *view* rather than from `IContextMenu`, so there is nothing to read them out of. They
are on `Ctrl+Shift+N`, `F5` and `Ctrl+V`.

### A menu action does not reload the folder

It used to, on the way out of every single one of them. There is no way to be told what a shell
verb did — rename, delete, extract, commit, nothing at all — so re-reading looked like the only
honest answer, and it meant a scan and a rebuilt listing after `Copy`, after `Properties`, after
`Open with`.

It is not the only answer, because [`watch`](src/watch.rs) is already running: every folder on
screen has a `ReadDirectoryChangesW` handle on it, so a verb that *did* change something is
noticed within a sixth of a second — whoever changed it, this program or Explorer or a terminal
or the extension the verb belonged to — and a verb that changed nothing costs nothing.
`a_change_on_disk_re_reads_the_folder_by_itself` is the test that the load-bearing half of that
works, through a real handle and a real file appearing.

### Which menu a right click gets

A row is mostly space: a 24-point row across a wide pane has ink on perhaps a third of it, and
the space around a name belongs to the listing's background as much as the gap under the last
file does. So the two halves of a row are two different questions.

- **On the name, the icon or one of the three values** — a row not already selected is selected
  first, and the menu is that file's. Which is what "right-click one file and act on it" looks
  like it should do.
- **In the space around them** — the selection is cleared and the menu is the *folder's*. That
  is the same rule the drag has always followed: from the ink it picks the file up, from the
  space it draws a rubber band.
- **A row that is already selected** is the selection's menu wherever in it the click lands.
  The files are picked out already; taking that away because the pointer was between two
  columns would undo work rather than ask a question.

And under the last file there is now **a row and a half of nothing**, which is what makes the
folder's menu reachable at all in a folder taller than the pane — before, every pixel from the
header to the status line was a row, and the gap at the end was whatever the last row happened
to leave. It is scrollable content rather than a margin, so at the top of a long folder the
pane is still all rows and the space appears as you reach the end.

It was three rows, and three is more than that argument buys: the end of a long folder read as
though the listing had stopped short of the pane, and slack that is content is charged twice —
a folder which nearly fills its pane grows a scrollbar for it. Half of it is still one and a half
times a click target. Half a row is also why the listing does its own virtualisation arithmetic:
`ScrollArea::show_rows` counts the extent in whole rows, so `filelist::rows` runs that function's
own sums over `show_viewport` with the tail in points. Nothing on screen says which of the two it
is — a listing virtualised wrongly looks perfect until it is scrolled — so the gap at the end of
the scroll is measured in a test.

### It does not say it is reading

Not for the first half second, anyway. A local folder comes back in single-digit milliseconds —
that is what the whole of "Why it is fast" is about — so a `Reading…` drawn the moment a folder is
asked for was a word that flashed up and vanished on every navigation, in the exact place the
listing was about to be.

Past half a second, silence is the thing that misleads: a window showing an empty folder that is
not empty looks like a program that has finished and got it wrong. So the message waits, and the
frame that would show it is booked for that moment — this program is idle between events, and
otherwise the one frame that noticed the threshold had passed would be a frame nothing was going
to ask for.

### Where the listing is scrolled to

Two rules that pull in opposite directions:

- **A re-read keeps its place.** Every file operation ends in one, and so does anything the
  context menu does — so a scroll that did not survive it would mean acting on a file two
  hundred rows down and being sent back to the top to find it again.
- **A different folder, or a different tab, starts where it belongs.** Row 200 of the folder you
  just left is not row 200 of anything.

Neither is free, because the offset does not belong to the listing as far as egui is concerned:
a `ScrollArea` is keyed by *where it is drawn*, so the pane's one scroll area goes on showing
whatever it was showing when the tab under it changes. So the tab owns the number —
`Tab::scroll_y` — and [`Pane::show_tab`](src/pane.rs) is the only way the active tab changes,
which is what makes "each tab keeps its own place" true rather than nearly true.

**And the edges say there is more.** A listing cut off mid-row looks like a folder that ends there;
the design system's `components::scroll_fades` is the rule, and this listing, the console log and both
preview panels all wear it. The one thing it cannot be told from the scroll area itself is the trailing
slack under the last file — that space is deliberate, and a fade over it would say there were more
files. So the bottom is measured from the rows: the fade goes out exactly as the last one arrives.

![The context menu](docs/menu.png)

Four things the drawing has to get right, because they are what a hosted `HMENU` would
have got for free:

- **The size is computed before anything is drawn**, from the design system's own row
  height rather than a number copied out of it — the position depends on the size, and a
  menu that learned its height a frame late would appear in the wrong place and then jump.
  A level taller than the window is capped and scrolls, so the last entry of a machine
  with a dozen shell extensions installed is still reachable.
- **Properties is pinned below the scroll**, with the divider above it, so it is the last
  thing on the menu whatever the scroll is doing. It is where a shell menu ends and it is
  what people go to the bottom of one *for* — which on a machine with a dozen extensions
  installed is the entry with the furthest to scroll to. Recognised by its `properties`
  verb rather than its label, since `Propriétés` is one localisation out of many. Nothing
  is *moved*: the pinned part is a suffix of the entries in the order the shell gave them,
  so anything an extension put below Properties is pinned with it. Showing the menu in an
  order Explorer does not would be worse than a menu that scrolls.
- **Every level opens at its first entry.** A `ScrollArea` keeps its offset in `egui`'s
  memory under an id that outlives the menu, so a right click, a scroll to the bottom and
  a dismissal left the *next* menu opening halfway down itself. A level is put back to the
  top on the frame it appears and left alone after that — per level rather than per menu,
  because sibling submenus share one scroll id and a long `Open with` was lending its
  offset to a short `Send to`.
- **It does not fade in.** An `egui::Area` fades over a tenth of a second by default,
  which is right for a tooltip that appeared on its own and wrong for a menu that was
  asked for and is already being read.

### Copy, cut, paste and delete

All four are the shell’s, through `IFileOperation` and the shell’s own clipboard formats, so
what appears is what Explorer would show and what is on the clipboard is what Explorer would
put there. Verified rather than assumed — `what_the_shell_puts_on_screen` lists what comes up:

| | |
| --- | --- |
| a copy onto an existing name | `Replace or Skip Files`, Explorer’s own conflict dialog |
| Shift+Delete | `Delete File`, the permanent-delete confirmation |
| Delete to the Recycle Bin | nothing, which is Windows’ own default |
| a copy into the folder it is already in | nothing, and a `one - Copy.txt` appears |

Four things had to be fixed to get there, and each of them is the kind that looks like nothing
until you try it:

- **A copy into the current folder stopped on the conflict dialog** instead of making
  `one - Copy.txt`. `FOF_RENAMEONCOLLISION`, and only when *every* item is already in the
  destination — a batch from several folders that happens to include one of the destination’s
  own is a real name clash for the others, and those are the user’s to answer.
- **A cut was taken off the clipboard when the paste *started*.** Answer the conflict dialog
  with Cancel and the cut was gone, with the sources still shown as pending and nothing left to
  paste them from. The clipboard is now finished when the shell says the move happened, which is
  also when `CFSTR_PASTESUCCEEDED` goes back to whoever owned the cut — the documented end of
  the protocol, and what stops Explorer showing its own items faded.
- **A paste only understood `CF_HDROP`.** Copy a file out of a `.zip` in Explorer and there is no
  `CF_HDROP` at all: the data object offers `Shell IDList Array`, `FileGroupDescriptorW` and
  `FileContents`. Ctrl+V did nothing whatsoever. A paste now asks the shell what the object
  holds, exactly as Explorer’s own paste does, and `IFileOperation` extracts from the archive.
- **A copy did not survive this program closing.** `OleSetClipboard` leaves the clipboard
  holding a *reference* into this process, so a copy taken here and pasted after the window
  closed pasted nothing. Rendered on the way out now; measured both ways round.

And one that was not about copying at all: **a path with forward slashes in it broke every
shell call.** `--open=C:/Windows` lists perfectly, because `std::fs` hands paths to the kernel,
which takes either separator. The shell *parses* them and refuses a slash — so in that state
there were no icons, no context menu, and copy, paste and delete all reported that the items
could not be found. One conversion in `shell::wide` fixes all of them at once.

### Ctrl+C and Ctrl+V are not key presses

They were bound to `Key::C` and `Key::V` with `command` held, which reads correctly and never
once fired. `egui-winit` recognises Ctrl+C, Ctrl+X and Ctrl+V *itself* and queues `Event::Copy`,
`Event::Cut` or `Event::Paste` **in place of** the key event, returning before `Event::Key` is
added at all. So `i.key_pressed(Key::C)` is never true for a copy.

Paste is worse. The `Event::Paste` it substitutes carries the clipboard’s **text**, and it is
only queued when there is some — so with files on the clipboard, pressing Ctrl+V produced no
event whatsoever. The shortcut did not exist.

Copy and cut now read the events, which always arrive. Paste has nothing to read, so
`paste_keystroke` in `main.rs` puts the keystroke back from the only place that still knows about
it: `GetAsyncKeyState`, edge-triggered so holding the keys pastes once, and guarded on
`focused` because that call answers for the whole desktop.

Every test of these three had synthesised `Event::Key` and passed against a program in which none
of them worked — which is the more useful lesson. The test now sends what winit sends, and fails
when the handling is removed.

Two more things had to be right before Ctrl+V worked *twice*, and neither was reachable from a
test harness. They were found by launching the window and pressing the keys with `SendInput`.

**The latch that stops one press pasting twice cannot be cleared by watching the key go up.** The
swallowed press still asks for a repaint, so there is a frame in which the keys read as down, and
the following frames need to know not to repeat it. But after a paste the window has nothing to
draw and stops running frames at all — so the release is never observed, the latch stays set,
and Ctrl+V works exactly once per session. Which is precisely what "worked once and then no
longer" is. `Key { key: V, pressed: false }` is *not* swallowed and always arrives, and delivering
it *is* a frame, so the release now clears the latch through the event queue rather than through a
sample nobody was awake to take.

**A refresh has to keep the selection.** Every file operation ends in one: the listing is dropped
so the folder is re-read, and the answer arrives frames later. The selection went with it, so the
second Ctrl+C had nothing selected and copied nothing. [`Tab::refresh`] puts the selected names
aside at the moment the listing is dropped, because by the time the new one lands there is nothing
left to ask. Names that have gone — moved, renamed, deleted — simply are not selected any
more. Navigating somewhere else still clears everything, since a different folder is a different
set of files.

### Renaming, and making a folder

**A rename opens with the stem selected, not the whole name.** Typing over `report.docx` means
replacing `report`; an editor that hands you the extension as well is an editor that turns every
rename into a file with no type. `file_stem` gets the awkward cases right on its own —
`.gitignore` comes back whole, which is what Explorer selects too, and `archive.tar.gz` comes
back as `archive.tar`, because only the last extension is one. A folder keeps its whole name
selected even with a dot in it. Counted in characters rather than bytes, or the caret lands
mid-glyph on `réunion.txt`.

**The row being renamed wears no selection.** A selected row’s fill and accent bar sit right
behind the field competing for the same edge, so the one row you are actually looking at reads
worst. Explorer drops the highlight while renaming too. The field has **no accent ring** either,
for the same reason: it is already an obviously editable box — a filled rectangle with a caret in
it, on a row that has had its own highlight taken away for exactly this purpose — and the ring on
top of that was the loudest thing in the window.

Taking that ring off is where a rename briefly became unreadable, and the reason is worth writing
down. `Visuals::selection::stroke` is **two unrelated things in one egui field**: the border round
a focused framed `TextEdit`, and — read as `stroke.color` by `text_selection::visuals` — *the
colour selected text is drawn in*. So `Stroke::NONE` removed the border and painted the selected
stem transparent: a rename opened as a flat accent block with the name invisible inside it. A
**zero-width** stroke removes the border and leaves the colour alone.

And the colour was wrong as well, in the design system, which is where that got fixed: it pointed
the field at `stroke-focus` — correct for the border half — which put selected text at an accent
on an accent, **2.38:1**. It is `text-on_accent` now, the token defined for ink on
`accent-default`, which measures 6.7:1 against the selection on screen. Azur's own `TextField` is
frameless and paints `stroke-focus` itself, so nothing lost a focus ring for it. A test in the
crate now holds the two halves of that pair to 4.5:1 together, which is the thing neither side was
checking: `style::tests::selected_text_reads_on_the_selection_behind_it`.

**The field is allowed to be wider than the Name column.** That column is as wide as the other
three leave it, which on a narrow pane is narrower than plenty of names, and a field pinned to it
scrolls a long name sideways under the caret: renaming `2026-04-report-final-approved-v3.xlsx`
meant editing eleven characters of it through a letterbox and guessing at the rest. So it grows to
fit its text and runs on over Size, Type and Modified — nothing anybody needs to read while typing
a name, and back the moment the rename ends. It grows only as far as the text needs, stops at the
right edge of the row, and is drawn after every other cell of every other row so that its own
row's remaining columns cannot be painted on top of it.

**The name does not move when the field opens.** Which sounds like nothing and was the most
obvious thing wrong with it: a field brings its own padding and its own idea of where a line sits
inside it, so the name jumped **two points right and one point down** the instant a rename began.
On the one word you are looking at that reads as a flinch.

So the box is given no padding at all, and both sides centre the line by the same rule — with the
same snap to *device* pixels, which is the part that is easy to miss: centring the field on the
text row and leaving egui to divide by two got within **half a point**, and half a point is a
blurred word rather than a sharp one. The fill is `background-layer`, the panel's own, rather than
`background-control`, which is a *control's* colour and drew a grey slab over the row. Opaque
rather than transparent, though, because covering the cells it runs over is the whole point of
being wider than the column.

Together those are what make it read as the row itself becoming editable rather than as a widget
appearing over it. `opening_a_rename_does_not_move_the_name` compares where the *text* was painted
in the frame before against the frame after, which is the thing the eye was complaining about —
asserting on the field's rect instead would only check the test's own arithmetic.

**A new folder arrives with its name already open for editing,** because `New folder` on its own
is half a gesture and nobody wants a folder called `New folder`. Which name to open is the
shell’s answer rather than a guess: ask for `New folder` where one exists and what appears is
`New folder (2)`, so the created name comes back through `IFileOperationProgressSink::PostNewItem`
and the row is selected and opened when the re-read brings it in.

### Dropping onto a folder

Two bugs sat underneath all of this, and the first is the worst thing found in this program.

**The `DROPEFFECT` bits were transposed.** Windows numbers them `NONE = 0, COPY = 1, MOVE = 2`;
they were written as `1 << 1` and `1 << 0`, which is the same pair the other way round. So a copy
taken here announced itself to every other program as a **cut**, a cut announced itself as a copy,
and reading somebody else’s copy came back as a *move*: copy a file in Explorer, paste it here,
and the original was gone. They are taken from the Win32 headers now rather than written out.

**A drop resolved its target in the wrong coordinate space.** `IDropTarget` is handed **screen**
coordinates and the drop zones are published in the window’s own. They were compared directly,
and the only reason anything ever appeared to work is that the two overlap when a window sits near
the top left of the screen. A drop landed in whichever zone the *screen* point fell in — almost
always the whole pane rather than the folder row under the pointer — so a file dropped on a
folder went into the folder already being shown, where it was filtered out as a no-op. Dragging did
nothing at all. `ScreenToClient`, once, in the one place both callbacks go through.

That second one had a companion waiting: the conversion helper took the same lock the callbacks
hold while they call it, so the moment a zone *did* match it would have deadlocked. The handle
belongs to the drop target now rather than to the shared block.

### Somewhere to drop onto

A drop takes the effect the pointer promised: `Ctrl` copies, `Shift` moves, and with neither, the
same volume moves and a different one copies — which is what the cursor showed while the drag
was over the window, decided in the `DragOver` callback that has to answer immediately.

What it lacked was somewhere to drop *onto*. The zones published for the OLE callbacks were one
per pane, mapped to the folder that pane was showing, so dragging a file onto a folder row put it
in the folder you were looking at instead of in the folder you dropped it on — the one thing
that gesture means. Every visible folder row is now a target of its own, published after the pane
so it wins, and the highlight follows the row rather than lighting up the whole pane and promising
the wrong destination. The destination is also the one resolved when the pointer was there, rather
than worked out again from the pane afterwards, which is how the original went wrong.

### What each button does

| | |
| --- | --- |
| left drag from a file | move or copy it, by the rule below |
| left drag from empty space | rubber-band selection |
| right drag | the same, and on drop it *asks*: `Copy here`, `Move here`, `Cancel` |
| middle, or anything past it | nothing — it used to pick files up |
| thumb buttons | back and forward |

A drag that starts on something already selected takes the whole selection; one that starts
anywhere else makes that row the selection first, so dragging a single file needs no click
beforehand.

Dropping a folder on **Bookmarks** pins it, which is why that group answers the pointer `LINK`
rather than copy or move: nothing is copied and nothing is moved, and a copy cursor there would
be promising a copy that is not going to happen. The source has to *offer* `LINK` for that to
work — OLE refuses a target that answers with an effect the source never allowed, so with only
copy and move on offer the drop was rejected and dragging a folder onto Bookmarks did nothing at
all.

`Ctrl` copies, `Shift` moves, and with neither, the same volume moves and a different one copies —
unless what is being dragged is a temporary, for which see below.

The thumb buttons are worth a note because "button 4 and 5" travel under four names on the way in:
Windows sends `WM_XBUTTONDOWN` with `XBUTTON1`/`XBUTTON2`, winit turns those into
`MouseButton::Back`/`Forward`, and egui into `PointerButton::Extra1`/`Extra2`. Anything past the
fifth button becomes `winit::MouseButton::Other` and is dropped before egui sees it.

### An effect is an instruction, not a preference

A drop out of a **7-Zip archive** deleted the files it was copying, halfway through. An archiver has
no paths to put in a `CF_HDROP` until it has extracted something, so what it offers is a temporary —
`%TEMP%\7zO…` — and `%TEMP%` is on `C:`, so the same-volume rule called a drop into any folder on
`C:` a **move**. `DROPEFFECT_MOVE` handed back to a source is not a description of what the user
wanted: it is an instruction to delete what it just handed over.

A source that materialises its files that way takes them back the moment the drag is over, so a
move out of one is meaningless whichever volume it lands on. `dnd::under_temp` is that test, applied
to the first path the data object offers, and anything inside the temporary directory is a copy.
Case-folded and component-wise, then again with both paths resolved, because `%TEMP%` is an 8.3
short path on some machines while `CF_HDROP` carries the long form and the two are the same
directory. `Shift` is left alone — an explicit request for a move still gets one.

The other half is that `pdwEffect` arrives holding the effects the source is *willing* to allow, and
none of that was ever read; it was only written. The rule is already recorded above from the source
side — a target answering with an effect the source never offered is a target OLE refuses, which is
why this program's own drags offer `LINK`. From the target side it means a foreign source offering
only a copy was answered `MOVE` and had its drop thrown away whole. Every answer goes through
`permitted` now, which narrows it to what was offered, a move degrading to a copy and never the
other way round: the fallback for an effect that is not on offer has to be the one that destroys
nothing. It is the same reason pinning may report a copy but must never report a move — Bookmarks
copies nothing at all, and a source told `MOVE` by it would delete a folder that had only been
pinned.

**Neither of those is what bit 7-Zip, on the evidence.** It renders `CF_HDROP` when the drop is real
and not before, so during the drag there are no paths, the source is unknown, `under_temp` never
fires and the default was already a copy — the cursor said so. What deletes the extraction there is
the cleanup that follows `DoDragDrop` returning, which is the next thing and not this one. Both rules
above are still wrong rules, and still worth fixing, and would still bite a source that answers
during the drag.

**So the whole bug is elsewhere**, and it is the next section.

### Claiming what the source is about to delete

The copy starts a frame after `Drop` returns, and a source that materialised its data is entitled to
take it back the moment it does. The two answers Windows offers are both bad here. Doing the copy
inside `Drop` means doing it on the UI thread, inside the message pump, with the window unable to
paint — a dropped 4 GB archive would freeze everything, and there is no scoping that away, because a
thread stuck in a callback cannot paint one tab and not another. The other answer,
`IDataObjectAsyncCapability` — `SetAsyncMode`, `StartOperation` before returning from `Drop`,
`EndOperation` when the job lands — only works for a source that implements it, and would still
leave a right-button `Move here` racing the cleanup while the menu waits to be answered.

What `Drop` *can* do in the microseconds it has is stop being the source's problem. The files are in
`%TEMP%` — that is what `under_temp` established — so a staging directory made alongside them is on
the same volume by construction, and moving them into it is a directory-entry rename. Instant,
whatever the archive weighs. The source is then welcome to delete a folder that is empty, and the
copy to the real destination runs on the ops thread like every other one, with the window live and
every tab usable. `dnd::claim`, called from `Drop` before it returns.

Which also makes the right-button menu safe, and that could not have been fixed any other way:
`Copy here` is answered whenever the user gets round to it, minutes after every source has cleaned
up. Nothing in the menu had to change — it carries the staged paths without knowing it.

Three things keep it honest. The claim is only ever applied to a temporary: the same rename against a
drag from Explorer would move the user's real files into a scratch folder, which is why the test is
narrow and why a pin never claims. A rename that fails — a source still holding a handle — leaves the
original path in place, so the worst case is the behaviour before any of this, not a lost file. And
the directory goes when the job that consumes it does, on that job's thread, by a guard rather than a
statement, so a panic cannot leak an extracted archive. `ops::claimed` recognises one from the path
rather than being handed it, which is why every route to a job gets the cleanup for free;
`dnd::is_staging` is the test that authorises the `remove_dir_all`, and it asks both for the name and
for the directory to be sitting directly in `%TEMP%`, because one of those on its own is not
something to trust with a recursive delete.

`dnd::sweep` runs once at startup for what is left when a run does not get to finish — a kill between
the claim and the copy, or a right-drag answered with `Cancel`, which discards the claim without ever
making a job. It skips any directory whose process is still running: another window may be copying out
of it, and that is the exact bug all of this exists to fix.

### A menu belongs to a moment

Losing the window closes every popup in it: the context menu, the popups egui keeps in its own
memory — the application menu under the mark at the top left — and the path bar's dropdowns with the
tracking mode they turn on. Alt-tab away and come back ten minutes later and a menu still standing
over the listing is not where you left off; it is a menu about a file you have stopped thinking
about, over a window that now takes two clicks to get back into. Windows dismisses a menu when its
owner loses activation, and this does the same.

`App::close_on_blur` runs on every unfocused frame rather than only on the one where focus was lost:
the transition needs a frame to be noticed in, a window that has just gone to the background is not
always given one, and "is anything open while we are not in front" has the same answer either way.
The flag is the *window's* focus, `InputState::focused`, and not egui's — a focused text field is a
different thing entirely, and closing a menu because the filter box has the caret would be a bug.
`RawInput::focused` defaults to `true`, so this can never fire on an integration that does not track
focus, which is also why every other test in the harness is untouched by it.

### The source is asked once for the cursor, and again for the drop

`GetData` is not a getter. It is a request that the source *render* what it is offering, and it may
do arbitrary work to answer one — the extraction above is a source rendering `CF_HDROP`. It was
called from the effect rule, which runs on every `DragOver`, so a drag crossing the window asked the
source to render its data dozens of times a second. It is read once now, on `DragEnter`, and kept in
`Incoming` for the cursor to be answered from.

**Caching that for the drop to use as well broke dropping out of an archive entirely** — no
extraction, no copy, nothing. A source is allowed to have nothing to give until the drop is real;
answering a speculative request mid-drag is exactly the part it may skip, and 7-Zip skips it. So the
drop asks again, which is the call that makes the extraction happen, and falls back to the drag-time
list only if that answer is empty. Two renders per drag rather than fifty, and the one that has to
work is the one at the end.

### A drag you can see happening

Two pieces of feedback are the whole gesture: the file you picked up shows as selected, and the
folder the drop will land in lights up — the row under the pointer, or the pane's own folder when
the pointer is not on a row. Both were being drawn. Neither could be seen.

`DoDragDrop` is modal: it runs its own message loop and does not return until the drop lands. On
the thread that owns the window that means **no frames at all** for the length of the drag — not
one repaint reaches the pane, so the highlight and the selection appeared only once the gesture
was already over, which is not feedback. Two attempts at forcing a paint from inside the drag
(`RDW_UPDATENOW` among them) produced no frame either; winit's loop is suspended underneath the
call and will not be re-entered.

So the drag gets a thread of its own, and the window goes on painting underneath it. Three things
make that work:

- **The drop target is in the UI thread's apartment**, and OLE marshals the callbacks back into
  it — the same machinery that lets Explorer call into this process at all. So `DragOver` still
  writes the hover point the frame reads, from a drag started anywhere.
- **`AttachThreadInput` joins the two threads' input queues** for the length of the drag. Mouse
  capture and key state are per *input queue* on Windows, not per process: from a fresh thread
  `DoDragDrop` would see no button held and end the drag before the pointer had moved. This is
  the part the old comment in this file said could not be made to work, and it was wrong.
- **Repaints are asked for by state, not by event.** While a drag is in flight the pointer belongs
  to OLE and no input reaches winit, so nothing would ever ask for the next frame. It is the one
  place in this program where painting is driven by a flag.

### The release that never came

And underneath it, the bug that made drag and drop look broken however it was implemented.

`DoDragDrop`'s loop consumes the button-up that ends the drag. The window never sees it, so egui
goes on believing the button is held — and a press that arrives while a button is already down
starts no new drag. **One drag per window, and then nothing.** Any unrelated click put the state
right again, so it looked intermittent rather than broken: drag, drop, drag again, nothing;
click somewhere, drag, and it works.

It cannot be fixed by waiting for the release, because the release is never delivered. So it is
stated instead — for whichever buttons egui still thinks are down, at the position egui already
has, injected into the next frame's raw input.

Every test of a drag passed while this was live, because each ran a single drag in a fresh window
and released the button itself. The harness now appends injected events the way the real input
hook does, and the test that guards this presses, travels, and *never releases* — three times
over.

### Never a stale row

One thread watches every folder on screen through `ReadDirectoryChangesW`, and a folder that
changes is read again.

It is one thread rather than one per folder because navigating changes the watched set on every
click, and a thread per folder would mean a thread spawned and joined per click. So there is a
directory handle and an event per folder, and the thread parks in `WaitForMultipleObjects` over
all of them plus one more event the UI thread signals when the set changes. Nothing spins and
nothing polls.

Three decisions worth stating:

- **The notifications are not read.** `ReadDirectoryChangesW` will say which file changed and
  how, and none of it is worth having: the answer to any of it is to re-read the folder, which
  is one scan either way. Not parsing the buffer also removes the two ways this API is usually
  got wrong — the alignment of `FILE_NOTIFY_INFORMATION`, and the overflow case where the buffer
  was too small so the contents are gone but the fact of the change is not.
- **`FILE_SHARE_DELETE` is not optional.** Without it the watcher's own handle would stop the
  folder from being deleted or renamed — so watching a folder would break the operations it
  is watching for. There is a test that deletes a watched folder.
- **The listing is not blanked.** `Tab::refresh` drops the rows, which is right for F5 —
  somebody asked, and a moment of `Reading...` is the honest answer. Here it would flash on
  every file a build wrote. So the rows stay up, only the re-read is asked for, and the
  selection comes across by name when it lands.

A burst settles for 150 ms first. Writing one file fires several notifications — the name, the
size, the timestamp — and one scan answers all of them.

### A right drag asks, even where a left drag would do nothing

Dropping a folder into itself is meaningless whatever button carried it, and a file dropped back
into the folder it is already in is meaningless too — for a *left* drag, where it means "move
this to where it already is". Both were filtered out before anything else happened.

But right-dragging a file onto its own folder is how Explorer is asked for a copy of it, and the
answer is `one - Copy.txt`. Filtering it out before the question was asked meant a right drag
inside a folder did nothing at all, which is the most obvious way anybody would try the gesture.
The filter now applies to left drags only.

### Shift+Delete is a cut

On Windows `egui-winit` recognises Shift+Delete as a legacy cut and queues `Event::Cut` for it —
the very same event Ctrl+X produces, with nothing to tell them apart but the modifiers. The arm
handling cuts was guarding itself with `!m.shift`, so Shift+Delete did nothing whatsoever: the cut
arm refused it and the `Delete` key it was waiting for never arrived either.

It is a permanent delete now, and `!m.command` is the discriminator. The direction it fails matters:
with Ctrl held this is Ctrl+X, or Ctrl+Shift+X with a thumb resting on Shift, and reading either of
those as "delete this for ever" would be the worst mistake this program could make. Anything
ambiguous is a cut, which moves nothing until something pastes.

### The test that asked Windows to delete the repository

The test guarding the paragraph above did it in the obvious way: select the first row, feed the
window a Shift+Delete, check the action that comes out is a delete and not a cut. What it missed
is that **a frame does not just produce actions, it applies them.** The harness opens
`CARGO_MANIFEST_DIR`, so the first row was `.cargo`, and the frame handed `IFileOperation` a
permanent delete of it and put the shell's confirmation dialog up to ask about it.

The job runs on a thread of its own — that is why the window stays live during a copy — so the
test finished and **passed** while the dialog was still on screen. Nothing in any test output said
a word. Whether the folder survived came down to a human not clicking Yes, and it was only found
because somebody watched a delete prompt appear out of nowhere while a suite was running.

`Operations::start_then` is the one place every copy, move, delete, rename and new folder goes
through, so the guard is there: under `cfg!(test)` the job reports back as done without the shell
being asked, unless `shell::ops::FOR_REAL` is set. Three tests set it, through an
`ops::for_real()` scope guard, and all three work inside `target/sandbox`. This is the same shape
as the `cfg!(test)` guard on `Config::save`, and it is there for the same reason: **a test process
has no business changing a user's files or settings**, and a suite that is green is not evidence
that it did not.

Its own test starts the exact job that was issued — `Shift+Delete` on `.cargo` — and checks the
folder is still there afterwards. It then proves the guard is not merely inert, by running a
`NewFolder` in a temp directory with the gate shut and open and counting what appears: nothing,
then one.

### An apartment that does not answer

Two of these were found the same way, and neither looked like what it was.

**Copy from the context menu put files on the clipboard that nothing could read.** The menu’s
Cut, Copy and Paste are the shell’s own entries, invoked by canonical verb on the modal thread.
`InvokeCommand` worked perfectly — read the clipboard *on that thread* and the file was right
there — and from anywhere else `IsClipboardFormatAvailable` said the clipboard held no files at
all. Clipboard data belongs to the apartment that put it there, and reading it from elsewhere is
a call back into that apartment. The modal thread was parked in `recv()`, answering nothing. It
now waits by answering, and a menu click costs at most 50 ms of latency for it.

**Right-clicking an empty folder did nothing.** `rows` is what wires a listing’s clicks up, and
a listing with nothing in it never reaches `rows`: an empty folder, one filtered down to
nothing, one still being read and one that refused to be read all draw a line of text over a
body nothing was listening to. Which is the one place `New folder` and `Paste` are most wanted.

### The clipboard is one lock, and this program has to answer for it

Two consecutive copies would sometimes fail with `CLIPBRD_E_CANT_OPEN`, which renders as
“OpenClipboard failed” and reads like somebody else’s fault. It was not.

`OleSetClipboard` copies nothing: it leaves the clipboard holding a reference to a data object
in *this* process. So a program that reads the clipboard — Windows’ clipboard history, first in
the queue every time — calls back into this apartment for the bytes, and it does so **holding
the clipboard**. A thread that does not answer that call leaves the lock taken and the next copy
refused.

The shape is what gave it away. Copies made back to back never failed, because the history
service had not started asking yet; copies made 200 ms apart failed eight to eleven times in
twelve. The same measurement against PowerShell’s `Set-Clipboard` never failed once.

So the retry waits with `CoWaitForMultipleHandles(COWAIT_DISPATCH_CALLS)` rather than sleeping:
it answers cross-apartment calls and deliberately leaves the window queue alone, because a copy
happens inside an egui pass and dispatching a `WM_PAINT` from there would have egui begin a
pass while already inside one. Nought failures in forty-eight, repeatedly.

Two smaller things came out of the same measurement. Emptying the clipboard was
`let _ = OleSetClipboard(None)` — an unretried call whose swallowed error meant a cut that had
been pasted stayed on the clipboard, ready to move files that had already moved. And a *read*
was only retried around getting hold of the object, not around reading it, so a paste could
report “there are no files on the clipboard” about files plainly on it.

## A console in a pane

`Ctrl+²` opens a console along the bottom of a pane, in the stack between the listing's rows and its
status line — a panel, not an overlay. Git Bash by default, running in the folder the pane is
showing, with `pwsh` and `cmd` in a dropdown that `Shift+Tab` cycles.

**It is not a terminal**, deliberately. There is no pseudoconsole, no escape-sequence interpreter and
no character grid: a shell is started on plain pipes and commands are written to its standard input.
That is four hundred lines instead of a terminal emulator, and it is the right shape for what a
console beside a file listing is actually for — run a build, run `git status`, read the answer.

**The key is the leftmost key of the number row**, whatever is printed on it: `²` on an AZERTY board,
`` ` `` on a QWERTY one. It is bound by *position*, which falls out of how egui resolves a key rather
than from anything asked for: it takes the logical key when it recognises the character and the
physical code when it does not, and `²` is not a character it names.

### The shell stays alive, so a command has to say when it is done

One shell per kind, living across commands — that is what makes `cd`, `export` and an activated
virtualenv still true for the next command. The cost of a live shell on a pipe is that **nothing marks
the end of a command's output**: with no terminal there is no prompt to recognise, and the pipe going
quiet is indistinguishable from the command being slow.

So each command is followed down the same pipe by a line this program wrote:

```text
cargo test
[AZUR:7:0:D:/Sources/MyTools]
```

Reading until that line arrives gives four things at once — the command finished, its exit status,
where the shell now is, and which command it belonged to. **Everything the panel does is built on
it**, including the part that makes a long session readable: a block *is* the output between one
sentinel and the next, so it has a header and it folds.

Three details that each cost an afternoon, every one of them found by a test that runs a real shell
rather than by reading the code:

- **The status has to be captured into a variable first.** `printf '…' "$?" "$(pwd -W)"` works only by
  an argument-evaluation-order argument, and when it stops working every command reports the status of
  the `pwd` inside its own sentinel — that is, everything succeeds.
- **PowerShell needs `${__azur}` with its braces.** Inside a double-quoted string a `$name:` is a
  scope-qualified variable — the same syntax as `$env:PATH` — so `"$__azur:$(…)"` is a *parse error*.
  It fails on stderr while the command itself succeeds, so the block never closes and the panel
  appears to hang with nothing anywhere to explain it.
- **`cmd` writes a prompt, and a prompt has no newline behind it.** So the sentinel continued that
  line, `D:\src>[AZUR:1:0:D:\src]` is not a line *beginning* with the marker, and no block in `cmd`
  ever closed: every command sat there saying "running" for ever. `@echo off` turns the prompt off,
  one `echo.` in the shell's opening commands finishes the one it has already written, and `cmd` reads
  each line back to us besides — which is dropped by matching the lines this program knows it wrote.
  It survived because `cmd` was the one shell the real-shell test did not cover. It is covered now.
- **A command can end without a newline too**, and then the sentinel shares that line in *any* shell:
  `printf 'a'` gives `a[AZUR:2:0:D:/src]` and the block hangs. So a line that *ends* with a well-formed
  sentinel is split into the two it should have been. Ending with one is the discriminator that matters:
  `see [AZUR:1:0:D:\x] in the log` has text after the bracket, and that is a program talking about the
  protocol rather than the protocol arriving late.

Since the sentinel carries the folder, **the pane follows a `cd` with no prompt hook and no polling**
— the shell says where it is after every command as a side effect of saying it finished. Only on the
frame a command finishes, because the pane's own folder is sent as a `cd` before every command, so any
difference at that moment is something the command itself did.

### The keyboard belongs to the panel, not to a field

**There is no `TextEdit` in it**, and that is the single decision the whole panel falls out of.

A `TextEdit` owns the focus it is given. It takes `Enter` and `Tab` before anything around it can look
at them, egui spends `Tab` on focus navigation before user code runs at all, and clicking the log to
select a line takes the focus off the field — so every one of `Enter`, `Tab`, `Shift+Tab`, `Shift+Up`
and a text selection is a separate fight with a widget trying to be helpful. Three rounds of that were
lost before the field was thrown away. **One focus id for the whole panel**, a hand-rolled line
editor, and a key pass that reads the event list in order and removes what it acts on.

Four things make it hold, and each of them was a bug first:

- **`set_focus_lock_filter` with `tab` set** is how a focused widget tells egui it wants `Tab` for
  itself rather than as "move to the next widget". Without it `Shift+Tab` never arrives — it has
  already been spent moving the focus somewhere else.
- **The panel decides who has the keyboard and tells egui again whenever egui has stopped agreeing.**
  egui's focus is not somewhere a claim can be left lying: it drops focus at the end of any frame in
  which the focused id did not put a rect on the record — a dead-man's switch for widgets that have
  gone away — *and* it revokes a widget's focus whenever a click completes while that widget is not
  the hovered one. The panel never is: the row or the field the click landed on is registered over the
  top of it. So a press claimed the keyboard and the matching release, one frame later, took it
  straight back. Which looked like this: the console on screen, the caret blinking in it, and `e`
  jumping to a folder in the rows above. A flag on the panel, and a `request_focus` on any frame egui
  disagrees, ends the argument.
- **Whenever it disagrees, and not every frame — because the first two cancel each other.**
  `Memory::request_focus` builds the focus record from scratch, and a fresh record carries the
  *default* event filter, every field false. So re-stating a claim already held threw the lock above
  away, every frame, before egui had ever read a key through it. A bare `Up` was then read as "move
  the focus up": the keyboard went to whichever rect egui found in that direction — a resize grip, the
  sidebar's, the listing's own hit area — and the panel took it back the frame after. On screen that
  was the selected file lighting up for one frame on every press of `Up` while walking the history,
  and the same for `Tab`, `Left`, `Right` and `Escape`. Claiming it only when it is not already yours
  costs nothing and the lock survives.

  One frame is still egui's: the one a click *completes* in, where it has revoked the panel's focus
  and the panel has not yet asked for it back. Nothing holds the keyboard in it, so `keeps_keys`
  counts "nobody has it" as the panel's — otherwise a key pressed in that frame goes nowhere and the
  listing lights up for it.

  All three cost a round, and all three were found the same way — by printing the focused id every
  frame and pressing keys at it. None of them is visible in the code.
- **Every handled event is taken out of the list.** The window's own key handling runs after the panes
  are drawn and stands down whenever anything holds focus, so consuming is what stops one `Escape`
  from both clearing a selection here and dropping the focus there.

Typing always goes to the prompt, whatever the pointer last touched. What the arrows and a copy are
*about* is one thing at a time: a range dragged out in the log, a block reached with `Shift+Up`, or the
command line. `Ctrl+C` means exactly one of them, and with nothing selected anywhere it stops what is
running — which is what somebody typing `Ctrl+C` at a running command meant.

**And the listing says it has stopped listening.** A window with two panes and a console in one of
them can show three selections at once, only one of which the arrow keys are about — so a selection in
a listing that does not have the keyboard goes quiet, whether the keyboard is in another pane or in
this pane's own console. Quieter rather than greyer: the rows are still selected and the next paste is
still about them, so the fill keeps its colour and loses its emphasis. See
`crate::ui::row_fill_quiet`, which is a candidate to move into the design system as soon as something
other than this window wants it.

The listing asks the panel — `console::State::keeps_keys`, the same call the panel makes before it reads
a key — rather than reading egui's focus for itself. One definition, so a row cannot draw itself
focused in a frame the keys were going somewhere else.

### Selecting text in the log

A single drag selects character by character. A double click selects the run under the pointer, and
dragging on from it extends by whole runs — which is what "select text like an editor" means everywhere
else.

Punctuation is its own run rather than part of a word, and that is the whole of a reported bug: in
`source.cpp` the dot is a run of one and the names either side of it are words, so `cpp` can be
selected on its own. Lumping them together selects `source.cpp` and nothing shorter.

**One character is one column, and a tab is not.** The whole panel is columns — the character under
the pointer is `(x − left) / advance`, the x of a character is the multiplication back, and a copy
slices the row by those same numbers — so a tab is expanded to spaces on the way in and the panel
never sees one. Control characters go the same way, having no glyph to be one column wide.

Skipping that was invisible rather than obviously broken, which is why it took a bug report: epaint
advances a tab to the next stop, so a single leading tab put every character after it four columns
along while this counted one. The highlight is drawn at the column the pointer was over, so it landed
on the glyphs you dragged across — and the copy took the characters at that column *index*, three of
which were the tab's. Drag across `LgsxDetailsWidget.h` on a `git status` line and the clipboard said
`xDetailsWidget.h`: the right number of characters, three along.

Three more things about it were wrong for one round each, and all three are the same class of mistake
— a number that looked right:

- **A double click is decided on the press, not on the release.** `Response::double_clicked` is
  reported when the button comes back up, which is a frame after the drag it would have to govern has
  already started — so word mode never engaged, and a double click on its own selected nothing at all.
- **`Galley::size()` is rounded to whole pixels.** `round_output_to_gui` does it so that a widget
  measured from text lands on the grid, and a rounded advance is wrong by a fraction *per column*:
  Consolas advances 8.4 and the galley reports 8, so by column twenty the arithmetic is a whole
  character out and by column forty it is two. The highlight stopped agreeing with the glyphs, a slow
  drag moved the selection in jumps, and the tail of a long line could not be reached at all — which
  is how `cpp` came to be unselectable in the first place. `Glyph::advance_width` is the unrounded
  number.

### Two edges, and one of them moves

The panel's top border is a resize handle, and it reaches four points either side of the one-point
seam — because a one-point handle is one nobody can hit, and being two pixels low landed on the log
and dragged a text selection across it instead.

**Which gesture a drag is gets decided when it starts, not while it runs.** Dragging the border moves
the border, so by the second frame the band the press began in is somewhere else — ask again and the
answer becomes "that press was in the log", and the output starts highlighting behind the resize. The
same latch is what puts a double click into word mode for as long as the button is held.

Two smaller things fell out of looking at it. The rows' hit rect is not the log: `ScrollArea` places
the first laid-out row wherever the offset puts it, up to a row *above* the panel, so it has to be
clipped or a press in the listing overhead counts as a press on a row. And the band reaches above the
panel, so a press there has to still count as *inside* — otherwise grabbing your own border hands the
keyboard back to the listing, and the `Ctrl+C` after a resize answers "Nothing selected" from
somewhere else entirely.

### Blocks fold, and the silent ones fold themselves

A block that succeeded and printed nothing starts folded: `cd`, `git add`, `mkdir` — the header is the
whole story, and a blank body under each of them is how a log of twenty commands stops being readable.
A silent *failure* stays open, because that is the one worth looking at. Only a running command and a
failed one are marked at all, so the two you care about are the only marks on the panel.

`Shift+Up` and `Shift+Down` walk the blocks, `Shift+Left` and `Shift+Right` fold and unfold the one
that is picked, `Ctrl+C` copies it whole as a transcript, and `Del` throws it away. A block is named by
an id rather than by its position: the cap drops blocks off the front and `Del` takes one out of the
middle, and either would otherwise move a selection silently onto its neighbour.

**The chevron is the only mark a command wears.** A header used to carry an accent `>` as well, in the
gutter right beside it — two marks saying the same thing, and the chevron is the one that also *does*
something. So a block with nothing to fold keeps its chevron too, drawn in `text-disabled`, which is
what every tree says about a twisty with no children. What tells a command from its output is the band
behind it; a copy has no band, so a copy keeps the marker.

`clear` and `cls` are the panel's, not the shell's, and either spelling works in any of them. Handed to
a shell they write the escape sequences that move a terminal's cursor about, and those are stripped
here — so the one command everybody types to tidy up would have done nothing at all.

### It is rows, and the rows are measured

Drawn at arithmetic rects through `ScrollArea::show_rows`, the same discipline as the file listing: a
thousand lines cost the forty on screen. The index is one pass over the *blocks* and never over their
lines — a five-thousand-line block costs the same as an empty one.

Two measurements, because "the text is not aligned" was reported twice and guessed at twice:

- **A row's height is rounded up to a whole device pixel.** Rows sit at `top + n × height`, so a
  fractional pitch puts every row on a different subpixel phase — and epaint rounds a baseline to a
  whole pixel relative to the galley's own origin, so each row got rounded a different way. Sharp
  text, visibly not on one line.
- **Text is placed by its ink, not by its box.** A galley's box is ascent *plus* descent and the ink of
  a line stops at the baseline, so text laid flush in a row leaves all of the slack underneath: ten
  pixels of ink at the top of a sixteen-pixel row and six empty ones below it. The distance between
  the two centres is measured off a laid-out glyph — which carries its baseline and its ink's own
  offset from it — rather than nudged by a constant. Centre the ink, and centring in the row becomes
  the right answer for everything beside it: the fold chevron, the exit code.

**Long lines wrap, and `Alt+Z` says otherwise.** On by default: a line that has run off the right edge
of a panel this shape is a line nobody read, and reaching for a horizontal scrollbar to find the end of
a compiler error is worse than losing the column `ls` and `git status` line their meaning up in. When
that column is the point, one keystroke turns it off and the log scrolls sideways instead.

Which of the two is on changes what the index *is*, which is why it is a mode rather than the behaviour.
Unwrapped, a line is a row: the index is one entry per **block** and a five-thousand-line block costs
the same as an empty one. Wrapped, a line is as many rows as it has slices, which no arithmetic over
the blocks can tell you — so the index becomes one entry per visual row, and it is rebuilt whenever the
blocks, their lengths, their folds or the column count move — and on no other frame, which matters now
that this is the mode the panel opens in: a hash over the blocks says whether anything moved, and a
frame where nothing did does nothing at all. Dragging the resize grip moves the column count, which is
the cost wrapping cannot avoid and the flat model does not have.

A slice is not a line, and a copy knows it: two rows that are slices of one logical line paste without a
line ending between them. A wrapped path with a newline dropped into the middle of it would be worse
than no wrapping at all.

**There is more above and there is more below**, said by a fade at each edge rather than by the
scrollbar — which is a thin thing at the far right of a panel nobody is looking at the far right of.
The rule and the mesh are the design system's, `components::scroll_fades`; what the log adds is its own
two figures, because the row of trailing slack under the last line is not something to fade for.

**A blank row before every block but the first.** Air, and the only thing in this panel that separates
one command's output from the next command — the band behind a header says *this row is a command* and
says nothing about where the block before it stopped. It is a row of the index like any other, so a
drag across it copies the blank line it looks like; it is not part of the block to look at, so a picked
block's fill starts at its header and not one row above it.

**The scrollbars are not part of the text.** Everything here — the row fills, the hit-testing, the
fades, the clip — happens inside the *page*, which is `ui.clip_rect()` taken inside `show_rows`: the
viewport left after egui reserved a gutter for whichever bars it is showing. Painting against the
panel's own rect instead is what made the bars behave like text, with a caret for a cursor and a
selection dragged along behind the scroll — the same mistake as the resize border, one layer down.

A gutter is reserved on whichever axis egui did *not* reserve one, and there is a row of trailing scroll
slack under the last line. Both are about the same moment: a horizontal scrollbar appears exactly when
a long line arrives, which is the worst time to reflow the text and to put a bar over the line somebody
scrolled down to read.

**Both of those are for the moment a horizontal bar arrives, so wrapped there are neither.** Lines that
wrap are exactly as wide as the page, and no long line can ever make a bar appear — so nothing is held
back at the foot: the log reaches the panel's bottom edge, the fade that says there is more below starts
there, and the slack row goes, because a strip of nothing under the last line is all it would be. The
sideways gutter stays either way, since a *vertical* bar comes and goes with the length of the log, and
that one moving while wrapped would rewrap every line in it.

The caret is **egui's own** — `visuals.text_cursor`, colour, width, blink and the repaint it schedules —
because this window has one caret whatever is drawing the text under it. Twenty points of it, centred in
the strip rather than filling it: a caret as tall as its row reads as a bar. Its phase is measured from
the last edit rather than from the epoch, because a caret that carries on blinking through what you are
typing is one that is on a timer rather than on the text.

### Stop is a kill, and this is the one real compromise

`GenerateConsoleCtrlEvent` signals a console *process group*, and this program is a `windows`
subsystem binary with no console to have a group in — so **there is no way to deliver a real `Ctrl+C`
from here.** What there is is a job object holding the shell and everything it started, and ending
that is reliable, immediate and total.

So Stop kills the tree and starts a new shell. The working directory survives, because the sentinel
has been reporting it all along; what is lost is anything `export`ed by hand. A killed program does
not get to clean up, and that is the honest cost of not having a pseudoconsole.

### What it cannot do

A pipe is not a terminal and every program can tell:

- **Nothing interactive.** `git rebase -i` opens an editor that is not there; `ssh` asks for a password
  nothing can type. Both hang until Stop. What can be turned off is: `PAGER=cat` and `GIT_PAGER=cat`,
  or a plain `git log` would hang in `less`; `TERM=dumb`; and `GIT_TERMINAL_PROMPT=0`, which turns a
  credential prompt into an error message you can read.
- **No colour**, and here that is a feature — every one of these tools tests `isatty` and prints plain
  text to a pipe. What arrives coloured anyway is stripped, because a panel full of `[32m` is worse
  than a panel with no colour in it. Standard error is coloured by *this* program instead, which is
  one place it beats a terminal: a real one interleaves the two streams and you cannot tell them apart
  again.
- **A carriage return overwrites the line**, so `cargo`'s progress bar is one line rather than three
  hundred. It replaces the line wholly rather than the characters it covers, which is what a log
  viewer should do and not quite what a terminal does.
- **Output is capped** at 5,000 lines a block and 200 blocks, and the block says how many lines it
  dropped. A stray `find /` must not be the reason this process ran out of memory.
- **Double-width characters are one column.** Every column in the log is one monospace advance wide,
  which is what makes the character under the pointer a division and the x of a character a
  multiplication — no galley laid out to hit-test a row. Tabs and control characters are expanded on
  the way in so that they hold to it; what is left is scripts that are genuinely wider than one
  column, which is the same trade a terminal makes.

## Keys

| | |
| --- | --- |
| `Ctrl+T` / `Ctrl+W` | new tab / close tab |
| `Ctrl+Shift+T` | reopen the last closed tab |
| `Ctrl+Tab`, `Ctrl+1`…`9` | switch tab |
| `Ctrl+\` | split to the right |
| `Alt+←` / `Alt+→` / `Alt+↑` | back / forward / up |
| `Backspace` | up |
| `Ctrl+L`, `Alt+D` | edit the path |
| `↓` / `↑` | in the path field: walk the completions, and put them up if they are down |
| `→`, `Tab` | in the path field: take the highlighted name and the separator after it |
| `Ctrl+F`, `F3` | filter — `F3` only when nothing else has the keyboard |
| `Ctrl+E` | flatten this folder's whole tree — works from inside the filter box |
| `→`, `←` | in a flattened **tree**: open the folder under the cursor, or shut it — and `←` on anything else steps out to the folder it is in |
| `F5`, `Ctrl+R` | refresh |
| `Ctrl+A` | select all |
| `Ctrl+H` | show hidden files |
| `Ctrl+D` | bookmark this folder |
| `Ctrl+P` | preview the selection — works from inside the filter box, like `Ctrl+E` |
| `Ctrl+X` / `Ctrl+C` / `Ctrl+V` | cut / copy / paste |
| `Delete` / `Shift+Delete` | recycle / delete permanently |
| `F2` | rename |
| `Ctrl+Shift+N` | new folder |
| `Ctrl+Shift+C` | copy the selected paths as text |
| `Ctrl+²` | show or hide this pane's console — also the switch at the left of [the status line](#the-status-line) |
| `↑` `↓` `PgUp` `PgDn` `Home` `End` | move the cursor (`Shift` extends) |
| `Enter` | open |
| any letter | jump to the next name starting with it |

Middle-clicking a folder, a breadcrumb segment or a sidebar entry opens it in a new
tab; middle-clicking a tab closes it.

Inside the console the keyboard is the panel's, and these are its:

| | |
| --- | --- |
| any letter | goes into the command line, whatever the pointer last touched |
| `Enter` | run it, and keep the keyboard |
| drag in the log | select by character |
| double click | select a run — a word, a space, or a mark; drag on to extend by runs |
| `↑` `↓` | walk the command history |
| `Shift+Tab` | switch shell — bash, pwsh, cmd, round |
| `Shift+↑` `Shift+↓` | walk the blocks; off the newest comes back to the prompt |
| `Shift+←` `Shift+→` | fold and unfold the block that is picked |
| `Ctrl+C` | copy the picked block, the dragged selection, or the command line — and with nothing selected, stop what is running |
| `Del` | throw the picked block away |
| `Ctrl+L`, or `clear` / `cls` | clear the log |
| `Alt+Z` | stop wrapping long lines, or start again |
| `Escape` | let go of one thing: the dropdown, then the selection, then the line |
| `Ctrl+²` | and out again |

## Git, by asking git

A folder in a repository says so: the branch and how far it is from its remote in the status line,
and what each row's own file is on the row.

| | |
| --- | --- |
| branch, or the short commit when the head is detached | `info`, amber when detached |
| `↑` commits the branch has that its upstream does not | `success` |
| `↓` commits the upstream has that the branch does not | `danger` |
| `N changed` — paths git has anything to say about, and [a button](#the-status-line) that lists them | `warning` |
| a clean tree | a tick, `success` |

Those four are `theme::Bar`'s and not `status.*` directly, because the bar is a rung off the surface
those roles were drawn for — see [One baseline, and one pixel grid](#one-baseline-and-one-pixel-grid),
where the figures are. Hovering the group names the upstream and gives the whole breakdown, which is
the one thing `↑2 ↓1` cannot say for itself: what it is counted against.

**The colour convention is posh-git's**, which is the one most people already read every day: green
for what is in the index, red for what is not, cyan for the branch — the nearest role Azur has being
`info`. Three departures, all because a status line is not a prompt. Changed is **amber, not red**:
posh-git's working-tree colour is `DarkRed`, and red in this line is where the window says something
has gone *wrong*, while having uncommitted work is the normal state of working. Untracked is
**grey**: it is the state of every build artefact on the disk, and alarming about those trains people
to ignore the colour. And staged is **amber too, not green** — green is the colour of nothing left to
do, and a staged file still has a commit owed on it, so it sits with the rest of the uncommitted work.

Per row, a badge in the icon's bottom-left corner — the shell's own corner, at the shell's own
proportion, which is where an eye trained on Explorer already looks:

| | | |
| --- | --- | --- |
| ● green tick | tracked and the same as HEAD | the only mark most rows wear |
| ● amber plus | staged, and new | the plus is what tells it from a plain change |
| ● amber | changed on disk, staged or not | plain, because it is the state a tree is usually in |
| ● red minus | gone from disk, still in the index | |
| ● blue arrow | moved or copied, and git saw it | |
| ○ grey ring | untracked, and not ignored | hollow: nothing is *in* git |
| ● red `!` | a merge left both sides in it | outranks everything |
| nothing | ignored, or not in a repository | |

A folder wears the strongest thing under it, however deep, so a change three levels down is visible
from the top without opening anything. Ignored files wear nothing at all, which is what makes
`target/` quiet.

### What changed in this file

The text preview shows it, on the file itself: the lines the working tree gained on a green band, the
ones it lost put back on a red one. Two buttons in the panel's own title bar, and one of them only
appears when there is something to show — a control that does nothing is worse than no control, and a
`±` appearing on the bar is itself the news that this file has changed.

| | | |
| --- | --- | --- |
| `±` | show what changed | **on** by default |
| the folded rows | leave out the stretches that did not | **off** by default |

Opposite defaults, and the same argument decides both. A file being previewed inside a repository is
nearly always one somebody is working on, and no other view answers *what did I change here* without
leaving the window — while collapsing hides most of the file, which is right when reviewing a change
and wrong when reading a file that happens to have one. Three lines of context are kept either side,
which is git's own `--unified` default.

**The diff view is a body of its own, not annotations over the file**, and both halves of the feature
need that. A removed line is not in the file: it has to be put back to be shown, numbered by where it
was in `HEAD` and in the danger ink, so a gutter that suddenly counts backwards cannot be read as one
of the file's own lines. And a collapsed region means most of the file's lines are not shown at all.
Everything downstream then works unchanged — the layout job, the find bar, the syntax colouring —
because all any of them ever sees is *a* string. Two consequences worth stating: a search searches
what is on screen, so it finds a removed line while the diff is on, and a lexer reading a collapsed
body resumes at each visible stretch, so a string or a block comment spanning a hidden region is
coloured from where the text comes back. That is a limit of showing part of a file rather than a bug.

`git diff HEAD -U0` is where it comes from — `-U0` because the file itself is already on screen, so a
hunk of context around every change would be most of a large file for nothing. Read on the same worker
and in the same answer as the file: the diff and the text it describes are one fact about one moment,
and fetching them separately would let a file arrive with the diff of the version before it. Three
details of the format, each of which was a bug first:

- **A count of one is left out.** `@@ -2 +2 @@` is one line replaced by one; only `@@ -2,3 +2,5 @@`
  spells the count.
- **The zero-count side means something else.** With no new lines, the new-side number is the line the
  removal *follows*; with new lines, the removal is shown in front of them. Reading it as a line
  number either way puts every deletion one row out.
- **`--- a/path` starts with a minus.** A parser that does not wait for the first `@@` reads the file's
  own name as a line that was deleted.

`--no-ext-diff` and `--no-color`, because a configured `diff.external` answers in its own format and a
configured `color.diff` answers in escape codes. Both are somebody else's preference about reading a
diff, and neither survives being parsed.

**A band covers every row of a wrapped line.** A line that wraps is several rows, and banding only the
row that *starts* it left the rest of an added paragraph bare — so the line's own answer is carried
across its continuation rows, while the number and the collapsed-region label stay on the first, the
way an editor's gutter numbers a wrapped line once.

### A row is as tall as the ink in it

Every band in this view is drawn on the **row**, and one of them is not this program's to move: egui
paints a text selection as vertices in the row it belongs to, spanning its full height, and so does a
search hit's background. A diff band could be offset by hand to sit on the ink — and was — but that
moved one of the three, and a selection band two points off a diff band is worse than either being off
alone.

So the *row* is made to fit the text instead. A line box is not symmetric about its ink and has no
reason to be; measured on the two faces this view uses, at 14 points:

| | monospace | proportional |
| --- | --- | --- |
| line box | 16.00 | 19.00 |
| baseline — the face's ascent | 10.00 | 16.00 |
| ink, from the top of the box | 1.00 … 13.00 | 6.00 … 19.00 |
| **the height whose middle is the ink's** | **14.00** | 25.00 |

That last row is what the view asks for as its line height, and the arithmetic behind it is one fact:
**the baseline stays where the face puts it whatever the line height says** — epaint's `valign` cannot
move it, because a row's height and its sections' line heights are the same number and the term
cancels. Changing the height therefore moves the *box* around the ink rather than the ink inside the
box, and `top + bottom` is the height that lands the box's middle on the ink's.

**And it only ever tightens.** Monospace comes down from 16 to 14, which centres it — the code view's
lines are two points closer together than they were, which is the price. Proportional would have to go
the other way, since its ink already reaches the bottom of its box, and 25 points for a 14-point
paragraph is a line and a half of leading — so prose keeps its own line box and its bands stay a little
low. One rule, applied where it makes a row fit its text and declined where it would make a paragraph
fall apart.

The two-point offsets that used to be applied to the diff band by hand are gone with it: a band is the
row, and the row is the ink.

### A picture, and the one in the last commit

A `.png` has no lines to put a red band behind, so `±` means something else on a picture: **the two
versions and the difference between them**, which is the view [two selected
pictures](#two-pictures-compared) already get. `last commit`, `working copy`, `differences` — one zoom,
one pan, and the same `%differs` figure on the bar. The nested toggle drops to the difference alone.

It is the same preference behind the same button, because it is the same question. What is different is
that it changes *what is read*: the panel asks for `Ask::AgainstHead` instead of `Ask::One`, so turning
the toggle off has to be a different question or the panel would go on showing the comparison it is
holding.

`git cat-file --filters HEAD:./name` is where the older version comes from. `./` so git resolves the
path against the process's own directory, which the command is already run in — one less thing to get
wrong in a worktree or a submodule. The bytes are decoded from memory rather than written out and read
back: they are already in memory, and a preview that left temporary files behind would be a preview
that left temporary files behind. Both bounds a file gets, a blob gets too — the megapixel cap and the
scale to `CAP` are inside the decode rather than beside the file-opening.

**`--filters`, and not `git show`**, which is what this asked first and what made it "works for `.svg`
and not for `.bmp`". `show` hands back the **stored object**, and a stored object is not always a file.
Two things stand between them, and both are ordinary configuration in any repository big enough to hold
pictures:

- **Git LFS.** What is committed is a pointer and the picture lives beside the repository. Measured on
  the repository this was reported from: `show` gave **129 bytes** of
  `version https://git-lfs.github.com/spec/v1` where the file is **154,544**. Nothing decodes that, so
  the panel fell back to one picture — and whether a given extension goes through LFS is a line in that
  repository's `.gitattributes`, which is the whole of why one format worked and another did not.
- **End-of-line conversion.** With `* text=auto` and `core.autocrlf=true`, git converts anything it
  *decides* is text, and its decision is "no NUL in the first 8 KB" — which a 24-bit bitmap of a
  photograph can pass. The blob then has `\n` where the file has `\r\n`, and every pixel row after the
  first is shifted.

`--filters` runs the smudge filter and the eol conversion, which is to say it answers the question
actually being asked: what would this file *be* if it were checked out. It is also why the path matters
to the command and not only to the lookup — filters are configured per path. The test stands eol
conversion in for LFS, since it needs no external program to set up and the distinction being checked is
the same one.

Whether the button appears at all is read off **the marks the listing already has**, so the badge on
the row and the button in the panel cannot disagree, and asking costs a hash lookup rather than a
process. Three of the seven states have something to compare with — staged, modified, conflicted — and
the two that look like they should are the interesting ones: an **untracked** file and a **renamed**
one both differ from what is committed and neither has a path in `HEAD` to look up, so a comparison
against either would be a comparison against nothing. Every path through it ends in a picture anyway:
the file is there, and somebody asked to see it.

### Why the program and not a library

`libgit2` (via `git2`) and `gitoxide` are both real options, and this takes neither.

- **`git status` is the hard part, and git is the fastest thing that does it.** A status is a walk of
  the worktree against the index, and git has spent twenty years making that cheap: the untracked
  cache, the index's stat data, `core.fsmonitor` where it is switched on, the sparse index. A library
  reimplements the walk and gets none of the user's own configuration for it.
- **It is the same answer the user's own tooling gives.** Their git, their config, their `.gitignore`
  chain, their worktrees, their submodules. A second implementation is a second set of answers, and
  the first time those differ is the last time this window is believed.
- **Everything deeper is one more command.** Log, blame, diff, stash, a commit: an argument list
  rather than an API to grow into.

What it costs is a process per question. Measured, on this repository:

```
cargo test --release -- --ignored --nocapture what_a_git_query_costs
```

| | |
| --- | --- |
| a folder in a repository | **113 ms**, two `git` processes started together |
| the same, before the measurement | 240 ms, three processes run one after the other |
| a folder with no repository above it | **0.23 ms**, and no process at all |

Both halves of that were bugs the number found. The third process was `rev-parse --show-prefix`,
asking git where the folder sits inside the repository — an answer already in hand, since the walk
that found `.git` found the root, and on Windows a `git` that does nothing at all still costs about
80 ms of startup. And the two that remain had been run in sequence, which is two startups for no
reason: neither answer feeds the other.

Which leaves ~110 ms per folder, off the UI thread, in a window whose own listing takes 0.3 ms. It is
by far the most expensive thing here, and it is the floor for this approach: it is one git startup.
The part a library would genuinely win is a hot loop over object contents — a graph walk, a diff per
file — and if this window ever draws a commit graph, that part can move to `gix` without any of the
rest changing: the answer would still arrive as one `Repo` on the same channel.

### There is no cache, and that is the feature

TortoiseSVN's overlays are the cautionary tale: a status cache with its own lifetime, its own
invalidation and its own opinion of when to believe itself — and the failure mode is a green tick on a
file you just changed, which is worse than no tick at all.

So nothing here outlives the listing it belongs to. A `Repo` is asked for when a folder's listing
lands, it hangs off that tab's view of that folder, and it dies with it. Every event that re-reads a
folder asks git again, because it is the *same* event: a file operation, `F5`, a navigation, the
watcher noticing somebody else's change. One source of truth, one lifetime, and no third state where
two of them disagree.

What keeps that affordable is not caching but **not asking**: the two or three `stat` calls above,
and only the active tab of each pane — a window with twelve tabs open would otherwise start twelve
`git status` runs on every navigation, eleven of them for folders nobody is looking at.

**And `.git` is watched as well as the folder**, which is the one gap the design has to close on
purpose. A commit, a pull, a branch switch or a rebase changes what git says about a folder without
changing the folder, so watching the folder alone would leave every mark describing the last read with
nothing able to disprove it — a `git commit` in the console panel would leave its own files looking
modified. A change under `.git` asks git again and leaves the listing alone: the working tree did not
move, so re-reading it would be a scan for nothing.

### Reading porcelain v2

`--porcelain=v2 --branch -z`, which exists so that programs can read it and is documented as stable.
Three details, each of which was a bug first:

- **A rename is two records.** `2 R. … R100 <new>\0<orig>` — the second path is its own
  NUL-terminated field, so a parser that does not consume it reads "where it came from" as the next
  status line.
- **The path is whatever is left.** `my notes v2.txt` is one field with spaces in it, so the fields in
  front of it are *counted* rather than split on. Getting that count wrong by one is silent: every
  path comes out `None` and the listing simply wears no marks.
- **The oid comes before the head.** A detached head has no branch name to show, so it wears the
  short commit — which arrives on a line already gone past. Reading them in order and letting
  `(detached)` overwrite what the oid line set is the version that left a detached head nameless.

Every one of those has a test against real captured output, and one test builds a repository in the
temp directory and reads it with the developer's own `git` — which is what catches the two commands
whose output is *not* checked in: `ls-tree`, where a committed file's tick comes from, and the prefix
that decides what every path means.

## Why it is fast

Almost none of it is clever code. It is per-file costs that this program declines to
pay — and the two that matter are enormous. All of these are measured, not estimated,
over 60,000 files in a warm directory:

```
cargo test --release -- --ignored --nocapture scan_speed
```

| doing the same work | per entry | vs this scanner |
| --- | --- | --- |
| this scanner | 657 ns | — |
| `std::fs::read_dir` | 690 ns | 1.05× |
| `read_dir` + a `stat` per entry | 133 µs | **202× slower** |
| `SHGetFileInfo` for one type name | 1.1 ms | **1700× slower** |

The interesting row is not the first one. **The choice of enumeration call barely
matters** — `std::fs::read_dir` is within 5%, and a claim of a four-times speedup from
`FindFirstFileExW` would not survive a measurement. What matters is what happens
*after* the read:

1. **Nothing is asked about twice.** The directory read already gave us each name,
   size, timestamp and attribute word. Reaching for `Path::is_dir` or `fs::metadata`
   on top of that turns one sequential read into a round trip per file, and costs
   202×. Nothing here stats an entry it has already enumerated.
2. **The shell is never asked what a file is.** `SHGetFileInfo` with `SHGFI_TYPENAME`
   is a registry walk per file — 67 *seconds* for that folder. A static table of
   extensions gives the same answer in 45 ns.
3. **A listing is two allocations**, not one per entry: every name in one `String`,
   every record in one `Vec` of 32-byte structs. A natural-order sort of 60,000 of
   them takes 2.8 ms, because it sorts 4-byte indices and never touches a string
   except to break a tie.
4. **Only visible rows are drawn**, through one widget for the whole list rather than
   one per row, formatting into a buffer that is reused. A frame costs what the
   *window* is worth, never what the folder is.
5. **Icons are Explorer's, cached by file type.** These are the real shell icons out of
   the system image list — but asked for with `SHGFI_USEFILEATTRIBUTES`, so the shell
   answers from the *name* and never opens the file, and keyed by extension, so a folder
   of ten thousand `.rs` files performs **one** lookup. A type icon measures at under
   500 ns, against the 1.1 ms a naive `SHGFI_ICON` per row costs. Only the handful of
   kinds that carry their own icon — `.exe`, `.dll`, `.lnk`, `.ico` — are asked about
   per file, and those are resolved on workers with the generic icon showing until the
   answer lands.

### The icon call that froze the window

Everything above was true and the window still froze for seconds on a mapped network share.
Every *lookup* was on a worker; the thing that was not is the step nobody thinks of as a shell
call at all — pulling the bitmap out of the image list once the index is known.

`IImageList::GetIcon` looks local. It is not: for an icon the shell resolved from a file on a
share, it goes back over the network to extract it. Measured on `H:`, in one folder:

| | |
| --- | --- |
| `kind .png` (worker) | 24 ms |
| `place <folder>` (worker) | 870 ms |
| four ordinary bitmaps | 2 to 31 ms each |
| **one `.ico` bitmap** | **2.54 s** |

That last one ran in the frame, on the UI thread, inside `Icons::uv`. Two and a half seconds of
a window that takes input and draws nothing, which from the outside is a hang.

It has a thread of its own now — its own, rather than a share of the lookup thread, so one
slow extraction cannot hold up the type lookups every ordinary row is waiting on. `uv` asks and
answers `None`, exactly as the type lookup does for a type never seen before, and the row draws
its painted glyph until the pixels arrive. Measured with `SendMessageTimeout(WM_NULL)` against
the window's own thread, on two equally cold folders of the same share:

| | longest stall |
| --- | --- |
| bitmap in the frame | **3.11 s** |
| bitmap on a worker | **0.07 s** |

**And leaving a folder now cancels its questions.** The per-file lookups queue, and each one
opens a file — on a share, slowly. Changing folder used to mean the new folder's icons waited
behind every question about the old one; the worker now drops a question whose view is no longer
on screen before paying for it. There is a test that leaves a folder and checks the answer never
comes, and another that checks a live view still gets its own.

Two more things keep frames honest rather than fast:

- **A synchronous cache probe before any request.** Back, Forward and revisiting a
  folder are not asynchronous at all — the listing is on screen in the same frame as
  the click, with no spinner and no flash of empty. Moving the cursor onto a folder
  quietly prefetches it, so `Enter` is usually already answered.
- **The one genuinely slow call is off the startup path.** Asking a mapped network
  drive that is not currently reachable for its volume label takes **22 seconds** on
  this machine, in one blocking syscall. The sidebar lists drive letters immediately
  (microseconds, no I/O) and describes each volume on its own thread, so a stalled
  share delays one row instead of the window.

The status line shows how long the current folder took to read. That is not
decoration: a program that claims to be fast should be willing to be checked, and a
folder that suddenly takes 200 ms is how you find out something is wrong.

### Settings, and not writing over somebody's

`YAFE_PROFILE=<dir>` replaces the settings directory. It exists because of a bug rather than
for convenience: driving this program with real mouse and keyboard input means launching it for
real, and a real launch writes real settings on the way out — so a test run replaced the
user's window size, open tabs and bookmarks with a fixture's.

`cargo test` was doing it too, and that one was worse. A test that adds a bookmark or changes
the theme marks the settings dirty, and the frame it runs in writes them; the suite passed every
time while quietly emptying the bookmark list. `Config::save` now returns early under
`cfg!(test)` — a test process has no business writing a user's settings under any
circumstances — and every save keeps the previous contents in `config.ini.bak`, which is what
made the loss recoverable at all.

### What browsing keeps

A cache holds what you might come back to, so "memory grows as I browse" is the expected
shape until it settles — which makes an actual leak hard to see. So it is measured rather
than argued: `app::click_tests` walks a few hundred real folders with a counting global
allocator installed, alongside the process's own private bytes and its GDI and USER handle
counts, and asserts the numbers plateau.

```
cargo test --release browsing_hundreds -- --ignored --nocapture --test-threads=1
YAFE_WALK=C:\Windows cargo test --release browsing_hundreds -- --ignored --nocapture --test-threads=1
```

The handle counts are in there because a leaked `HICON`, `HBITMAP` or `HDC` costs memory
without a single Rust allocation, and this program asks the shell for an icon per file type it
meets. Two things came out of running it:

- **A finished listing is shrunk to what it holds.** `DirBuilder` reserves 8 KB of names and
  256 records up front, which is exactly right for building — a folder of a few hundred files
  fills without one reallocation — and exactly wrong for holding, because the listing then
  goes into a cache that keeps scores of them. 96 cached folders came to **1.6 MB while
  holding about 300 entries between them**. One `shrink_to_fit` per folder read, on the worker
  thread that did the reading, took the same cache to **0.45 MB**.
- **The budget was eight times what a session uses.** The cache is bounded in *entries*, since
  60 small folders and 60 huge ones are not the same thing, and the bound was 1,200,000. An
  entry measured **140 bytes** over `C:\Windows\WinSxS` — 27,636 entries for 3.8 MB, a folder
  whose names are long — so that bound was 160 MB of listings nobody was going to look at
  again. The point of the cache is that Back, Up and stepping back into the folder you just
  left are instant, and that is a working set of a dozen folders. It is **150,000 entries and
  32 folders** now, about 21 MB at that rate.
- **Every per-file icon answer now belongs to the folder that asked.** This is the rule the
  design was overhauled to keep: *anything allocated because of a folder is freed when you
  leave it.* Per-file icons broke it worst. They lived in a `HashMap<PathBuf, i32>` on the icon
  service — the obvious design, and a leak with a cache's manners: scrolling System32's 4,910
  rows put **4,345 entries** in it, a heap-allocated path per executable ever seen, none of
  which went away when you left. Twenty folders like that is 87,000 paths.

  A request now carries the *view* that asked and the *row* it asked about, and the answer
  comes back addressed the same way; the tab holds a `Vec<i32>`, four bytes a row, sized when
  the listing lands and dropped when the tab moves. An answer that arrives after you have gone
  finds no view to belong to and is discarded. The same scroll: **+8.7 MB → +1.4 MB**, and
  what remains in the service is 7 file types, 13 sidebar places and 12 bitmaps.

  It also stopped building a `PathBuf` per visible row per frame — 1,500 allocations a second
  at 60fps — because a row's icon is now an index into its own view's column.
- **One worker thread instead of one per question.** Per-file lookups spawned a thread each:
  4,910 of them for that folder, every one with a stack. They are a bounded queue in front of
  a single thread now. A dropped question costs a row the generic icon until it comes round
  again, which is cheaper than remembering every path in order to avoid asking twice.
- **The texture map is bounded too**, by least-recently-used with a two-minute stale sweep —
  a `TextureHandle` is a GL object with the driver's own cost behind it, invisible to any Rust
  allocator counter.
- **Listings are dropped when they go cold**, on the same two-minute rule, so a session that
  browsed a hundred folders and then settled on one gives the memory back rather than holding
  its high-water mark until the window closes. A folder a tab is still showing stays alive
  through its `Arc` regardless, which is right: what is on screen is not stale.

Measured after all of it, driving the real window through 380 folders of `C:\Windows` at four a
second: **182 MB to 200 MB, then flat**, with the caches at their caps. A 27,636-entry folder
costs 3.8 MB and gives all of it back when you leave it. Dropping `dll` from the per-path list
was tried too — it cut the questions from 4,345 to 710 and did not measurably cut the memory,
because that cost is the shell's own cache rather than this program's, so the fidelity was not
worth spending.

### The one that was not ours

Scrolling a single folder up and down — the same rows, the same icons, the same text, over and
over — grew the process by **7 MB a second**, and none of it came back on leaving the folder.
Every cache was pinned while it happened: 1 folder cached, 23 icons, GDI flat. The counting
allocator showed the Rust heap flat too, because it is not Rust memory.

`examples/spin.rs` is what found it: forty lines of eframe with nothing of this program in it.

| what it draws, per frame | private bytes over 30 s |
| --- | --- |
| 40 textured quads, then text | **flat** |
| the same 40, interleaved with the text | **+190 MB** |
| the interleaved version under `wgpu` | **flat** |

egui begins a new draw call whenever the texture changes between primitives, and `egui_glow`
— or the OpenGL driver under it — leaks a couple of kilobytes per draw call per frame. Text,
fills and painted glyphs all come out of egui's font atlas; a shell icon is the only thing in a
row that does not. So drawing an icon inside the row loop split the frame at every row, and
forty rows became eighty draw calls.

Two changes, and both are things a hand-painted listing should have been doing anyway:

- **One atlas for every shell icon.** A 32×8 grid of 16px cells in a single 512×128 texture,
  slots reused least-recently-drawn first. It was a texture per icon, so a frame broke once per
  distinct icon on screen; now the icons cannot break it at all among themselves.
- **Icons drawn in one run**, held back from the row loop and flushed after it — in the listing
  and in the sidebar. Invisible: an icon sits in its own column, over the row fill and clear of
  the text.

That took **7 MB/s to 1.16** — it cut the draw calls, which is what the leak is charged per.
What removed the rest was changing backend.

`wgpu` does not have the bug, and the backend it runs on decides the baseline, which is the
number this program has a budget for — and, it turned out, decides whether the frame reaches the
screen intact at all:

| | private bytes at rest | scrolling one folder | survives composition |
| --- | --- | --- | --- |
| `glow` | 180 MB | +1.16 MB/s | — |
| `wgpu`, D3D12 | 443 MB | flat | **no** |
| **`wgpu`, Vulkan** | **426 MB** | **flat** | **yes** |
| `wgpu`, OpenGL | 187 MB | flat | **no** |

For a long time this window ran **`wgpu` over OpenGL** on the first three columns alone: the same
driver `glow` was using, reached through a painter that does not leak, for 134 ms more to the
first frame and a rendering that is pixel-identical — 25 pixels of 302,400 differ on a
screenshot, all of them antialiasing, measured flat across a full minute of continuous scrolling.
The fourth column is what moved it to Vulkan, and
[the blur that no screenshot could show](#the-blur-that-no-screenshot-could-show) is that story.

Three escape hatches, all named for the design system rather than for this program, because all
three Azur applications read the same ones — the choices live in `azur_egui_theme::render` and
what they protect is the token set:

| | what it does | why it exists |
| --- | --- | --- |
| `AZUR_GLOW=1` | back to the leaking OpenGL painter | a machine where `wgpu` will not start at all |
| `AZUR_BACKEND=gl` \| `d3d12` | another backend, still `wgpu` | a window that comes back from sleep unable to draw — an OpenGL context is the most fragile of the three across a suspend or a GPU switch — and telling this program's fault from a driver's, since a window soft on Vulkan too is soft for another reason |
| `AZUR_ADAPTER=low` | render on the integrated GPU | about 100 MB lighter, and lossless *here*; whether it is on another machine depends on how the displays are wired, so it is opt-in |

Whether it *is* that, this program now says rather than leaves you guessing. wgpu reports a lost
device exactly once and only to somebody who asked, so it is asked: a lost device, a lost
surface, or a frame that could not be presented for any other reason all name themselves in the
status line and on the console. A window that cannot present goes on taking input and showing
the last thing it drew, and without a word from it that is indistinguishable from a hang.

Worth stating plainly: the icon atlas and the batching are still worth having — they are how a
hand-painted listing should draw, and they are what makes the fallback bearable — but the leak
itself was never this program's, and no amount of restructuring here would have closed it.

### The blur that no screenshot could show

The window was sharp, and then a few seconds after the last touch the whole of it went soft —
like a photograph saved too small — and a few seconds after the mouse moved it came back. Under
ten seconds to fail, seconds to recover, on every external monitor tried, and on an AMD machine
never once.

Every screenshot of it looked fine, which is what made it hard. Worse than fine: a series of
captures taken across the failing window and the healthy one measured *identically*, to three
decimal places, which reads as proof that nothing is wrong. It was proof of the opposite. **A
screen capture forces the compositor to compose**, and composing is the broken path, so every
capture landed on it — including the ones taken while the screen looked sharp. The instrument was
manufacturing the fault it had been sent to find, and reporting it as uniform.

What broke the deadlock was a reference the compositor never touches: `--shot`, which reads the
frame back off the GPU. Against a capture of the same window on screen:

| backend | pixels differing | hairline of amplitude 29.99 | edge pixels altered |
| --- | --- | --- | --- |
| `wgpu`, OpenGL | 4.85% | **22.99**, leaking into the row below | 68.5% |
| `wgpu`, D3D12 | 4.84% | — | — |
| **`wgpu`, Vulkan** | **0.37%** | **29.99**, intact | **2.8%** |

The frame itself is not the variable. All three backends render the same 1024×600 image, and
D3D12 and Vulkan differ from each other by 0.000% of pixels. What differs is the trip to the
screen.

Fitting the difference says what the trip does to it. Not a shift — no integer offset improves
it. Not gamma and not alpha — flat areas and mean luminance are untouched. It is a **vertical
resample**: the offset drifts from 0.10 rows at the top of the frame to a full row at the bottom,
and the residual falls to *zero* wherever it lands on a whole row. That is a one-row size
mismatch, stretched to fit. It reproduces at every window height from 596 to 604, so there is
nothing to dodge by choosing a size.

What it costs is specific, and it is specific to this interface: a hairline loses a quarter of its
contrast, a line moves by up to a row, and the faintest ones — the seam at ΔL\* 6 that
[one surface, divided by lines](#one-surface-divided-by-lines) argues for — vanish. A design
system specified in single pixels does not survive being resampled, which is exactly why the
backend is `azur_egui_theme::render`'s decision and not this program's.

The intermittence was the last piece, and it follows from the rest. A window presenting frames
continuously reaches the display on a path that skips composition, and on that path it is
correct. A window that paints on demand stops presenting the moment you stop moving; the
compositor takes it back; the resample arrives. Hence under ten seconds to fail, seconds to
recover, and a screenshot that triggers it instantly.

Vulkan costs about **200 MB** of private bytes over OpenGL on this window — 181 MB against 398 in
one run and 216 against 413 in another, the spread being what the window had been doing rather
than the backend. That is a great deal to pay for a compositor's arithmetic, and it is still the
right way round. A listing that goes soft whenever nobody is touching it is soft nearly all the
time, and a program that renders the design system and then has it resampled away has not
rendered it.

The fix is one line, and it is not in this program: the backend is
`azur_egui_theme::render::backends()`, so every window built on the design system gets it. What
proves it stuck is the same comparison, run both ways — **0.39%** of pixels differing on the
default against **4.82%** with `AZUR_BACKEND=gl` forced back, which is also the check that the
measurement can still see the fault it was built for.

A running window will report the same figures with `--trace`, every three seconds, beside what
the cache is holding — the two together say whether growth is the cache filling up or
something being kept:

```
    4.1  private  177032 KB   gdi    63   user   31   cache   2 folders /     5039 entries
```

That baseline is mostly the GL driver and the shell's own DLLs, not this program's data.

## The shape of it

| module | responsibility |
| --- | --- |
| [`fs/`](src/fs/) | everything that touches the disk, and nothing that touches the screen |
| [`loader.rs`](src/loader.rs) | scans on worker threads, with an LRU cache in front |
| [`pane.rs`](src/pane.rs) | tabs: where they point, how they are sorted, what is selected |
| [`dock.rs`](src/dock.rs) | the binary tree that arranges panes |
| [`ui/`](src/ui/) | painting, at explicit rects |
| [`app.rs`](src/app.rs) | state, and the single place anything changes |
| [`theme.rs`](src/theme.rs) | Azur's roles, plus the file-kind hues |
| [`icons.rs`](src/icons.rs) | painted glyphs on a 16-unit grid, for the fallbacks |
| [`shell/`](src/shell/) | the parts that *are* the shell: icons, clipboard, `IFileOperation`, `IContextMenu` content, OLE drag and drop |
| [`ui/menu.rs`](src/ui/menu.rs) | drawing that content as our own menu |

Two structural decisions are worth knowing before reading it:

**Nothing mutates during drawing.** A pane's breadcrumb can close a tab in another
pane; the sidebar navigates whichever pane has focus; dropping a tab restructures the
very tree the drawing loop is walking. None of that can be done with a `&mut` in
hand — so the drawing code pushes an [`Action`](src/app.rs) and `App::apply` performs
it after the frame, when nothing is borrowed.

**The UI paints, it does not lay out.** A row is six painter calls against a computed
rect, not a `horizontal()` of `Label`s that egui has to measure, allocate, hit-test
and assign a widget id to. The design system's own collections work the same way.

**Click targets are tested with real pointer events.** Everything else here can be
checked by reading it; a click target cannot. An interaction rect that another one
happens to cover reads perfectly correctly at the call site and simply does not
respond. So [`app.rs`](src/app.rs)'s `click_tests` drive synthetic `PointerButton`
events through whole frames of the real interface and assert on what the application
actually did — which is how the bug below was found, and why it cannot come back.

That bug is worth recording. The window has no platform title bar, so it draws its own
resize borders, and the obvious way to do that is one foreground `egui::Area` per edge
positioned with `fixed_pos` and filled with `allocate_rect`. An `Area` lays out relative
to its own origin and then clamps that origin so the area fits on screen — so handing it
an *absolute* rect makes the content measure from wherever the origin currently is, the
clamp move the origin to fit, and the content measure larger still. The two chase each
other and settle at **half the window**. The east band, asked for six pixels down the
right-hand edge, came out as `[600, 16]-[1200, 784]`, and the south band as
`[16, 400]-[1184, 800]`. Between them they silently swallowed every click in the right
half and the bottom half of the window: the middle of any row, three of the four column
headers, the refresh and bookmark buttons, and every sidebar entry below the halfway
line. Nothing in the source suggested it. The bands are now plain `Ui::interact` calls
on the root `Ui`, whose coordinate space *is* screen space — so there is no layout to feed back.
They used to be sized to sit in canvas the panels never reached, which meant there was no contest
to win either; the panels reach the edges now, so `chrome::RESIZE_BAND` documents what the four
points overlap and [One surface, divided by lines](#one-surface-divided-by-lines) prices it.

## The mark

A folder in one stroked outline, `AZURE_70` on a `GRAY_2` plate. **It is not this
program's** — [`azur-egui-theme/app-icons/`](../azur-egui-theme/app-icons/) holds the marks
for four applications and their rasterised sets, and this is a reference to the
file-explorer one. [`brand.rs`](src/brand.rs) is 90 lines of wiring and a name.

| form | where you see it | comes from |
| --- | --- | --- |
| a stroked outline, one theme colour | the title bar, at 16px | the design system's `marks.rs`, via `#[path]` |
| one RGBA bitmap, decoded at startup | the taskbar button, Alt-Tab | `app-64.png`, via `include_bytes!` |
| a nine-size `.ico` in the executable | Explorer, a pinned shortcut, Alt-Tab before launch | `app.ico`, handed to the linker by `build.rs` |

Ship the first two and Explorer shows the generic application icon. Ship only the third and
the taskbar button changes identity the moment the window opens.

`app-icons/README.md` says to copy its `marks.rs` into the application. This references it
in place instead, at the same relative path `Cargo.toml` already needs to find the theme at
all — the point of the design system holding the geometry is that four applications cannot
drift apart, and a vendored copy is exactly how they would. The three other marks come along
unused; they are `const` data nothing refers to, so they cost nothing in the binary.

Two things about it worth knowing:

- **The plate is dark, not azure.** This is §15's "dark chrome" row rather than the
  white-on-azure default, and the design system records the consequence: against a dark
  Windows 11 taskbar the plate is 1.1:1 and effectively invisible, so the icon reads there as
  a blue folder floating on the bar. Deliberate, and `PLATE` in the theme's
  `examples/app_icons.rs` is the one place to revisit it.
- **The window icon is the 64, not the 256** the design system's README reaches for. There is
  one slot — `IconData` is a single bitmap — and Windows scales it down to 32 for the taskbar
  button and 16 for the window's own corner. 64 halves exactly into both; 256 is the
  eighth-scale reduction the brief calls grey mush. The hand-snapped 16 and 20 exist only in
  the `.ico`, because nothing in the window-icon path can take more than one size.

Two tests, both guarding the wiring rather than the art: the window bitmap decodes to 64×64
in the two palette colours the design system names, and the `.ico` really carries all nine
sizes — otherwise a truncated file is a `cargo:warning` nobody reads and an executable
wearing the generic icon.

## The design system

The window is [`azur-egui-theme`](../azur-egui-theme) throughout: the surface ladder,
the accent, a 2px accent bar down a selected row, the type ramp, the 4px spacing scale, the focus
ring, and the components where there is one — `TextField`, `MenuItem`, `ContextMenu`,
`Menu`, the window buttons, the sort triangles, the chevrons.

**Most of what used to be listed here as a deviation is now the design system's own**, in
[`azur_egui_theme::desktop`](../azur-egui-theme/src/desktop.rs) — the opt-in preset for a dense
application window, which this program installs with `desktop::apply` in `crate::theme` and reads
back through `ui::hover_fill`, `ui::row_fill`, `ui::seam`, `ui::squared` and the rest. They were
this window's decisions and they turned out to be every dense window's, so they were made once
instead of per application. What is left below is either still local, or the record of how the
shared version got its numbers.

- **The hover grey**, and it is the *one* thing in the window that hovers.
  `background-control-hover` and `background-card-hover` go three rungs along Azur's neutral ramp
  from the `GRAY_5` it specifies, because Azur's ladder is written for a control that wants to look
  liftable off `background-layer`, and most of what this window hovers is not a control. It is a
  row, a path segment, a folder in a dropdown: a *place*, on a listing that is nearly black, where
  one rung is not enough to see.

  **Setting the token rather than the call sites is the whole point.** It got done at the call sites
  first, and the result was two hover greys in one window: the rows, the sidebar and the path bar's
  segments moved, while Back, Forward, Up and Refresh went on wearing `GRAY_5` because they reach
  their fill through `ui::control_fills` instead of `ui::row_fill`. Four buttons in the middle of the
  window, a rung and a half off everything around them, and nothing in the code looked wrong at
  either end. One token is what the context menu, the column headers, the caption buttons, the tab
  strip, the application mark, `control_fills` and the design system's own `MenuItem` all read.
  `every_hover_in_the_window_is_the_same_grey` hovers four widgets that get there four different
  ways and insists they agree; `the_window_wears_the_desktop_preset` catches the three things it
  cannot see — the preset never being applied, a helper here drifting back to a value of its own,
  and the two hover tokens parting company.

  A press is one rung further out again. That let `control_fills` stop looking at the surface it is
  painted on: Azur's `control-active` *is* [`seam`] in the light theme, so a pressed segment on the
  path bar used to paint the surface's own colour over the surface and the fill vanished at the
  moment of the press. `control-active` is left to the two places that use it as an inert fill
  rather than as a press — a sidebar row and a tab go quiet while they are dragged.

  The **light** theme was wrong here for as long as the rule lived in this file, and consolidating
  it is what found that: this program handed light the dark theme's `GRAY_9`, a hex value chosen
  while looking at the dark theme, which puts near-black body text on a hovered row at **3.26:1** —
  under AA. A hover band is not an inverted row, so the fill has to stay on the ink's side of the
  palette; the shared version mirrors the dark move *by measurement* instead, 2.5× the distance the
  token asks for on each side's own ramp, and lands on `GRAY_13` at 9.8:1.

- **A selected row is `accent.active` in the dark theme** — `AZURE_40`, `#184e80` — with
  `accent.default` under the pointer as well, rather than the `accent-subtle` navy the design
  system's list vocabulary asks for. Still design-system rungs, and the ramp meant for this: "the
  accent as a *surface*", which stops two below the CSS on purpose because `AZURE_60` is where white
  text starts to pass and a window open all day wants one further than the threshold.

  It walked there from the wrong end, and every figure below is sampled off the framebuffer rather
  than reasoned about. `AZURE_ACCENT` (`#3aa0ff`) was asked for first: the name on a selected row
  measured **2.5:1** and the Size, Type and Modified columns **1.06:1**, which is to say they were
  gone. `accent.default` was halfway back at 6.05:1 and 2.51:1. One rung further again reads
  **7.96:1 and 3.31:1**. Every step down bought legibility instead of costing it, which is the tell
  that the first guess was three rungs past where it should have stopped — and the metadata columns,
  still the weakest thing on a selected row, are the reason the last step was worth taking.

  The light theme keeps `accent-subtle`, which is the same correction the hover needed and was found
  the same way. A row's ink does not change when the row is selected, so a dark fill on that side
  put near-black metadata at **1.7:1** — the identical failure, pointing the other way.

  The 2px bar down a selected row moved with it, to `accent.mark`. It had been `accent.default`,
  which the moment the *fill* became `accent.default` made it a bar the colour of what it is drawn
  on — and `mark` is the token the design system names for a 2px selection bar anyway.

  **A latched toggle wears the same surface**, through `desktop::latched` — the flatten button is
  the one control in this window that latches, and it was `accent-subtle`, Azur's own latch tint,
  which in the dark theme is a navy barely off the bar it sits on: the toggle read as *nearly* on.
  One surface means "on" here, whatever is wearing it, and its glyph takes `text-primary` like a
  selected row's name rather than `accent-mark` — 7.96:1 against 3.13:1 on that fill.

- **The sidebar does not mark the folder that is open** — no fill, no bar, only the pointer's own
  highlight. Two rows wearing the selection colour for two different reasons is one too many: in
  the listing it means *the user selected this*, and in the sidebar it meant *this is where you
  are*, which is not the same statement and does not deserve the same band. Where you are is still
  said, quietly, by the row's label sitting at `text-primary` while its neighbours are
  `text-secondary`, and loudly by the breadcrumb, which is the bar whose whole job that is.

- **`ROW_HEIGHT` is 24, not `tokens::row::TABLE`'s 36.** Azur's table row is sized for
  a form. A file listing is read by scanning hundreds of lines, and every point of row
  height is a file that did not fit. A sidebar row is 22, one body line and a point of
  air, for the same reason. Both are `tokens::row::DENSE` and `row::TIGHT` now — named
  in the design system, because the argument is about listings rather than about files.
- **Rows are contiguous — no `item_spacing` between them.** The 4px scale is right for
  controls and wrong for a list of names: the hover fill *is* the row, and 8 points
  between rows turns a sidebar into a column of buttons and costs a third of the panel.
  Groups get their air explicitly instead. The same goes for the menu.
- **Menu geometry comes from the design system, not from here.** `menu_item_height()`,
  `menu_divider_height()` and `menu_item_metrics()` are asked for by name rather than
  re-derived from `LINE_BODY + space::S3 * 2` and friends. All three exist because the
  re-derived version was wrong: the height drifted 8 points per row when the design system
  changed its density, the divider 4 points when it tightened its padding, and the *width*
  was wrong in two places at once — the leading icon gutter added only for entries that had
  an icon, when `MenuItem` reserves it for every entry, and the gap before a shortcut a step
  too wide. Those two errors cancelled often enough to hide, because a shell menu always has
  one long `Restore previous versions` in it that pushes the width to its cap anyway. Then
  the menu started opening with this program's six entries alone while the shell was still
  being asked, and came up 16 points short with `Copy pa…` in it. The design system now
  states its own arithmetic and asserts that a `MenuItem` asks for exactly that.

- **A control's states cannot be read off the ladder alone.** `control`, `control-hover` and
  `control-active` are written for a control on `background-layer`, where each step is a step away
  from the surface. A control on a surface that is itself one of those steps has nowhere to go, and
  paints the surface's own colour onto the surface. `ui::control_fills` used to answer that by
  asking what it was standing on; naming the two rungs outright answers it for every surface at
  once, which is why the `surface` argument is still in the signature and no longer read. See
  [One surface, divided by lines](#one-surface-divided-by-lines).
- **A text field selects its contents when focus arrives** — a change in the design system, not
  here, because "any field you have just arrived in is a field whose value you mean to replace"
  is a statement about fields rather than about this application. Not keyed on
  `Response::gained_focus`, which misses one of the ways in: a caller that hands a field focus
  with `request_focus()` *after* the widget has run — which is how a field that opens already
  focused is written — has it granted at the end of the frame, so the widget never sees the
  transition. It remembers last frame's answer per id instead, which catches a click, a tab and a
  request alike. The rename field is the app's own `TextEdit` and keeps its special case: the
  **stem** only, so the extension survives.
- **Counting tokens is not measuring contrast.** The dark gray ramp is perceptually finer than
  the light paper ladder, so "four steps up" in one theme and "two steps up" in the other can be
  the same amount of contrast — `control-active` against `background-layer` is ΔL\* 15.4 dark and
  13.9 light. Any claim about a colour being too loud or too close belongs in CIELAB, not in
  token names. `azur_egui_theme::contrast` is the ruler, and public for exactly this: `ratio` for
  ink, `apart` for two surfaces, `over` first for anything translucent.
- **Panels are square and unringed.** `radius-medium` and `stroke-subtle` are right for a card
  floating on canvas and wrong for a panel that is part of the window's structure. A dense window
  has two radii and no third: `components::control_radius` and `desktop::structural_radius`.
- **Everything in a row is centred on its ink, and text sits on a baseline.** A glyph is centred
  by its ink and not by its box — the box is what a caller centres, so art drawn low in its box
  sits low in every row it appears in. Text centred by its *line box* hangs below it, because a
  line box reserves room for descenders a file name does not use. The design system states both
  now, and states the second one as arithmetic: `components::ink_baseline` for a row that holds a
  glyph, `row_baseline` for a row of text alone, one baseline per row in either case. The three
  `CELL_LIFT` / `TEXT_LIFT` constants here are the same rule found by eye three separate times,
  and predate it; at 100% scaling they and the measured 1.5 land on the same pixel row.
- **File-kind hues extend `tokens::chart`.** The accent and the two status hues keep
  their meaning; teal and lime are added at the same lightness so no kind shouts louder
  than another in a mixed listing. This is the documented pattern for an app with
  domain colours — a struct that owns a `Theme` and derefs to it.
- **The code palette is here too**, for the same reason and by the same pattern: seven hues, a
  recessed fill for a code block, and a pair for a diff's two sides. Unlike every other colour in
  this window it is *not* derived from the token set — it is a transcription of an editor's
  scheme — which makes the measurement the only thing standing behind it. `contrast`'s own header
  says an application that adds a role of its own should hold itself to the same floor in its own
  tests, and `every_syntax_colour_can_be_read` is that, against **two** surfaces, since a fenced
  block puts the whole palette on the fill rather than on the panel. That second surface is what
  chose the fill; see [Source is coloured](#source-is-coloured-and-it-is-a-lexer-rather-than-a-parser).
- **A shortcut in a tooltip is `text-secondary`** — and that one went the other way, into the
  design system, because `Refresh (F5)` is a sentence and an aside in every window rather than in
  this one. It is *recognised* in the string rather than passed in as a second argument, which is
  what makes it apply to the tooltips that were already written. The guard and the reasoning are
  [in its README](../azur-egui-theme/README.md#a-shortcut-in-a-tooltip-is-set-in-text-secondary).
- **Folders are filled; files are outlined.** At 14 pixels that distinction has to be
  legible before any colour is.

### Crisp text

Five things, and none of them is the font's weight — Azur's body face is regular Segoe UI. The
first two are the design system's, so every Azur window has them; the rest are this window's:

- **Glyph coverage reaches the atlas unmodified** (`FontColorTransferFunction::Off`), in
  both palettes. egui's dark default lifts every partially-covered pixel to compensate for
  light-on-dark thinning; over Azur's `#14171A` canvas it over-corrects, and the window
  reads semibold. Measured over the sidebar, it was 10% more lit pixels than the glyphs
  actually cover. It also means the weight no longer changes when the palette does, which
  egui's per-side defaults made it do.
- **One rasterisation per glyph, at a whole pixel** (`subpixel_binning = false`). egui
  otherwise renders each glyph at up to four fractional offsets — its own documentation
  says that "lead to text looking more blurry". At UI sizes that trade is the wrong way
  round, and Windows snaps glyph positions rather than sampling between them.
- **Text origins are snapped to whole *device* pixels**, with `round_to_pixels` rather than
  `f32::round`, because a logical point is not a pixel: at 125% scaling, rounding to a whole
  point leaves the glyph on a quarter-pixel. Panes are split by fractions of a window, so a
  column's left edge is fractional about half the time.

- **Nothing is dithered.** eframe dithers by default, and dithering perturbs anything sampled
  from a texture — the glyph atlas being a texture. It earns its keep across a wide gradient;
  this window has none, and 14px text is the wrong thing to add noise to. Turning it off was
  worth **5% more fully-hard glyph edges** on identical content, measured at 34,009 against
  35,721. `multisampling` is 0 for the same reason: egui already antialiases by feathering an
  edge one pixel, and multisampling would only soften what feathering has already handled.
- **Text, lines and rectangles are held on the physical pixel grid**, pinned by
  `azur_egui_theme::style` rather than left to egui's defaults. They *are* egui's defaults today.
  They are set explicitly because the day one of them changes upstream, every glyph in every Azur
  window softens and nothing in the code would say why — so a test asserts them instead.

Between the first two: partially-covered pixels went from 47% of the lit ones to 39%, and
fully-lit from 31% to 37%. What remains between this and Explorer is that Azur's body size is
14px where Explorer's list is 9pt (12px at 96 DPI), and that Explorer uses ClearType's
subpixel rendering where egui antialiases in grayscale.

## What it does not do

Deliberately:

- **No archive browsing, no search, no properties of its own.** Properties comes from
  the shell menu, which is the right place for it.
- **The Recycle Bin opens in Explorer.** It is a shell namespace extension stitched
  together from a per-volume `$Recycle.Bin\<SID>` plus an index of original paths, not
  a directory — enumerating it would show mangled `$R…` names and no way to restore
  anything, so this hands it over rather than lying about it.
- **Collation is code-point order beyond ASCII.** Digits sort as numbers and case is
  ignored, so `file2` precedes `file10`; accented names sort consistently but not by
  the locale's rules. Doing that properly needs ICU, and the sort would stop being
  free.
- **Only the details view.** Icons, tiles and a tree are not there.

## Command line

| | |
| --- | --- |
| `--open=<path>` | repeatable — one pane per path, so `--open=A --open=B` opens side by side |
| `--reveal=<name>` | select and scroll to an entry once the listing lands |
| `--filter=<text>` | put a line in the first pane's filter box before anything is drawn — `--filter=@git` for a listing of what has changed |
| `--size=WxH` | pin the window size |
| `--light` / `--dark` | override the remembered palette for this launch |
| `--shot=<file>` | write the frame as a PNG and exit, for regenerating the images above |
| `--menu` | raise the folder's context menu, so `--shot` can capture one |
| `--rename` | open the selected name for editing, for the same reason |
| `--preview` | open the preview panel on the selected file — or the first previewable one — for the same reason |
| `--compare` | select the first two pictures and open the panel on them, so a capture can show the comparison |
| `--console[=<a;b;c>]` | open the console panel, and run these commands in it |
| `--flat`, `--flat=list\|tree` | open every pane flattened, for looking at that view without pressing the button — and in a stated mode, so photographing the other one does not mean editing the settings and remembering to put them back |
| `--tiles` | open every pane as [large icons](#rows-or-tiles). The one view with *no* other way in for a capture run: it is behind a click on a switch, and deliberately not a setting |
| `--stack` | open the extra `--open=` paths *below* rather than beside, for the same reason |
| `--trace` | print a line whenever the window's size, scale, focus or minimised flag changes, and the memory, handle and cache figures every three seconds |
| `--walk=<dir>` | browse subfolder after subfolder by itself, four a second, so `--trace` can measure a real session |

A path given to `--open=` may use either slash. It is rewritten to backslashes on the way
in, because every file API accepts `D:/x` and `SHParseDisplayName` does not — and a path
the shell cannot parse is a path with no context menu, silently.

`--console` exists because the panel is behind a keystroke and its log is behind a shell having
answered, so without it there is no way to *look* at the thing — which is how three rounds of blind
fixes to it got shipped. A capture waits for the commands to finish rather than photographing an empty
log. The keyboard half cannot be photographed at all and is driven with `SendInput` against the real
build instead: synthesised egui events bypass `egui-winit`, which is exactly where `Tab` and `Ctrl+C`
are decided, so they pass against a panel where the gesture is broken.

`--trace` is for one specific bug: text going soft for a few seconds after a window gesture.
This program paints on demand, so between events the screen holds the last frame — and if the
window's *shape* changed since, the compositor has a surface of the wrong size and stretches
it. The window now keeps asking for frames for 750ms after its size, scale, focus or
minimised flag moves, which covers a restore animation and a monitor with another scale. If
it still happens, `--trace` says which of the four moved and when.

## Building and deploying

```text
cargo run                     the debug build
cargo run --release           the one to judge anything by
cargo deploy                  release build, then copy it to D:\Programs
cargo deploy E:\Tools         somewhere else
cargo deploy --debug          the debug build, for once
```

`cargo deploy` is an alias in `.cargo/config.toml` standing for
`run --release --example deploy`, because an alias can only stand for a cargo subcommand — it can
build or it can run, but nothing in cargo will build and *then* copy. So the alias runs a small
program that does both, and that program is
[`azur_egui_theme::deploy`](../azur-egui-theme/src/deploy.rs), shared with the other Azur
applications: it refuses to copy anything if the build failed, and it can replace a copy that is
*running*, which is the state `D:\Programs` is in whenever you have this open to look at what you
just changed. The theme crate's README has the whole of the reasoning. `examples/deploy.rs` here is
one call, and what it passes are the two things only this package knows — its binary and its
manifest — because `env!` can only answer where it is written.

`$AZUR_DEPLOY_DIR` moves the default destination for every Azur application at once; an argument
still beats it.

The destination file keeps the *source's* modification time, because that is what `CopyFileEx`
does and it is the more useful of the two: the timestamp on a deployed binary says when it was
built rather than when it was last copied.

## What this is not doing itself

Worth being explicit, because it is the point rather than a shortcut: the icons in the
listing *and in the sidebar*, the clipboard, the copy engine, the context menu and the
drag and drop are all the shell's.
This program decides *what* to ask for and *when*, and draws the listing. It does not
re-implement any of it — which is why a file cut here pastes into Explorer, why Delete
means the Recycle Bin and `Ctrl+Z`, why a conflict prompt looks familiar and offers to
keep both, and why the context menu has your version-control extension in it.

Two things would have been easier to fake. The **icons** — and faking them is what makes
shell-backed file managers slow, so see "Why it is fast" above for the measurement that
decided how to do it properly instead. And the **context menu**, where the easy version is
to host Windows' own `HMENU` with `TrackPopupMenuEx`: it works, and it looks like a
different application opened. Reading the content out and drawing it here costs more code
and is the only way to have both the extensions and one coherent window.

## Settings

`%APPDATA%\Azur File Explorer\config.ini` — a `key=value` file you can edit by
hand. Bookmarks, sidebar width, which sidebar groups are open, the window's size and
position, the palette, and every tab that was open — **grouped by the pane it was in**, with
the tree that arranged them. Anything unparseable is skipped rather than fatal.

The window size is remembered, so **1024×600 is what a first launch gets**; delete the
`window=` line to go back to it.

`diff=1` shows what changed in a previewed file — the bands on a text file, the three views on a
picture — and `diff_collapse=0` leaves the unchanged stretches in. Both are toggles in the preview's
own title bar, and both are preferences about how you read rather than facts about a file. `diff` is
the one flag here whose default is *on*, so it is read as "anything but `0`": a settings file written
before it existed has no line for it, and the default has to stand.

`console_share=` is how much of a pane the console panel takes and `console_shell=` is the one it
opens on — `bash`, `pwsh` or `cmd`, written as the word the dropdown shows so the file stays something
you can read and edit. Both are the same kind of preference as the preview panel's position: how tall
you like a console and which shell you work in are habits, and whether one is *open* is a decision
about what you are doing right now. See [A console in a pane](#a-console-in-a-pane).

`flatten=list` or `flatten=tree` is which of the two views the flatten button produces, as a word for
the same reason. The same kind of preference again — and *whether* a tab is flattened is not here,
for the same reason its preview being open is not. A file without the line comes back as the list,
which is what the button produced before there was a choice. See
[Flattening a folder](#flattening-a-folder).

`regroup=1` is whether a tree shows a chain of folders with nothing in them but each other as one row
— `src > main > java`. The **second** flag here whose default is on, and read the same way as `diff`
for the same reason: a settings file from before it existed has no line for it, and what it turns on is
the absence of rows that never had anything to say. See
[A chain of folders is one row](#a-chain-of-folders-is-one-row).

`forward_slashes=1` is whether the path field writes `/` between the parts of a path instead of `\`.
Off by default, so a missing line and a `0` mean the same thing — `\` is what Windows shows
everywhere else, and a path bar that disagreed with the rest of the desktop out of the box would be
this program being clever. Ticked in the field's own context menu, which is the one control it is
about; see [The path bar](#the-path-bar).

### Coming back the way you left it

```ini
window=1380,840
position=-1920,-8      ; physical pixels
maximized=0
layout=h0.400(0,v0.500(1,2))
focus=1
pane=1                 ; pane 0's tabs, the second of them in front
path=D:\Sources
path=D:\Sources\MyTools
pane=0                 ; pane 1's
path=C:\Users
pane=0                 ; pane 2's
path=D:\
```

`layout=` is [`dock::Node`](src/dock.rs)'s own one-line form of the tree: `h`/`v` for how a
split divides its space, the number after it for the share the first child gets, and a bare
integer for a pane — numbered by position in layout order, because a `PaneId` is a counter
that runs for the life of one window and means nothing to the next one. A `layout=` that is
not a tree over exactly the panes below it is refused *whole*, and the window opens with every
tab in one pane: a partly restored layout would be a window missing one of the folders that
were open in it, which is worse than an honest default. A file from before panes were
remembered has bare `path=` lines and no `pane=`, and that is exactly what it meant.

**`position=` is the odd one out: physical pixels, not points.** A desktop of two monitors at
different scale factors has no single coordinate space in points — a logical position means
something different depending on which monitor resolves it, and the platform resolves it
against the monitor it happens to *open* the window on rather than the one it is being sent
to. So the position is saved in pixels and applied in pixels, straight to the platform from
the creation closure: the window exists by then, nothing has been painted into it, and eframe
keeps it hidden until something has been. `ViewportCommand::OuterPosition` from inside the
first frame is exact too, and lands one step too late — eframe reveals the window in
`post_rendering`, which runs before a frame's viewport commands are applied.

Before any of that, the position is checked against the monitors that are actually connected.
A window last closed on a monitor that has since been unplugged names a place that no longer
exists, Windows does not clamp it back into view, and this window has no caption of its own
for the platform's Move command to work on — so it would open as a taskbar button with
nothing on screen, which is indistinguishable from a program that failed to start.

Settings written under the program's previous name are read once, if there is nothing under
the current one, and re-saved in the new place — a rename is not a reason to lose somebody's
bookmarks.

### Arriving somewhere the trail runs past

The path bar keeps showing the folders below the one you are in — going up leaves the trail alone,
so what you came out of stays one click away instead of being something to go and find. Which means
that on arrival the bar is already pointing at the row you are most likely to want, and the listing
now selects it and scrolls to it: standing in `a/b` with `a/b/c` on the bar, `c` is selected. For a
folder of five thousand names that is the difference between going up a level and losing your place.

It is decided in `Tab::go_to`, which is the one place the path and the trail change, so all four
ways of arriving get it — `navigate`, `go_up`, `go_back` and `go_forward`. `go_up` and `go_back` each
used to do a version of it for themselves, and both were limited to a single step: they asked
whether the place being landed on was the *parent* of the place being left. Clicking a segment three
levels up highlighted nothing at all, and going forward never highlighted anything. The child is
asked of `fs::breadcrumb_segments` rather than worked out from path components, so that "what the
breadcrumb shows" means exactly that — a component walk hands back `C:` without its root, and has
nothing above it to call This PC.

### Undoing a closed tab

`Ctrl+Shift+T` puts the last closed one back, up to ten deep, most recent first. It reopens into
whichever pane has focus, the way a browser reopens into the current window — the pane a tab was
closed *from* may well have gone with it, since closing a pane's last tab closes the pane.

**Only the path is kept.** A closed tab's own Back and Forward trail is deliberately not carried:
reopening is "put that folder back", not "restore that tab as it was", and ten tabs' worth of
navigation history is a great deal of state to hold for a gesture that exists to undo a misplaced
click. A reopened tab starts with one place in its history, exactly as a tab opened any other way
does, and `a_closed_tab_comes_back_with_nothing_behind_it` navigates the tab somewhere first so
that it is asserting a trail was dropped rather than that there was never one.

Two closes are not remembered. The last tab of the last pane is the *window* closing, and the
history goes with the process; and a tab moved into another pane or pulled out into one of its own
was never closed at all, which `a_tab_pulled_out_into_a_pane_of_its_own_was_not_closed` holds in
place, because that path runs through `take_tab` and it would be easy to route it through the
close.

The keyboard test is the one worth having. `Ctrl+T` and `Ctrl+Shift+T` hang off the same key, and
the branch that reads `Shift` is the only thing stopping both from firing: without it the gesture
reopened the closed tab *and* opened a blank one beside it. Nothing but driving the real key path
sees that — `ctrl_shift_t_puts_a_tab_back_and_does_not_also_open_a_new_one` asserts on what is
*not* in the journal, and with the guard taken out it reports
`["NewTab", "ReopenTab", "NavigateNewTab"]`.
