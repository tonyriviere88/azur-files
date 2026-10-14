//! The path field: what is typed into it, what it offers, and the dropdown those offers sit in.

use super::*;

/// How many offers the completion dropdown shows before it starts scrolling.
///
/// **Stated here rather than left to `Menu`'s own 320-point ceiling**, which is 11.43 rows — so the
/// twelfth was drawn as a two-point sliver along the bottom edge. That is worse than either answer
/// either side of it: a list you walk with the arrow keys should end where a *row* ends, because a
/// half-drawn row reads as a rendering fault, and saying "there is more below" is the scrollbar's
/// job and it already does it.
///
/// Ten is a number the eye takes in without counting. Past this many candidates the listing behind
/// the dropdown is the better way to find what you are after — which is the same reasoning
/// [`MENU_LIMIT`] rests on, two orders of magnitude further out.
pub(crate) const OFFERS_SHOWN: usize = 10;

/// How many folders out of the history and the bookmarks go above the folder's own offers.
///
/// Fewer than [`OFFERS_SHOWN`], so that typing into a folder still shows what is in it: these are
/// the few places you most likely meant, and the rows under them are everything that is there.
pub(crate) const RECALLED_SHOWN: usize = 5;

/// What the field recalls from: where the panes have been, the bookmarks, and the folder this pane
/// is in, which is never offered — you are already there.
///
/// `bookmarks` is `None` with `From bookmarks` unticked; `From history` is the history's own
/// [`crate::history::History::paused`].
pub struct Recall<'a> {
    pub visits: &'a crate::history::History,
    pub bookmarks: Option<&'a crate::ui::sidebar::Bookmarks>,
    pub here: &'a Path,
}

impl Recall<'_> {
    /// What the places recalled were worked out from, so a change to either source — a folder
    /// forgotten, or a tick in the menu — shows on the next frame.
    fn key(&self) -> (u64, bool) {
        (self.visits.generation(), self.bookmarks.is_some())
    }
}

/// What the path field is offering to complete, and which of it the keyboard is on.
///
/// **The chevron dropdown, opened by typing.** The bar already answers "what is inside that
/// folder" with a menu of folder rows carrying the shell's own icons, and a completion is the
/// same question asked with the keyboard instead of the pointer — so it is the same
/// [`azur_egui_theme::components::Menu`], the same [`MenuItem`]s and the same icons, and the only
/// new part is what moves down it:
///
/// - **Down** and **Up** walk the offers, and either one puts the dropdown up if it is not.
///   **Down stops on the last offer**, and **Up from the first is *what you typed***, which is
///   how you get back to your own text without deleting anything.
/// - **Right** and **Tab** put the highlighted name in the field with the separator after it, so
///   what is offered next is what is inside it. That is the whole gesture: `Down Right Down
///   Right` walks a tree from the keyboard without a pointer or an `Enter` anywhere in it.
/// - **Enter** on an offer goes there. On nothing, it navigates to what is typed, exactly as it
///   did before any of this existed.
///
/// Matching is by prefix and **case-insensitive**, because a Windows file name is: having to
/// match the case of a folder whose name you are asking for would be no help at all.
///
/// **Above the folder's own offers, the places you have been**, as a browser's address bar offers
/// its history: up to [`RECALLED_SHOWN`] folders out of [`crate::history`] and the bookmarks, every
/// word typed somewhere in their path, best first, each with its whole path for a label and a clock
/// or a star for an icon. They are walked by the same keys as the rows under them — Right and Tab
/// put the whole path in the field — and `Shift+Delete` on one takes it out of the history.
///
/// **Nothing here reads the disk.** The folder is asked of [`crate::loader`] — the same service
/// the listings come from, which reads on a worker — and the offers are whatever has come back.
/// See [`crate::fs::typed_folder`] for why that matters more here than anywhere else in the
/// window: this runs on a keystroke, against whatever has been typed, and what has been typed
/// can name a share that is not there.
#[derive(Default)]
pub struct PathComplete {
    /// Whose field this is, and `None` when none is open.
    ///
    /// The reset is keyed on it: a field that has just opened starts with nothing highlighted and
    /// nothing showing, which is [`Self::hidden`]'s first job.
    pub(crate) pane: Option<PaneId>,
    /// The text the offers were worked out from. Anything else in the field means they are stale.
    pub(crate) typed: String,
    /// The folder [`Self::typed`] names, worked out when the text changed and not on every frame:
    /// a `shell:` name is resolved by the shell, which is several COM calls — cheap once per
    /// keystroke, and not something to repeat sixty times a second while the field sits there.
    pub(crate) folder: Option<PathBuf>,
    /// The folder already asked of the loader, so one it cannot read is asked for once.
    ///
    /// A failed read is deliberately not cached — see [`crate::loader::Loader`], and it is the
    /// right rule, because a share can come back. But it means `cached` keeps saying no, and an
    /// ask driven off that answer alone would be a fresh scan of a dead path on every frame,
    /// each one waking the window to ask again.
    pub(crate) asked: Option<PathBuf>,
    /// Whether the listing behind [`Self::offers`] had actually arrived. While it has not, the
    /// offers are rebuilt on every frame — which is two frames, the keystroke's and the one the
    /// scan lands on.
    pub(crate) ready: bool,
    /// Every child of the typed folder whose name starts with the typed name: what to show, and
    /// where it leads.
    pub(crate) offers: Vec<(String, PathBuf)>,
    /// Whether there were more than [`MENU_LIMIT`] of them.
    pub(crate) truncated: bool,
    /// The folders out of the history and the bookmarks that match what is typed, drawn above
    /// [`Self::offers`]. Indices into the dropdown count these first — see [`Self::target`].
    pub(crate) recalled: Vec<crate::history::Recalled>,
    /// What [`Self::recalled`] was worked out against — the history's generation and whether the
    /// bookmarks were asked — so an entry forgotten with `Shift+Delete`, or a source ticked off in
    /// the menu, leaves the list on the next frame. See [`Recall::key`].
    pub(crate) recalled_at: (u64, bool),
    /// Which offer the keyboard is on. `None` is what was typed, and is where it starts: `Enter`
    /// on a field you have only typed into has always gone where the text says, and a highlight
    /// that arrived by itself would quietly change what that key does.
    pub(crate) hot: Option<usize>,
    /// Put away — the field has only just opened, or something outside the dropdown was clicked.
    /// The next keystroke brings it back, and so does Down.
    pub(crate) hidden: bool,
    /// Bring the highlighted offer into view, on the frame the highlight moves and not on the
    /// ones after it: a list that scrolled itself every frame could not be scrolled by hand.
    pub(crate) follow: bool,
    /// The keyboard has the dropdown, and the pointer has not moved since it took it.
    ///
    /// **One highlight at a time**, as in a browser's bar. The dropdown comes up under wherever the
    /// pointer happens to be resting — usually somewhere over the listing — and a row that lit up
    /// beside the one the arrows are on would be a second highlight nobody asked for. Worse, it lit
    /// up *late*: this window draws only when something happens, so the hover appeared on the next
    /// caret blink, half a second after the keystroke, on whichever row was under the pointer. While
    /// this is set the rows take no hover at all; the first movement of the pointer clears it, and
    /// from then on the highlight goes where the pointer does. See [`edit_field`].
    pub(crate) quiet: bool,
}

