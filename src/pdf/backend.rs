use std::{path::{Path, PathBuf}, sync::OnceLock};

use directories::ProjectDirs;
use pdfium_render::prelude::*;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::document::{DocumentMetadata, LinkTarget, OutlineItem, PageLink, PageRect, RawGlyph, RawPage, SourcePosition};

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const BUNDLED_PDFIUM: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/vendor/pdfium/linux-x64/lib/libpdfium.so"
));

const BUNDLED_PDFIUM_VERSION: &str = "157.0.8086.0";
static BUNDLED_PDFIUM_PATH: OnceLock<PathBuf> = OnceLock::new();

#[derive(Debug, Error)]
pub enum PdfError {
    #[error("the bundled PDF engine could not be loaded")]
    MissingPdfium,
    #[error("the bundled PDF engine could not be prepared: {0}")]
    Runtime(String),
    #[error("could not open PDF: {0}")]
    Open(String),
    #[error("this PDF requires a password")]
    PasswordRequired,
    #[error("could not read page {0}")]
    Page(u32),
}

#[derive(Debug, Clone)]
pub struct OpenedPdf {
    pub path: PathBuf,
    pub document_id: String,
    pub title: String,
    pub page_count: u32,
    pub metadata: DocumentMetadata,
    pub outline: Vec<OutlineItem>,
}

#[derive(Debug)]
pub struct PageData {
    pub page: u32,
    pub rgba: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub raw: RawPage,
}

pub trait PdfBackend {
    fn inspect(&self, path: &Path, password: Option<&str>) -> Result<OpenedPdf, PdfError>;
    fn load_page(&self, path: &Path, password: Option<&str>, page: u32, target_width: i32, rotation: u16, render: bool) -> Result<PageData, PdfError>;
}

pub struct PdfiumBackend {
    pdfium: Pdfium,
}

