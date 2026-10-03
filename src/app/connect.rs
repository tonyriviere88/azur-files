//! Signing in to a share, when a listing came back saying who wants to know.
//!
//! Explorer's behaviour, and worth being precise about what that is: opening a UNC path that the
//! server will not serve unauthenticated raises **Windows' own** credential dialog, and if you
//! satisfy it the folder opens. Not a dialog of this program's — see
//! [`crate::fs::drives::connect`], which hands the whole thing to `mpr.dll` so that the saved
//! accounts, the domain list and "Remember my credentials" are the ones the rest of the system
//! uses. No password is ever in this process.
//!
//! Three things make it safe to do automatically:
//!
//! - **It is raised by a *navigation*, never by a prefetch.** Moving the cursor down a listing
//!   quietly reads the folder under it — see [`crate::loader::Priority`] — and a prefetch that
//!   could pop a modal dialog would make the arrow keys dangerous. The trigger is in
//!   [`super::App::collect_scans`], keyed on a listing a *tab asked for*, which a prefetch's
//!   answer never is.
//! - **Once per path.** The dialog is cancellable, and a cancel that led straight back to a
//!   re-read would raise it again for ever. [`Connecting::asked`] remembers, and only an explicit
//!   retry — F5 — clears it.
//! - **Never from a test.** It shows a dialog and makes a real network connection, so it answers to
//!   the same switch a job handed to `IFileOperation` does — [`crate::shell::ops::FOR_REAL`],
//!   checked one line above the syscall in [`crate::fs::drives::connect`] rather than by a `cfg` on
//!   the spawn below. Everything decidable without the network — which paths qualify, what gets
//!   signed in to, the once-only rule — is tested below.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

use super::{App, PaneId};

/// What came back from a sign-in: which pane was waiting, and whether to look again.
pub struct Signed {
    pane: PaneId,
    path: PathBuf,
    worked: bool,
}

/// The paths already asked about, and the channel the answers come back on.
pub struct Connecting {
    /// Paths a dialog has already been raised for. Not a `bool`: two panes can be sitting on two
    /// different unreachable shares, and each is one question.
    asked: HashSet<PathBuf>,
    answers: Receiver<Signed>,
    /// Kept so the channel stays open while a dialog is up.
    sender: Sender<Signed>,
}

impl Default for Connecting {
    fn default() -> Self {
        let (sender, answers) = channel();
        Self {
            asked: HashSet::new(),
            answers,
            sender,
        }
    }
}

impl Connecting {
    /// Forget that a path was asked about, so a retry asks again.
    ///
    /// F5 on a share that was refused is a request to try again, including with a different
    /// account — so it has to be able to raise the dialog a second time.
    pub fn forget(&mut self, path: &Path) {
        self.asked.remove(path);
    }
}

impl App {
    /// Raise the credential dialog for `path`, unless it has already been raised for it.
    ///
    /// Returns nothing and reports nothing: the answer arrives on the channel, and the only
    /// visible outcome is the listing being read again.
    pub(super) fn ask_credentials(&mut self, pane: PaneId, path: &Path, ctx: &egui::Context) {
        // A connection is to a machine or a share, never to a folder inside one.
        let Some(target) = crate::fs::drives::connect_target(path) else {
            return;
        };
        // Keyed on the *path that failed* rather than the target, so two refused shares on one
        // server are two questions — and so F5 on the folder you are looking at clears the one
        // that concerns it.
        if !self.connecting.asked.insert(path.to_path_buf()) {
            return;
        }

        let sender = self.connecting.sender.clone();
        let path = path.to_path_buf();
        let owner = self.owner;
        let ctx = ctx.clone();

        // Detached, like a volume probe: a dialog nobody answered when the window closes is not
        // worth holding the process open for.
        //
        // **Nothing here is gated on `cfg(test)`** — the guard is inside
        // [`crate::fs::drives::connect`], one line above the syscall, which is where a rule about
        // not touching the real system belongs. Gating this call site instead made the function
        // dead code in a test build, which is a warning telling the truth: a guard the compiler
        // removes along with its subject has not been tested either.
        let _ = std::thread::Builder::new()
            .name("connect".to_owned())
            .spawn(move || {
                let worked = crate::fs::drives::connect(&target, owner).is_ok();
                if sender.send(Signed { pane, path, worked }).is_ok() {
                    ctx.request_repaint();
                }
            });
    }

