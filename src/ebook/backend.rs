use std::{fs::File, io::Read, path::Path};

use ebook_rs::{Book, DomNode, EbookDomTree, EbookError, LayoutMode, NavPoint};
use image::ImageReader;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::document::{Block, BlockKind, Document, DocumentCapabilities, DocumentFormat, DocumentMetadata, OpenedDocument, OutlineItem, SourceRange};

const MAX_BOOK_BYTES: u64 = 512 * 1024 * 1024;
const MAX_TEXT_CHARS: usize = 64 * 1024 * 1024;
const MAX_SECTIONS: usize = 100_000;
const MAX_COVER_BYTES: usize = 32 * 1024 * 1024;
const MAX_COVER_PIXELS: u64 = 40_000_000;

#[derive(Debug, Error)]
pub enum EbookOpenError {
    #[error("this book is DRM protected; koiflow supports DRM-free books only")]
    DrmProtected,
    #[error("fixed-layout EPUB is not supported yet")]
    FixedLayout,
    #[error("unsupported or mismatched book format: {0}")]
    InvalidFormat(String),
    #[error("the book is too large to open safely")]
    TooLarge,
    #[error("could not read book: {0}")]
    Open(String),
}

#[derive(Debug, Clone)]
pub struct EbookCover {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct OpenedEbook {
    pub opened: OpenedDocument,
    pub document: Document,
    pub cover: Option<EbookCover>,
}

pub fn open_ebook(path: &Path) -> Result<OpenedEbook, EbookOpenError> {
    let expected = DocumentFormat::from_path(path).filter(|format| format.is_ebook())
        .ok_or_else(|| EbookOpenError::InvalidFormat("expected EPUB, MOBI, or AZW3".into()))?;
    let metadata = std::fs::metadata(path).map_err(|error| EbookOpenError::Open(error.to_string()))?;
    if metadata.len() > MAX_BOOK_BYTES { return Err(EbookOpenError::TooLarge); }
    validate_container(path, expected)?;

    let bytes = std::fs::read(path).map_err(|error| EbookOpenError::Open(error.to_string()))?;
    let preferred = matches!(expected, DocumentFormat::Mobi | DocumentFormat::Azw3).then(|| preferred_kf8_rendition(&bytes)).flatten();
    let parse_bytes = preferred.as_deref().unwrap_or(&bytes);
    let fallback = path.file_stem().and_then(|name| name.to_str()).unwrap_or("Untitled");
    let book = Book::from_bytes_with_title(parse_bytes, fallback).map_err(map_error)?;
    if book.layout().layout_mode == LayoutMode::PrePaginated
        || book.metadata().meta_properties.iter().any(|(key, value)| key.contains("rendition:layout") && value.contains("pre-paginated")) {
        return Err(EbookOpenError::FixedLayout);
    }
    let sections = book.get_all_sections_hydrated();
    if sections.len() > MAX_SECTIONS || sections.iter().map(|section| section.char_count).sum::<usize>() > MAX_TEXT_CHARS {
        return Err(EbookOpenError::TooLarge);
    }

    let document_id = format!("{:x}", Sha256::digest(&bytes));
    let fallback_title = path.file_stem().and_then(|name| name.to_str()).unwrap_or("Untitled");
    let title = nonempty(&book.metadata().title).unwrap_or(fallback_title).to_owned();
    let author = book.metadata().creators.join(", ");
    let document_metadata = DocumentMetadata {
        title: title.clone(),
        author,
        subject: book.metadata().subjects.join(", "),
        keywords: String::new(),
    };
    let outline = build_outline(book.toc(), &sections);
    let blocks = sections.iter().flat_map(section_blocks).collect();
    let section_count = sections.len() as u32;
    let opened = OpenedDocument {
        path: path.to_owned(), document_id: document_id.clone(), title: title.clone(), page_count: section_count,
        metadata: document_metadata.clone(), outline: outline.clone(), format: expected,
        capabilities: DocumentCapabilities::for_format(expected),
    };
    let document = Document { id: document_id, title, page_count: section_count, blocks, outline, metadata: document_metadata, format: expected };
    let cover = book.cover_image().and_then(|(bytes, _)| decode_cover(&bytes));
    Ok(OpenedEbook { opened, document, cover })
}

fn validate_container(path: &Path, format: DocumentFormat) -> Result<(), EbookOpenError> {
    let mut file = File::open(path).map_err(|error| EbookOpenError::Open(error.to_string()))?;
    let mut header = [0_u8; 72];
    let read = file.read(&mut header).map_err(|error| EbookOpenError::Open(error.to_string()))?;
    let valid = match format {
        DocumentFormat::Epub => read >= 4 && header.starts_with(b"PK\x03\x04"),
        DocumentFormat::Mobi | DocumentFormat::Azw3 => read >= 68 && (&header[60..68] == b"BOOKMOBI" || &header[60..64] == b"BOOK" || &header[64..68] == b"MOBI"),
        DocumentFormat::Pdf => false,
    };
    valid.then_some(()).ok_or_else(|| EbookOpenError::InvalidFormat(format!("the file contents do not match .{}", format.as_str())))
}

/// Hybrid Mobipocket files store a legacy rendition followed by a BOUNDARY
/// record and a KF8 record set. Build a private in-memory PDB view rooted at
/// that KF8 header so the parser consistently chooses the modern rendition.
fn preferred_kf8_rendition(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 86 { return None; }
    let count = u16::from_be_bytes([bytes[76], bytes[77]]) as usize;
    if count < 3 || bytes.len() < 78 + count.checked_mul(8)? { return None; }
    let offsets: Vec<usize> = (0..count).map(|index| {
        let at = 78 + index * 8;
        Some(u32::from_be_bytes(bytes[at..at + 4].try_into().ok()?) as usize)
    }).collect::<Option<_>>()?;
    let rec0_start = *offsets.first()?;
    let rec0_end = offsets.get(1).copied().unwrap_or(bytes.len());
    if rec0_start + 24 > rec0_end || rec0_end > bytes.len() || &bytes[rec0_start + 16..rec0_start + 20] != b"MOBI" { return None; }
    let mobi_len = u32::from_be_bytes(bytes[rec0_start + 20..rec0_start + 24].try_into().ok()?) as usize;
    let exth = rec0_start.checked_add(16)?.checked_add(mobi_len)?;
    if exth + 12 > rec0_end || &bytes[exth..exth + 4] != b"EXTH" { return None; }
    let exth_count = u32::from_be_bytes(bytes[exth + 8..exth + 12].try_into().ok()?) as usize;
    let mut cursor = exth + 12;
    let mut boundary = None;
    for _ in 0..exth_count {
        if cursor + 8 > rec0_end { break; }
        let tag = u32::from_be_bytes(bytes[cursor..cursor + 4].try_into().ok()?);
        let len = u32::from_be_bytes(bytes[cursor + 4..cursor + 8].try_into().ok()?) as usize;
        if len < 8 || cursor + len > rec0_end { break; }
        if tag == 121 && len >= 12 { boundary = Some(u32::from_be_bytes(bytes[cursor + 8..cursor + 12].try_into().ok()?) as usize); break; }
        cursor += len;
    }
    let first = boundary?.checked_add(1)?;
    if first >= count { return None; }
    let new_count = count - first;
    let table_end = 78_usize.checked_add(new_count.checked_mul(8)?)?.checked_add(2)?;
    let mut output = Vec::with_capacity(table_end + bytes.len().saturating_sub(offsets[first]));
    output.extend_from_slice(&bytes[..78]);
    output[76..78].copy_from_slice(&(new_count as u16).to_be_bytes());
    let mut data_offset = table_end;
    for old_index in first..count {
        output.extend_from_slice(&(data_offset as u32).to_be_bytes());
        let entry = 78 + old_index * 8;
        output.extend_from_slice(&bytes[entry + 4..entry + 8]);
        let end = offsets.get(old_index + 1).copied().unwrap_or(bytes.len());
        data_offset = data_offset.checked_add(end.checked_sub(offsets[old_index])?)?;
    }
    output.extend_from_slice(&[0, 0]);
    output.extend_from_slice(&bytes[offsets[first]..]);
    Some(output)
}

fn map_error(error: EbookError) -> EbookOpenError {
    match error {
        EbookError::DrmProtected(_) => EbookOpenError::DrmProtected,
        EbookError::InvalidFormat(message) => EbookOpenError::InvalidFormat(message),
        other => EbookOpenError::Open(other.to_string()),
    }
}

fn section_blocks(section: &ebook_rs::Section) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut offset = 0_u32;
    let mut semantic = Vec::new();
    let mut tree = EbookDomTree::parse(&section.raw_html);
    tree.strip_elements(&["script", "style", "iframe", "object", "embed"]);
    collect_semantic_blocks(&tree.root_nodes, &mut semantic);
    if semantic.is_empty() && !section.plain_text.trim().is_empty() { semantic.push((BlockKind::Paragraph, section.plain_text.trim().to_owned())); }
    for (index, (kind, paragraph)) in semantic.into_iter().enumerate() {
        let count = paragraph.chars().count().min(u32::MAX as usize) as u32;
        let id = ((section.index as u64) << 32) | (index as u64 + 1);
        blocks.push(Block { id, kind, content: paragraph, source: vec![SourceRange { page: section.index as u32, start_char: offset, end_char: offset.saturating_add(count) }], confidence: 1.0 });
        offset = offset.saturating_add(count).saturating_add(1);
    }
    blocks
}