/// What a key asked of the offers.
pub(crate) enum Pick {
    /// Put the highlighted name in the field, and offer what is inside it.
    Append,
    /// Go there.
    Go,
}

impl PathComplete {
    /// Work out what to offer for what is in the field.
    ///
    /// Called before anything is drawn and before a key is read, because both depend on it: what
    /// Down and Right do is a question about whether there is anything to move onto. Costs two
    /// comparisons on a frame where neither the text nor the listing has changed, which is nearly
    /// all of them.
    pub(crate) fn refresh(
        &mut self,
        pane: PaneId,
        text: &str,
        loader: &mut crate::loader::Loader,
        recall: &Recall<'_>,
    ) {
        let opened = self.pane != Some(pane);
        let changed = opened || text != self.typed;
        // The places recalled, when the text changes or the history does. The highlight stays
        // where it was for the second — an entry forgotten from under it should leave the keyboard
        // on its neighbour, as a browser's does — and the folder's own offers are worked out again,
        // since which of them the list above already holds may have changed.
        if changed || recall.key() != self.recalled_at {
            self.recalled_at = recall.key();
            self.recalled = recall.visits.recall(
                text,
                recall.bookmarks.into_iter().flat_map(|marks| marks.paths()),
                recall.here,
                crate::history::now(),
                RECALLED_SHOWN,
            );
            self.ready = false;
            self.hot = self.hot.filter(|at| *at < self.len());
        }
        if changed {
            self.pane = Some(pane);
            self.typed = text.to_owned();
            self.folder = fs::typed_folder(split_typed(text).0);
            self.hot = None;
            self.ready = false;
            // **Nothing is offered for a path that has only been shown.** `Ctrl+L` fills the
            // field with where you already are, so on the frame it opens the one thing that
            // matches is the folder you are standing in — a dropdown in the way, saying
            // something you can already read on the bar behind it. The first keystroke brings it
            // up, and so does Down.
            self.hidden = opened;
            self.quiet = true;
        }

        let (_, leaf) = split_typed(text);
        let folder = self.folder.clone();
        // Asked once per folder, and once only for one that cannot be read. See `asked`.
        ask_once(&mut self.asked, folder.as_deref(), loader);
        if self.ready {
            return;
        }

        self.offers.clear();
        self.truncated = false;
        match &folder {
            // Nothing names a folder yet, and what a path typed from nothing can still become is
            // a volume: `d` offers `D:`. The places you have been are offered above these, as
            // Explorer offers them — see [`Self::recalled`].
            None => {
                for drive in fs::drives::list_letters() {
                    if starts_with_folded(&drive.letter, leaf) {
                        self.offers.push((drive.letter, drive.path));
                    }
                }
                // And so is a variable: `%app` offers `%APPDATA%`, which the field already
                // expands. Only once a `%` is typed, so an empty field is still the drives.
                if leaf.starts_with('%') {
                    self.offers.extend(variables(leaf, std::env::vars_os()));
                }
            }
            Some(folder) => {
                // Whatever the loader already has. A miss leaves the offers empty and `ready`
                // false, and the worker wakes the window when it lands.
                let Some(dir) = loader.cached(folder) else {
                    return;
                };
                // Nothing below the bin can be typed: its rows are named by where they came from,
                // and `shell:RecycleBinFolder\C:\Users\…` is not a path.
                let rows = if dir.recycled { 0 } else { dir.len() };
                for i in 0..rows {
                    let entry = &dir.entries[i];
                    // Folders only. The field takes a file too — `Enter` on one opens it — but a
                    // completion that offered every file in `C:\Windows` would bury the four
                    // folders in it, and it is the folders you are typing through.
                    if !entry.is_dir() {
                        continue;
                    }
                    // **Hidden folders are offered once a name is being typed, and not before.**
                    // With nothing after the separator this is the chevron menu's question and
                    // gets the chevron menu's answer, which leaves `$Recycle.Bin` and `System
                    // Volume Information` off the top of every drive. With a name half typed it
                    // is a different question, and a folder that exists and is not offered reads
                    // as a bug in the completion.
                    if entry.is_hidden() && leaf.is_empty() {
                        continue;
                    }
                    let name = dir.name(i);
                    if !starts_with_folded(name, leaf) {
                        continue;
                    }
                    // Once in the dropdown: a folder the list above already offers is not offered
                    // again down here.
                    let target = dir.target(i);
                    if self.recalled.iter().any(|offer| {
                        offer.path.as_os_str().eq_ignore_ascii_case(target.as_os_str())
                    }) {
                        continue;
                    }
                    self.offers.push((name.to_owned(), target));
                }
                self.offers.sort_by(|a, b| fs::sort::natural_cmp(&a.0, &b.0));
                // A menu is for picking one of a few, exactly as it is for a chevron.
                self.truncated = cap(&mut self.offers);
            }
        }
        self.ready = true;
    }

