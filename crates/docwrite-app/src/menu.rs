//! The File menu: a PixelKit-drawn drop-down under the toolbar's File
//! button, holding the file commands and the recent-files list.
//!
//! The window host owns dialogs and the recent list; the menu only turns a
//! click or a key into a [`TabCommand`] the host takes with
//! [`Editor::take_file_command`].

use std::path::PathBuf;

use crate::Editor;
use crate::tabs::TabCommand;
use pixelkit_shell::KeyInput;

/// Menu row height in logical pixels.
const ROW_H: f32 = 22.0;
/// Separator row height in logical pixels.
const SEPARATOR_H: f32 = 7.0;
/// Menu width in logical pixels.
const MENU_W: f32 = 360.0;

/// One row of the File menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    /// The text shown.
    pub label: String,
    /// The accelerator shown at the right, if any.
    pub shortcut: String,
    /// What choosing the row does; `None` for a heading or a disabled row.
    pub command: Option<TabCommand>,
    /// A rule is drawn above this row.
    pub separator: bool,
}

impl MenuItem {
    fn command(label: &str, shortcut: &str, command: TabCommand) -> Self {
        Self {
            label: label.into(),
            shortcut: shortcut.into(),
            command: Some(command),
            separator: false,
        }
    }

    fn heading(label: &str) -> Self {
        Self {
            label: label.into(),
            shortcut: String::new(),
            command: None,
            separator: false,
        }
    }

    fn above_rule(mut self) -> Self {
        self.separator = true;
        self
    }

    /// Whether the row can be chosen.
    pub fn enabled(&self) -> bool {
        self.command.is_some()
    }
}

/// An open File menu and its geometry in window pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct FileMenu {
    /// The rows, top to bottom.
    pub items: Vec<MenuItem>,
    /// The row the pointer or the arrow keys are on.
    pub highlighted: Option<usize>,
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub w: i32,
    /// Total height.
    pub h: i32,
    /// Physical pixels per logical pixel when it was laid out.
    scale: f32,
}

impl FileMenu {
    fn new(items: Vec<MenuItem>, x: i32, y: i32, scale: f32) -> Self {
        let h: i32 = items
            .iter()
            .map(|item| Self::row_height(item, scale))
            .sum::<i32>()
            + (8.0 * scale) as i32;
        Self {
            items,
            highlighted: None,
            x,
            y,
            w: (MENU_W * scale).round() as i32,
            h,
            scale,
        }
    }

    fn row_height(item: &MenuItem, scale: f32) -> i32 {
        let row = (ROW_H * scale).round() as i32;
        if item.separator {
            row + (SEPARATOR_H * scale).round() as i32
        } else {
            row
        }
    }

    /// The rectangle of row `index`: (top, height), in window pixels. The
    /// separator's own strip is excluded.
    pub fn row_rect(&self, index: usize) -> Option<(i32, i32)> {
        let mut top = self.y + (4.0 * self.scale) as i32;
        for (i, item) in self.items.iter().enumerate() {
            let full = Self::row_height(item, self.scale);
            let row = (ROW_H * self.scale).round() as i32;
            if i == index {
                return Some((top + full - row, row));
            }
            top += full;
        }
        None
    }

    /// Whether the point is inside the menu.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x as f32
            && x < (self.x + self.w) as f32
            && y >= self.y as f32
            && y < (self.y + self.h) as f32
    }

    /// The enabled row under the point.
    pub fn row_at(&self, x: f32, y: f32) -> Option<usize> {
        if !self.contains(x, y) {
            return None;
        }
        (0..self.items.len()).find(|&index| {
            self.items[index].enabled()
                && self
                    .row_rect(index)
                    .is_some_and(|(top, h)| y >= top as f32 && y < (top + h) as f32)
        })
    }

    fn step(&mut self, down: bool) {
        let count = self.items.len();
        if count == 0 {
            return;
        }
        let mut index = match self.highlighted {
            Some(current) => current,
            None if down => count - 1,
            None => 0,
        };
        for _ in 0..count {
            index = if down {
                (index + 1) % count
            } else {
                (index + count - 1) % count
            };
            if self.items[index].enabled() {
                self.highlighted = Some(index);
                return;
            }
        }
    }
}

impl Editor {
    /// Tell the menu which files were opened recently, newest first. The
    /// window host keeps the list; the menu shows it.
    pub fn set_recent_files(&mut self, paths: Vec<PathBuf>) {
        self.recent = paths;
    }

