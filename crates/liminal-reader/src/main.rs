#![allow(missing_docs)]
// A double-clicked viewer must not flash a console window. The Windows GUI
// subsystem is what suppresses it; stderr stays attached when the binary is
// launched from an existing terminal, so command-line use is unaffected.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::sync::Arc;

use liminaldoc_reader::ViewerApp;
use liminaldoc_reader::cli::{LaunchMode, USAGE, parse_launch_config};
use liminaldoc_reader::document::empty::EmptyEngine;
use liminaldoc_reader::document::engine::DocumentEngine;
use liminaldoc_reader::document::session::{UpdateQueue, WakeSink};
use liminaldoc_reader::document::synthetic::SyntheticEngine;
use liminaldoc_reader::event::ViewerEvent;
use winit::event_loop::{EventLoop, EventLoopProxy};

struct WinitWake {
    proxy: EventLoopProxy<ViewerEvent>,
}

impl std::fmt::Debug for WinitWake {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("WinitWake").finish_non_exhaustive()
    }
}

impl WakeSink for WinitWake {
    fn wake(&self) {
        let _ = self.proxy.send_event(ViewerEvent::Wake);
    }
}

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    let load_started = std::time::Instant::now();
    let config = match parse_launch_config(std::env::args_os().skip(1)) {
        Ok(config) if config.mode == LaunchMode::Help => {
            println!("{USAGE}");
            return Ok(());
        }
        Ok(config) => config,
        Err(error) => {
            report_startup_error(&format!("{error}\n\n{USAGE}"));
            std::process::exit(2);
        }
    };
    let event_loop = EventLoop::<ViewerEvent>::with_user_event().build()?;
    let wake = Arc::new(WinitWake {
        proxy: event_loop.create_proxy(),
    });
    let updates = UpdateQueue::new(4096, wake);

    // Named documents open on their own thread once the loop runs: parsing
    // a large PDF or laying out a long Markdown file overlaps window and
    // presenter creation instead of delaying the first frame. The viewer
    // starts on the empty engine and adopts the document when it arrives;
    // an encrypted one reaches the password prompt the same way.
    let (engine, document): (Arc<dyn DocumentEngine>, Option<std::path::PathBuf>) = match config
        .mode
    {
        LaunchMode::Synthetic { page_count } => (Arc::new(SyntheticEngine::new(page_count)), None),
        LaunchMode::Pdf(path) | LaunchMode::Markdown(path) => {
            (Arc::new(EmptyEngine::new()), Some(path))
        }
        LaunchMode::Empty | LaunchMode::Help => (Arc::new(EmptyEngine::new()), None),
    };
    let mut app = ViewerApp::new_with_presenter(engine, updates, config.presenter)?;
    app.trace_document_load(load_started);
    app.set_event_proxy(event_loop.create_proxy());
    if let Some(path) = document {
        app.open_at_startup(path, load_started);
    }
    event_loop.run_app(&mut app)?;
    Ok(())
}

/// Startup failures still have to reach someone who launched the viewer from
/// the desktop rather than a shell.
fn report_startup_error(message: &str) {
    eprintln!("liminaldoc-reader: {message}");
    let _ = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("Liminaldoc")
        .set_description(message)
        .show();
}