    /// Move the highlight, and put the dropdown up if it was down.
    ///
    /// **Down stops on the last offer.** It used to go on past it to `None` — what was typed — and
    /// round again, which with one offer made every press of Down a toggle: the row lit, went dark,
    /// lit again, and holding the key made it flicker. Nobody pressing Down means "and now nothing".
    ///
    /// **Up from the first offer is `None`**, still, because the text you wrote is one of the
    /// choices: a list with no way back to it would leave `Escape`, which throws the whole field
    /// away, as the only way out. Up from `None` starts at the end.
    pub(crate) fn step(&mut self, down: bool) {
        self.hidden = false;
        self.follow = true;
        self.quiet = true;
        let last = self.len().saturating_sub(1);
        self.hot = match (self.hot, down) {
            (None, true) => Some(0),
            (None, false) => Some(last),
            (Some(at), true) if at >= last => Some(last),
            (Some(at), true) => Some(at + 1),
            (Some(0), false) => None,
            (Some(at), false) => Some(at - 1),
        };
    }

    /// Put the highlighted offer in the field: the prefix exactly as it was typed, the offer's
    /// name, and the separator that starts the next one.
    ///
    /// **The prefix is not rewritten**, which is why [`split_typed`] cuts the text rather than the
    /// resolved path: somebody who typed `%appdata%\` or `~\` keeps what they typed, and the field
    /// stays something they can read. Returns where the field now points, or `None` if nothing was
    /// highlighted.
    pub(crate) fn accept(&self, text: &mut String, slashes: bool) -> Option<PathBuf> {
        let hot = self.hot?;
        // A place recalled is a whole path rather than a name to put after one, so it replaces the
        // text — written with the slash the text was being typed with, and the setting's if neither.
        if let Some(offer) = self.recalled.get(hot) {
            let slashes = match (text.contains('/'), text.contains('\\')) {
                (true, false) => true,
                (false, true) => false,
                _ => slashes,
            };
            let mut next = offer.path.display().to_string();
            next.push(std::path::MAIN_SEPARATOR);
            *text = with_separator(&next, slashes);
            return Some(offer.path.clone());
        }
        let (name, path) = self.offers.get(hot - self.recalled.len())?;
        let (prefix, _) = split_typed(text);
        // The separator they have been using. A path typed with forward slashes lists, navigates
        // and draws a breadcrumb — `fs::normalize` is what sees to that at the door — so turning
        // one into a mixture of both at the moment of helping would be this program's own doing.
        //
        // With none used yet the setting decides, and that is not a corner: a path typed from
        // nothing completes to a *drive* first, so `d` becomes `D:/` with `Use / in path` on and
        // `D:\` with it off. From then on there is a prefix again, carrying the separator this
        // put there. See [`with_separator`].
        let sep = match prefix.chars().next_back() {
            Some('/') => '/',
            Some('\\') => '\\',
            _ if slashes => '/',
            _ => std::path::MAIN_SEPARATOR,
        };
        let next = format!("{prefix}{name}{sep}");
        *text = next;
        Some(path.clone())
    }