    /// The recent files the menu shows.
    pub fn recent_files(&self) -> &[PathBuf] {
        &self.recent
    }

    /// Open the File menu below the toolbar (or close it when it is open).
    pub fn toggle_file_menu(&mut self) {
        if self.menu.is_some() {
            self.close_file_menu();
        } else {
            self.open_file_menu();
        }
    }

    /// Open the File menu below the toolbar. A prompt or footnote session
    /// closes first so typed text has one destination.
    pub fn open_file_menu(&mut self) {
        self.cancel_prompt();
        self.abandon_footnote();
        let accel = if cfg!(target_os = "macos") {
            "\u{2318}"
        } else {
            "Ctrl+"
        };
        let alt = if cfg!(target_os = "macos") {
            "\u{2325}"
        } else {
            "Alt+"
        };
        let mut items = vec![
            MenuItem::command("New", &format!("{accel}N"), TabCommand::New),
            MenuItem::command("Open\u{2026}", &format!("{accel}O"), TabCommand::Open),
            MenuItem::command(
                "Open Book Folder\u{2026}",
                &format!("{accel}{alt}O"),
                TabCommand::OpenBook,
            ),
            MenuItem::command("Save", &format!("{accel}S"), TabCommand::Save).above_rule(),
            MenuItem::command(
                "Save As / Export\u{2026}",
                &format!("{accel}Shift+S"),
                TabCommand::SaveAs,
            ),
        ];
        if self.recent.is_empty() {
            items.push(MenuItem::heading("No recent files").above_rule());
        } else {
            items.push(MenuItem::heading("Recent").above_rule());
            for (index, path) in self.recent.iter().enumerate() {
                let shortcut = if index < 9 {
                    (index + 1).to_string()
                } else {
                    String::new()
                };
                items.push(MenuItem::command(
                    &crate::recent::recent_label(path),
                    &shortcut,
                    TabCommand::OpenRecent(index),
                ));
            }
            items.push(MenuItem::command("Clear Recent", "", TabCommand::ClearRecent).above_rule());
        }
        let s = self.ui_scale;
        let menu = FileMenu::new(items, (4.0 * s) as i32, self.toolbar_height(), s);
        self.menu = Some(menu);
    }

    /// Close the File menu without choosing anything.
    pub fn close_file_menu(&mut self) {
        self.menu = None;
    }

    /// The open File menu, if any.
    pub fn file_menu(&self) -> Option<&FileMenu> {
        self.menu.as_ref()
    }

    /// Choose row `index`: the command goes to the host and the menu closes.
    pub fn choose_menu_item(&mut self, index: usize) -> bool {
        let Some(command) = self
            .menu
            .as_ref()
            .and_then(|menu| menu.items.get(index))
            .and_then(|item| item.command)
        else {
            return false;
        };
        self.file_command = Some(command);
        self.menu = None;
        true
    }

    /// A key while the menu is open. Returns whether it was used; an unused
    /// key closes the menu and is then handled as usual.
    pub fn menu_key(&mut self, key: &KeyInput) -> bool {
        let Some(menu) = self.menu.as_mut() else {
            return false;
        };
        match key {
            KeyInput::Escape => {
                self.menu = None;
                true
            }
            KeyInput::Down | KeyInput::Tab => {
                menu.step(true);
                true
            }
            KeyInput::Up => {
                menu.step(false);
                true
            }
            KeyInput::Enter | KeyInput::Right => {
                if let Some(index) = menu.highlighted {
                    self.choose_menu_item(index);
                } else {
                    self.menu = None;
                }
                true
            }
            KeyInput::Character(digit) if digit.is_ascii_digit() && *digit != '0' => {
                let wanted = TabCommand::OpenRecent((*digit as u8 - b'1') as usize);
                let index = menu
                    .items
                    .iter()
                    .position(|item| item.command == Some(wanted));
                match index {
                    Some(index) => self.choose_menu_item(index),
                    None => true,
                }
            }
            _ => {
                self.menu = None;
                false
            }
        }
    }

    /// Pointer motion over the menu highlights its row.
    pub(crate) fn menu_hover(&mut self, x: f32, y: f32) {
        if let Some(menu) = self.menu.as_mut() {
            let row = menu.row_at(x, y);
            if row.is_some() || !menu.contains(x, y) {
                menu.highlighted = row;
            }
        }
    }

