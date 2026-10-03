# The application icon

A blue folder behind a stack of pages with a magnifier over it, on a near-black rounded
plate (`#191D23`). One rendered image at ten sizes. `src/brand.rs` is the wiring and says
which form goes where; this file says how the files are made.

| file | who reads it |
| --- | --- |
| `master.png` | nothing at runtime — the 512px source every other size is resampled from |
| `app-16` … `app-64.png` | `src/brand.rs`, via `include_bytes!`: the title bar's ladder, and the 64 is also the window's taskbar bitmap |
| `app-128.png`, `app-256.png` | nothing at runtime — the `.ico` carries its own copies |
| `app.ico` | `build.rs`, which hands it to the linker; Windows reads it in Explorer, on a pinned shortcut, and in Alt-Tab before the process exists |

## Rebuilding it

The plate arrived as a render on an opaque black backdrop, so the alpha is not in the
source — it is recovered, and two details in that are worth keeping if this is ever redone:

- **The backdrop is found by reachability, not by threshold.** The plate's own interior is
  as dark as the backdrop around it (`#191D23` against `#000000`), so "everything darker
  than *n*" eats the mark. What separates them is that the backdrop is connected to the
  image border: flood-fill inwards from all four edges over pixels darker than about 6,
  and whatever is not reached is the plate.
- **Resample premultiplied.** The plate's edge is a bright rim light. Reducing
  straight-alpha RGBA mixes the transparent black outside the plate into that rim and
  leaves a dark halo at every size. Multiply by alpha, resize colour and alpha separately
  with Lanczos, then divide back out — clamping both ends, because Lanczos rings.

Then crop to the plate's bounding box (measured 1042×1057, so it squashes 1.4% on the way
into a square — invisible, and it lets the plate fill the icon edge to edge as Windows
expects), resample to 16, 20, 24, 32, 40, 48, 64, 128 and 256, and pack all nine into
`app.ico` as PNG payloads — which is what the entries are, not DIBs, at every size.

Sizes are not arbitrary at either end. The nine in the `.ico` are the ones Windows asks
for. The seven up to 64 are `LADDER` in `src/brand.rs`, one per scale factor at the title
bar's 16pt; 128 and 256 stay out of the binary because nothing in the window draws the mark
that big and they would be 78 KB of it.
