use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use crate::present::PresenterPreference;

pub const USAGE: &str = "Usage: liminaldoc-reader [--presenter auto|gpu|software] [document.pdf|document.md]\n       liminaldoc-reader [--presenter auto|gpu|software] --synthetic [PAGE_COUNT]\n\nWithout a document the viewer opens an empty window; use its Open button\n(or Ctrl+O) to choose a PDF or Markdown file.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchMode {
    Pdf(PathBuf),
    Markdown(PathBuf),
    Synthetic {
        page_count: u32,
    },
    /// No document was named. The viewer opens on its empty state and waits
    /// for the user to pick a file.
    Empty,
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchConfig {
    pub mode: LaunchMode,
    pub presenter: PresenterPreference,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CliError {
    #[error("unknown option: {0}")]
    UnknownOption(String),
    #[error("--synthetic accepts at most one page count")]
    TooManySyntheticArguments,
    #[error("synthetic page count must be a positive integer")]
    InvalidSyntheticPageCount,
    #[error("only one document can be opened at a time")]
    TooManyDocuments,
    #[error("--presenter requires auto, gpu, or software")]
    MissingPresenter,
    #[error("unknown presenter: {0}")]
    InvalidPresenter(String),
    #[error("--presenter may only be specified once")]
    DuplicatePresenter,
    #[error("--synthetic cannot be combined with a document")]
    ConflictingDocumentModes,
}

pub fn parse_launch_mode<I, S>(arguments: I) -> Result<LaunchMode, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    parse_launch_config(arguments).map(|config| config.mode)
}

pub fn parse_launch_config<I, S>(arguments: I) -> Result<LaunchConfig, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let arguments = arguments.into_iter().map(Into::into).collect::<Vec<_>>();
    let mut presenter = None;
    let mut synthetic = false;
    let mut synthetic_count = None;
    let mut document = None;
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        if argument == OsStr::new("-h") || argument == OsStr::new("--help") {
            return Ok(LaunchConfig {
                mode: LaunchMode::Help,
                presenter: presenter.unwrap_or_default(),
            });
        }
        if argument == OsStr::new("--presenter") {
            if presenter.is_some() {
                return Err(CliError::DuplicatePresenter);
            }
            index += 1;
            let value = arguments.get(index).ok_or(CliError::MissingPresenter)?;
            presenter = Some(match value.to_string_lossy().as_ref() {
                "auto" => PresenterPreference::Auto,
                "gpu" => PresenterPreference::Gpu,
                "software" => PresenterPreference::Software,
                _ => {
                    return Err(CliError::InvalidPresenter(
                        value.to_string_lossy().into_owned(),
                    ));
                }
            });
        } else if argument == OsStr::new("--synthetic") {
            if synthetic {
                return Err(CliError::TooManySyntheticArguments);
            }
            if document.is_some() {
                return Err(CliError::ConflictingDocumentModes);
            }
            synthetic = true;
        } else if argument.to_string_lossy().starts_with('-') {
            return Err(CliError::UnknownOption(
                argument.to_string_lossy().into_owned(),
            ));
        } else if synthetic && synthetic_count.is_none() {
            synthetic_count = Some(
                argument
                    .to_string_lossy()
                    .parse::<u32>()
                    .ok()
                    .filter(|count| *count > 0)
                    .ok_or(CliError::InvalidSyntheticPageCount)?,
            );
        } else if document.is_some() {
            return Err(if synthetic {
                CliError::TooManySyntheticArguments
            } else {
                CliError::TooManyDocuments
            });
        } else if synthetic {
            return Err(CliError::TooManySyntheticArguments);
        } else {
            document = Some(PathBuf::from(argument));
        }
        index += 1;
    }
    let mode = if synthetic {
        LaunchMode::Synthetic {
            page_count: synthetic_count.unwrap_or(10_000),
        }
    } else if let Some(path) = document {
        classify_document_path(&path)
    } else {
        LaunchMode::Empty
    };
    Ok(LaunchConfig {
        mode,
        presenter: presenter.unwrap_or_default(),
    })
}

