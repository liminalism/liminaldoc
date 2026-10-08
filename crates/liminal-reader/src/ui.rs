use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use arboard::Clipboard;
use cosmic_text::{
    Align, Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Weight,
};
use pdf_font::StandardFont;

use crate::geometry::RectI;
use crate::paint::PixelSurface;
use crate::scene::SceneSurface;

#[derive(Debug, Clone)]
pub struct TextPaint {
    pub text: String,
    pub x: i32,
    pub y: i32,
    pub max_width: u32,
    pub size: f32,
    pub color: u32,
    pub bold: bool,
    pub centered: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct RectPaint {
    pub rect: RectI,
    pub color: u32,
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
fn register_embedded_family(font_system: &mut FontSystem, font: StandardFont) -> Option<String> {
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

#[allow(missing_debug_implementations)]
pub struct UiTextRenderer {
    font_system: FontSystem,
    swash_cache: SwashCache,
    /// Family name of the embedded regular face, when it registered.
    regular_family: Option<String>,
    /// Family name of the embedded bold face. The bundled bold program is a
    /// *separate family* (`"Chrom Sans OTF Bold"`, subfamily `Regular`) rather
    /// than a heavy weight within the regular family, so asking for
    /// `Weight::BOLD` on the regular family would never reach it.
    bold_family: Option<String>,
    revision: u64,
    surfaces: HashMap<[u64; 4], (u64, Arc<SceneSurface>)>,
}

impl UiTextRenderer {
    pub fn new() -> Self {
        // `FontSystem::new()` scans the system's installed fonts, which costs
        // roughly 60ms once at startup. That scan is load-bearing and must not
        // be traded away for an embedded-only database: the fallback lists
        // `PlatformFallback` supplies are *family names* ("Noto Sans CJK JP",
        // "DejaVu Sans", "Noto Sans Symbols"…) that are resolved against this
        // same database. The viewer draws PDF outline titles (`app.rs`, from
        // the document's own bookmarks) and the search field's IME preedit
        // through this renderer, so it routinely receives CJK, Arabic,
        // Cyrillic and other scripts the embedded Latin face has no glyphs
        // for. With no system faces to fall back to, every one of those
        // renders as tofu.
        let mut font_system = FontSystem::new();
        let regular_family = register_embedded_family(&mut font_system, StandardFont::Helvetica);
        let bold_family = register_embedded_family(&mut font_system, StandardFont::HelveticaBold);
        Self {
            font_system,
            swash_cache: SwashCache::new(),
            regular_family,
            bold_family,
            revision: 0,
            surfaces: HashMap::new(),
        }
    }

    pub fn render(
        &mut self,
        key: [u64; 4],
        width: u32,
        height: u32,
        background: u32,
        rectangles: &[RectPaint],
        text: &[TextPaint],
    ) -> Arc<SceneSurface> {
        let stride = width as usize;
        let mut pixels = vec![background; stride.saturating_mul(height as usize)];
        for paint in rectangles {
            fill_rect(&mut pixels, stride, width, height, paint.rect, paint.color);
        }
        for paint in text {
            self.draw_text(&mut pixels, stride, width, height, paint);
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        width.hash(&mut hasher);
        height.hash(&mut hasher);
        pixels.hash(&mut hasher);
        let fingerprint = hasher.finish();
        if let Some((cached_fingerprint, surface)) = self.surfaces.get(&key)
            && *cached_fingerprint == fingerprint
        {
            return Arc::clone(surface);
        }
        self.revision = self.revision.wrapping_add(1).max(1);
        let surface = Arc::new(SceneSurface {
            key,
            revision: self.revision,
            pixels: PixelSurface {
                width,
                height,
                stride,
                pixels: pixels.into(),
            },
        });
        self.surfaces
            .insert(key, (fingerprint, Arc::clone(&surface)));
        surface
    }

    fn draw_text(
        &mut self,
        pixels: &mut [u32],
        stride: usize,
        width: u32,
        height: u32,
        paint: &TextPaint,
    ) {
        if paint.text.is_empty() || paint.max_width == 0 {
            return;
        }
        let line_height = (paint.size * 1.35).ceil();
        // Pick the embedded face for the requested weight, and fall back to
        // the generic sans-serif family only when a program failed to
        // register. The weight is still requested either way: if the bold
        // program is missing, it lets the fallback chain pick a heavier face
        // instead of silently rendering bold labels at regular weight.
        let family = if paint.bold {
            self.bold_family
                .as_deref()
                .or(self.regular_family.as_deref())
        } else {
            self.regular_family.as_deref()
        };
        let mut buffer = Buffer::new(&mut self.font_system, Metrics::new(paint.size, line_height));
        {
            let mut borrowed = buffer.borrow_with(&mut self.font_system);
            borrowed.set_size(Some(paint.max_width as f32), Some(line_height));
            let attrs = Attrs::new()
                .family(match family {
                    Some(name) => Family::Name(name),
                    None => Family::SansSerif,
                })
                .weight(if paint.bold {
                    Weight::BOLD
                } else {
                    Weight::NORMAL
                });
            borrowed.set_text(&paint.text, &attrs, Shaping::Advanced, None);
            if paint.centered
                && let Some(line) = borrowed.lines.first_mut()
            {
                line.set_align(Some(Align::Center));
            }
            borrowed.shape_until_scroll(true);
        }
        let color = Color::rgb(
            ((paint.color >> 16) & 0xff) as u8,
            ((paint.color >> 8) & 0xff) as u8,
            (paint.color & 0xff) as u8,
        );
        buffer.draw(
            &mut self.font_system,
            &mut self.swash_cache,
            color,
            |x, y, glyph_width, glyph_height, glyph_color| {
                let argb = glyph_color.0;
                for row in 0..glyph_height as i32 {
                    let target_y = paint.y + y + row;
                    if target_y < 0 || target_y >= height as i32 {
                        continue;
                    }
                    for column in 0..glyph_width as i32 {
                        let target_x = paint.x + x + column;
                        if target_x < 0 || target_x >= width as i32 {
                            continue;
                        }
                        let index = target_y as usize * stride + target_x as usize;
                        pixels[index] = blend_xrgb(pixels[index], argb);
                    }
                }
            },
        );
    }
}

impl Default for UiTextRenderer {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(missing_debug_implementations)]
pub struct SystemClipboard {
    context: Option<Clipboard>,
}

impl SystemClipboard {
    /// Connect lazily: a display that appears after startup (or a context
    /// that went stale) must not wedge copy and paste for the session.
    /// Every use retries the connection when there is none.
    pub fn new() -> Self {
        Self { context: None }
    }

    fn ensure_context(&mut self) -> Option<&mut Clipboard> {
        if self.context.is_none() {
            self.context = Clipboard::new().ok();
        }
        self.context.as_mut()
    }

    pub fn get(&mut self) -> Result<String, &'static str> {
        if let Some(context) = self.ensure_context()
            && let Ok(text) = context.get_text()
        {
            return Ok(text);
        }
        // Drop a poisoned context so the next use reconnects instead of
        // failing on the same stale handle.
        self.context = None;
        external_clipboard_get().ok_or("clipboard unavailable")
    }

    pub fn set(&mut self, text: String) -> Result<(), &'static str> {
        if let Some(context) = self.ensure_context()
            && context.set_text(text.clone()).is_ok()
        {
            return Ok(());
        }
        self.context = None;
        if external_clipboard_set(&text) {
            Ok(())
        } else {
            Err("clipboard unavailable")
        }
    }
}

/// Copy out through a desktop clipboard tool when the linked backend cannot
/// serve the session. Arboard already covers X11 and Wayland; the tools are
/// belt and braces for forwarded, minimal, or otherwise odd sessions.
/// Missing tools report false fast (failed spawn), so probing them on each
/// copy costs nothing when none exist.
#[cfg(unix)]
fn external_clipboard_set(text: &str) -> bool {
    const CANDIDATES: &[(&str, &[&str])] = &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard", "-i"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    try_external_set(text, CANDIDATES)
}

/// Paste twin of [`external_clipboard_set`], for the search field on
/// sessions the linked backend cannot serve.
#[cfg(unix)]
fn external_clipboard_get() -> Option<String> {
    const CANDIDATES: &[(&str, &[&str])] = &[
        ("wl-paste", &["-n"]),
        ("xclip", &["-selection", "clipboard", "-o"]),
        ("xsel", &["--clipboard", "--output"]),
    ];
    try_external_get(CANDIDATES)
}

#[cfg(not(unix))]
fn external_clipboard_set(_text: &str) -> bool {
    false
}

#[cfg(not(unix))]
fn external_clipboard_get() -> Option<String> {
    None
}

/// Pipe `text` into the first candidate that accepts it. Candidates are
/// explicit so tests can point at stub scripts.
///
/// Success is "spawned and swallowed stdin", deliberately not "exited
/// zero": clipboard holders (`wl-copy`, `xclip -i`) stay alive to serve
/// the selection and never exit on their own, so waiting would wedge the
/// UI on every copy. A reaper thread collects the exit status so the
/// detached holder never becomes a zombie.
fn try_external_set(text: &str, candidates: &[(&str, &[&str])]) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};
    candidates.iter().any(|(program, args)| {
        let mut child = match Command::new(program)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(_) => return false,
        };
        let written = child
            .stdin
            .as_mut()
            .is_some_and(|stdin| stdin.write_all(text.as_bytes()).is_ok());
        drop(child.stdin.take());
        if !written {
            return false;
        }
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        true
    })
}

