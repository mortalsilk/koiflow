# Third-party notices

koiflow uses `ebook-rs` 0.16.6 for DRM-free EPUB, MOBI, and AZW3 container parsing and reconstruction.

- Project: https://github.com/SV-stark/ebook-rs
- License: MIT

The complete dependency version and checksum are pinned in `Cargo.lock`. koiflow links this Rust dependency into the application and does not require Calibre, `libmobi`, or another system ebook tool at runtime.

Other Rust dependencies and their exact versions are recorded in `Cargo.lock` and retain their respective licenses.

koiflow bundles DejaVu Sans and DejaVu Serif from the `dejavu` crate under its permissive font license. The license text is distributed by the source crate and the exact crate version is pinned in `Cargo.lock`.
