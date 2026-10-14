# Liminaldoc

Liminaldoc brings the Lege PDF reader and page-native manuscript writer into one product workspace. The `liminaldoc` entry point starts the matching backend executable for the file you open. This process boundary keeps the PDF renderer, GPU presenter, tile cache and search workers out of a writing-only session, and keeps the editor and typesetter out of a reading-only session.

## Build and run

```sh
cargo build --workspace
cargo run -p liminaldoc -- draft.legebook
cargo run -p liminaldoc -- paper.pdf
cargo run -p liminaldoc -- --reader
cargo run -p liminaldoc -- --writer
```

Build the whole workspace before running the launcher: it expects `liminaldoc-reader` and `liminaldoc-writer` beside its own executable. `liminaldoc export BOOK.legebook OUTPUT.pdf` delegates to the writer. PDF opens in the reader; `.legebook`, DOCX, ODT, Markdown and text open in the writer. DjVu is reserved for the next reader backend and currently reports that it is unsupported.

The reader keeps its Lege rendering, navigation, search, selection and processing code. The writer keeps editable document tabs, native file dialogs, `.legebook` persistence and PDF/DOCX/Markdown/text output. Opening a PDF from the writer starts a reader window on demand. A shared-window mixed PDF/manuscript tab host is still pending; the separate backends preserve both experiences during the extraction.

## Writing and navigation

The writer prioritizes a minimal, terminal-like appearance and low latency for
books and large articles. Page Up and Page Down turn physical pages. Ctrl+G
(Cmd+G on macOS) opens a small **Go to page** prompt: enter a page number and
press Enter, or Escape to cancel. The prompt also works in fullscreen (F11,
or Ctrl+Shift+F / Cmd+Shift+F if the desktop reserves F11).
Pages are numbered from 1 in document order, including blank pages. Seeking
places the caret at the first editable line when the page has manuscript text;
blank and generated pages are shown without moving the insertion point.

Ctrl+F opens Find; F3 or Ctrl+Shift+G repeats the search (use Cmd instead of Ctrl
on macOS). Navigation reuses the current layout and does not re-typeset the book.

Autosave runs every **15 minutes** by default. Ctrl+Alt+S (Cmd+Option+S on macOS)
opens a small prompt to choose **15–120 whole minutes**; Enter saves the setting
for future launches, Escape cancels. It works in fullscreen and applies to all
open manuscripts, including inactive tabs. Continued typing does not postpone
autosave; serialization and disk writes run in the background. Ctrl+S saves
immediately and restarts the timer. New or imported documents need an initial
Save As to establish their `.legebook` destination. Failed autosaves are reported,
leave changes unsaved, and retry at the next interval.

## Repository dependencies

The reader and writer have been copied into their own Git repository. Their original ecosystem copies remain during migration but are excluded from its Cargo workspace; its `cargo viewer` and `cargo docwrite` aliases point here. This repository still uses path dependencies in the sibling `Lege-ecosystem` and `pixelkit` workspaces. A release build must pin those dependencies or vendor the needed crates. The AKR ledger records the shared-window integration and platform menu work.
