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

## Repository dependencies

The reader and writer have been copied into their own Git repository. Their original ecosystem copies remain during migration but are excluded from its Cargo workspace; its `cargo viewer` and `cargo docwrite` aliases point here. This repository still uses path dependencies in the sibling `Lege-ecosystem` and `pixelkit` workspaces. A release build must pin those dependencies or vendor the needed crates. The AKR ledger records the shared-window integration and platform menu work.
