//! The developer flags: everything on the command line except `--open=`.
//!
//! **Not in a shipped build.** This module, and `app::capture` behind it, is compiled for tests and
//! for the `devtools` feature, and for nothing else. A release build ignores every one of these
//! words, because a flag nobody can find in the interface is a hidden feature — surface area and
//! bytes that serve whoever builds the program rather than whoever runs it. `cargo devtools` is the
//! build that has them: release optimisation, its own profile so it never overwrites
//! `target/release`, and these flags. See `devtools` in `Cargo.toml`.
//!
//! What they are for is reaching a state that otherwise takes a click, a key or a shell having
//! answered, so a screenshot can be taken of it (`--shot`), and driving a real window for a
//! measurement (`--walk`, `--scroll`, `--trace`). Each flag's own comment says which.

use crate::app::App;
use crate::config::Config;
use crate::pane;

/// A pending `--shot=` capture: where to write it, and how many frames have gone
/// past. Fonts, the first directory scan and the scroll area all settle over the
/// first few, so the capture waits for them.
struct Shot {
    path: std::path::PathBuf,
    frame: u32,
}

/// Every developer flag a run was given, and what the window has done about each so far.
#[derive(Default)]
pub struct Devtools {
    shot: Option<Shot>,
    /// `--stack`: put each extra `--open=` pane below the first rather than beside it.
    stack: bool,
    reveal: Option<String>,
    filter: Option<String>,
    lens: Option<pane::Lens>,
    trace: bool,
    walk: Option<std::path::PathBuf>,
    scroll: bool,
    flat: bool,
    sizes: bool,
    /// `--menu`: raise the folder's context menu, so a capture can show one.
    menu: bool,
    /// `--rename`: open the selected row's name for editing. The one part of this interface a
    /// screenshot cannot otherwise reach, since it needs a keystroke on a focused row — and
    /// where the caret lands is the whole thing worth looking at.
    rename: bool,
    /// `--batch[=pattern]`: select the first pane's files and open the batch rename panel over them,
    /// in Pattern mode with that pattern when one is given. Behind a selection and F2, which a
    /// capture run has no keyboard for — the same reason `--rename` exists.
    batch: Option<Option<String>>,
    /// `--palette[=text]`: put the command palette up, with that typed into it. Behind a chord, which
    /// a capture run has no keyboard for.
    palette: Option<String>,
    /// `--path=<text>`: open the path field with that text half-typed in it, so a capture can show
    /// the completion dropdown. Behind `Ctrl+L` and then a keystroke, which a capture run has no
    /// keyboard for — the same reason `--rename` exists.
    path: Option<String>,
    /// `--tiles`: show the listing as large icons rather than as rows.
    ///
    /// Behind a click on the switch at the left of the status line, and behind nothing else at all —
    /// the view is deliberately not a setting, so unlike `--flat` there is no config key a capture run
    /// could reach it through. See `App::show_tiles_here`.
    tiles: bool,
    /// `--diff[=all|changes|names]`: open a folder diff of the first pane's folder against the
    /// second's, as `Folder diff...` in the application menu would — showing the rows the word says,
    /// or without one the rows the settings say. See [`crate::diff`].
    diff: Option<Option<crate::diff::Show>>,
    /// `--against=<path>`: the right half of `--diff`, rather than the next pane's folder — so a
    /// capture of a diff can be one pane wide.
    against: Option<std::path::PathBuf>,
    /// `--preview`: put the keyboard on the first previewable file in the listing and open the
    /// preview panel on it, for the same reason — it is behind `Ctrl+P` and a focused row.
    preview: bool,
    /// `--console[=<commands>]`: open the console panel, and run these in it.
    ///
    /// The panel is behind `Ctrl+²` and its log is behind a shell having answered, so a capture run
    /// is the only way to look at the thing at all. It waits for the commands to finish — see
    /// `App::console_busy` — because a screenshot of a log that has not printed yet says nothing
    /// about the log.
    console: Option<Vec<String>>,
    /// `--copy=<path>`, repeatable: copy each into the first pane's folder with the fast engine, one
    /// job apiece, so a capture can show the transfer panel — which is only up while a copy is. See
    /// `App::copy_here`.
    copy: Vec<std::path::PathBuf>,
    /// `--find=<text>`: and then open the find bar on that text, for a capture of the text viewer
    /// with something found in it.
    find: Option<String>,
    /// `--deps=<module>`: and then pick that row of a binary's dependency tree, for a capture of the
    /// two symbol panels — which are behind a click on a row.
    ///
    /// Applied later than the rest and not at a frame number: the walk arrives on a worker thread, so
    /// a pick made before the graph is here has no row to land on. See where it is driven.
    deps: Option<String>,
    /// `--compare`: select the first *two* pictures instead, so a capture can show the comparison.
    compare: bool,
    /// `--previews=<n>`: select that many previewable files, so a capture can show the panel tiled.
    /// Zero is "leave the selection alone", which is what every run without the flag is.
    previews: usize,
    /// Frames a `--menu` capture has spent waiting for the shell's half of that menu.
    waited: u32,
}

