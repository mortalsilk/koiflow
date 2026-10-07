# KoiFlow

KoiFlow is a desktop-first PDF reader written in Rust. It provides both faithful page rendering and a calm, customizable reflow view for text-heavy books.

## What is implemented

- Virtualized continuous PDF pages with fit-width, fit-page, zoom, rotation, page jump, and a 256 MiB LRU texture budget
- Multiple document tabs, restored sessions, drag-and-drop, command-line opening, and password-protected PDFs
- Native PDF metadata, outlines, internal links, external links, glyph geometry, and source-character mappings
- Progressive prioritized extraction with request generations, cancellation, stale-result rejection, and background indexing
- Geometry-aware semantic reflow with one/two-column ordering, headings, paragraphs, lists, and soft-hyphen repair
- Selectable reflow text and live font, line-height, letter-spacing, alignment, width, margin, spacing, and theme controls
- Indexed local library and SQLite FTS search within a document or across the library
- Non-destructive highlights, notes, bookmarks, and an annotation navigator
- Versioned, compressed semantic and rendered-page caches with atomic writes
- Debounced logical reading progress, missing-file handling, cache management, keyboard shortcuts, and distraction-free mode

## Requirements

- Rust 1.85 or newer
- Linux on x86-64

PDFium is bundled into the KoiFlow executable and installed into KoiFlow's private data directory automatically on first use. No `apt` command, system package, environment variable, or manual library setup is required.

## Run

```sh
cargo run --release
```

Open a PDF with the **Open** button or `Ctrl+O`. Toggle the library with `Ctrl+B`, and use `F11` for distraction-free reading.

PDFs can also be dropped onto the window or passed on the command line:

```sh
cargo run --release -- /path/to/book.pdf
```

## Install without root access

```sh
sh packaging/install-user.sh
```

This installs the executable, desktop entry, icon, and PDF MIME association under `~/.local`. It does not use `apt` or require administrator access.

## Validate

```sh
cargo check
cargo test
```

## Architecture

The application is a modular monolith with deliberately strict boundaries:

- `pdf` owns PDFium binding, rendering, and the serialized background worker.
- `extraction` turns physical PDF text into logical blocks using pure, testable functions.
- `document` is the UI-independent semantic model and source mapping.
- `reading` owns modes and typography/layout preferences.
- `app` coordinates progressive work and presents the desktop interface.
- `storage` persists user state separately from versioned extraction caches.

This keeps typography changes and mode switching independent from PDF extraction. OCR, complex table/math reconstruction, form filling, PDF modification, cloud sync, and non-Linux packaging remain later-phase work.
