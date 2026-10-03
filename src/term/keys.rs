//! Turning what egui says happened into what a terminal expects.
//!
//! The one part of a terminal that no library can supply, because it is entirely about the window
//! toolkit on this side of it. `alacritty_terminal` parses what comes *out* of a shell; what goes
//! *in* is bytes, and deciding which bytes is this file.
//!
//! # Text is not keys
//!
//! egui reports a keystroke twice: as [`egui::Event::Key`], and — if it produced a character — as
//! [`egui::Event::Text`]. Only the second knows about the keyboard layout, dead keys and the IME, so
//! **printable characters come from `Text` and nothing else**. Reconstructing them from `Key` is how
//! a terminal ends up unable to type `é`, or typing `$` on a keyboard whose `$` is somewhere else.
//!
//! `Key` is left with what has no character: the arrows, the function keys, `Enter`, `Backspace`,
//! and the control combinations.
//!
//! # Application cursor mode is not optional
//!
//! `vim`, `less` and anything using readline put the terminal into `DECCKM`, after which an up arrow
//! is `ESC O A` rather than `ESC [ A`. A terminal that sends the wrong one has arrow keys that
//! insert letters in `vim`. So the mode is read out of the grid on the way past — see
//! [`crate::ui::term`] — and handed to [`encode`].

/// What the grid is doing that changes what a key means.
#[derive(Clone, Copy, Default, Debug)]
pub struct Mode {
    /// `DECCKM`: the cursor keys report `ESC O x` instead of `ESC [ x`.
    pub app_cursor: bool,
}

/// The modifier parameter a CSI sequence carries, in xterm's numbering.
///
/// One-based, and the bits are shift, alt, control in that order — so `Ctrl+Shift` is 1+1+4 = 6.
/// Returns `None` when nothing is held, which is the case where the *unmodified* form of the
/// sequence has to be sent: `ESC [ A` and not `ESC [ 1 ; 1 A`, which some programs do not accept.
fn modifier(mods: &egui::Modifiers) -> Option<u8> {
    let bits = u8::from(mods.shift) | (u8::from(mods.alt) << 1) | (u8::from(mods.ctrl) << 2);
    (bits != 0).then_some(bits + 1)
}

/// A cursor-key or edit-key sequence, with its modifier parameter if there is one.
fn csi(mods: &egui::Modifiers, mode: Mode, last: char, tilde: Option<u8>) -> Vec<u8> {
    match (modifier(mods), tilde) {
        // `ESC [ 1 ; 5 A` — modified cursor keys are always the bracket form, never `ESC O`.
        (Some(m), None) => format!("\x1b[1;{m}{last}").into_bytes(),
        (Some(m), Some(number)) => format!("\x1b[{number};{m}~").into_bytes(),
        (None, Some(number)) => format!("\x1b[{number}~").into_bytes(),
        // Unmodified, and this is where the application cursor mode applies.
        (None, None) if mode.app_cursor => format!("\x1bO{last}").into_bytes(),
        (None, None) => format!("\x1b[{last}").into_bytes(),
    }
}