/// Classify a document for opening. Content wins over the extension: a PDF
/// without a `.pdf` suffix (or with a wrong one) still opens as a PDF, and a
/// text file misnamed `.pdf` is routed to the Markdown engine instead of
/// failing in the PDF engine. Unknown binary content keeps the PDF engine's
/// honest parse error rather than rendering as mojibake, and unreadable
/// paths fall back to the extension so a missing file still produces the
/// engine's own IO error.
pub fn classify_document_path(path: &std::path::Path) -> LaunchMode {
    if content_is_pdf(path) {
        LaunchMode::Pdf(path.to_path_buf())
    } else if content_is_text(path) || is_markdown_path(path) {
        LaunchMode::Markdown(path.to_path_buf())
    } else {
        LaunchMode::Pdf(path.to_path_buf())
    }
}

/// Whether the first bytes of `path` carry a PDF header. The scan tolerates
/// leading blank lines and a UTF-8 BOM, which lenient writers emit; anything
/// unreadable reports false so the caller falls back to the extension.
pub fn content_is_pdf(path: &std::path::Path) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = [0u8; 1024];
    let Ok(read) = file.read(&mut head) else {
        return false;
    };
    let head = &head[..read];
    head.windows(b"%PDF-".len())
        .any(|window| window == b"%PDF-")
}

/// Whether `path` looks like readable text: no NUL bytes and valid UTF-8 in
/// the opening chunk. An empty file counts as text and renders blank.
pub fn content_is_text(path: &std::path::Path) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = [0u8; 8192];
    let Ok(read) = file.read(&mut head) else {
        return false;
    };
    let head = &head[..read];
    !head.contains(&0) && std::str::from_utf8(head).is_ok()
}

/// Whether `path` names a Markdown document, by extension. Plain `.txt`
/// files render as Markdown too, matching the WPF viewer's open set.
pub fn is_markdown_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "md" | "markdown" | "mdown" | "mkd" | "mkdn" | "txt"
            )
        })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;
    use std::io::Write;

    /// A scratch document with a deliberately chosen (possibly wrong or
    /// missing) extension and content.
    fn scratch_file(name: &str, content: &[u8]) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("liminaldoc-classify-{}-{name}", std::process::id()));
        let mut file = std::fs::File::create(&path).expect("scratch file");
        file.write_all(content).expect("scratch content");
        path
    }

    #[test]
    fn content_wins_over_extension_in_both_directions() {
        let pdf_no_ext = scratch_file("report", b"%PDF-1.7\ntrailer");
        let pdf_wrong_ext = scratch_file("report.md", b"\n%PDF-1.4\ntrailer");
        let text_pdf_ext = scratch_file("notes.pdf", b"# Notes\n\njust text\n");
        let text_no_ext = scratch_file("README", b"plain text, no suffix\n");
        let binary_unknown = scratch_file("blob.xyz", &[0, 1, 2, b'%'.wrapping_add(1), 0]);
        for path in [
            &pdf_no_ext,
            &pdf_wrong_ext,
            &text_pdf_ext,
            &text_no_ext,
            &binary_unknown,
        ] {
            assert!(path.is_file(), "fixture missing: {}", path.display());
        }

        assert!(matches!(
            classify_document_path(&pdf_no_ext),
            LaunchMode::Pdf(_)
        ));
        assert!(matches!(
            classify_document_path(&pdf_wrong_ext),
            LaunchMode::Pdf(_)
        ));
        assert!(matches!(
            classify_document_path(&text_pdf_ext),
            LaunchMode::Markdown(_)
        ));
        assert!(matches!(
            classify_document_path(&text_no_ext),
            LaunchMode::Markdown(_)
        ));
        // Unknown binary keeps the PDF engine's honest parse error.
        assert!(matches!(
            classify_document_path(&binary_unknown),
            LaunchMode::Pdf(_)
        ));

        for path in [
            pdf_no_ext,
            pdf_wrong_ext,
            text_pdf_ext,
            text_no_ext,
            binary_unknown,
        ] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn missing_files_fall_back_to_the_extension() {
        let missing_pdf = PathBuf::from("definitely-not-here-1.pdf");
        let missing_md = PathBuf::from("definitely-not-here-2.md");
        assert!(!missing_pdf.exists() && !missing_md.exists());
        assert!(matches!(
            classify_document_path(&missing_pdf),
            LaunchMode::Pdf(_)
        ));
        assert!(matches!(
            classify_document_path(&missing_md),
            LaunchMode::Markdown(_)
        ));
    }
}
