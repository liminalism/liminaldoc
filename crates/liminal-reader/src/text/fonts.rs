//! One system font scan per process, shared by every shaper in the reader.
//!
//! `cosmic_text::FontSystem::new()` walks the installed font directories,
//! which costs tens of milliseconds on a desktop with a full font set and up
//! to a second on a slow disk. The reader has several shapers — the chrome
//! text renderer on the UI thread, the Markdown layout engine, one per
//! Markdown raster worker — and they all used to pay that scan separately, on
//! the startup path. The scan is load-bearing (the platform fallback lists
//! are *family names* resolved against this database, so an embedded-only
//! database draws every non-Latin outline title as tofu) and must not be
//! removed; it just must not be repeated.
//!
//! The database is scanned once, the embedded base-14 programs are
//! registered into it once, and every shaper is built from a clone. A clone
//! shares the face data (sources are reference counted) and copies only the
//! face table, so a new shaping context costs microseconds instead of a
//! directory walk. Each `FontSystem` keeps its own parsed-face and match
//! caches, which is what the shapers want: they run on different threads.

use std::sync::OnceLock;

use cosmic_text::FontSystem;
use pdf_font::StandardFont;

/// Family names the embedded base-14 programs registered under, discovered
/// from the database rather than assumed from the file names.
#[derive(Debug, Clone, Default)]
pub struct EmbeddedFamilies {
    /// Helvetica-equivalent regular face.
    pub regular: Option<String>,
    /// Helvetica-equivalent bold face. The bundled bold program is a
    /// *separate family* rather than a heavy weight within the regular
    /// family, so a weight request on the regular family never reaches it.
    pub bold: Option<String>,
    /// Courier-equivalent monospace face.
    pub mono: Option<String>,
}

struct SharedDatabase {
    locale: String,
    db: cosmic_text::fontdb::Database,
    families: EmbeddedFamilies,
}

static SHARED: OnceLock<SharedDatabase> = OnceLock::new();

fn shared() -> &'static SharedDatabase {
    SHARED.get_or_init(|| {
        let mut font_system = FontSystem::new();
        let families = EmbeddedFamilies {
            regular: register_embedded_family(&mut font_system, StandardFont::Helvetica),
            bold: register_embedded_family(&mut font_system, StandardFont::HelveticaBold),
            mono: register_embedded_family(&mut font_system, StandardFont::Courier),
        };
        let (locale, db) = font_system.into_locale_and_db();
        SharedDatabase {
            locale,
            db,
            families,
        }
    })
}

/// A shaping context over the shared database, with the embedded families
/// already registered. The first call scans the system fonts; every later
/// call clones the face table.
pub fn font_system() -> (FontSystem, EmbeddedFamilies) {
    let shared = shared();
    (
        FontSystem::new_with_locale_and_db(shared.locale.clone(), shared.db.clone()),
        shared.families.clone(),
    )
}

/// Register one of pdf-font's embedded base-14 programs and report the family
/// name it actually landed under.
///
/// The name is discovered rather than assumed. This used to be hardcoded as
/// `"FoxitSans"`, matching the *file* name of the program pdf-font ships
/// (`fonts/FoxitSans.otf`), but the face inside it declares its family as
/// `"Chrom Sans OTF"`. `fontdb` matches on the declared family, so the query
/// never resolved, and every label in the viewer was silently drawn with
/// whatever system font the fallback chain happened to reach — a different
/// typeface on every machine, and nothing at all on a host with no fonts
/// installed. Reading the name back from the database keeps that from
/// recurring the next time the bundled program is swapped.
///
/// `fontdb::Database::load_font_data` returns no id, so the new face is found
/// by diffing the id set around the load.
pub(crate) fn register_embedded_family(
    font_system: &mut FontSystem,
    font: StandardFont,
) -> Option<String> {
    let before: std::collections::HashSet<_> =
        font_system.db().faces().map(|face| face.id).collect();
    font_system
        .db_mut()
        .load_font_data(font.program_data().to_vec());
    font_system
        .db()
        .faces()
        .find(|face| !before.contains(&face.id))
        .and_then(|face| face.families.first().map(|(name, _)| name.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_font_systems_agree_on_the_embedded_families_and_face_table() {
        let (first, first_families) = font_system();
        let (second, second_families) = font_system();
        assert_eq!(first_families.regular, second_families.regular);
        assert_eq!(first_families.bold, second_families.bold);
        assert_eq!(first_families.mono, second_families.mono);
        assert!(
            first_families.regular.is_some(),
            "embedded regular face registered"
        );
        assert!(
            first_families.mono.is_some(),
            "embedded monospace face registered"
        );
        let first_ids: Vec<_> = first.db().faces().map(|face| face.id).collect();
        let second_ids: Vec<_> = second.db().faces().map(|face| face.id).collect();
        assert_eq!(first_ids, second_ids, "clones share one face table");
    }

    #[test]
    fn a_second_font_system_costs_a_clone_not_a_scan() {
        let _ = font_system();
        let started = std::time::Instant::now();
        for _ in 0..8 {
            let _ = font_system();
        }
        // Eight directory walks would take far longer than this even on a
        // machine with few fonts; eight face-table clones do not.
        assert!(
            started.elapsed() < std::time::Duration::from_millis(400),
            "font system clones took {:?}",
            started.elapsed()
        );
    }
}