    /// A press while the menu is open: a row is chosen, or the menu closes.
    /// Returns whether the press was consumed.
    pub(crate) fn menu_pointer(&mut self, pressed: bool) -> bool {
        let Some(menu) = self.menu.as_ref() else {
            return false;
        };
        if !pressed {
            return true;
        }
        let (x, y) = self.cursor;
        if let Some(index) = menu.row_at(x, y) {
            self.choose_menu_item(index);
            return true;
        }
        let on_file_button = y < self.toolbar_height() as f32
            && self.button_at(x, y, self.frame_width) == Some(crate::Action::FileMenu);
        self.menu = None;
        // Clicking the File button again just closes; anything else closes
        // and is swallowed so a stray click never lands in the manuscript.
        let _ = on_file_button;
        true
    }

    /// Draw the open menu over the pages.
    pub(crate) fn paint_file_menu(
        &mut self,
        painter: &mut pixelkit_raster::Painter<'_>,
        face: &docwrite_typeset::Face,
    ) {
        let Some(menu) = self.menu.clone() else {
            return;
        };
        let s = self.ui_scale;
        let theme = self.theme_now;
        let border = s.round().max(1.0) as i32;
        painter.fill_rect(
            pixelkit_raster::Rect::new(
                menu.x - border,
                menu.y - border,
                menu.w + 2 * border,
                menu.h + 2 * border,
            ),
            theme.rule,
        );
        painter.fill_rect(
            pixelkit_raster::Rect::new(menu.x, menu.y, menu.w, menu.h),
            theme.chrome,
        );
        let size = 11.0 * s;
        let inset = (12.0 * s) as i32;
        for (index, item) in menu.items.iter().enumerate() {
            let Some((top, h)) = menu.row_rect(index) else {
                continue;
            };
            if item.separator {
                let rule_y = top - (SEPARATOR_H * s / 2.0) as i32;
                painter.fill_rect(
                    pixelkit_raster::Rect::new(menu.x + inset / 2, rule_y, menu.w - inset, border),
                    theme.rule,
                );
            }
            if menu.highlighted == Some(index) && item.enabled() {
                painter.fill_rect(
                    pixelkit_raster::Rect::new(menu.x + border, top, menu.w - 2 * border, h),
                    theme.selection,
                );
            }
            let baseline = top + h - (6.0 * s) as i32;
            let color = if item.enabled() {
                theme.chrome_ink
            } else {
                theme.rule
            };
            painter.push_clip(pixelkit_raster::Rect::new(menu.x, top, menu.w, h));
            let shortcut_w: f32 = if item.shortcut.is_empty() {
                0.0
            } else {
                face.shape(&item.shortcut, size, &[])
                    .map(|glyphs| glyphs.iter().map(|glyph| glyph.x_advance).sum())
                    .unwrap_or(0.0)
            };
            let label_room = menu.w - 2 * inset - shortcut_w as i32 - (16.0 * s) as i32;
            let label = fit_label(face, &item.label, size, label_room as f32);
            self.paint_label_with_color(
                painter,
                face,
                &label,
                menu.x + inset,
                baseline,
                size,
                color,
            );
            if shortcut_w > 0.0 {
                self.paint_label_with_color(
                    painter,
                    face,
                    &item.shortcut,
                    menu.x + menu.w - inset - shortcut_w as i32,
                    baseline,
                    size,
                    color,
                );
            }
            painter.pop_clip();
        }
    }
}

