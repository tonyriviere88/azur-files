//! Which key does what: the one table every printed shortcut and every bound one is read from.
//!
//! The keys used to be written down twice — once as the `if` in [`App::keyboard`] that acts on a
//! press, and again as the text the command palette, the menus and the tooltips print beside the
//! command — and nothing held the two together. They had already parted: the palette called the
//! console's key ``Ctrl+` `` while its own button called it `Ctrl+²`, and `Alt+D` edited the path
//! without anything saying so. Now [`BINDINGS`] is the only place a chord is spelled. The keyboard
//! asks it whether a command's chord was pressed, and everything that prints a shortcut asks it for
//! the text, so a key moved here moves everywhere at once.
//!
//! # What the table decides, and what it does not
//!
//! **Whether a press is a command's.** Not what the command then does: the [`Action`] is still built
//! in [`App::keyboard`], next to the state it needs (which tab is up, what is selected), exactly as
//! before. And not *when* a key is read — a field with the caret, a menu, a video holding the arrows
//! all still decide that where they always did.
//!
//! **Modifiers match exactly.** `Ctrl+T` is Ctrl and T and nothing else, so `Ctrl+Shift+T` is not
//! also a new tab and no order of checks is needed to keep them apart. egui's own
//! [`egui::InputState::consume_key`] matches *logically* — an extra Shift or Alt is ignored — which
//! is why `Ctrl+Shift+P` once opened the preview panel along with the palette. AltGr arrives on
//! Windows as Ctrl+Alt, so exact matching is also what keeps a character typed with it from being
//! read as a chord.
//!
//! [`Chord::Named`] is a key this table prints and does not match. Mostly because the press never
//! reaches a `key_pressed`: `egui-winit` turns Ctrl+X, C and V into clipboard events, egui does its
//! own zoom, Windows keeps `Alt+F4`, and egui does not carry the Windows key at all — see
//! `App::window_keys`. And `Enter`, which the listing reads with the cursor's own keys because
//! what it opens depends on them. They are listed so they are printed from here like the rest.
//!
//! # Several chords
//!
//! A command can have more than one (`F5` and `Ctrl+R`). The first is what gets printed; any of them
//! acts.
//!
//! [`App::keyboard`]: super::App
//! [`Action`]: super::Action

pub(crate) use super::palette::Command;

use egui::{InputState, Key, Modifiers};

/// One way to ask for a command.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Chord {
    /// A key with exactly these modifiers held, read by [`App::keyboard`](super::App).
    Key(Modifiers, Key),
    /// A key read somewhere this table does not decide (see the module header): printed, never
    /// matched.
    Named(&'static str),
}

const fn bare(key: Key) -> Chord {
    Chord::Key(Modifiers::NONE, key)
}

const fn ctrl(key: Key) -> Chord {
    Chord::Key(Modifiers::COMMAND, key)
}

const fn ctrl_shift(key: Key) -> Chord {
    Chord::Key(Modifiers::COMMAND.plus(Modifiers::SHIFT), key)
}

const fn alt(key: Key) -> Chord {
    Chord::Key(Modifiers::ALT, key)
}

const fn shift(key: Key) -> Chord {
    Chord::Key(Modifiers::SHIFT, key)
}

/// The command palette's own key. Not a [`Command`], since the palette does not list itself.
pub(crate) const PALETTE: Chord = ctrl_shift(Key::P);

/// `Ctrl+Enter` in the filter box: select what the filter matched. Not a [`Command`] either: it means
/// something only while the box has the caret, so the box reads it, and the status line prints it
/// from here as the hint that says it exists.
pub(crate) const FILTER_SELECT: Chord = ctrl(Key::Enter);