    /// Take delivery of any sign-in that has finished, and read the folder again if it worked.
    ///
    /// A failure is deliberately silent. The two ways to get one are the user pressing Cancel and
    /// the credentials being wrong, and in both cases the listing already says *Access denied* in
    /// the middle of the pane — a second message would be this program telling somebody what they
    /// had just typed.
    pub(super) fn collect_connections(&mut self, ctx: &egui::Context) {
        let signed: Vec<Signed> = self.connecting.answers.try_iter().collect();
        for done in signed {
            if !done.worked {
                continue;
            }
            // The share is reachable now, so nothing cached about it is right — including the
            // machine's share list, which was refused a moment ago and will answer this time.
            self.loader.invalidate(&done.path);
            self.connecting.forget(&done.path);
            // A sign-in Windows accepted has just made a connection, and the panel's machine list is
            // otherwise only rebuilt on F5 and on regaining focus — so the row for the machine
            // somebody has this moment authenticated to would not appear until something else
            // happened to ask. See [`crate::loader::Volumes::relist`].
            self.volumes.relist();
            self.perform(ctx, super::Action::Refresh(done.pane));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::drives::connect_target;

    /// What gets signed in to, which is never the folder that failed.
    ///
    /// A server authenticates a *tree* — a share — so a refused folder five levels down is signed
    /// in to at its share, and everything under it opens with no further asking. A bare machine
    /// stays a machine: a session has to exist before it will say what shares it has, which is the
    /// `\\DESKTOP-3A9VGG7` case where even listing the shares is denied.
    #[test]
    fn a_sign_in_is_to_a_machine_or_a_share() {
        let target = |text: &str| connect_target(Path::new(text)).map(|p| p.display().to_string());
        assert_eq!(
            target("\\\\fileserver\\web\\owncloud\\apps"),
            Some("\\\\fileserver\\web".to_owned())
        );
        assert_eq!(
            target("\\\\fileserver\\web"),
            Some("\\\\fileserver\\web".to_owned())
        );
        assert_eq!(
            target("\\\\fileserver"),
            Some("\\\\fileserver".to_owned()),
            "a machine is signed in to as itself, or it will not list its shares"
        );
        assert_eq!(
            target("\\\\fileserver\\"),
            Some("\\\\fileserver".to_owned()),
            "a trailing separator is not a share"
        );
        // Nobody to sign in to.
        assert_eq!(target("C:\\Windows"), None);
        assert_eq!(target("\\\\"), None);
    }

    /// Which failures are worth a dialog, and the half of the test that is about the *path*.
    #[test]
    fn only_a_network_path_is_worth_a_credential_prompt() {
        use crate::fs::scan::wants_credentials;
        let unc = Path::new("\\\\fileserver\\Alice");
        let local = Path::new("C:\\Windows\\System32\\config");

        // `ERROR_ACCESS_DENIED` is the ambiguous one, and the path is what settles it: the same
        // code on a local folder means an ACL that excludes this account, where a credential
        // dialog could not succeed and would cover the message that explained why.
        assert!(wants_credentials(5, unc));
        assert!(!wants_credentials(5, local));
        // The unambiguous ones.
        assert!(wants_credentials(1326, unc), "ERROR_LOGON_FAILURE");
        assert!(wants_credentials(86, unc), "ERROR_INVALID_PASSWORD");
        assert!(wants_credentials(1244, unc), "ERROR_NOT_AUTHENTICATED");
        // And the failures no sign-in fixes. 1219 is the one worth naming: Windows refuses a
        // second identity for a server already connected under another, so a prompt returns the
        // same error with a dialog in front of it.
        assert!(!wants_credentials(1219, unc), "credential conflict");
        assert!(!wants_credentials(53, unc), "the machine is not there");
        assert!(!wants_credentials(2, unc), "the folder is gone");
        assert!(!wants_credentials(161, unc), "ERROR_BAD_PATHNAME");
    }

    /// One dialog per path, until something asks again.
    ///
    /// The guard that keeps a cancelled prompt from coming straight back: the listing is read
    /// again on the way past, fails again, and would ask again for ever.
    #[test]
    fn a_path_is_asked_about_once() {
        let mut connecting = Connecting::default();
        let share = PathBuf::from("\\\\fileserver\\Alice");
        assert!(connecting.asked.insert(share.clone()), "the first ask");
        assert!(!connecting.asked.insert(share.clone()), "and not a second");
        // F5 is the retry, and it has to be able to raise the dialog again — a different account
        // is exactly why somebody would press it.
        connecting.forget(&share);
        assert!(connecting.asked.insert(share), "after a refresh, once more");
    }
}