impl Devtools {
    /// Read every developer flag off the command line, applying to `config` the ones that are
    /// settings for this run rather than things to do — the palette, the window's size, the flatten
    /// mode. `--open=` is `main`'s, and skipped here.
    pub fn from_args(mut config: Config) -> (Config, Self) {
        let mut d = Self::default();
        for arg in std::env::args().skip(1) {
            d.take(&arg, &mut config);
        }
        (config, d)
    }

    fn take(&mut self, arg: &str, config: &mut Config) {
        if arg.starts_with("--open=") {
            // `main`'s own, and the one flag a shipped build reads.
        } else if let Some(path) = arg.strip_prefix("--shot=") {
            self.shot = Some(Shot {
                path: std::path::PathBuf::from(path),
                frame: 0,
            });
        } else if arg == "--light" {
            config.palette = crate::theme::Palette::Light;
        } else if arg == "--dark" {
            config.palette = crate::theme::Palette::Dark;
        } else if let Some(name) = arg.strip_prefix("--theme=") {
            // Every palette by name, which `--light` and `--dark` above are shorthands for. An
            // unrecognised word leaves the run as it was for the reason `--flat=` does: a capture
            // flag should not be the thing that refuses to start. `Palette::key` is the spelling.
            if let Some(palette) = crate::theme::Palette::parse(name) {
                config.palette = palette;
            }
        } else if let Some(name) = arg.strip_prefix("--reveal=") {
            self.reveal = Some(name.to_owned());
        } else if let Some(text) = arg.strip_prefix("--filter=") {
            self.filter = Some(text.to_owned());
        } else if let Some(name) = arg.strip_prefix("--lens=") {
            // `--lens=git`, `--lens=images`: open showing one of the filter funnel's listings, which
            // are behind a menu and so out of reach of a run with no pointer. **What the menu entry
            // sets alongside the lens is set here too** — the flatten either way, and the tiles for
            // pictures — because those are part of the listing rather than side effects of it; see
            // `app::Action::SetLens`. An unrecognised word leaves the run as it was, for the reason
            // `--flat=` does: a capture flag should not be the thing that refuses to start.
            if let Some(picked) = pane::Lens::parse(name) {
                self.lens = Some(picked);
                self.flat = true;
                self.tiles |= picked.wants_tiles();
            }
        } else if arg == "--menu" {
            self.menu = true;
        } else if arg == "--rename" {
            self.rename = true;
        } else if arg == "--batch" {
            self.batch = Some(None);
        } else if let Some(pattern) = arg.strip_prefix("--batch=") {
            self.batch = Some(Some(pattern.to_owned()));
        } else if arg == "--palette" {
            self.palette = Some(String::new());
        } else if let Some(text) = arg.strip_prefix("--palette=") {
            self.palette = Some(text.to_owned());
        } else if let Some(text) = arg.strip_prefix("--path=") {
            self.path = Some(text.to_owned());
        } else if arg == "--tiles" {
            self.tiles = true;
        } else if arg == "--diff" {
            self.diff = Some(None);
        } else if let Some(text) = arg.strip_prefix("--diff=") {
            self.diff = Some(crate::diff::Show::parse(text));
        } else if let Some(text) = arg.strip_prefix("--against=") {
            // Either slash, as `--open=` takes it, and for the same reason: the shell cannot parse
            // `D:/x`, and a right half the shell cannot parse has no context menu.
            self.against = Some(std::path::PathBuf::from(text.replace('/', "\\")));
        } else if arg == "--preview" {
            self.preview = true;
        } else if arg == "--compare" {
            self.preview = true;
            self.compare = true;
        } else if let Some(text) = arg.strip_prefix("--previews=") {
            // `--previews=3`: select that many previewable files, so a capture can show the panel
            // tiled. `--compare` is the two-picture *blend*; this is the general case of the
            // gesture, and the two are different pictures of the same panel.
            self.preview = true;
            self.previews = text.parse().unwrap_or(0);
        } else if let Some(name) = arg.strip_prefix("--deps=") {
            // `--deps=shell32.dll`: open the panel on a binary and pick that row of its dependency
            // tree, so a capture shows the two symbol panels rather than the tree on its own. The
            // panel itself comes with it, the same way `--find=` brings it.
            self.preview = true;
            self.deps = Some(name.to_owned());
        } else if let Some(text) = arg.strip_prefix("--find=") {
            self.preview = true;
            self.find = Some(text.to_owned());
        } else if let Some(path) = arg.strip_prefix("--copy=") {
            // `--copy=<path>`, repeatable: copy each into the first pane's folder with the fast
            // engine, one job apiece, so a capture can show the transfer panel — and several of
            // them stacked. Either slash, as `--open=` takes it.
            self.copy.push(crate::fs::from_outside(path));
        } else if arg == "--stack" {
            self.stack = true;
        } else if arg == "--trace" {
            self.trace = true;
        } else if arg == "--scroll" {
            self.scroll = true;
        } else if arg == "--console" {
            self.console = Some(Vec::new());
        } else if let Some(commands) = arg.strip_prefix("--console=") {
            // `--console=pwd;git status`: open the panel and run these, which is the only way a
            // capture can show a log with anything in it. Semicolons, because a comma is a
            // character shells use and this list is not one a person types twice.
            self.console = Some(
                commands
                    .split(';')
                    .map(str::trim)
                    .filter(|command| !command.is_empty())
                    .map(str::to_owned)
                    .collect(),
            );
        } else if arg == "--sizes" {
            // `--sizes`: count every folder on show and bar each row's share of the total, which is
            // behind the measure button on the status line and behind nothing else — there is no config
            // key for it, since it is a question about the folder in front of you. See
            // `App::measuring`, and `App::sizes_pending` for why a capture then has to wait.
            self.sizes = true;
        } else if arg == "--flat" {
            self.flat = true;
        } else if let Some(mode) = arg.strip_prefix("--flat=") {
            // `--flat=tree`, `--flat=list`: the flatten mode this run uses, whatever the settings
            // say. The mode is a saved preference — see `pane::FlatMode` — so without this the only
            // way to photograph the other one is to change the settings first and remember to
            // change them back. An unrecognised word is still a flatten, in whichever mode was
            // saved: a capture flag should not be the thing that refuses to start.
            self.flat = true;
            if let Some(mode) = pane::FlatMode::parse(mode) {
                config.flat_mode = mode;
            }
        } else if let Some(dir) = arg.strip_prefix("--walk=") {
            self.walk = Some(crate::fs::normalize(std::path::Path::new(dir)));
        } else if let Some(size) = arg.strip_prefix("--size=") {
            if let Some((w, h)) = size.split_once('x') {
                if let (Ok(w), Ok(h)) = (w.parse(), h.parse()) {
                    config.window = Some([w, h]);
                    config.maximized = false;
                    // A capture is a window of a stated size and nothing else about the last
                    // session: where it was left is not part of what is being photographed,
                    // and a screenshot run that moves the window about is a nuisance.
                    config.position = None;
                    // Nor how far it was zoomed: a stated size is a statement about the picture.
                    config.zoom = 1.0;
                }
            }
        }
    }