/// Every bound command, in the order the palette lists them.
pub(crate) static BINDINGS: &[(Command, &[Chord])] = &[
    // Tabs and panes
    (Command::NewTab, &[ctrl(Key::T)]),
    (Command::ReopenTab, &[ctrl_shift(Key::T)]),
    (Command::CloseTab, &[ctrl(Key::W)]),
    (Command::NextTab, &[ctrl(Key::Tab)]),
    (Command::PreviousTab, &[ctrl_shift(Key::Tab)]),
    (Command::SplitRight, &[ctrl(Key::Backslash)]),
    // Navigation
    (Command::Back, &[alt(Key::ArrowLeft)]),
    (Command::Forward, &[alt(Key::ArrowRight)]),
    (Command::Up, &[alt(Key::ArrowUp), bare(Key::Backspace)]),
    (Command::Refresh, &[bare(Key::F5), ctrl(Key::R)]),
    (Command::EditPath, &[ctrl(Key::L), alt(Key::D)]),
    // Read by the filter box itself, which needs its own response to hand the caret to. A bare key
    // there stands aside while another field has the keyboard — see `ui::breadcrumb`.
    (Command::Filter, &[ctrl(Key::F), bare(Key::F3)]),
    (Command::Bookmark, &[ctrl(Key::D)]),
    // View: this pane. `Backtick` is the leftmost key of the number row by position, which is `²`
    // on the keyboard this was written on — see the console's key in `App::keyboard`.
    (Command::Tiles, &[ctrl(Key::Num1)]),
    (Command::Sizes, &[ctrl(Key::Num2)]),
    (Command::Console, &[ctrl(Key::Backtick)]),
    (Command::Preview, &[ctrl(Key::P)]),
    (Command::Flatten, &[ctrl(Key::E)]),
    // View: the window's
    (Command::Hidden, &[ctrl(Key::H)]),
    (Command::Sidebar, &[ctrl(Key::B), Chord::Named("Ctrl+Win+←")]),
    (Command::ZoomIn, &[Chord::Named("Ctrl++")]),
    (Command::ZoomOut, &[Chord::Named("Ctrl+-")]),
    (Command::ZoomReset, &[Chord::Named("Ctrl+0")]),
    // Files. `Enter` is read with the listing's own keys, beside the cursor it opens.
    (Command::Open, &[Chord::Named("Enter")]),
    (Command::Cut, &[Chord::Named("Ctrl+X")]),
    (Command::Copy, &[Chord::Named("Ctrl+C")]),
    (Command::Paste, &[Chord::Named("Ctrl+V")]),
    (Command::CopyPaths, &[ctrl_shift(Key::C)]),
    (Command::Rename, &[bare(Key::F2)]),
    (Command::NewFolder, &[ctrl_shift(Key::N)]),
    (Command::Delete, &[bare(Key::Delete)]),
    (Command::DeletePermanently, &[shift(Key::Delete)]),
    (Command::Undo, &[ctrl(Key::Z)]),
    (Command::Redo, &[ctrl(Key::Y), ctrl_shift(Key::Z)]),
    (Command::SelectAll, &[ctrl(Key::A)]),
    (Command::ClearSelection, &[bare(Key::Escape)]),
    (Command::InvertSelection, &[ctrl(Key::I)]),
    (Command::SelectSameExtension, &[ctrl_shift(Key::A)]),
    // Window
    (Command::SpanScreens, &[Chord::Named("Ctrl+Win+↑")]),
    (Command::ResetSize, &[Chord::Named("Ctrl+Win+↓")]),
    (Command::CloseWindow, &[Chord::Named("Alt+F4")]),
];

/// The chords bound to `command`, the one to print first. Empty for none.
pub(crate) fn chords(command: &Command) -> &'static [Chord] {
    // A batch rename is what `F2` begins over a selection of more than one, so it prints the key of
    // the rename it is.
    let command = match command {
        Command::Batch(_) => &Command::Rename,
        command => command,
    };
    BINDINGS
        .iter()
        .find(|(bound, _)| bound == command)
        .map_or(&[], |(_, chords)| chords)
}

/// Whether one of `command`'s chords was pressed this frame.
pub(crate) fn pressed(i: &InputState, command: &Command) -> bool {
    chords(command).iter().any(|chord| chord.pressed(i))
}

