//! Single-line prompt editing, independent of manuscript selection and undo.

use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

use crate::{Editor, PromptKind};

/// A caret movement within a toolbar prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptMotion {
    /// One grapheme to the left, or the start of the selection.
    Left,
    /// One grapheme to the right, or the end of the selection.
    Right,
    /// The start of the prompt.
    Home,
    /// The end of the prompt.
    End,
}

/// A one-line text entry shown in the toolbar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    /// What the text is for.
    pub kind: PromptKind,
    /// The text typed so far.
    pub text: String,
    anchor: usize,
    caret: usize,
}

impl Prompt {
    pub(crate) fn new(kind: PromptKind, text: String) -> Self {
        let caret = text.len();
        Self {
            kind,
            text,
            anchor: caret,
            caret,
        }
    }

    /// The caret's UTF-8 byte offset.
    pub fn caret(&self) -> usize {
        self.caret
    }

    /// Selected UTF-8 bytes in document order, empty for a collapsed caret.
    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }

    fn previous(&self) -> usize {
        self.text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .take_while(|i| *i < self.caret)
            .last()
            .unwrap_or(0)
    }

    fn next(&self) -> usize {
        self.text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .find(|i| *i > self.caret)
            .unwrap_or(self.text.len())
    }

    fn replace(&mut self, text: &str) {
        let range = self.selection();
        let end = range.start + text.len();
        self.text.replace_range(range, text);
        // Insertion or deletion can join graphemes across the edit boundary.
        // Land after the resulting cluster, never inside a combining sequence.
        self.caret = self
            .text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .find(|i| *i >= end)
            .unwrap_or(self.text.len());
        self.anchor = self.caret;
    }
}

impl Editor {
    /// Insert single-line text at the prompt caret, replacing its selection.
    /// Returns false when no prompt is open.
    pub fn prompt_type(&mut self, text: &str) -> bool {
        let Some(prompt) = self.prompt.as_mut() else {
            return false;
        };
        let text: String = text.chars().filter(|ch| !ch.is_control()).collect();
        if !text.is_empty() {
            prompt.replace(&text);
        }
        true
    }

    /// Move the prompt caret; Shift-style motion preserves the selection anchor.
    pub fn prompt_move(&mut self, motion: PromptMotion, extend: bool) {
        let Some(prompt) = self.prompt.as_mut() else {
            return;
        };
        let selection = prompt.selection();
        prompt.caret = match motion {
            PromptMotion::Left if !extend && !selection.is_empty() => selection.start,
            PromptMotion::Right if !extend && !selection.is_empty() => selection.end,
            PromptMotion::Left => prompt.previous(),
            PromptMotion::Right => prompt.next(),
            PromptMotion::Home => 0,
            PromptMotion::End => prompt.text.len(),
        };
        if !extend {
            prompt.anchor = prompt.caret;
        }
    }

    /// Select all the open prompt's text.
    pub fn prompt_select_all(&mut self) {
        if let Some(prompt) = self.prompt.as_mut() {
            prompt.anchor = 0;
            prompt.caret = prompt.text.len();
        }
    }

    /// The prompt selection for Copy; None if no text is selected.
    pub fn prompt_selected_text(&self) -> Option<String> {
        let prompt = self.prompt.as_ref()?;
        let range = prompt.selection();
        (!range.is_empty()).then(|| prompt.text[range].to_string())
    }

    /// Remove and return the prompt selection for Cut.
    pub fn prompt_cut(&mut self) -> Option<String> {
        let text = self.prompt_selected_text()?;
        self.prompt.as_mut()?.replace("");
        Some(text)
    }

    /// Delete the selection or the grapheme before the prompt caret.
    pub fn prompt_backspace(&mut self) -> bool {
        self.prompt_delete(true)
    }

    /// Delete the selection or the grapheme after the prompt caret.
    pub fn prompt_delete_forward(&mut self) -> bool {
        self.prompt_delete(false)
    }

    fn prompt_delete(&mut self, backward: bool) -> bool {
        let Some(prompt) = self.prompt.as_mut() else {
            return false;
        };
        if prompt.selection().is_empty() {
            prompt.anchor = if backward {
                prompt.previous()
            } else {
                prompt.next()
            };
        }
        prompt.replace("");
        true
    }
}
