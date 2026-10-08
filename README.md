# koiflow

koiflow is a desktop-first document reader written in Rust. It provides faithful PDF page rendering and a calm, customizable reader-first reflow view for PDF, EPUB, MOBI, and AZW3 books.

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
- Editorial Library interface with bundled SVG icons and fonts, seven application themes, responsive animated drawers, reduced-motion support, and independent paper/sepia/dark reading surfaces
- Complete virtualized library home with continue-reading, local title/author filtering, recent/title sorting, progress cards, cached cover thumbnails, and deterministic typographic placeholders
- DRM-free EPUB 2/3, MOBI, and AZW3 support with metadata, covers, spine order, outlines, search, annotations, and restored reading positions

## Requirements

- Rust 1.85 or newer
- Linux on x86-64

PDFium and the ebook parser are self-contained. PDFium is installed into koiflow's private data directory automatically on first use, while ebook parsing is implemented in Rust. No `apt` command, Calibre install, system package, environment variable, or manual library setup is required.

## Run

```sh
cargo run --release
```

Open a supported document with the **Open** button or `Ctrl+O`. Toggle the library with `Ctrl+B`, and use `F11` for distraction-free reading.

Documents can also be dropped onto the window or passed on the command line:

```sh
cargo run --release -- /path/to/book.pdf
```

## Install without root access

```sh
sh packaging/install-user.sh
```

This installs the executable, desktop entry, icon, and PDF/EPUB/Mobipocket MIME associations under `~/.local`. It does not use `apt` or require administrator access.

## Validate

```sh
cargo check
cargo test
```

## Architecture

The application is a modular monolith with deliberately strict boundaries:

- `pdf` owns PDFium binding, rendering, and the serialized background worker.
- `ebook` owns bounded EPUB/MOBI/AZW3 parsing and its independent cancellable worker.
- `extraction` turns physical PDF text into logical blocks using pure, testable functions.
- `document` is the UI-independent semantic model and source mapping.
- `reading` owns modes and typography/layout preferences.
- `app` coordinates progressive work and presents the desktop interface.
- `storage` persists user state separately from versioned extraction caches.

This keeps typography changes and mode switching independent from PDF extraction. OCR, complex table/math reconstruction, form filling, PDF modification, cloud sync, and non-Linux packaging remain later-phase work.
