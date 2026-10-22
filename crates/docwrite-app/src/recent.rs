//! The manuscripts and imports opened most recently, for the File menu.
//!
//! The list is a plain text file, one absolute path per line, newest
//! first, in the writer's settings directory. Paths that no longer exist
//! are dropped when the list is shown, not when it is loaded, so a volume
//! that is merely unmounted keeps its entries.

use std::io::Write;
use std::path::{Path, PathBuf};

/// How many entries the list keeps.
pub const MAX_RECENT: usize = 10;

/// Most-recently-used document paths, newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecentFiles {
    paths: Vec<PathBuf>,
}

impl RecentFiles {
    /// An empty list.
    pub fn new() -> Self {
        Self::default()
    }

    /// Where the list is stored, when a settings directory exists.
    pub fn settings_path() -> Option<PathBuf> {
        crate::autosave::settings_dir().map(|dir| dir.join("recent-files"))
    }

    /// Read the list at `path`. A missing file is an empty list; anything
    /// else unreadable is an error so a corrupt list is reported, not hidden.
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let mut list = Self::new();
                for line in text.lines().rev() {
                    let line = line.trim();
                    if !line.is_empty() {
                        list.touch(PathBuf::from(line));
                    }
                }
                Ok(list)
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::new()),
            Err(err) => Err(format!("cannot read {}: {err}", path.display())),
        }
    }

    /// Write the list at `path`, creating its directory, atomically.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let parent = path
            .parent()
            .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("cannot create {}: {err}", parent.display()))?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)
            .map_err(|err| format!("cannot write in {}: {err}", parent.display()))?;
        for entry in &self.paths {
            writeln!(temp, "{}", entry.to_string_lossy())
                .map_err(|err| format!("cannot write {}: {err}", path.display()))?;
        }
        temp.persist(path)
            .map_err(|err| format!("cannot replace {}: {err}", path.display()))?;
        Ok(())
    }

    /// Move `path` to the front, dropping any older copy and anything
    /// beyond [`MAX_RECENT`].
    pub fn touch(&mut self, path: PathBuf) {
        self.paths.retain(|entry| *entry != path);
        self.paths.insert(0, path);
        self.paths.truncate(MAX_RECENT);
    }

    /// Forget `path`, for an entry that could not be opened.
    pub fn remove(&mut self, path: &Path) {
        self.paths.retain(|entry| entry != path);
    }

    /// Forget everything.
    pub fn clear(&mut self) {
        self.paths.clear();
    }

    /// The entries, newest first.
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// The entries that still exist on disk, newest first.
    pub fn existing(&self) -> Vec<PathBuf> {
        self.paths
            .iter()
            .filter(|path| path.exists())
            .cloned()
            .collect()
    }

    /// Whether there is nothing to show.
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

/// A short label for a recent entry: the file name, then its folder.
pub fn recent_label(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let folder = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| {
            let shown = parent.to_string_lossy();
            match std::env::var_os("HOME").map(PathBuf::from) {
                Some(home) if parent.starts_with(&home) => {
                    let rest = parent.strip_prefix(&home).unwrap_or(parent);
                    if rest.as_os_str().is_empty() {
                        "~".to_string()
                    } else {
                        format!("~/{}", rest.to_string_lossy())
                    }
                }
                _ => shown.into_owned(),
            }
        });
    match folder {
        Some(folder) => format!("{name}  \u{2014}  {folder}"),
        None => name,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn touch_moves_to_front_dedupes_and_caps() {
        let mut list = RecentFiles::new();
        for i in 0..12 {
            list.touch(PathBuf::from(format!("/books/{i}.legebook")));
        }
        assert_eq!(list.paths().len(), MAX_RECENT);
        assert_eq!(list.paths()[0], Path::new("/books/11.legebook"));
        list.touch(PathBuf::from("/books/5.legebook"));
        assert_eq!(list.paths()[0], Path::new("/books/5.legebook"));
        assert_eq!(
            list.paths()
                .iter()
                .filter(|p| **p == Path::new("/books/5.legebook"))
                .count(),
            1
        );
        list.remove(Path::new("/books/5.legebook"));
        assert_eq!(list.paths()[0], Path::new("/books/11.legebook"));
    }

    #[test]
    fn save_and_load_round_trip_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings").join("recent-files");
        assert!(RecentFiles::load(&path).unwrap().is_empty());
        let mut list = RecentFiles::new();
        list.touch(PathBuf::from("/a/first.legebook"));
        list.touch(PathBuf::from("/b/second.txt"));
        list.save(&path).unwrap();
        let loaded = RecentFiles::load(&path).unwrap();
        assert_eq!(loaded, list);
        assert_eq!(loaded.paths()[0], Path::new("/b/second.txt"));
        let existing = dir.path().join("real.md");
        std::fs::write(&existing, "# hi").unwrap();
        list.touch(existing.clone());
        assert_eq!(list.existing(), vec![existing]);
        list.clear();
        list.save(&path).unwrap();
        assert!(RecentFiles::load(&path).unwrap().is_empty());
    }

    #[test]
    fn labels_show_name_then_folder() {
        let label = recent_label(Path::new("/srv/books/Novel.legebook"));
        assert!(label.starts_with("Novel.legebook"));
        assert!(label.ends_with("/srv/books"));
    }
}