/// The bytes a key press sends, or `None` if it sends nothing.
///
/// `None` is not failure: most keys that produce a character are deliberately handled by
/// [`text`] instead, and a modifier on its own sends nothing at all.
pub fn encode(key: egui::Key, mods: &egui::Modifiers, mode: Mode) -> Option<Vec<u8>> {
    use egui::Key as K;

    // Alt is an ESC prefix on everything it does not already change — which is how a terminal
    // spells `Meta`, and what `Alt+f`/`Alt+b` in readline are.
    let esc = |bytes: Vec<u8>| -> Vec<u8> {
        if mods.alt {
            let mut out = vec![0x1b];
            out.extend(bytes);
            out
        } else {
            bytes
        }
    };

    let bytes = match key {
        // `\r` and not `\n`. A terminal's Enter is a carriage return; sending a line feed makes
        // every shell think a line was pasted rather than entered, and `bash` swallows it.
        K::Enter => esc(vec![b'\r']),
        K::Tab if mods.shift => vec![0x1b, b'[', b'Z'],
        K::Tab => esc(vec![b'\t']),
        // DEL, not BS. This is the one everybody gets wrong: a terminal's Backspace has been
        // `0x7f` since DEC, and `0x08` is what `Ctrl+Backspace` sends — a shell configured the
        // usual way deletes a word with it.
        K::Backspace if mods.ctrl => vec![0x08],
        K::Backspace => esc(vec![0x7f]),
        K::Escape => vec![0x1b],

        K::ArrowUp => csi(mods, mode, 'A', None),
        K::ArrowDown => csi(mods, mode, 'B', None),
        K::ArrowRight => csi(mods, mode, 'C', None),
        K::ArrowLeft => csi(mods, mode, 'D', None),
        K::Home => csi(mods, mode, 'H', None),
        K::End => csi(mods, mode, 'F', None),

        K::Insert => csi(mods, mode, '~', Some(2)),
        K::Delete => csi(mods, mode, '~', Some(3)),
        K::PageUp => csi(mods, mode, '~', Some(5)),
        K::PageDown => csi(mods, mode, '~', Some(6)),

        // The function keys, in xterm's two families: the first four are `ESC O`, the rest are
        // numbered, and the numbering skips 16, 22, 27, 30 and 35 for historical reasons nobody
        // needs to know beyond writing the table out.
        K::F1 => vec![0x1b, b'O', b'P'],
        K::F2 => vec![0x1b, b'O', b'Q'],
        K::F3 => vec![0x1b, b'O', b'R'],
        K::F4 => vec![0x1b, b'O', b'S'],
        K::F5 => csi(mods, mode, '~', Some(15)),
        K::F6 => csi(mods, mode, '~', Some(17)),
        K::F7 => csi(mods, mode, '~', Some(18)),
        K::F8 => csi(mods, mode, '~', Some(19)),
        K::F9 => csi(mods, mode, '~', Some(20)),
        K::F10 => csi(mods, mode, '~', Some(21)),
        K::F11 => csi(mods, mode, '~', Some(23)),
        K::F12 => csi(mods, mode, '~', Some(24)),

        // Control codes. `Ctrl+C` is the one everything depends on, and it arrives here rather
        // than as text because egui reports no character for a control combination.
        _ if mods.ctrl => return control(key).map(|byte| esc(vec![byte])),
        // Everything else is a character, and a character comes from `Text`.
        _ => return None,
    };
    Some(bytes)
}

/// `Ctrl` plus a key, as the single byte it has meant since teletypes.
///
/// The letters are the alphabet's position — `Ctrl+A` is 1, `Ctrl+C` is 3 — and the six that are
/// not letters are the ones a shell actually uses: `Ctrl+Space` for a null, and the four above `Z`
/// that `readline` and `tmux` are bound to.
fn control(key: egui::Key) -> Option<u8> {
    use egui::Key as K;
    let byte = match key {
        K::A => 0x01,
        K::B => 0x02,
        K::C => 0x03,
        K::D => 0x04,
        K::E => 0x05,
        K::F => 0x06,
        K::G => 0x07,
        K::H => 0x08,
        K::I => 0x09,
        K::J => 0x0a,
        K::K => 0x0b,
        K::L => 0x0c,
        K::M => 0x0d,
        K::N => 0x0e,
        K::O => 0x0f,
        K::P => 0x10,
        K::Q => 0x11,
        K::R => 0x12,
        K::S => 0x13,
        K::T => 0x14,
        K::U => 0x15,
        K::V => 0x16,
        K::W => 0x17,
        K::X => 0x18,
        K::Y => 0x19,
        K::Z => 0x1a,
        K::Space => 0x00,
        K::OpenBracket => 0x1b,
        K::Backslash => 0x1c,
        K::CloseBracket => 0x1d,
        K::Minus => 0x1f,
        _ => return None,
    };
    Some(byte)
}