    /// Where each extra `--open=` pane lands.
    pub fn side(&self) -> pane::Side {
        if self.stack {
            pane::Side::Bottom
        } else {
            pane::Side::Right
        }
    }

    /// Hand the flags that shape the opening state to the application, before its first frame.
    pub fn configure(&mut self, app: App) -> App {
        app.revealing(self.reveal.take())
            .filtering(self.filter.take())
            .with_lens(self.lens.take())
            .tracing(self.trace)
            .walking(self.walk.take())
            .scrolling(self.scroll)
            .flattened(self.flat)
            .measuring(self.sizes)
    }

    /// Whether this run is a capture, whose window should not rewrite the user's settings.
    pub fn capturing(&self) -> bool {
        self.shot.is_some()
    }

    /// The staging that happens before the application draws its frame. `frames` counts every frame
    /// drawn so far, this one included.
    pub fn before_frame(&mut self, app: &mut App, ui: &mut egui::Ui, frames: u32) {
        // Before the frame, and only once: the listing has landed by now, and the menu
        // then has the four frames before `SETTLE` to lay itself out.
        if self.menu && self.shot.as_ref().is_some_and(|s| s.frame == 8) {
            self.menu = false;
            app.open_folder_menu(ui.ctx());
        }
        // Once the listing has landed and not before: a scan arriving afterwards resets the
        // cursor and takes the rename with it, which is what happened to the first version of
        // this and produced a screenshot of an ordinary listing.
        if self.rename && self.shot.as_ref().is_some_and(|s| s.frame >= 8) && app.has_rows() {
            self.rename = false;
            app.begin_rename_here();
        }
        // The same moment, for the same reason: a scan landing afterwards would take the selection.
        if self.batch.is_some() && self.shot.as_ref().is_some_and(|s| s.frame >= 8) && app.has_rows() {
            let pattern = self.batch.take().flatten();
            app.begin_batch_here(pattern);
        }
        // Once the listing is in, since what the palette lists depends on it.
        if self.palette.is_some() && self.shot.as_ref().is_some_and(|s| s.frame >= 8) && app.has_rows() {
            let text = self.palette.take().unwrap_or_default();
            app.open_palette_with(&text);
        }
        // The same window, for the same reason: the panel follows the keyboard, and a scan
        // landing afterwards would move the keyboard off the row that was chosen for it.
        // Before the preview flag and before the frame, so the tiles are what the pane draws from its
        // very first pass rather than a listing that turns into a grid three frames in.
        if self.tiles {
            self.tiles = false;
            app.show_tiles_here();
        }
        if let Some(show) = self.diff.take() {
            app.folder_diff_here(show, self.against.take());
        }
        if self.preview && self.shot.as_ref().is_some_and(|s| s.frame >= 8) && app.has_rows() {
            self.preview = false;
            if self.compare {
                app.compare_here();
            }
            if self.previews > 1 {
                app.preview_here(self.previews);
            }
            app.open_preview_here();
            if let Some(text) = self.find.take() {
                app.find_here(&text);
            }
        }
        // **Early**, and this is the one capture flag where that matters: a popup fades in over
        // `animation_time`, and a dropdown photographed three frames after it opened comes out
        // half transparent with the listing showing through it. `SETTLE` is twelve frames, so this
        // goes as soon as there is a listing — the folder the field completes against is the one
        // the pane is showing, and `has_rows` is how we know the loader has it.
        //
        // Off `frames` rather than off `Shot::frame`, so it works when a capture is not what
        // this run is for: a flag that silently does nothing without `--shot` is how `--console`
        // came to open no console at all.
        if frames >= 4 && app.has_rows() {
            if let Some(text) = self.path.take() {
                app.type_path_here(&text);
            }
        }
        // Once the listing has landed, so the shell starts in the folder that was asked for rather
        // than in whatever the pane was showing before its scan came back.
        if self.console.is_some() && frames >= 8 && app.has_rows() {
            let commands = self.console.take().unwrap_or_default();
            app.open_console_here(&commands);
        }
        // Once the listing has landed, for the same reason: the copy goes into the folder the pane
        // is showing, and that is only known once the scan is back.
        if !self.copy.is_empty() && frames >= 8 && app.has_rows() {
            let ctx = ui.ctx().clone();
            for from in std::mem::take(&mut self.copy) {
                app.copy_here(from, &ctx);
            }
        }
        // **Once the walk has landed**, which is not a frame number: the graph is read on a worker
        // thread, so a pick made before it is here finds no row. Tried every frame until it takes, and
        // *not* consumed on the first attempt — opening the panel does not itself start the read, so at
        // the frame the panel opens there is no content and nothing pending either, and a flag consumed
        // there photographed the tree on its own.
        //
        // Off `frames` and `preview_pending`, like `--console` and `--path` above and for the
        // reason the paragraph over them gives: it read `!self.preview` — another flag's
        // *consumed*-ness — which is only cleared under `Shot::frame`, so it was the kind of flag that
        // silently does nothing without `--shot`. The wait below is what gives it its frames.
        if frames >= 8
            && !app.preview_pending()
            && self
                .deps
                .as_deref()
                .is_some_and(|name| app.pick_dependency(name))
        {
            self.deps = None;
        }
    }

