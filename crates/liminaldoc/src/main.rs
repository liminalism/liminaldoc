//! Lightweight entry point. Reader and writer live in separate executables
//! so opening either mode cannot initialize the other's renderer or editor.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::{Command, ExitCode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Reader,
    Writer,
}

impl Mode {
    fn executable(self) -> &'static str {
        match self {
            Self::Reader => "liminaldoc-reader",
            Self::Writer => "liminaldoc-writer",
        }
    }
}

fn route(args: &[OsString]) -> Result<(Mode, &[OsString]), String> {
    if let Some(first) = args.first() {
        if first == "--reader" {
            return Ok((Mode::Reader, &args[1..]));
        }
        if first == "--writer" {
            return Ok((Mode::Writer, &args[1..]));
        }
        if first == "export" || first == "--fullscreen" || first == "--source" {
            return Ok((Mode::Writer, args));
        }
        if first == "--synthetic" || first == "--presenter" {
            return Ok((Mode::Reader, args));
        }
        if first == "--help" || first == "-h" {
            return Err("usage: liminaldoc [DOCUMENT.pdf|DOCUMENT.legebook|DOCUMENT.docx|DOCUMENT.odt|DOCUMENT.md|DOCUMENT.txt]\n       liminaldoc --reader [PDF] | --writer [DOCUMENT]\n       liminaldoc export BOOK.legebook OUTPUT".into());
        }
        let extension = Path::new(first).extension().and_then(OsStr::to_str)
            .unwrap_or("").to_ascii_lowercase();
        if extension == "pdf" {
            return Ok((Mode::Reader, args));
        }
        if extension == "djvu" || extension == "djv" {
            return Err("DjVu reading is not implemented yet".into());
        }
    }
    Ok((Mode::Writer, args))
}

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let (mode, forwarded) = match route(&args) {
        Ok(route) => route,
        Err(message) => {
            eprintln!("{message}");
            return if args.first().is_some_and(|arg| arg == "--help" || arg == "-h") {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            };
        }
    };
    let Ok(executable) = std::env::current_exe() else {
        eprintln!("liminaldoc: could not locate its installed reader and writer");
        return ExitCode::FAILURE;
    };
    let backend = executable.with_file_name(format!("{}{}", mode.executable(), std::env::consts::EXE_SUFFIX));
    match Command::new(&backend).args(forwarded).status() {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1).clamp(0, 255) as u8),
        Err(error) => {
            eprintln!("liminaldoc: could not start {}: {error}", backend.display());
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Mode, route};
    use std::ffi::OsString;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn document_formats_route_without_constructing_a_backend() {
        assert_eq!(route(&args(&["paper.pdf"])).unwrap().0, Mode::Reader);
        assert_eq!(route(&args(&["draft.legebook"])).unwrap().0, Mode::Writer);
        assert_eq!(route(&args(&["draft.docx"])).unwrap().0, Mode::Writer);
        assert_eq!(route(&args(&["export", "draft.legebook", "draft.pdf"])).unwrap().0, Mode::Writer);
        assert!(route(&args(&["scan.djvu"])).is_err());
    }
}