impl PdfiumBackend {
    pub fn new() -> Result<Self, PdfError> {
        let bundled = bundled_pdfium_path()?;
        let bindings = Pdfium::bind_to_library(&bundled)
            .or_else(|_| Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(".")))
            .or_else(|_| Pdfium::bind_to_system_library())
            .map_err(|_| PdfError::MissingPdfium)?;
        Ok(Self { pdfium: Pdfium::new(bindings) })
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn bundled_pdfium_path() -> Result<PathBuf, PdfError> {
    if let Some(path) = BUNDLED_PDFIUM_PATH.get() {
        return Ok(path.clone());
    }

    let data_dir = ProjectDirs::from("org", "KoiFlow", "KoiFlow")
        .map(|dirs| dirs.data_local_dir().to_owned())
        .unwrap_or_else(std::env::temp_dir);
    let runtime_dir = data_dir.join("runtime").join(BUNDLED_PDFIUM_VERSION);
    let library_path = runtime_dir.join("libpdfium.so");
    let expected_digest = Sha256::digest(BUNDLED_PDFIUM);
    let installed_is_valid = std::fs::read(&library_path)
        .map(|bytes| Sha256::digest(bytes) == expected_digest)
        .unwrap_or(false);

    if !installed_is_valid {
        std::fs::create_dir_all(&runtime_dir)
            .map_err(|error| PdfError::Runtime(error.to_string()))?;
        let temporary_path = runtime_dir.join("libpdfium.so.part");
        std::fs::write(&temporary_path, BUNDLED_PDFIUM)
            .map_err(|error| PdfError::Runtime(error.to_string()))?;
        std::fs::rename(&temporary_path, &library_path)
            .map_err(|error| PdfError::Runtime(error.to_string()))?;
    }

    let _ = BUNDLED_PDFIUM_PATH.set(library_path.clone());
    Ok(library_path)
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn bundled_pdfium_path() -> Result<PathBuf, PdfError> {
    Err(PdfError::Runtime(
        "this build does not contain a PDFium runtime for the current platform".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    fn bundled_runtime_loads_and_renders_without_system_pdfium() {
        let backend = PdfiumBackend::new()
            .expect("bundled PDFium should load without a system package");
        let fixture = Path::new("/usr/share/cups/data/form_english.pdf");

        // Standard CUPS installations provide this small PDF, letting local builds also exercise
        // the real rendering and text APIs without checking a generated fixture into the project.
        if fixture.exists() {
            let metadata = backend.inspect(fixture, None).expect("fixture should open");
            assert!(metadata.page_count > 0);
            let page = backend.load_page(fixture, None, 0, 640, 0, true).expect("fixture should render");
            assert!(page.width > 0 && page.height > 0 && !page.rgba.is_empty());
        }
    }
}

impl PdfBackend for PdfiumBackend {
    fn inspect(&self, path: &Path, password: Option<&str>) -> Result<OpenedPdf, PdfError> {
        let document = self.pdfium
            .load_pdf_from_file(path, password)
            .map_err(map_open_error)?;
        let file_title = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Untitled").to_owned();
        let get_meta = |kind| document.metadata().get(kind).map(|tag| tag.value().trim().to_owned()).unwrap_or_default();
        let metadata = DocumentMetadata {
            title: get_meta(PdfDocumentMetadataTagType::Title),
            author: get_meta(PdfDocumentMetadataTagType::Author),
            subject: get_meta(PdfDocumentMetadataTagType::Subject),
            keywords: get_meta(PdfDocumentMetadataTagType::Keywords),
        };
        let title = if metadata.title.is_empty() { file_title } else { metadata.title.clone() };
        let mut outline = Vec::new();
        if let Some(root) = document.bookmarks().root() {
            collect_outline(root, 0, &mut outline);
        }
        let bytes = std::fs::read(path).map_err(|error| PdfError::Open(error.to_string()))?;
        let document_id = format!("{:x}", Sha256::digest(bytes));
        Ok(OpenedPdf { path: path.to_owned(), document_id, title, page_count: document.pages().len() as u32, metadata, outline })
    }

    fn load_page(&self, path: &Path, password: Option<&str>, page: u32, target_width: i32, rotation: u16, render: bool) -> Result<PageData, PdfError> {
        let document = self.pdfium
            .load_pdf_from_file(path, password)
            .map_err(map_open_error)?;
        let pdf_page = document.pages().get(page as i32).map_err(|_| PdfError::Page(page))?;
        let page_text = pdf_page.text().map_err(|_| PdfError::Page(page))?;
        let text = page_text.all();
        let glyphs = page_text.chars().iter().filter_map(|glyph| {
            let ch = glyph.unicode_char()?;
            let bounds = glyph.loose_bounds().ok()?;
            Some(RawGlyph {
                source: SourcePosition { page, char_index: glyph.index() as u32 },
                ch,
                bounds: PageRect { left: bounds.left().value, top: bounds.top().value, right: bounds.right().value, bottom: bounds.bottom().value },
                font_name: glyph.font_name(),
                font_size: glyph.scaled_font_size().value,
            })
        }).collect();
        let links = pdf_page.links().iter().map(|link| {
            let bounds = link.rect().ok().map(|rect| PageRect { left: rect.left().value, top: rect.top().value, right: rect.right().value, bottom: rect.bottom().value }).unwrap_or_default();
            let target = if let Some(destination) = link.destination() {
                destination.page_index().ok().map(|page| LinkTarget::Page(page as u32)).unwrap_or(LinkTarget::Unknown)
            } else if let Some(action) = link.action().and_then(|action| action.as_uri_action().and_then(|uri| uri.uri().ok()).map(LinkTarget::Uri)) {
                action
            } else { LinkTarget::Unknown };
            PageLink { bounds, target }
        }).collect();
        let pdf_rotation = match rotation % 360 { 90 => PdfPageRenderRotation::Degrees90, 180 => PdfPageRenderRotation::Degrees180, 270 => PdfPageRenderRotation::Degrees270, _ => PdfPageRenderRotation::None };
        let bitmap = if render { Some(pdf_page
            .render_with_config(&PdfRenderConfig::new().set_target_width(target_width.max(480)).rotate(pdf_rotation, true))
            .map_err(|_| PdfError::Page(page))?
            .as_image()
            .map_err(|_| PdfError::Page(page))?
            .to_rgba8()) } else { None };
        let (width, height) = bitmap.as_ref().map(|bitmap| bitmap.dimensions()).unwrap_or((0, 0));
        Ok(PageData {
            page,
            rgba: bitmap.map(|bitmap| bitmap.into_raw()).unwrap_or_default(),
            width: width as usize,
            height: height as usize,
            raw: RawPage { page, width: pdf_page.width().value, height: pdf_page.height().value, glyphs, text, links },
        })
    }
}

fn map_open_error(error: PdfiumError) -> PdfError {
    if error.to_string().to_ascii_lowercase().contains("password") { PdfError::PasswordRequired } else { PdfError::Open(error.to_string()) }
}

fn collect_outline(bookmark: PdfBookmark<'_>, depth: u8, output: &mut Vec<OutlineItem>) {
    output.push(OutlineItem {
        title: bookmark.title().unwrap_or_else(|| "Untitled section".into()),
        page: bookmark.destination().and_then(|destination| destination.page_index().ok()).map(|page| page as u32),
        depth,
    });
    if let Some(child) = bookmark.first_child() { collect_outline(child, depth.saturating_add(1), output); }
    if let Some(sibling) = bookmark.next_sibling() { collect_outline(sibling, depth, output); }
}
