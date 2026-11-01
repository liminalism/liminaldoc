//! Recovery of untitled drafts.
//!
//! An untitled manuscript has no destination the author chose, so it is
//! autosaved into an application-owned directory under the user settings
//! directory, as an ordinary `.legebook` bundle named after the process that
//! owns it. It is removed once the draft is saved under a real name or its tab
//! closes. A draft whose owner is gone is offered again on the next launch.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// How often a changed untitled draft is written to recovery. Shorter than the
/// author's autosave interval: it never touches a file the author named.
pub const INTERVAL: Duration = Duration::from_secs(60);

/// The recovery directory under the writer's settings directory.
pub fn default_root() -> Option<PathBuf> {
    crate::autosave::settings_dir().map(|dir| dir.join("recovery"))
}

/// A fresh bundle path in `root` for the current process.
pub fn new_draft_path(root: &Path) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    root.join(format!("draft-{}-{nanos}.legebook", std::process::id()))
}

/// Drafts left by processes that are no longer running, oldest first.
pub fn pending(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut drafts: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter_map(|path| {
            // A save interrupted mid-swap leaves only `.<name>.old`.
            let name = path.file_name()?.to_str()?;
            match name.strip_prefix('.').and_then(|n| n.strip_suffix(".old")) {
                Some(live) if live.ends_with(".legebook") => Some(path.with_file_name(live)),
                Some(_) => None,
                None => Some(path),
            }
        })
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "legebook")
                && lege_model_is_bundle(path)
                && !owner_alive(path)
        })
        .collect();
    drafts.sort();
    drafts.dedup();
    drafts
}

fn lege_model_is_bundle(path: &Path) -> bool {
    docwrite_model::Book::is_bundle(path)
        || path
            .parent()
            .zip(path.file_name())
            .is_some_and(|(parent, name)| {
                // An interrupted swap leaves only the old generation.
                docwrite_model::Book::is_bundle(
                    &parent.join(format!(".{}.old", name.to_string_lossy())),
                )
            })
}

fn owner_alive(path: &Path) -> bool {
    let pid = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("draft-"))
        .and_then(|rest| rest.split('-').next())
        .and_then(|pid| pid.parse::<u32>().ok());
    match pid {
        Some(pid) if pid == std::process::id() => true,
        #[cfg(target_os = "linux")]
        Some(pid) => Path::new("/proc").join(pid.to_string()).exists(),
        _ => false,
    }
}

/// Remove a recovery bundle and any staging directories beside it.
pub fn remove(path: &Path) {
    let _ = std::fs::remove_dir_all(path);
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        for suffix in ["tmp", "old", "snapshots"] {
            let _ = std::fs::remove_dir_all(
                parent.join(format!(".{}.{suffix}", name.to_string_lossy())),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Editor;
    use crate::autosave::AutosaveInterval;

    fn draft(root: &Path, text: &str) -> Editor {
        let mut editor = Editor::new_document("Untitled");
        editor.set_recovery_root(root);
        editor.type_text(text);
        editor
    }

    #[test]
    fn an_untitled_draft_is_autosaved_to_recovery_and_offered_after_a_crash() {
        let dir = tempfile::tempdir().unwrap();
        let interval = AutosaveInterval::default();
        let mut editor = draft(dir.path(), "A long unsaved draft.");
        assert!(editor.needs_save_tick());
        assert!(!editor.autosave_at(interval, editor.last_save_attempt));
        assert!(editor.autosave_at(interval, editor.last_save_attempt + INTERVAL));
        editor.wait_for_background_save().unwrap();
        assert!(editor.take_save_error().is_none());
        assert!(editor.bundle().is_none());
        // Unchanged since the write: nothing more to do.
        assert!(!editor.needs_save_tick());
        let path = editor.recovery_path().unwrap().to_path_buf();
        // The owner is this process, so it is not offered while it lives.
        assert!(pending(dir.path()).is_empty());
        // Simulate a crash: a draft owned by a process that no longer exists.
        let orphan = dir.path().join("draft-4000000000-1.legebook");
        std::fs::rename(&path, &orphan).unwrap();
        std::mem::forget(editor);
        assert_eq!(pending(dir.path()), vec![orphan.clone()]);
        let recovered = Editor::open_recovered(&orphan).unwrap();
        assert_eq!(recovered.book().plain_text(), "A long unsaved draft.");
        assert!(recovered.bundle().is_none());
    }

    #[test]
    fn an_interrupted_swap_still_offers_the_old_generation() {
        let dir = tempfile::tempdir().unwrap();
        let orphan = dir.path().join("draft-4000000000-2.legebook");
        let mut book = docwrite_model::Book::new("Untitled");
        book.insert("Survived.").unwrap();
        book.save_bundle(&orphan).unwrap();
        std::fs::rename(&orphan, dir.path().join(".draft-4000000000-2.legebook.old")).unwrap();
        assert_eq!(pending(dir.path()), vec![orphan.clone()]);
        let recovered = Editor::open_recovered(&orphan).unwrap();
        assert_eq!(recovered.book().plain_text(), "Survived.");
    }

    #[test]
    fn save_as_and_closing_remove_the_recovery_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let interval = AutosaveInterval::default();
        let mut editor = draft(dir.path(), "Text.");
        assert!(editor.autosave_at(interval, editor.last_save_attempt + INTERVAL));
        editor.wait_for_background_save().unwrap();
        let path = editor.recovery_path().unwrap().to_path_buf();
        assert!(path.exists());
        editor
            .save_as_bundle(&dir.path().join("Named.legebook"))
            .unwrap();
        assert!(!path.exists());
        assert!(editor.recovery_path().is_none());

        let mut closed = draft(dir.path(), "Other.");
        assert!(closed.autosave_at(interval, closed.last_save_attempt + INTERVAL));
        closed.wait_for_background_save().unwrap();
        let path = closed.recovery_path().unwrap().to_path_buf();
        drop(closed);
        assert!(!path.exists());
    }

    #[test]
    fn a_blank_untitled_document_is_not_written() {
        let dir = tempfile::tempdir().unwrap();
        let mut editor = Editor::new_document("Untitled");
        editor.set_recovery_root(dir.path());
        editor.type_text("x");
        editor.backspace();
        let interval = AutosaveInterval::default();
        assert!(!editor.autosave_at(interval, editor.last_save_attempt + INTERVAL));
        assert!(pending(dir.path()).is_empty());
    }
}