    /// How many rows the dropdown has: the places recalled, then the folder's own offers.
    pub(crate) fn len(&self) -> usize {
        self.recalled.len() + self.offers.len()
    }

    /// Where the row at `at` leads, counting the places recalled first.
    pub(crate) fn target(&self, at: usize) -> Option<&Path> {
        match self.recalled.get(at) {
            Some(offer) => Some(&offer.path),
            None => self
                .offers
                .get(at - self.recalled.len())
                .map(|(_, path)| path.as_path()),
        }
    }

    /// The field on `pane` has gone. Forget what it was offering, so the next one opens fresh.
    ///
    /// Keyed on the pane because both panes draw their own bar every frame, and the one without a
    /// field open must not clear the state of the one that has.
    pub(crate) fn close(&mut self, pane: PaneId) {
        if self.pane != Some(pane) {
            return;
        }
        *self = Self::default();
    }

    /// Show the offers as though the last character had been typed rather than put there.
    ///
    /// For `--path=`, which fills the field from outside and would otherwise get the dropdown a
    /// freshly opened one has: down, because a path that has only been *shown* has nothing to say.
    /// Claiming the pane here is what makes [`Self::refresh`] read the text as a change rather than
    /// as an opening — the same distinction, approached from the other side.
    #[cfg(any(test, feature = "devtools"))]
    pub fn type_ahead(&mut self, pane: PaneId) {
        self.pane = Some(pane);
        self.typed.clear();
        self.hidden = false;
    }

    /// The field's text has been rewritten from outside and it is the same path: leave the dropdown
    /// exactly as it was, up or down.
    ///
    /// [`Self::type_ahead`]'s opposite, and for the one thing that does this — `Use / in path`,
    /// ticked with a field open, which swaps every separator in it. To [`Self::refresh`] that is
    /// indistinguishable from a keystroke, and a keystroke puts the dropdown up: tick the setting on
    /// a field holding where you are and a list of the folder you are standing in appears under it,
    /// which is precisely the list `Ctrl+L` goes out of its way not to show.
    ///
    /// Claiming the text is what does it — `refresh` reads an unchanged field as nothing having
    /// happened. The offers are still rebuilt, because they carry the old text's prefix, and the
    /// highlight goes because they are about to move under it.
    pub fn rewritten(&mut self, pane: PaneId, text: &str) {
        if self.pane != Some(pane) {
            return;
        }
        self.typed = text.to_owned();
        self.ready = false;
        self.hot = None;
    }

    /// What is on offer and which of it is highlighted, for the tests.
    #[cfg(test)]
    pub fn offering(&self) -> (Vec<&str>, Option<usize>) {
        (
            self.offers.iter().map(|(name, _)| name.as_str()).collect(),
            self.hot,
        )
    }

    /// The places recalled above the folder's own offers, for the tests.
    #[cfg(test)]
    pub fn recalling(&self) -> Vec<&Path> {
        self.recalled.iter().map(|offer| offer.path.as_path()).collect()
    }

    /// Whether the dropdown is up, which is the same question as whether there is anything in it
    /// that has not been put away.
    #[cfg(test)]
    pub fn showing(&self) -> bool {
        !self.hidden && self.len() > 0
    }
}

/// Split what has been typed into the folder part and the name being typed inside it.
///
/// The separator stays with the folder, so the two halves put back together are exactly the text
/// that came in — which is what lets [`PathComplete::accept`] leave everything in front of the
/// name alone.
pub(crate) fn split_typed(text: &str) -> (&str, &str) {
    match text.rfind(['\\', '/']) {
        // Both separators are one byte, so this is a character boundary.
        Some(at) => text.split_at(at + 1),
        None => ("", text),
    }
}

/// The environment variables that name a folder, written `%NAME%`, whose name starts with `typed`
/// — which carries its opening `%`, and may carry the closing one too.
///
/// **Decided from the value's spelling alone**, because this runs on a keystroke and a variable
/// can point at a share that has gone away: see [`fs::typed_folder`]. So a folder is a value that
/// is an absolute path, is not a list (`PATH`, `PSModulePath`), and does not end in an extension
/// [`fs::fmt::file_type`] knows — which is what keeps `%ComSpec%`, `cmd.exe`, off the list while
/// `%JAVA_HOME%`, ending `jdk-17.0.2`, stays on it.
///
/// Accepting one puts the variable in the field and not its value, as [`PathComplete::accept`]
/// keeps every prefix as it was typed; the path is what Enter and a click go to.
pub(crate) fn variables(
    typed: &str,
    vars: impl Iterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = vars
        .filter_map(|(name, value)| {
            let (name, value) = (name.into_string().ok()?, value.into_string().ok()?);
            // Windows keeps a few `=C:`-style entries for each drive's current directory.
            if name.is_empty() || name.starts_with('=') || value.contains(';') {
                return None;
            }
            let path = PathBuf::from(value.trim());
            if !path.is_absolute() {
                return None;
            }
            let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
            if fs::fmt::file_type(ext).is_some() {
                return None;
            }
            let label = format!("%{name}%");
            starts_with_folded(&label, typed).then_some((label, path))
        })
        .collect();
    out.sort_by(|a, b| fs::sort::natural_cmp(&a.0, &b.0));
    out
}

