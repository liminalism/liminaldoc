# Reader interaction pipeline and writer keystroke path, 2026-11-03

Headless measurements on a 4-core Linux sandbox (no GPU, release profile,
`opt-level 3`, fat LTO), before and after the changes landed with this
document. Absolute numbers depend on the machine; the ratios are the point.
The raw JSON beside this file is what the ledger evidence cites.

Reproduce:

```text
cargo run --release -p docwrite-app --example bench_corpus -- CORPUS 100
cargo run --release -p liminaldoc-reader --example reader_bench -- --generate CORPUS
cargo run --release -p liminaldoc-reader --example reader_bench -- CORPUS/book.pdf --json out.json
cargo run --release -p docwrite-app --example typing_profile -- CORPUS/book.legebook 120 --json out.json
cargo test --release -p liminaldoc-reader --lib -- ui_thread_cost --ignored --nocapture
```

The reader bench times each step from the moment the viewport intent is
published to the moment the painter's own immutable tile snapshot holds the
surfaces it would draw. "First pixels" counts any fallback (preview or
text-first placeholder); "exact" needs every visible tile at the Final tier.
Page turns are sequential PageDown steps with the window's 140 ms settle
between them; seeks are a deterministic pseudo-random page walk. Index sweep
and search run after the interactions. The writer profile types, deletes and
undoes at the start, middle and end of the manuscript through the same
`Editor` path the window drives, then pages, seeks, finds, saves and reopens.

What changed, in one line each:

- Reader chrome (toolbar, search, status, outline rows) is no longer shaped
  and rasterized on every frame; the retained surface is keyed on its inputs.
- The outline synthesizer bisects a line's characters instead of filtering
  the whole page per line, on the UI thread, for every indexed page.
- The conductor's index sweep no longer skips a resident page it never
  indexed, so the index reaches the last page and search misses nothing.
- Markdown layout shapes blocks across all cores with byte-identical output,
  and every shaper shares one system font scan per process.
- Document opens run on their own thread; the startup document overlaps
  window creation and a toolbar open no longer freezes the previous document.
- The writer keeps paragraphs whose block revision is unchanged instead of
  rebuilding every paragraph per keystroke, keeps the id index across
  keystrokes, caches shaped chrome labels, hashes atlas and id keys cheaply,
  skips page rows outside the window, and runs the model's full
  consistency walk only in debug builds.

