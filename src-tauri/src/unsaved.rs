//! Which open documents have changes that are not saved.
//!
//! The window knows what was edited (it keeps the undo history), but the things that can throw
//! changes away are in this process: closing the window, quitting the app, and the engines being
//! stopped for an update. So the window tells this process whenever the state of a document
//! changes, and the three places that could lose work ask here first:
//!
//! - a request to close the window or to quit is held back while a document has unsaved changes,
//!   and the window is asked to put the question to the user;
//! - an update is not installed while a document has unsaved changes (the install ends the
//!   engines, and the changes with them);
//! - a reload of the page forgets everything: the documents are gone, and so are their changes.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

#[derive(Default)]
pub struct Unsaved {
    docs: Mutex<HashSet<u64>>,
    /// Set when the user has answered the question and quitting may go on.
    exit_allowed: AtomicBool,
}

impl Unsaved {
    fn docs(&self) -> std::sync::MutexGuard<'_, HashSet<u64>> {
        self.docs.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The window says whether a document has unsaved changes.
    pub fn set(&self, doc: u64, dirty: bool) {
        let mut docs = self.docs();
        if dirty {
            docs.insert(doc);
        } else {
            docs.remove(&doc);
        }
    }

    /// A document was closed.
    pub fn forget(&self, doc: u64) {
        self.docs().remove(&doc);
    }

    /// The window that knew the documents is gone.
    pub fn clear(&self) {
        self.docs().clear();
        self.exit_allowed.store(false, Ordering::SeqCst);
    }

    /// How many documents have unsaved changes.
    pub fn count(&self) -> usize {
        self.docs().len()
    }

    pub fn any(&self) -> bool {
        self.count() > 0
    }

    /// True when a request to close the window or to quit has to wait for the user.
    pub fn holds_exit(&self) -> bool {
        self.any() && !self.exit_allowed.load(Ordering::SeqCst)
    }

    /// The user has decided (saved, or chosen to throw the changes away): quitting may go on.
    pub fn allow_exit(&self) {
        self.exit_allowed.store(true, Ordering::SeqCst);
    }

    /// The user took the decision back, or the question was answered with "stay".
    pub fn disallow_exit(&self) {
        self.exit_allowed.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_document_with_changes_holds_back_closing_until_it_is_clean_or_the_user_decides() {
        let unsaved = Unsaved::default();
        assert!(!unsaved.holds_exit());
        unsaved.set(1, true);
        unsaved.set(2, true);
        unsaved.set(2, true);
        assert_eq!(unsaved.count(), 2);
        assert!(unsaved.holds_exit());

        unsaved.set(1, false);
        assert!(unsaved.holds_exit(), "document 2 still has changes");
        unsaved.forget(2);
        assert!(!unsaved.holds_exit());

        unsaved.set(3, true);
        assert!(unsaved.holds_exit());
        unsaved.allow_exit();
        assert!(!unsaved.holds_exit(), "the user said to quit");
        unsaved.disallow_exit();
        assert!(unsaved.holds_exit());
    }

    #[test]
    fn a_reload_of_the_window_forgets_the_changes_and_the_permission() {
        let unsaved = Unsaved::default();
        unsaved.set(1, true);
        unsaved.allow_exit();
        unsaved.clear();
        assert!(!unsaved.any());
        // A new change is held again: the permission was for the old window.
        unsaved.set(5, true);
        assert!(unsaved.holds_exit());
    }
}