/// What a typed character sends.
///
/// Straight through as UTF-8. The shell is told `TERM=xterm-256color` and `COLORTERM=truecolor`
/// over a UTF-8 pseudoconsole, so there is nothing to translate — and translating is how a
/// terminal loses every character that is not ASCII.
pub fn text(typed: &str) -> Option<Vec<u8>> {
    // A control character that arrived as text would be a second copy of something `encode`
    // already sent — egui reports `Ctrl+J` as both a key and a line feed.
    let clean: String = typed.chars().filter(|c| !c.is_control()).collect();
    (!clean.is_empty()).then(|| clean.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> egui::Modifiers {
        egui::Modifiers::NONE
    }

    fn ctrl() -> egui::Modifiers {
        egui::Modifiers {
            ctrl: true,
            command: true,
            ..egui::Modifiers::NONE
        }
    }

    /// The four bytes everything else is built on, and the two that are most often wrong.
    ///
    /// `Enter` is `\r` and `Backspace` is `0x7f`. Sending `\n` for the first makes every shell
    /// treat a typed line as a paste; sending `0x08` for the second is `Ctrl+Backspace`, so a
    /// shell bound the usual way deletes a whole word every time you meant one character.
    #[test]
    fn the_bytes_a_shell_cannot_do_without() {
        let mode = Mode::default();
        assert_eq!(encode(egui::Key::Enter, &none(), mode), Some(vec![b'\r']));
        assert_eq!(encode(egui::Key::Backspace, &none(), mode), Some(vec![0x7f]));
        assert_eq!(encode(egui::Key::Backspace, &ctrl(), mode), Some(vec![0x08]));
        assert_eq!(encode(egui::Key::Tab, &none(), mode), Some(vec![b'\t']));
        assert_eq!(encode(egui::Key::Escape, &none(), mode), Some(vec![0x1b]));
        // The one every build in a terminal depends on.
        assert_eq!(encode(egui::Key::C, &ctrl(), mode), Some(vec![0x03]));
        assert_eq!(encode(egui::Key::D, &ctrl(), mode), Some(vec![0x04]));
    }

    /// An arrow key changes shape when the program asks it to.
    ///
    /// `vim`, `less` and readline all set `DECCKM`, and a terminal that keeps sending the bracket
    /// form has arrow keys that insert letters in `vim`. A *modified* arrow is always the bracket
    /// form, whatever the mode — which is the part that is easy to get wrong once the mode is
    /// handled at all.
    #[test]
    fn the_cursor_keys_follow_the_mode_the_program_asked_for() {
        let plain = Mode::default();
        let app = Mode { app_cursor: true };
        assert_eq!(encode(egui::Key::ArrowUp, &none(), plain), Some(b"\x1b[A".to_vec()));
        assert_eq!(encode(egui::Key::ArrowUp, &none(), app), Some(b"\x1bOA".to_vec()));

        // Shift is 1+1, control is 1+4: xterm's numbering, one-based.
        let shift = egui::Modifiers {
            shift: true,
            ..egui::Modifiers::NONE
        };
        assert_eq!(encode(egui::Key::ArrowLeft, &shift, plain), Some(b"\x1b[1;2D".to_vec()));
        assert_eq!(encode(egui::Key::ArrowRight, &ctrl(), app), Some(b"\x1b[1;5C".to_vec()));
        assert_eq!(
            encode(egui::Key::Home, &none(), app),
            Some(b"\x1bOH".to_vec()),
            "Home is a cursor key too"
        );
        // An unmodified sequence must not carry `;1` — some programs reject it.
        assert_eq!(encode(egui::Key::Delete, &none(), plain), Some(b"\x1b[3~".to_vec()));
        assert_eq!(encode(egui::Key::Delete, &ctrl(), plain), Some(b"\x1b[3;5~".to_vec()));
    }

    /// Alt is how a terminal spells Meta, and it is a prefix rather than a key of its own.
    #[test]
    fn alt_is_an_escape_in_front() {
        let alt = egui::Modifiers {
            alt: true,
            ..egui::Modifiers::NONE
        };
        // `Alt+Backspace` deletes a word in readline, and this is the byte pair it is.
        assert_eq!(encode(egui::Key::Backspace, &alt, Mode::default()), Some(vec![0x1b, 0x7f]));
        assert_eq!(encode(egui::Key::Enter, &alt, Mode::default()), Some(vec![0x1b, b'\r']));
    }

    /// Printable characters are the layout's business, not this file's.
    ///
    /// The whole reason `Text` is the only source: a key press reports which *key* moved, and on an
    /// AZERTY keyboard, with a dead key, or through an IME, that is not what was typed. A terminal
    /// that rebuilds characters from key codes cannot type `é` and puts `$` in the wrong place.
    #[test]
    fn characters_come_from_the_text_event_and_control_codes_do_not() {
        // Nothing to send for a plain letter: `Text` carries it.
        assert_eq!(encode(egui::Key::A, &none(), Mode::default()), None);
        assert_eq!(text("é"), Some("é".as_bytes().to_vec()));
        assert_eq!(text("ls -la"), Some(b"ls -la".to_vec()));
        // A control character in the text stream is a duplicate of what `encode` already sent.
        assert_eq!(text("\n"), None);
        assert_eq!(text("\r\n"), None);
        assert_eq!(text(""), None);
        assert_eq!(text("a\nb"), Some(b"ab".to_vec()));
    }
}