/// Whether `name` begins with `typed`, whatever case either of them is in.
///
/// Character by character rather than `to_lowercase()` on both, because this runs once per entry
/// per keystroke over folders that can hold thousands of them: the allocation is the expensive
/// part, and a comparison stops at the first character that differs. `char::to_lowercase` rather
/// than the ASCII fold, because a folder can be named in any language and this is the one place
/// where getting that wrong means a folder you can see is not offered.
pub(crate) fn starts_with_folded(name: &str, typed: &str) -> bool {
    let mut name = name.chars().flat_map(char::to_lowercase);
    let mut typed = typed.chars().flat_map(char::to_lowercase);
    loop {
        match (typed.next(), name.next()) {
            // Everything typed has been matched.
            (None, _) => return true,
            // The name ran out first, or the two disagree.
            (Some(_), None) => return false,
            (Some(a), Some(b)) => {
                if a != b {
                    return false;
                }
            }
        }
    }
}

/// Put the caret at the end of the field.
///
/// For the one moment the field's text is changed by something other than the field: a name has
/// just been appended, and egui keeps a `TextEdit`'s caret in its own memory, where it knows
/// nothing about a buffer written from outside. Without this the caret stays where the typing left
/// it and the next keystroke lands in the middle of the name that was just completed.
///
/// Stored after the widget has run, so it is the *next* frame that reads it — which is the frame
/// the next keystroke arrives in.
pub(crate) fn caret_to_end(ctx: &egui::Context, id: Id, text: &str) {
    use egui::text::{CCursor, CCursorRange};

    let Some(mut state) = egui::TextEdit::load_state(ctx, id) else {
        return;
    };
    state
        .cursor
        .set_char_range(Some(CCursorRange::one(CCursor::new(text.chars().count()))));
    state.store(ctx, id);
}

/// A path written with the separator the path field is set to show.
///
/// Both directions, because the setting can be turned off as well as on, and exact either way:
/// **neither slash can appear in a Windows file name**, so every one of them in a path is a
/// separator and nothing else. It is the same length in bytes as what came in, which is why nothing
/// has to be done about the caret afterwards — the character it sits in front of is still there.
///
/// Not [`crate::fs::normalize`], which is the same swap in the one direction the disk cares about
/// and hands back a `PathBuf`. This is about what the field *shows*, and what the field holds is a
/// `String` somebody may be halfway through typing.
pub(crate) fn with_separator(text: &str, slashes: bool) -> String {
    if slashes {
        text.replace('\\', "/")
    } else {
        text.replace('/', "\\")
    }
}

