//! What git says about the folder on screen, and when it is worth asking again.
//!
//! Off the UI thread, like every other answer here, and debounced: a working tree that is
//! being written to would otherwise be re-read on every notification the watcher raises.

use super::*;

/// How long a change under a watched folder or its `.git` is presumed to be our own git write
/// echoing back, rather than something worth asking git about again.
///
/// Generous against how fast the echo actually arrives: the write happens inside the same
/// `git::read` call whose answer just landed, so the two are always close, typically well under a
/// second. See [`App::collect_changes`] for the loop this exists to break, and
/// [`crate::pane::Tab::git_settled_at`] for the timestamp it is measured against.
pub(super) const GIT_WRITE_SETTLE: f64 = 2.0;

impl App {
    /// Ask git about whatever each pane is showing, and hand the answers back.
    ///
    /// **Only the active tab of each pane**, because only that one is on screen: a window with twelve
    /// tabs open would otherwise start twelve `git status` runs on every navigation, eleven of them
    /// for folders nobody is looking at. A background tab asks when it is switched to, which is the
    /// frame its listing is drawn in.
    ///
    /// Once per view of a folder. The answer is dropped if the tab has moved on — see
    /// [`crate::pane::Tab::view`] — which is the same discipline the scans and the icon lookups use,
    /// and the reason none of this needs cancelling.
    pub(super) fn collect_git(&mut self, now: f64) {
        for pane in &mut self.panes {
            let tab = pane.tab_mut();
            // A listing has to exist first: git is asked about the folder that is *on screen*, and
            // "This PC" — the synthetic listing with an empty path — is not a folder at all.
            if tab.git_asked || tab.dir.is_none() || tab.path.as_os_str().is_empty() {
                continue;
            }
            tab.git_asked = true;
            let (view, path) = (tab.view, tab.path.clone());
            self.git.request(view, &path);
            self.git_waiting += 1;
        }

        let arrived: Vec<_> = self.git.drain().collect();
        self.git_waiting = self.git_waiting.saturating_sub(arrived.len());
        for answer in arrived {
            for pane in &mut self.panes {
                for tab in &mut pane.tabs {
                    if tab.view == answer.view {
                        tab.git = answer.repo.clone();
                        tab.git_answered = true;
                        // See [`crate::pane::Tab::git_settled_at`]: the read that produced this
                        // answer may have just written the repository's own index, and that write
                        // is what `collect_changes` is about to see arrive under `.git`.
                        tab.git_settled_at = Some(now);
                        // **A listing that asked about git has been waiting for this.**
                        // [`crate::pane::Lens::Git`] cannot be evaluated until the answer is here —
                        // see [`crate::pane::Tab::git_answered`] — so until now the listing has been
                        // showing every row, and this is the frame it narrows in. Only when the lens
                        // asks: nothing else about the order depends on git, and rebuilding it for
                        // every folder in a repository would be a sort per navigation for nothing.
                        if tab.filters_on_git() {
                            tab.rebuild_order();
                        }
                    }
                }
            }
        }
    }

    /// Whether git says the row the keyboard is on has a version in the last commit that is not the
    /// one on disk.
    ///
    /// Read off the marks the listing already has — the same answer the row's own badge is drawn from,
    /// so the badge and the panel's diff button can never disagree — and so it costs a hash lookup
    /// rather than a git process. `false` for a folder outside a repository, for a listing that has not
    /// landed, and while the answer from git is still on its way.
    pub(super) fn changed_here(tab: &Tab) -> bool {
        let (Some(repo), Some(dir), Some(at)) = (tab.git.as_ref(), tab.dir.as_ref(), tab.cursor)
        else {
            return false;
        };
        tab.entry_at(at)
            .and_then(|entry| repo.state(dir.name(entry)))
            .is_some_and(crate::git::State::differs_from_head)
    }
}
