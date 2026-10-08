use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub type DocumentId = String;
pub type DocumentKey = String;
pub type BlockId = u64;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentFormat {
    #[default]
    Pdf,
    Epub,
    Mobi,
    Azw3,
}

impl DocumentFormat {
    pub fn from_path(path: &std::path::Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "pdf" => Some(Self::Pdf),
            "epub" => Some(Self::Epub),
            "mobi" => Some(Self::Mobi),
            "azw3" => Some(Self::Azw3),
            _ => None,
        }
    }

    pub fn is_ebook(self) -> bool { self != Self::Pdf }
    pub fn as_str(self) -> &'static str { match self { Self::Pdf => "pdf", Self::Epub => "epub", Self::Mobi => "mobi", Self::Azw3 => "azw3" } }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentCapabilities {
    pub original_pages: bool,
    pub reflow: bool,
    pub rotation: bool,
    pub zoom: bool,
}

impl DocumentCapabilities {
    pub fn for_format(format: DocumentFormat) -> Self {
        Self { original_pages: format == DocumentFormat::Pdf, reflow: true, rotation: format == DocumentFormat::Pdf, zoom: format == DocumentFormat::Pdf }
    }
}

#[derive(Debug, Clone)]
pub struct OpenedDocument {
    pub path: PathBuf,
    pub document_id: DocumentId,
    pub title: String,
    pub page_count: u32,
    pub metadata: DocumentMetadata,
    pub outline: Vec<OutlineItem>,
    pub format: DocumentFormat,
    pub capabilities: DocumentCapabilities,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TypedSourcePosition {
    Pdf { page: u32, char_index: u32 },
    Ebook { section: u32, text_offset: u32 },
}

impl TypedSourcePosition {
    pub fn logical_index(self) -> u32 { match self { Self::Pdf { page, .. } => page, Self::Ebook { section, .. } => section } }
    pub fn text_offset(self) -> u32 { match self { Self::Pdf { char_index, .. } => char_index, Self::Ebook { text_offset, .. } => text_offset } }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Document {
    pub id: DocumentId,
    pub title: String,
    pub page_count: u32,
    pub blocks: Vec<Block>,
    pub outline: Vec<OutlineItem>,
    pub metadata: DocumentMetadata,
    #[serde(default)]
    pub format: DocumentFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub id: BlockId,
    pub kind: BlockKind,
    pub content: String,
    pub source: Vec<SourceRange>,
    pub confidence: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockKind {
    Heading { level: u8 },
    Paragraph,
    ListItem,
    Quote,
    Code,
    Figure,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SourceRange {
    pub page: u32,
    pub start_char: u32,
    pub end_char: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReadingAnchor {
    pub block_id: BlockId,
    pub char_offset: usize,
    pub source: Option<SourcePosition>,
    pub quote: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourcePosition {
    pub page: u32,
    pub char_index: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct PageRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawGlyph {
    pub source: SourcePosition,
    pub ch: char,
    pub bounds: PageRect,
    pub font_name: String,
    pub font_size: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawPage {
    pub page: u32,
    pub width: f32,
    pub height: f32,
    pub glyphs: Vec<RawGlyph>,
    pub text: String,
    pub links: Vec<PageLink>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageLink {
    pub bounds: PageRect,
    pub target: LinkTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LinkTarget {
    Page(u32),
    Uri(String),
    Unknown,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocumentMetadata {
    pub title: String,
    pub author: String,
    pub subject: String,
    pub keywords: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutlineItem {
    pub title: String,
    pub page: Option<u32>,
    pub depth: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnnotationKind {
    Highlight,
    Note,
    Bookmark,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Annotation {
    pub id: i64,
    pub document_id: DocumentKey,
    pub kind: AnnotationKind,
    pub start: SourcePosition,
    pub end: SourcePosition,
    pub color: u32,
    pub quote: String,
    pub note: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub document_id: DocumentKey,
    pub path: String,
    pub title: String,
    pub block_id: BlockId,
    pub page: u32,
    pub snippet: String,
}