/// The editable path field, and the completions under it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn edit_field(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    pane: PaneId,
    tab: &mut Tab,
    complete: &mut PathComplete,
    loader: &mut crate::loader::Loader,
    icons_cache: &mut crate::shell::icons::Icons,
    recall: &Recall<'_>,
    slashes: bool,
    out: &mut Vec<Action>,
) {
    let field = Rect::from_min_size(
        pos2(rect.left(), (rect.center().y - TOOL_SIZE * 0.5).round()),
        vec2(rect.width(), TOOL_SIZE),
    );

    // **What is on offer, before anything is drawn and before a key is read.** Both of the things
    // below need it: what Down and Right do is a question about whether there is anything to move
    // onto. See [`PathComplete::refresh`], which is cheap on the frames where nothing changed.
    complete.refresh(pane, &tab.edit_text, loader, recall);

    // ---- The keys the dropdown takes -------------------------------------
    //
    // Read *and consumed* here, above the widget, because every one of them already means
    // something to a `TextEdit`: Up and Down move the caret, Right moves it a character, Enter
    // surrenders the keyboard and Tab hands it to the next widget in the window. A field that saw
    // them first would answer them first, and this is the only place they can be taken out of the
    // queue before it runs.
    //
    // Right and Enter are only taken while an offer is highlighted. With nothing highlighted they
    // are the field's own, which is the whole reason the highlight starts at nothing: `Enter` on a
    // path you have typed out in full goes there, as it always has, and `Right` moves the caret.
    // Nothing is ever highlighted while the dropdown is down — every path that puts it away clears
    // the highlight with it — so those two need no separate test for that.
    let mut pick: Option<Pick> = None;
    if complete.len() > 0 {
        ui.input_mut(|i| {
            const NONE: egui::Modifiers = egui::Modifiers::NONE;
            // Down and Up put the dropdown up if it is not up *and* land on an offer, in the one
            // press: an arrow that only revealed a list, and then had to be pressed again to move
            // into it, would be a wasted press every single time.
            if i.consume_key(NONE, egui::Key::ArrowDown) {
                complete.step(true);
            }
            if i.consume_key(NONE, egui::Key::ArrowUp) {
                complete.step(false);
            }
            // **Tab completes with nothing highlighted and with the dropdown still down**, taking
            // the first offer — which is what Tab has meant in every shell for forty years, and
            // what somebody who typed `Ctrl+L`, three letters and Tab is asking for. It is also
            // why the field has to hold on to the key; see [`keep_tab`].
            if i.consume_key(NONE, egui::Key::Tab) {
                if complete.hot.is_none() {
                    complete.step(true);
                }
                pick = Some(Pick::Append);
            }
            if complete.hot.is_some() {
                if i.consume_key(NONE, egui::Key::ArrowRight) {
                    pick = Some(Pick::Append);
                }
                if i.consume_key(NONE, egui::Key::Enter) {
                    pick = Some(Pick::Go);
                }
            }
            // **`Shift+Delete` forgets a place recalled from the history**, as it does in a
            // browser's bar. Only on one of those: on a folder's own offer it would be a delete
            // key next to a folder, which is the last thing this field should look like it does.
            if let Some(offer) = complete.hot.and_then(|at| complete.recalled.get(at)) {
                if i.consume_key(egui::Modifiers::SHIFT, egui::Key::Delete) {
                    out.push(Action::ForgetVisit(offer.path.clone()));
                }
            }
        });
    }

    // Where the field now points, if a key sent it somewhere.
    let mut go: Option<PathBuf> = None;
    // Whether the text was written from out here, and so whether the caret has to be moved.
    let mut appended = false;
    match pick {
        Some(Pick::Append) => {
            if complete.accept(&mut tab.edit_text, slashes).is_some() {
                appended = true;
                // What is on offer now is what is inside the folder just named. Refreshed here
                // rather than left to the next frame, so the dropdown never spends a frame
                // showing the old folder's children under a field that has moved on.
                complete.refresh(pane, &tab.edit_text, loader, recall);
            }
        }
        Some(Pick::Go) => {
            go = complete
                .hot
                .and_then(|at| complete.target(at))
                .map(Path::to_path_buf);
        }
        None => {}
    }

    // The dropdown hangs off *this* rather than off the field's own response: a `TextField` hands
    // back its inner `TextEdit`'s, whose rect is the field minus its padding, and a menu aligned
    // to that sits a few points in from the edge the eye reads the field by. Registered before the
    // field, and sensing hover only, so the field is what the pointer lands on.
    let anchor = ui.interact(field, Id::new(("crumb-complete", pane)), Sense::hover());

    // Square, like the breadcrumb it replaces. See [`crate::ui::squared`].
    let response = crate::ui::squared(ui, |ui| {
        ui.put(
            field,
            azur_egui_theme::components::TextField::new(&mut tab.edit_text)
                .size(Size::Small)
                .width(rect.width()),
        )
    });
    // The field is created and focused in the same frame it is opened.
    if !response.has_focus() && !response.lost_focus() {
        response.request_focus();
    }
    if appended {
        caret_to_end(ui.ctx(), response.id, &tab.edit_text);
    }
    keep_tab(ui, response.id);

    // ---- The field's own menu --------------------------------------------
    //
    // Which slash the field writes, and where its suggestions come from. See [`field_menu`].
    let menu_rect = field_menu(
        ui,
        &response,
        slashes,
        recall.bookmarks.is_some(),
        !recall.visits.paused(),
        out,
    );
    // **A click in that menu is not a click away from the field.**
    //
    // A field surrenders the keyboard to a click outside it — egui's rule, and the right one — and a
    // tick in a popup is a click outside it. So the frame that ticks the entry would also be the
    // frame that closes the field and puts the breadcrumb back, which is the one thing this setting
    // has to be able to show: the path, still there, now written with the other slash. The field
    // asks for the keyboard back as soon as it notices it has gone, and this is what keeps it from
    // being thrown away in between.
    //
    // Keyed on the pointer being *in* the menu rather than on the menu being open, because those
    // are not the same question. `egui::Popup::show` decides to close after its body has run, so
    // the frame a click *outside* the popup lands on is a frame the popup is still open for — an
    // open menu is exactly what clicking away into the listing looks like as well, and where the
    // pointer is is the only thing that tells the two apart. egui goes on reporting `lost_focus`
    // for as long as nothing else has taken the keyboard, so the coarser test closes the field a
    // frame late rather than never; a frame late is still a frame spent holding the keyboard over a
    // listing somebody has already clicked in, and resting on that is resting on a detail of how
    // egui keeps its focus history.
    let in_menu = menu_rect.is_some_and(|rect| {
        ui.input(|i| i.pointer.interact_pos())
            .is_some_and(|at| rect.contains(at))
    });
    // And back on the same frame, not the next one. The field gives the keyboard up while the
    // press in the menu is being handled, and the check above only notices on the frame after —
    // a frame in which a key typed straight after the tick has nowhere to go.
    if in_menu && !ui.ctx().memory(|m| m.has_focus(response.id)) {
        response.request_focus();
    }

    // ---- The offers ------------------------------------------------------
    //
    // Built every frame the field is up, open or not, for the same reason the chevrons' menus are:
    // the popup is what puts itself up, so one constructed only once it is already open is one
    // nothing can open. `open` comes back `false` when it has put itself away — `Escape`, or a
    // click outside it — which is how this learns to stay down until asked again.
    //
    // **Assembled here rather than with `azur::components::Menu`, and the whole reason is the
    // height.** The frame, the rows and the widths are that component's, taken from it directly so
    // this dropdown and a chevron's cannot drift apart; what it cannot express is a list that has
    // to be a stated number of rows tall *whatever it was tall a moment ago*. See
    // [`offers_height`], which is the bug this is written around.
    let showing = !complete.hidden && complete.len() > 0;
    let mut open = showing;
    // The pointer has moved, or pressed — so the rows answer to it again. See
    // [`PathComplete::quiet`], and [`crate::ui::pointer_woke`] for why a press counts on its own.
    if crate::ui::pointer_woke(ui.ctx()) {
        complete.quiet = false;
    }
    let quiet = complete.quiet;
    // The row the pointer is over, which becomes the highlighted one — see below.
    let mut hovered: Option<usize> = None;
    let hot = complete.hot;
    let follow = std::mem::take(&mut complete.follow);
    let mut clicked: Option<PathBuf> = None;
    let width = field.width();
    let wanted = offers_height(complete.recalled.len(), complete.offers.len());
    egui::Popup::menu(&anchor)
        .open_bool(&mut open)
        // **Under the field, always.** A popup that does not fit picks another side by itself, and
        // for a bar along the top of a pane the other side is two rows of nothing above it. There
        // is never more room up there, so there is never a decision to make.
        .align(egui::RectAlign::BOTTOM_START)
        .align_alternatives(&[])
        .frame(azur_egui_theme::components::menu_frame(t))
        .gap(space::S1)
        // As wide as the field, so the two read as one control rather than as a menu that happens
        // to have opened nearby.
        .width(width)
        .show(|ui| {
            // **The room the rows need, taken rather than asked for**, and covered while the
            // keyboard has them. See [`crate::ui::claimed_list`] for the `Area` and `ScrollArea`
            // behaviour that made a dropdown asking for its height stay at the size of the first
            // list it ever showed — which is the bug this is written around.
            let origin = ui.max_rect().min;
            crate::ui::claimed_list(
                ui,
                origin,
                width,
                wanted,
                ("crumb-complete", pane),
                quiet,
                Id::new(("crumb-complete-quiet", pane)),
                |ui| {
                    offers(
                        ui, t, complete, hot, follow, icons_cache, slashes, quiet,
                        &mut hovered, &mut clicked,
                    );
                },
            );
        });
    // The popup has put itself away — `Escape`, or a click outside it. Recorded, or it would come
    // straight back up on the next frame; the next keystroke brings it back, and so does Down.
    //
    // **Unless the click was in the field**, which is somebody placing the caret rather than
    // dismissing anything. It *is* a click outside the popup, so the popup was right to close, and
    // this is the only place that knows better. No frame is lost putting it back: a popup draws
    // itself before it decides to go.
    if showing && !open && !response.clicked() {
        complete.hidden = true;
        complete.hot = None;
    }
    // **The highlight goes where the pointer does**, once the pointer is moving: one highlight,
    // the keyboard's and the pointer's being the same thing, so `Enter` goes where the eye is. The
    // next arrow key carries on from there.
    if !quiet && hovered.is_some() {
        complete.hot = hovered;
    }
    // Somewhere the field is to go, by the arrow keys or by a click on an offer.
    let go = go.or(clicked);

    let (enter, escape) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::Enter),
            i.key_pressed(egui::Key::Escape),
        )
    });
    // `Escape` with the menu up closes the menu, which egui has already done by here. One press
    // dismisses one thing; the next one throws the field away, as it always has.
    let escape = escape && menu_rect.is_none();
    if escape || response.lost_focus() && !enter && !in_menu {
        tab.editing_path = false;
    }
    // An offer was chosen. No `resolve_input`, and so no question asked of the disk: the offer came
    // out of a listing, and it carries the path it came from.
    if let Some(path) = go {
        tab.editing_path = false;
        out.push(Action::Navigate { pane, path });
    } else if enter {
        tab.editing_path = false;
        // **No `is_file` here.** `resolve_input` already stat'ed the path and says which it is; a
        // second question of the disk from inside the frame is a second chance to block on a share
        // that has gone away. See [`fs::Typed`].
        match fs::resolve_input(&tab.edit_text) {
            Some(fs::Typed::File(path)) => out.push(Action::Open(path)),
            Some(fs::Typed::Folder(path)) => out.push(Action::Navigate { pane, path }),
            // Control Panel and the like: somewhere only Explorer can draw, so Explorer is asked
            // to — `Reveal` is what opens a shell name there. See [`fs::shell::reveal`].
            Some(fs::Typed::Elsewhere(name)) => out.push(Action::Reveal(name)),
            // Nothing there. Leave the text as typed so it can be corrected. **Never a UNC path**,
            // which is handed on whether it answered or not — see [`fs::resolve_input`].
            None => tab.editing_path = true,
        }
    }
    if !tab.editing_path {
        complete.close(pane);
    }
}