fn collect_semantic_blocks(nodes: &[DomNode], output: &mut Vec<(BlockKind, String)>) {
    for node in nodes {
        let DomNode::Element { tag_name, attributes, children } = node else { continue };
        let tag = tag_name.to_ascii_lowercase();
        let kind = match tag.as_str() {
            "h1" => Some(BlockKind::Heading { level: 1 }), "h2" => Some(BlockKind::Heading { level: 2 }),
            "h3" => Some(BlockKind::Heading { level: 3 }), "h4" | "h5" | "h6" => Some(BlockKind::Heading { level: 4 }),
            "p" => Some(BlockKind::Paragraph), "li" => Some(BlockKind::ListItem), "blockquote" => Some(BlockKind::Quote),
            "pre" | "code" => Some(BlockKind::Code),
            "img" => Some(BlockKind::Figure),
            _ => None,
        };
        if let Some(kind) = kind {
            let raw = if tag == "img" { attributes.get("alt").map(|value| value.as_str()).unwrap_or("Illustration").to_owned() } else { node_text(children) };
            let text = normalize_text(&decode_entities(&raw));
            if !text.is_empty() { output.push((kind, text)); }
        } else { collect_semantic_blocks(children, output); }
    }
}

fn node_text(nodes: &[DomNode]) -> String {
    let mut text = String::new();
    for node in nodes {
        match node {
            DomNode::Text(value) => { text.push_str(value); text.push(' '); }
            DomNode::Element { tag_name, children, .. } => {
                if tag_name.eq_ignore_ascii_case("br") { text.push('\n'); }
                text.push_str(&node_text(children));
            }
            DomNode::Comment(_) => {}
        }
    }
    text
}

