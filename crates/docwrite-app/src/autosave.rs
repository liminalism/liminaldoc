//! Autosave interval validation and the writer's persistent user preference.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A whole-minute autosave interval, bounded to 15 minutes through two hours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutosaveInterval(u16);

impl Default for AutosaveInterval {
    fn default() -> Self {
        Self(15)
    }
}

impl AutosaveInterval {
    /// Validate a value entered in minutes.
    pub fn parse(text: &str) -> Result<Self, String> {
        text.trim()
            .parse::<u16>()
            .ok()
            .filter(|n| (15..=120).contains(n))
            .map(Self)
            .ok_or_else(|| "Enter whole minutes from 15 to 120:".into())
    }

    /// Interval in whole minutes.
    pub fn minutes(self) -> u16 {
        self.0
    }

    /// Interval as elapsed time.
    pub fn duration(self) -> Duration {
        Duration::from_secs(u64::from(self.0) * 60)
    }
}

/// Platform user configuration location; no directory is created until changed.
pub fn settings_path() -> Option<PathBuf> {
    settings_dir().map(|dir| dir.join("autosave-minutes"))
}

/// The writer's per-user settings directory (`liminaldoc` under the
/// platform configuration root). Nothing is created until a setting is written.
pub fn settings_dir() -> Option<PathBuf> {
    let env_path = |name| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let root = if cfg!(target_os = "windows") {
        env_path("APPDATA")
    } else if cfg!(target_os = "macos") {
        env_path("HOME").map(|home| home.join("Library/Application Support"))
    } else {
        env_path("XDG_CONFIG_HOME")
            .filter(|path| path.is_absolute())
            .or_else(|| env_path("HOME").map(|home| home.join(".config")))
    }?;
    Some(root.join("liminaldoc"))
}

/// Read the saved interval, defaulting to 15 minutes when no preference exists.
pub fn load_interval(path: &Path) -> Result<AutosaveInterval, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => AutosaveInterval::parse(&text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(AutosaveInterval::default()),
        Err(err) => Err(err.to_string()),
    }
}

/// Replace this preference atomically, leaving the old value intact on failure.
pub fn save_interval(path: &Path, interval: AutosaveInterval) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "No settings directory".to_string())?;
    std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|err| err.to_string())?;
    writeln!(file, "{}", interval.minutes()).map_err(|err| err.to_string())?;
    file.as_file().sync_all().map_err(|err| err.to_string())?;
    file.persist(path).map_err(|err| err.to_string())?;
    Ok(())
}

impl crate::Editor {
    /// Edit the application interval without leaving the fullscreen writer.
    pub fn open_autosave_prompt(&mut self, interval: AutosaveInterval) {
        self.open_prompt(crate::PromptKind::AutosaveInterval);
        self.prompt_type(&interval.minutes().to_string());
        self.prompt_select_all();
    }

    /// A validated interval change for the host to persist and apply to all tabs.
    pub fn take_autosave_request(&mut self) -> Option<AutosaveInterval> {
        self.autosave_request.take()
    }