/// Read pasted text from the first candidate that runs and exits
/// successfully. Candidates are explicit so tests can point at stub scripts.
fn try_external_get(candidates: &[(&str, &[&str])]) -> Option<String> {
    use std::process::{Command, Stdio};
    candidates.iter().find_map(|(program, args)| {
        let output = Command::new(program)
            .args(*args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    })
}

impl Default for SystemClipboard {
    fn default() -> Self {
        Self::new()
    }
}

fn fill_rect(pixels: &mut [u32], stride: usize, width: u32, height: u32, rect: RectI, color: u32) {
    let left = rect.x.max(0).min(width as i32) as usize;
    let top = rect.y.max(0).min(height as i32) as usize;
    let right = rect.right().max(0).min(width as i32) as usize;
    let bottom = rect.bottom().max(0).min(height as i32) as usize;
    for row in top..bottom {
        pixels[row * stride + left..row * stride + right].fill(color);
    }
}

fn blend_xrgb(destination: u32, source_argb: u32) -> u32 {
    let alpha = (source_argb >> 24) & 0xff;
    let inverse = 255 - alpha;
    let channel = |shift: u32| {
        let source = (source_argb >> shift) & 0xff;
        let destination = (destination >> shift) & 0xff;
        (source * alpha + destination * inverse + 127) / 255
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn text_surface_is_opaque_and_nonempty() {
        let mut renderer = UiTextRenderer::new();
        let surface = renderer.render(
            [1, 2, 3, 4],
            180,
            40,
            0x00ff_ffff,
            &[],
            &[TextPaint {
                text: "Search".to_owned(),
                x: 4,
                y: 4,
                max_width: 160,
                size: 15.0,
                color: 0,
                bold: false,
                centered: false,
            }],
        );
        assert!(
            surface
                .pixels
                .pixels
                .iter()
                .any(|pixel| *pixel != 0x00ff_ffff)
        );
    }

    /// The embedded programs must actually resolve. This is the regression
    /// that hid for as long as the family was hardcoded to the file name:
    /// `Family::Name("FoxitSans")` matched nothing, so the bundled face was
    /// loaded and then never used.
    #[test]
    fn embedded_faces_register_and_are_resolvable() {
        let renderer = UiTextRenderer::new();
        let regular = renderer
            .regular_family
            .clone()
            .expect("the embedded regular program must register");
        let bold = renderer
            .bold_family
            .clone()
            .expect("the embedded bold program must register");
        assert_ne!(regular, bold, "bold must be its own family, not a weight");
        for family in [&regular, &bold] {
            let query = cosmic_text::fontdb::Query {
                families: &[cosmic_text::fontdb::Family::Name(family)],
                ..Default::default()
            };
            assert!(
                renderer.font_system.db().query(&query).is_some(),
                "family {family:?} does not resolve in the database"
            );
        }
    }

    /// System fonts are the fallback source for everything the embedded Latin
    /// face cannot cover. PDF outline titles and the search field's IME
    /// preedit both arrive here as arbitrary Unicode, so the database must
    /// carry more than the two bundled faces.
    #[test]
    fn system_faces_remain_available_for_fallback() {
        let renderer = UiTextRenderer::new();
        assert!(
            renderer.font_system.db().len() > 2,
            "only the embedded faces are present: non-Latin text would render \
             as tofu. FontSystem::new()'s system scan must not be removed."
        );
    }

    /// Non-Latin labels must still put ink on the canvas. On a host with no
    /// fonts installed at all there is nothing to fall back to, so this
    /// asserts only when the database actually has system faces.
    #[test]
    fn non_latin_labels_render_through_fallback() {
        let mut renderer = UiTextRenderer::new();
        if renderer.font_system.db().len() <= 2 {
            return;
        }
        for (index, sample) in [
            "\u{7b2c}\u{4e00}\u{7ae0}",
            "\u{413}\u{43b}\u{430}\u{432}\u{430}",
        ]
        .into_iter()
        .enumerate()
        {
            let surface = renderer.render(
                [9, index as u64, 0, 0],
                240,
                40,
                0x00ff_ffff,
                &[],
                &[TextPaint {
                    text: sample.to_owned(),
                    x: 4,
                    y: 4,
                    max_width: 220,
                    size: 16.0,
                    color: 0,
                    bold: false,
                    centered: false,
                }],
            );
            assert!(
                surface
                    .pixels
                    .pixels
                    .iter()
                    .any(|pixel| *pixel != 0x00ff_ffff),
                "{sample:?} drew nothing: script fallback is broken"
            );
        }
    }

    /// Write an executable stub script and return its path. Stubs stand in
    /// for `wl-copy`-style tools so the fallback is exercised without a
    /// display server or real clipboard utilities installed.
    #[cfg(unix)]
    fn stub_script(name: &str, body: &str) -> std::path::PathBuf {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let mut path = std::env::temp_dir();
        path.push(format!(
            "liminaldoc-clipboard-{}-{name}",
            std::process::id()
        ));
        let mut file = std::fs::File::create(&path).expect("stub script");
        file.write_all(body.as_bytes()).expect("stub body");
        drop(file);
        let mut permissions = std::fs::metadata(&path).expect("stub meta").permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&path, permissions).expect("stub chmod");
        path
    }

    /// The external fallback pipes the copied text into the first tool that
    /// runs, skipping missing programs without failing the copy.
    #[cfg(unix)]
    #[test]
    fn external_copy_skips_missing_tools_and_pipes_text() {
        let mut sink_path = std::env::temp_dir();
        sink_path.push(format!(
            "liminaldoc-clipboard-{}-sink.out",
            std::process::id()
        ));
        let sink = stub_script(
            "sink.sh",
            &format!("#!/bin/sh\ncat > '{}'\n", sink_path.display()),
        );
        let missing = sink.with_extension("missing");
        let _ = std::fs::remove_file(&missing);
        let sink_str = sink.to_string_lossy().into_owned();
        let missing_str = missing.to_string_lossy().into_owned();
        let ok = try_external_set(
            "copied selection",
            &[(missing_str.as_str(), &[]), (sink_str.as_str(), &[])],
        );
        assert!(ok, "the stub must receive the piped text");
        // The copy returns once stdin is accepted, without waiting for the
        // tool to finish, so poll for its output with a deadline.
        let stored = (0..500)
            .find_map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(10));
                std::fs::read_to_string(&sink_path).ok()
            })
            .expect("stub output never landed");
        assert_eq!(stored, "copied selection");
        let _ = std::fs::remove_file(&sink);
        let _ = std::fs::remove_file(&sink_path);
    }

    /// Clipboard holders never exit on their own (`wl-copy` and `xclip -i`
    /// stay alive to serve the selection): the copy must return without
    /// waiting for them, or every fallback copy wedges the UI.
    #[cfg(unix)]
    #[test]
    fn external_copy_returns_without_waiting_for_the_holder() {
        let holder = stub_script("hold.sh", "#!/bin/sh\ncat > /dev/null\nsleep 30\n");
        let holder_str = holder.to_string_lossy().into_owned();
        let started = std::time::Instant::now();
        let ok = try_external_set("held text", &[(holder_str.as_str(), &[])]);
        assert!(ok, "accepting stdin must count as success");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "the copy waited for a holder that sleeps 30s"
        );
        let _ = std::fs::remove_file(&holder);
    }

    /// A failing tool does not poison the attempt: the next candidate is
    /// tried, and paste reads back what the tool printed.
    #[cfg(unix)]
    #[test]
    fn external_paste_reads_from_the_first_working_tool() {
        let failing = stub_script("fail.sh", "#!/bin/sh\nexit 3\n");
        let printer = stub_script("print.sh", "#!/bin/sh\nprintf 'pasted'\n");
        let failing_str = failing.to_string_lossy().into_owned();
        let printer_str = printer.to_string_lossy().into_owned();
        let pasted = try_external_get(&[(failing_str.as_str(), &[]), (printer_str.as_str(), &[])]);
        let _ = std::fs::remove_file(&failing);
        let _ = std::fs::remove_file(&printer);
        assert_eq!(pasted.as_deref(), Some("pasted"));
        assert_eq!(try_external_get(&[]), None);
    }
}