    /// The waiting that happens after it, and the capture once everything has settled.
    pub fn after_frame(&mut self, app: &mut App, ui: &mut egui::Ui, frames: u32) {
        // A menu waiting on the shell is a menu with `Loading...` in it, and the four frames
        // before `SETTLE` are nowhere near long enough for `QueryContextMenu` — so the
        // capture waits, or it would photograph the placeholder. Bounded, because the thing
        // being waited for is other people's code: ten seconds of not answering is an
        // answer, and a screenshot of the placeholder beats a run that never ends.
        const PATIENCE: u32 = 600;
        if (app.menu_pending()
            || app.preview_pending()
            || app.console_busy()
            || app.git_pending()
            || app.sizes_pending()
            || app.thumbs_pending()
            || app.cloud_pending()
            || app.hard_links_pending()
            || app.diff_pending()
            // The copy, until its panel is on screen to be photographed.
            || !self.copy.is_empty()
            || app.copy_pending()
            // The pick above, once it has had the frames to be attempted in. Waiting on it any earlier
            // would be waiting on the capture counter that opens the panel, and that counter only
            // advances when a capture is attempted — a deadlock that complained below about a name
            // which was there all along.
            || (frames >= 8 && self.deps.is_some()))
            && self.waited < PATIENCE
        {
            self.waited += 1;
            ui.ctx().request_repaint();
        } else {
            // Still set once everything has settled: nothing on show is called that. Which is worth
            // saying rather than handing back a screenshot that quietly shows the tree on its own —
            // and the likelier of the two reasons is the second half of it, since the panel only
            // opens for a capture run at all.
            if frames >= 8 {
                if let Some(name) = self.deps.take() {
                    eprintln!(
                        "--deps: nothing called `{name}` is on show in the dependency tree \
                         (is the preview open on a binary?)"
                    );
                }
            }
            self.capture(app, &ui.ctx().clone());
        }
    }