fn normalize_text(text: &str) -> String { text.split_whitespace().collect::<Vec<_>>().join(" ") }

fn decode_entities(text: &str) -> String {
    let mut decoded = text.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&nbsp;", " ");
    while let Some(start) = decoded.find("&#") {
        let Some(relative_end) = decoded[start..].find(';') else { break };
        let end = start + relative_end;
        let entity = &decoded[start + 2..end];
        let value = entity.strip_prefix(['x', 'X']).and_then(|hex| u32::from_str_radix(hex, 16).ok()).or_else(|| entity.parse().ok()).and_then(char::from_u32);
        let Some(value) = value else { break };
        decoded.replace_range(start..=end, &value.to_string());
    }
    decoded
}

fn build_outline(points: &[NavPoint], sections: &[ebook_rs::Section]) -> Vec<OutlineItem> {
    fn visit(points: &[NavPoint], sections: &[ebook_rs::Section], depth: u8, output: &mut Vec<OutlineItem>) {
        for point in points {
            let target = point.full_path.split('#').next().unwrap_or(&point.full_path);
            let page = sections.iter().position(|section| section.full_path == target || section.href.split('#').next() == Some(target)).map(|index| index as u32);
            output.push(OutlineItem { title: point.label.clone(), page, depth });
            visit(&point.subitems, sections, depth.saturating_add(1), output);
        }
    }
    let mut output = Vec::new();
    visit(points, sections, 0, &mut output);
    output
}

fn decode_cover(bytes: &[u8]) -> Option<EbookCover> {
    if bytes.len() > MAX_COVER_BYTES { return None; }
    let reader = ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    let image = reader.decode().ok()?;
    if u64::from(image.width()) * u64::from(image.height()) > MAX_COVER_PIXELS { return None; }
    let rgba = image.into_rgba8();
    Some(EbookCover { width: rgba.width() as usize, height: rgba.height() as usize, rgba: rgba.into_raw() })
}

fn nonempty(value: &str) -> Option<&str> { (!value.trim().is_empty()).then_some(value.trim()) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_extensions_are_case_insensitive() {
        assert_eq!(DocumentFormat::from_path(Path::new("book.EPUB")), Some(DocumentFormat::Epub));
        assert_eq!(DocumentFormat::from_path(Path::new("book.azw3")), Some(DocumentFormat::Azw3));
        assert_eq!(DocumentFormat::from_path(Path::new("book.txt")), None);
    }

    #[test]
    fn semantic_html_preserves_common_reader_blocks() {
        let section = ebook_rs::Section { index: 2, idref: "c".into(), href: "c.xhtml".into(), full_path: "c.xhtml".into(), raw_html: "<h1>A &amp; B</h1><p>Hello <em>world</em>.</p><ul><li>First</li></ul><img alt='Map'/>".into(), processed_html: String::new(), plain_text: String::new(), plain_text_lower: String::new(), char_count: 0, viewport_width: None, viewport_height: None };
        let blocks = section_blocks(&section);
        assert_eq!(blocks.len(), 4);
        assert!(matches!(blocks[0].kind, BlockKind::Heading { level: 1 }));
        assert_eq!(blocks[0].content, "A & B");
        assert!(matches!(blocks[2].kind, BlockKind::ListItem));
        assert!(matches!(blocks[3].kind, BlockKind::Figure));
    }

    #[test]
    fn opens_epub_spine_metadata_outline_and_unicode() {
        let path = std::env::temp_dir().join(format!("koiflow-epub-fixture-{}.epub", std::process::id()));
        std::fs::write(&path, ebook_rs::generate_sample_epub().unwrap()).unwrap();
        let opened = open_ebook(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(opened.opened.format, DocumentFormat::Epub);
        assert_eq!(opened.opened.page_count, 3);
        assert!(opened.opened.title.contains("Rustonomicon"));
        assert!(!opened.document.outline.is_empty());
        assert!(opened.document.blocks.iter().any(|block| matches!(block.kind, BlockKind::Heading { .. })));
        assert!(opened.document.blocks.iter().all(|block| block.source[0].end_char >= block.source[0].start_char));
    }
}
