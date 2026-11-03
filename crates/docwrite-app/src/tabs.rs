//! Open manuscripts and their active tab.

use std::ops::{Deref, DerefMut};
use std::path::Path;

use crate::Editor;

/// Action selected from the document tab strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabCommand {
    /// Activate the tab at this index.
    Select(usize),
    /// Create a blank document.
    New,
    /// Open a standard document file.
    Open,
    /// Open a native bundle directory.
    OpenBook,
    /// Choose a manuscript or export destination.
    SaveAs,
    /// Save the active manuscript to its bundle (or ask where, the first time).
    Save,
    /// Open the entry at this index of the recent-files list.
    OpenRecent(usize),
    /// Forget every recent file.
    ClearRecent,
}

/// Shared drawing and hit geometry for the strip above the editor.
#[derive(Clone, Copy, Debug)]
pub struct TabStripLayout {
    /// Strip height in physical pixels.
    pub height: i32,
    /// Width of one document tab.
    pub tab_width: i32,
    /// Number of document tabs.
    pub count: usize,
}

impl TabStripLayout {
    /// Fit document selectors into one thin row; commands live in the toolbar.
    pub fn new(width: i32, scale: f32, count: usize) -> Self {
        Self {
            height: (20.0 * scale).round().max(1.0) as i32,
            tab_width: if count == 0 {
                0
            } else {
                (width / count as i32).min((150.0 * scale) as i32)
            },
            count,
        }
    }

    /// Resolve a pointer press to its document tab.
    pub fn hit(&self, x: i32, y: i32) -> Option<TabCommand> {
        if y < 0 || y >= self.height || x < 0 {
            return None;
        }
        if self.tab_width > 0 {
            let index = (x / self.tab_width) as usize;
            if index < self.count {
                return Some(TabCommand::Select(index));
            }
        }
        None
    }
}

/// Owns every open editor, including inactive books with pending saves.
#[allow(missing_debug_implementations)]
pub struct DocumentTabs {
    documents: Vec<Editor>,
    active: usize,
}

impl DocumentTabs {
    /// Start with one open editor.
    pub fn new(editor: Editor) -> Self {
        Self {
            documents: vec![editor],
            active: 0,
        }
    }

    /// Number of open documents.
    pub fn len(&self) -> usize {
        self.documents.len()
    }

    /// Selected tab index.
    pub fn active_index(&self) -> usize {
        self.active
    }

    /// Select a tab if it exists.
    pub fn select(&mut self, index: usize) -> bool {
        if index >= self.documents.len() {
            return false;
        }
        self.documents[self.active].cancel_pointer();
        self.active = index;
        true
    }

    /// Cycle to the next or previous tab.
    pub fn next(&mut self, backward: bool) {
        self.documents[self.active].cancel_pointer();
        let count = self.documents.len();
        self.active = if backward {
            (self.active + count - 1) % count
        } else {
            (self.active + 1) % count
        };
    }

    /// Add a document and select it.
    pub fn push(&mut self, editor: Editor) {
        self.documents[self.active].cancel_pointer();
        self.documents.push(editor);
        self.active = self.documents.len() - 1;
    }

    /// Close the active document, leaving a blank tab if it was the last.
    pub fn close_active(&mut self) {
        if self.documents.len() == 1 {
            self.documents[0] = Editor::new_document("Untitled");
        } else {
            self.documents.remove(self.active);
            self.active = self.active.min(self.documents.len() - 1);
        }
    }

    /// Find an already open native bundle.
    pub fn find_bundle(&self, path: &Path) -> Option<usize> {
        self.documents
            .iter()
            .position(|editor| editor.bundle() == Some(path))
    }

    /// Visit all documents for save polling.
    pub fn iter(&self) -> impl Iterator<Item = &Editor> {
        self.documents.iter()
    }

    /// Visit all documents for autosave and exit handling.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Editor> {
        self.documents.iter_mut()
    }

    /// Labels in tab order, with an unsaved marker when appropriate.
    pub fn labels(&self) -> Vec<String> {
        self.documents
            .iter()
            .map(|editor| {
                let name = editor
                    .bundle()
                    .and_then(|path| path.file_stem())
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| editor.book().title().to_string());
                if editor.is_dirty() || editor.bundle().is_none() {
                    format!("{name} *")
                } else {
                    name
                }
            })
            .collect()
    }
}

impl Deref for DocumentTabs {
    type Target = Editor;

    fn deref(&self) -> &Self::Target {
        &self.documents[self.active]
    }
}

impl DerefMut for DocumentTabs {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.documents[self.active]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_tabs_keeps_each_book_and_selection() {
        let mut tabs = DocumentTabs::new(Editor::new_document("First"));
        tabs.type_text("one");
        tabs.push(Editor::new_document("Second"));
        tabs.type_text("two");
        assert_eq!(tabs.active_index(), 1);
        tabs.select(0);
        assert_eq!(tabs.book().plain_text(), "one");
        tabs.select(1);
        assert_eq!(tabs.book().plain_text(), "two");
        tabs.close_active();
        assert_eq!(tabs.book().plain_text(), "one");
    }
}