    /// Show feedback while retaining the current prompt for correction.
    pub fn set_prompt_error(&mut self, message: String) {
        if let Some(prompt) = self.prompt.as_mut() {
            prompt.error = Some(message);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::Editor;
    use std::sync::atomic::Ordering;
    use std::time::Instant;

    #[test]
    fn settings_validate_and_survive_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings/autosave-minutes");
        assert_eq!(load_interval(&path).unwrap().minutes(), 15);
        for value in ["15", " 60\n", "120"] {
            let interval = AutosaveInterval::parse(value).unwrap();
            save_interval(&path, interval).unwrap();
            assert_eq!(load_interval(&path).unwrap(), interval);
        }
        for value in ["", "0", "14", "121", "15.5", "two hours", "65536"] {
            assert!(AutosaveInterval::parse(value).is_err(), "{value}");
        }
        std::fs::write(&path, "corrupt").unwrap();
        assert!(load_interval(&path).is_err());
        assert!(save_interval(&path.join("blocked"), Default::default()).is_err());
    }

    #[test]
    fn elapsed_deadline_saves_despite_typing_and_preserves_newer_edits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Novel.legebook");
        let mut editor = Editor::open_or_create(&path).unwrap();
        let interval = AutosaveInterval::default();
        let due = editor.last_save_attempt + interval.duration();
        editor.type_text("Before");
        assert!(!editor.autosave_at(interval, due - Duration::from_secs(1)));
        editor.type_text(" deadline");
        assert!(editor.autosave_at(interval, due));
        editor.type_text(" and after");
        editor.wait_for_background_save().unwrap();
        assert!(editor.take_save_error().is_none());
        assert!(editor.is_dirty());
        assert_eq!(
            Editor::open_existing(&path).unwrap().book().plain_text(),
            "Before deadline"
        );
        assert!(!editor.autosave_at(interval, due + Duration::from_secs(1)));
        assert!(editor.autosave_at(interval, due + interval.duration()));
        editor.wait_for_background_save().unwrap();
        assert_eq!(
            Editor::open_existing(&path).unwrap().book().plain_text(),
            "Before deadline and after"
        );
        assert!(!editor.is_dirty());
        assert!(!editor.autosave_at(interval, due + interval.duration() * 2));
    }

    #[test]
    fn two_hour_interval_manual_reset_and_overlap_guard() {
        let dir = tempfile::tempdir().unwrap();
        let mut editor = Editor::open_or_create(dir.path().join("Novel.legebook")).unwrap();
        let interval = AutosaveInterval::parse("120").unwrap();
        editor.last_save_attempt = Instant::now() - interval.duration();
        editor.type_text("Saved manually");
        let before_save = Instant::now();
        editor.save_now().unwrap();
        assert!(editor.last_save_attempt >= before_save);
        editor.type_text(" More");
        let due = editor.last_save_attempt + interval.duration();
        assert!(!editor.autosave_at(interval, due - Duration::from_secs(1)));
        editor.saving.store(true, Ordering::Release);
        assert!(!editor.autosave_at(interval, due));
        assert!(editor.is_dirty());
        editor.saving.store(false, Ordering::Release);
        assert!(editor.autosave_at(interval, due));
        editor.wait_for_background_save().unwrap();
        assert!(editor.take_save_error().is_none());
    }

    #[test]
    fn unnamed_documents_wait_for_save_as() {
        let mut editor = Editor::new_document("Untitled");
        editor.type_text("Unsaved draft");
        let interval = AutosaveInterval::default();
        assert!(!editor.autosave_at(interval, editor.last_save_attempt + interval.duration()));
        assert!(editor.is_dirty());
        assert!(!editor.needs_save_tick());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Draft.legebook");
        editor.save_as_bundle(&path).unwrap();
        editor.type_text(" continues");
        assert!(editor.autosave_at(interval, editor.last_save_attempt + interval.duration()));
        editor.wait_for_background_save().unwrap();
        assert_eq!(
            Editor::open_existing(&path).unwrap().book().plain_text(),
            "Unsaved draft continues"
        );
    }

    #[test]
    fn failed_write_stays_recoverable_and_retries_at_next_interval() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("blocked");
        std::fs::write(&parent, "not a directory").unwrap();
        let path = parent.join("Novel.legebook");
        let mut editor = Editor::new_document("Novel");
        editor.open_bundle(&path);
        editor.type_text("Recover me");
        let interval = AutosaveInterval::default();
        let due = editor.last_save_attempt + interval.duration();
        assert!(editor.autosave_at(interval, due));
        editor.wait_for_background_save().unwrap();
        assert!(editor.take_save_error().is_some());
        assert!(editor.is_dirty());
        assert!(!editor.autosave_at(interval, due + Duration::from_secs(1)));
        std::fs::remove_file(&parent).unwrap();
        std::fs::create_dir(&parent).unwrap();
        assert!(editor.autosave_at(interval, due + interval.duration()));
        editor.wait_for_background_save().unwrap();
        assert!(editor.take_save_error().is_none());
        assert_eq!(
            Editor::open_existing(&path).unwrap().book().plain_text(),
            "Recover me"
        );
        assert!(!editor.is_dirty());
    }
}
