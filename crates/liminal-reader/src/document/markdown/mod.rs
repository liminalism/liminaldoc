//! Markdown documents for the reader.
//!
//! A port of `MarkdownViewer.Wpf`'s `MarkdownDocumentService` onto the
//! reader's page model: [`model`] parses CommonMark into the same block and
//! inline set the WPF viewer converts from its Markdig AST, [`theme`] carries
//! its light/dark palettes and type scale verbatim, [`layout`] shapes and
//! paginates through cosmic-text, and [`engine`] serves the result behind the
//! standard [`DocumentEngine`](super::engine::DocumentEngine) boundary, so
//! search, selection, outlines, links, and tiling all work unchanged.

pub mod engine;
pub mod layout;
pub mod model;
pub mod theme;

pub use engine::{MarkdownEngine, MarkdownLine, MarkdownPage};
pub use layout::{MdInk, MdLink};
pub use model::{Block, Inline};