/// Shorten `text` with an ellipsis in the middle until it fits `room`
/// pixels, keeping the file name at the start readable.
fn fit_label(face: &docwrite_typeset::Face, text: &str, size: f32, room: f32) -> String {
    let width = |candidate: &str| -> f32 {
        face.shape(candidate, size, &[])
            .map(|glyphs| glyphs.iter().map(|glyph| glyph.x_advance).sum())
            .unwrap_or(0.0)
    };
    if width(text) <= room {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut keep = chars.len();
    while keep > 4 {
        keep -= 1;
        let head = keep * 2 / 3;
        let tail = keep - head;
        let candidate: String = chars[..head]
            .iter()
            .chain(std::iter::once(&'\u{2026}'))
            .chain(chars[chars.len() - tail..].iter())
            .collect();
        if width(&candidate) <= room {
            return candidate;
        }
    }
    "\u{2026}".into()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn menu_lists_commands_then_recents_and_keys_walk_enabled_rows() {
        let mut editor = Editor::new_document("Menu");
        editor.open_file_menu();
        let menu = editor.file_menu().unwrap();
        assert_eq!(menu.items[0].command, Some(TabCommand::New));
        assert!(
            menu.items
                .iter()
                .any(|item| item.label == "No recent files" && !item.enabled())
        );
        editor.close_file_menu();
        editor.set_recent_files(vec![
            PathBuf::from("/books/One.legebook"),
            PathBuf::from("/books/Two.md"),
        ]);
        editor.open_file_menu();
        let menu = editor.file_menu().unwrap();
        let recents: Vec<_> = menu
            .items
            .iter()
            .filter_map(|item| match item.command {
                Some(TabCommand::OpenRecent(index)) => Some((index, item.shortcut.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(recents, vec![(0, "1".to_string()), (1, "2".to_string())]);
        assert!(
            menu.items
                .iter()
                .any(|item| item.label == "Recent" && !item.enabled())
        );
        assert_eq!(
            menu.items.last().unwrap().command,
            Some(TabCommand::ClearRecent)
        );
        // Down from nothing lands on the first row; Up from the first wraps
        // to the last enabled row, skipping the heading.
        assert!(editor.menu_key(&KeyInput::Down));
        assert_eq!(editor.file_menu().unwrap().highlighted, Some(0));
        assert!(editor.menu_key(&KeyInput::Up));
        let last = editor.file_menu().unwrap().items.len() - 1;
        assert_eq!(editor.file_menu().unwrap().highlighted, Some(last));
        assert!(editor.menu_key(&KeyInput::Up));
        assert_eq!(
            editor.file_menu().unwrap().highlighted,
            Some(last - 1),
            "Two.md"
        );
        assert!(editor.menu_key(&KeyInput::Enter));
        assert!(editor.file_menu().is_none());
        assert_eq!(editor.take_file_command(), Some(TabCommand::OpenRecent(1)));
        editor.open_file_menu();
        assert!(editor.menu_key(&KeyInput::Character('1')));
        assert_eq!(editor.take_file_command(), Some(TabCommand::OpenRecent(0)));
        editor.open_file_menu();
        assert!(!editor.menu_key(&KeyInput::Character('x')));
        assert!(
            editor.file_menu().is_none(),
            "an unused key closes the menu"
        );
        assert!(editor.take_file_command().is_none());
    }

    #[test]
    fn clicking_a_row_sends_its_command_and_clicking_outside_closes() {
        let mut editor = Editor::new_document("Menu");
        editor.set_recent_files(vec![PathBuf::from("/books/One.legebook")]);
        let mut frame = pixelkit_raster::WindowBuffer::new(1100, 800);
        editor.paint(&mut frame);
        editor.open_file_menu();
        editor.paint(&mut frame);
        let menu = editor.file_menu().unwrap().clone();
        let index = menu
            .items
            .iter()
            .position(|item| item.command == Some(TabCommand::OpenRecent(0)))
            .unwrap();
        let (top, h) = menu.row_rect(index).unwrap();
        editor.hover((menu.x + 20) as f32, (top + h / 2) as f32);
        assert_eq!(editor.file_menu().unwrap().highlighted, Some(index));
        assert_eq!(editor.pointer_shape(), crate::PointerShape::Hand);
        editor.pointer(true);
        editor.pointer(false);
        assert_eq!(editor.take_file_command(), Some(TabCommand::OpenRecent(0)));
        assert!(editor.file_menu().is_none());
        // The menu covers the page; a click outside it closes it and does
        // not move the caret.
        editor.type_text("abc");
        let before = editor.book().selection();
        editor.paint(&mut frame);
        editor.open_file_menu();
        editor.hover(900.0, 600.0);
        editor.pointer(true);
        editor.pointer(false);
        assert!(editor.file_menu().is_none());
        assert_eq!(editor.book().selection(), before);
        assert!(editor.take_file_command().is_none());
    }

    #[test]
    fn long_labels_are_shortened_to_fit() {
        let mut editor = Editor::new_document("Menu");
        editor.refresh_layout();
        let face = editor.document().unwrap().face();
        let long = "A-very-long-manuscript-name-that-goes-on.legebook  \u{2014}  ~/Documents/Writing/Drafts/2026";
        let fitted = fit_label(face, long, 11.0, 120.0);
        assert!(fitted.contains('\u{2026}'));
        assert!(fitted.chars().count() < long.chars().count());
        assert_eq!(fit_label(face, "short", 11.0, 1000.0), "short");
    }
}
