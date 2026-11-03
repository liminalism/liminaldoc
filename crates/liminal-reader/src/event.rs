use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use crate::document::engine::{DocumentEngine, DocumentEngineError};

#[derive(Debug, Clone)]
pub enum ViewerEvent {
    Wake,
    FatalBackgroundError(String),
    /// Device discovery finished off the UI thread; surface creation stays on it.
    GpuPrepared(Result<(), String>),
    /// A document open finished off the UI thread.
    DocumentOpened(DocumentOpened),
}

/// The outcome of one background document open.
///
/// Parsing a PDF's object tree or laying out a book-length Markdown file
/// takes anywhere from a few milliseconds to half a second. That used to
/// run on the UI thread — before the window existed at startup, and with the
/// previous document frozen on screen for a toolbar open — so the open
/// happens on its own thread and arrives here. `request` lets the viewer
/// ignore a result that a newer open has superseded.
#[derive(Debug, Clone)]
pub struct DocumentOpened {
    pub request: u64,
    pub path: PathBuf,
    /// Whether a password was supplied for this attempt.
    pub password_tried: bool,
    /// When the open was requested, for the load trace.
    pub started: Instant,
    pub result: Result<Arc<dyn DocumentEngine>, DocumentEngineError>,
}