/// The text printed beside `command`: its first chord, or nothing.
pub(crate) fn label(command: &Command) -> String {
    chords(command).first().map(Chord::label).unwrap_or_default()
}

/// `tip` with `command`'s key after it in brackets, as a tooltip names one — or `tip` alone.
pub(crate) fn tip(tip: &str, command: &Command) -> String {
    match label(command) {
        key if key.is_empty() => tip.to_owned(),
        key => format!("{tip} ({key})"),
    }
}

impl Chord {
    /// Pressed this frame, with exactly its modifiers held.
    pub(crate) fn pressed(&self, i: &InputState) -> bool {
        match *self {
            Self::Key(modifiers, key) => i.modifiers.matches_exact(modifiers) && i.key_pressed(key),
            Self::Named(_) => false,
        }
    }

    /// As [`Self::pressed`], and taken out of the queue so nothing drawn after sees it.
    pub(crate) fn consume(&self, i: &mut InputState) -> bool {
        match *self {
            Self::Key(modifiers, key) => {
                i.modifiers.matches_exact(modifiers) && i.consume_key(modifiers, key)
            }
            Self::Named(_) => false,
        }
    }

    /// A key with no modifier, which belongs to whatever field has the caret.
    pub(crate) fn is_bare(&self) -> bool {
        matches!(self, Self::Key(modifiers, _) if modifiers.is_none())
    }

    /// `Ctrl+Shift+T`, as the window prints it.
    pub(crate) fn label(&self) -> String {
        let (modifiers, key) = match *self {
            Self::Key(modifiers, key) => (modifiers, key),
            Self::Named(text) => return text.to_owned(),
        };
        let mut text = String::new();
        if modifiers.command || modifiers.ctrl {
            text.push_str("Ctrl+");
        }
        if modifiers.alt {
            text.push_str("Alt+");
        }
        if modifiers.shift {
            text.push_str("Shift+");
        }
        text.push_str(match key {
            Key::ArrowLeft => "←",
            Key::ArrowRight => "→",
            Key::ArrowUp => "↑",
            Key::ArrowDown => "↓",
            Key::Backtick => "²",
            Key::Delete => "Del",
            Key::Escape => "Esc",
            Key::Backslash => "\\",
            key => key.name(),
        });
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_spell_the_modifiers_in_the_windows_order() {
        assert_eq!(ctrl_shift(Key::T).label(), "Ctrl+Shift+T");
        assert_eq!(alt(Key::ArrowLeft).label(), "Alt+←");
        assert_eq!(shift(Key::Delete).label(), "Shift+Del");
        assert_eq!(ctrl(Key::Num1).label(), "Ctrl+1");
        assert_eq!(ctrl(Key::Backtick).label(), "Ctrl+²");
        assert_eq!(bare(Key::F5).label(), "F5");
    }

    /// Two commands on one chord would both fire, in whatever order `App::keyboard` reads them.
    #[test]
    fn no_chord_is_bound_twice() {
        let mut seen: Vec<(Chord, &Command)> = Vec::new();
        for (command, chords) in BINDINGS {
            for chord in *chords {
                if let Some((_, other)) = seen.iter().find(|(c, _)| c == chord) {
                    panic!("{} is bound to both {other:?} and {command:?}", chord.label());
                }
                seen.push((*chord, command));
            }
        }
        assert!(
            !seen.iter().any(|(chord, _)| *chord == PALETTE),
            "the palette's own key is bound to a command too"
        );
        assert!(
            !seen.iter().any(|(chord, _)| *chord == FILTER_SELECT),
            "the filter box's Ctrl+Enter is bound to a command too"
        );
    }

    #[test]
    fn a_command_is_listed_once() {
        for (at, (command, _)) in BINDINGS.iter().enumerate() {
            assert!(
                !BINDINGS[at + 1..].iter().any(|(other, _)| other == command),
                "{command:?} has two entries, and only the first would ever be read"
            );
        }
    }
}