    /// Ask egui for the frame once things have settled, write it out, and quit.
    ///
    /// A PNG, so the file in `docs/` is the file this wrote. It used to be raw RGBA with the
    /// dimensions in the name, on the grounds that a PNG encoder was too much to carry for a
    /// developer-only flag — and then every screenshot needed converting by hand. The
    /// encoder arrived anyway, for the application icon.
    fn capture(&mut self, app: &App, ctx: &egui::Context) {
        const SETTLE: u32 = 12;
        let Some(shot) = &mut self.shot else { return };

        shot.frame += 1;
        ctx.request_repaint();
        if shot.frame < SETTLE {
            return;
        }
        if shot.frame == SETTLE {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            return;
        }

        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        let Some(image) = image else { return };

        let [w, h] = image.size;
        let bytes: Vec<u8> = image
            .pixels
            .iter()
            .flat_map(|p| p.to_srgba_unmultiplied())
            .collect();
        // `PngEncoder` by name, not `image::save_buffer`: that one chooses its encoder from the
        // extension at run time, so it links every encoder the enabled features carry — TIFF,
        // WebP, GIF's quantiser, JPEG and eight more — to write the one format this ever writes.
        let path = shot.path.with_extension("png");
        let written = std::fs::File::create(&path).map_err(image::ImageError::IoError).and_then(|file| {
            use image::ImageEncoder as _;
            image::codecs::png::PngEncoder::new(std::io::BufWriter::new(file)).write_image(
                &bytes,
                w as u32,
                h as u32,
                image::ExtendedColorType::Rgba8,
            )
        });
        match written {
            Ok(()) => println!("wrote {} ({w}x{h})", path.display()),
            Err(e) => eprintln!("could not write {}: {e}", path.display()),
        }
        // A `--copy` still running holds the window open and asks; a capture run's own copy is
        // nobody's to keep, so it is stopped first and the window closes once it has.
        app.cancel_copies();
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}