/// How tall the dropdown is for a given number of offers.
///
/// [`OFFERS_SHOWN`] rows, or fewer if there are fewer — it is a ceiling and not a size, because a
/// menu padded out to a fixed height with empty space would be a menu claiming to hold something it
/// does not.
///
/// **A whole number of rows**, which is why the figure is stated rather than left to
/// `Menu`'s own 320-point default: that is 11.43 rows, so the twelfth came out as a two-point sliver
/// along the bottom edge. A list you walk with the arrow keys should end where a row ends — a
/// half-drawn one reads as a rendering fault, and "there is more below" is the scrollbar's job.
///
/// The places recalled count as rows like any other, and the divider under them is added on top:
/// they are at the head of the list, so a dropdown scrolled to the top shows them, the divider and
/// then whole rows down to the bottom edge.
pub(crate) fn offers_height(recalled: usize, offers: usize) -> f32 {
    let rows = (recalled + offers).clamp(1, OFFERS_SHOWN) as f32;
    let divider = if recalled > 0 && offers > 0 {
        azur_egui_theme::components::menu_divider_height()
    } else {
        0.0
    };
    rows * azur_egui_theme::components::menu_item_height() + divider
}

/// The rows of the dropdown: the places recalled, a divider, and the folder's own offers.
///
/// Split out of [`edit_field`] only because the popup body it goes in is already three closures
/// deep; nothing here is reusable and nothing else calls it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn offers(
    ui: &mut Ui,
    t: &Theme,
    complete: &PathComplete,
    hot: Option<usize>,
    follow: bool,
    icons_cache: &mut crate::shell::icons::Icons,
    slashes: bool,
    quiet: bool,
    hovered: &mut Option<usize>,
    clicked: &mut Option<PathBuf>,
) {
    // Every row as drawn, with the slot reserved behind it for the highlight — painted once they
    // are all down, because which row it goes on depends on the pointer. See
    // [`crate::ui::paint_lit`].
    let mut rows: Vec<(egui::layers::ShapeIdx, Rect)> = Vec::with_capacity(complete.len());
    // One row of either kind. Its index in the dropdown is how many came before it, which is what
    // `hot` and `hovered` count in — the places recalled first, then the folder's own offers.
    // `aside` is drawn at the row's right, where a menu entry puts its shortcut; empty is nothing.
    let mut add = |ui: &mut Ui,
                   label: String,
                   aside: String,
                   icon: azur_icons::Icon<'_>,
                   path: &Path| {
        let index = rows.len();
        // Into view on the frame the highlight moved, to the middle of the list, and only if it is
        // not already in view — see [`crate::ui::list_row`].
        let (slot, row) = crate::ui::list_row(
            ui,
            MenuItem::new(label).shortcut(aside).icon(icon),
            follow && hot == Some(index),
            Some(egui::Align::Center),
        );
        rows.push((slot, row.rect));
        if row.hovered() {
            *hovered = Some(index);
        }
        // Clicking an offer goes there, which is what clicking an entry in any of this bar's
        // dropdowns does.
        if row.clicked() {
            *clicked = Some(path.to_path_buf());
        }
    };

    for offer in &complete.recalled {
        // The whole path, since the point of these is that they are not inside the folder typed —
        // and in the separator the field is showing, so the row reads as what Right would put there.
        // A star for a bookmark, a clock for somewhere you have only been: painted rather than the
        // shell's, because asking the shell for a folder's icon is asking it to go and look, and the
        // folder may be on a share that has gone.
        let label = with_separator(&offer.path.display().to_string(), slashes);
        let glyph: azur_icons::Icon<'_> = if offer.bookmarked {
            &icons::star_filled
        } else {
            &icons::history
        };
        add(ui, label, String::new(), glyph, &offer.path);
    }
    if !complete.recalled.is_empty() && !complete.offers.is_empty() {
        azur_egui_theme::components::menu_divider(ui);
    }
    for (name, path) in &complete.offers {
        // Windows' own icon, exactly as a chevron dropdown gets it: by path for a volume,
        // because that is the only way to a drive's own glyph, and by kind for a folder,
        // which is one lookup shared by every row here and for the rest of the session.
        let paint = texture_or_folder(crumb_icon(icons_cache, path));
        // A variable says where it leads, in the separator the field is showing: `%TEMP%` alone
        // does not tell you which of two temp folders it is. Only a variable can start with `%`
        // where nothing names a folder yet — see [`variables`].
        let aside = if complete.folder.is_none() && name.starts_with('%') {
            with_separator(&path.display().to_string(), slashes)
        } else {
            String::new()
        };
        add(ui, name.clone(), aside, &paint, path);
    }
    // The keyboard's highlight moves to the hovered row in [`edit_field`] only after this frame;
    // this is where the frame itself agrees. See [`PathComplete::quiet`].
    crate::ui::paint_lit(ui, t, &rows, hot, *hovered, quiet);
    if complete.truncated {
        truncated_note(ui);
    }
}
