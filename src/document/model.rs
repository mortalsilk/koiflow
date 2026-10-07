use serde::{Deserialize, Serialize};

pub type DocumentId = String;
pub type DocumentKey = String;
pub type BlockId = u64;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Document {
    pub id: DocumentId,
    pub title: String,
    pub page_count: u32,
    pub blocks: Vec<Block>,
    pub outline: Vec<OutlineItem>,
    pub metadata: DocumentMetadata,
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
