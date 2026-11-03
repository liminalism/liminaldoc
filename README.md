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

Build the whole workspace before running the launcher: it expects `liminaldoc-reader` and `liminaldoc-writer` beside its own executable. `liminaldoc export BOOK.legebook OUTPUT.pdf` delegates to the writer. PDF opens in the reader; `.legebook`, DOCX, ODT, Apple Pages, Markdown and text open in the writer. DjVu is reserved for the next reader backend and currently reports that it is unsupported.

The reader keeps its Lege rendering, navigation, search and selection. It reads documents and does not process them. The writer keeps editable document tabs, native file dialogs, `.legebook` persistence and PDF/DOCX/Markdown/text output. Opening a PDF from the writer starts a reader window on demand. A shared-window mixed PDF/manuscript tab host is still pending; the separate backends preserve both experiences during the extraction.

## Writing and navigation

The writer prioritizes a minimal, terminal-like appearance and low latency for
books and large articles. File and editing controls share one compact, persistent
row below the title bar; a slim document-tab row appears when multiple documents
are open. The chapter map uses a narrow, resizable column. Page Up and Page Down
turn physical pages, fitted to the available height so their text stays visible.
The page counter keeps a reserved space on the command row. Ctrl+G
(Cmd+G on macOS) opens a small **Go to page** prompt: enter a page number and
press Enter, or Escape to cancel. The prompt also works in fullscreen (F11,
or Ctrl+Shift+F / Cmd+Shift+F if the desktop reserves F11).
Pages are numbered from 1 in document order, including blank pages. Seeking
places the caret at the first editable line when the page has manuscript text;
blank and generated pages are shown without moving the insertion point.

Ctrl+F opens Find; Ctrl+H opens Replace; F3 or Ctrl+Shift+G repeats the search (use Cmd instead of Ctrl
on macOS). Navigation reuses the current layout and does not re-typeset the book.

Drag across text to select it, or Shift-click to extend the selection from its
anchor. Double-click selects a word; triple-click selects a paragraph. Dragging
to the top or bottom edge of the window scrolls through the manuscript. Typing
replaces the selected text. Ctrl+A/C/X/V select all, copy, cut and paste;
Ctrl+Z undoes and Ctrl+Y or Ctrl+Shift+Z redoes (Cmd on macOS). Shift with the
arrow keys extends a selection. Home/End move within the displayed line;
Ctrl+Home/End reach the start/end of the manuscript. Ctrl+Left/Right move by
word, and Ctrl+Backspace/Delete delete by word (Option for word operations on
macOS).

Choose the font size or paragraph style (Body, Epigraph, Block Quote, and so on)
from its toolbar dropdown. Use the arrow keys and Enter to choose, Escape to
cancel, or scroll a list when the window is short. New and imported plain
documents have no automatic chapter. **Chapter** names a chapter starting on
the visible page, or selects its existing title for renaming. On a later page
it starts a new chapter there, keeping the text that follows; Undo removes
the boundary. Ctrl+Shift+N and Ctrl+Shift+R use the same Chapter control.

Autosave runs every **15 minutes** by default. Ctrl+Alt+S (Cmd+Option+S on macOS)
opens a small prompt to choose **15–120 whole minutes**; Enter saves the setting
for future launches, Escape cancels. It works in fullscreen and applies to all
open manuscripts, including inactive tabs. Continued typing does not postpone
autosave; serialization and disk writes run in the background. Ctrl+S saves
immediately and restarts the timer. New or imported documents need an initial
Save As to establish their `.legebook` destination while editing. Closing a tab
or the window saves automatically without a dialog: named books save in place;
unnamed documents receive a unique name under `~/Documents/Liminaldoc/` and
appear in Recent. Untouched blank documents create no file. Failed autosaves are reported,
leave changes unsaved, and retry at the next interval.

## Repository dependencies

The reader and writer have been copied into their own Git repository. Their original ecosystem copies remain during migration but are excluded from its Cargo workspace; its `cargo viewer` and `cargo docwrite` aliases point here. This repository still uses path dependencies in the sibling `Lege-ecosystem` and `liminalkit` workspaces. A release build must pin those dependencies or vendor the needed crates. The AKR ledger records the shared-window integration and platform menu work.

Modern Apple Pages (`.pages`, ZIP or package directory) opens directly in the writer.
Body text and table cells become an unsaved editable manuscript; Save creates a
`.legebook` copy. Table identities, coordinates, column widths and merge extents
survive native save/reopen. Cells reflow in columns, with multiline paragraphs
kept in their cell. Very tall rows flow cell by cell across pages. Vertical
merges, original fonts, styling and floating positions are not reproduced;
floating tables follow the anchored text. Images, text boxes, headers, footnotes
and page-layout documents currently report unsupported content. Formula cells
import their cached values. Table structure commands (insert/delete rows or
columns) remain future work; deletion across cell boundaries is refused. PDF
exports use the reflowed layout; DOCX/Markdown/text exports of tables are refused
until those formats can preserve cell relationships.

The reader owns read-only viewing, navigation, search and copying for PDF and
Markdown. The writer owns manuscript editing and imports of rich source formats
such as DOCX, ODT and Pages. Markdown can be read in the reader and imported for
editing in the writer; Pages belongs to the writer. Opening a format does not
imply saving back to that source format.
