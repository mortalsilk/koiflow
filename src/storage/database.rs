use std::{
    io::Read,
    path::{Path, PathBuf},
};

use directories::ProjectDirs;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{
    document::{Annotation, AnnotationKind, Document, DocumentFormat, OpenedDocument, ReadingAnchor, SearchHit, SourcePosition},
    reading::{AppTheme, ReaderStyle, ReaderTheme, ReadingMode},
};

const SCHEMA_VERSION: i64 = 4;
// Version 3 rebuilds semantic text produced before native character ordering
// was restored after geometry-based line detection.
const PDF_EXTRACTION_CACHE_VERSION: u32 = 3;
const EBOOK_EXTRACTION_CACHE_VERSION: u32 = 1_001;
const MAX_DOCUMENT_CACHE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_PAGE_CACHE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct CachedDocument { version: u32, document: Document }

#[derive(Serialize, Deserialize)]
struct CachedPage { version: u32, width: usize, height: usize, rgba: Vec<u8> }

#[derive(Debug, Clone)]
pub struct LibraryEntry {
    pub document_id: String,
    pub path: PathBuf,
    pub title: String,
    pub author: String,
    pub page_count: u32,
    pub progress: f32,
    pub missing: bool,
    pub format: DocumentFormat,
}

#[derive(Debug, Clone)]
pub struct SavedTab { pub path: PathBuf, pub active: bool, pub position: u32 }

pub struct Storage { connection: Connection, pub data_dir: PathBuf }

impl Storage {
    pub fn open() -> rusqlite::Result<Self> {
        let data_dir = ProjectDirs::from("org", "KoiFlow", "KoiFlow")
            .map(|dirs| dirs.data_local_dir().to_owned())
            .unwrap_or_else(|| PathBuf::from(".koiflow"));
        std::fs::create_dir_all(data_dir.join("cache")).ok();
        std::fs::create_dir_all(data_dir.join("page-cache")).ok();
        std::fs::create_dir_all(data_dir.join("cover-cache")).ok();
        let connection = Connection::open(data_dir.join("reader.sqlite"))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&connection)?;
        Ok(Self { connection, data_dir })
    }

    pub fn load_style(&self) -> Option<ReaderStyle> {
        let json: String = self.connection.query_row("SELECT json FROM settings WHERE id=1", [], |row| row.get(0)).ok()?;
        let legacy_theme = serde_json::from_str::<serde_json::Value>(&json).ok()
            .and_then(|value| value.get("theme").and_then(|value| value.as_str()).map(str::to_owned));
        let had_app_theme = serde_json::from_str::<serde_json::Value>(&json).ok()
            .is_some_and(|value| value.get("app_theme").is_some());
        let mut style: ReaderStyle = serde_json::from_str(&json).ok()?;
        if !had_app_theme && legacy_theme.as_deref() == Some("Dark") {
            style.app_theme = AppTheme::Dark;
            style.theme = ReaderTheme::Dark;
        }
        style.sanitize();
        Some(style)
    }

    pub fn save_style(&self, style: &ReaderStyle) {
        if let Ok(json) = serde_json::to_string(style) {
            self.connection.execute("INSERT INTO settings(id,json) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET json=?1", [json]).ok();
        }
    }

    pub fn remember_document(&self, opened: &OpenedDocument) {
        let Ok(transaction) = self.connection.unchecked_transaction() else { return };
        if transaction.execute(
            "DELETE FROM search_blocks WHERE document_id IN (SELECT document_id FROM library_documents WHERE path=?1 AND document_id<>?2)",
            params![opened.path.to_string_lossy(), opened.document_id],
        ).is_err() { return; }
        if transaction.execute(
            "DELETE FROM library_documents WHERE path=?1 AND document_id<>?2",
            params![opened.path.to_string_lossy(), opened.document_id],
        ).is_err() { return; }
        if transaction.execute(
            "INSERT INTO library_documents(document_id,path,title,author,page_count,last_opened,missing,format)
             VALUES(?1,?2,?3,?4,?5,strftime('%s','now'),0,?6)
             ON CONFLICT(document_id) DO UPDATE SET path=?2,title=?3,author=?4,page_count=?5,last_opened=strftime('%s','now'),missing=0,format=?6",
            params![opened.document_id, opened.path.to_string_lossy(), opened.title, opened.metadata.author, opened.page_count, opened.format.as_str()],
        ).is_err() { return; }
        transaction.commit().ok();
    }

    pub fn library(&self) -> Vec<LibraryEntry> {
        let Ok(mut query) = self.connection.prepare(
            "SELECT document_id,path,title,author,page_count,progress,missing,format FROM library_documents ORDER BY last_opened DESC"
        ) else { return vec![] };
        query.query_map([], |row| Ok(LibraryEntry {
            document_id: row.get(0)?, path: PathBuf::from(row.get::<_, String>(1)?), title: row.get(2)?, author: row.get(3)?,
            page_count: nonnegative_u32(row.get(4)?), progress: finite_progress(row.get(5)?), missing: row.get::<_, i64>(6)? != 0,
            format: parse_format(row.get::<_, String>(7)?.as_str()),
        })).map(|rows| rows.filter_map(Result::ok).map(|mut entry| { entry.missing |= !entry.path.exists(); entry }).collect()).unwrap_or_default()
    }

    pub fn save_progress(&self, document_id: &str, path: &Path, anchor: &ReadingAnchor, page: u32, mode: ReadingMode, page_count: u32) {
        let mode = if mode == ReadingMode::Reflow { "reflow" } else { "original" };
        let page = if page_count == 0 { 0 } else { page.min(page_count - 1) };
        let source_page = anchor.source.map(|source| source.page).unwrap_or(page);
        let source_page = if page_count == 0 { 0 } else { source_page.min(page_count - 1) };
        let source_char = anchor.source.map(|source| source.char_index).unwrap_or(0);
        let progress = if page_count > 1 { page as f32 / (page_count - 1) as f32 } else { 0.0 };
        let location_kind = self.location_kind(document_id);
        self.connection.execute(
            "INSERT INTO reading_progress(document_id,path,block_id,char_offset,source_page,source_char,quote,page,mode,updated_at,location_kind)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,strftime('%s','now'),?10)
             ON CONFLICT(document_id) DO UPDATE SET path=?2,block_id=?3,char_offset=?4,source_page=?5,source_char=?6,quote=?7,page=?8,mode=?9,updated_at=strftime('%s','now'),location_kind=?10",
            params![document_id, path.to_string_lossy(), anchor.block_id as i64, anchor.char_offset as i64, source_page, source_char, anchor.quote, page, mode, location_kind],
        ).ok();
        self.connection.execute("UPDATE library_documents SET progress=?2 WHERE document_id=?1", params![document_id, progress]).ok();
    }

    pub fn load_progress(&self, document_id: &str) -> Option<(ReadingAnchor, u32, ReadingMode)> {
        self.connection.query_row(
            "SELECT block_id,char_offset,source_page,source_char,quote,page,mode FROM reading_progress WHERE document_id=?1", [document_id],
            |row| {
                let mode: String = row.get(6)?;
                Ok((ReadingAnchor {
                    block_id: nonnegative_u64(row.get(0)?),
                    char_offset: nonnegative_usize(row.get(1)?),
                    source: Some(SourcePosition { page: nonnegative_u32(row.get(2)?), char_index: nonnegative_u32(row.get(3)?) }),
                    quote: row.get(4)?,
                }, nonnegative_u32(row.get(5)?), if mode == "original" { ReadingMode::Original } else { ReadingMode::Reflow }))
            },
        ).optional().ok().flatten()
    }

    pub fn save_tabs(&mut self, tabs: &[SavedTab]) {
        let Ok(transaction) = self.connection.transaction() else { return };
        if transaction.execute("DELETE FROM open_tabs", []).is_err() { return; }
        for tab in tabs {
            if transaction.execute("INSERT INTO open_tabs(position,path,active) VALUES(?1,?2,?3)", params![tab.position, tab.path.to_string_lossy(), tab.active]).is_err() { return; }
        }
        transaction.commit().ok();
    }

    pub fn load_tabs(&self) -> Vec<SavedTab> {
        let Ok(mut query) = self.connection.prepare("SELECT position,path,active FROM open_tabs ORDER BY position") else { return vec![] };
        query.query_map([], |row| Ok(SavedTab { position: nonnegative_u32(row.get(0)?), path: PathBuf::from(row.get::<_, String>(1)?), active: row.get(2)? }))
            .map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
    }

    pub fn index_document(&mut self, document: &Document, path: &Path) {
        let Ok(transaction) = self.connection.transaction() else { return };
        if transaction.execute("DELETE FROM search_blocks WHERE document_id=?1", [&document.id]).is_err() { return; }
        for block in &document.blocks {
            let page = block.source.first().map(|source| source.page).unwrap_or(0);
            if transaction.execute(
                "INSERT INTO search_blocks(document_id,path,title,block_id,page,content) VALUES(?1,?2,?3,?4,?5,?6)",
                params![document.id, path.to_string_lossy(), document.title, block.id as i64, page, block.content],
            ).is_err() { return; }
        }
        transaction.execute("UPDATE library_documents SET extraction_version=?2 WHERE document_id=?1", params![document.id, cache_version(document.format)]).ok();
        transaction.commit().ok();
    }

    pub fn search(&self, query: &str, document_id: Option<&str>, limit: usize) -> Vec<SearchHit> {
        if query.trim().is_empty() { return vec![]; }
        let sql = if document_id.is_some() {
            "SELECT document_id,path,title,block_id,page,snippet(search_blocks,5,'[',']',' … ',18) FROM search_blocks WHERE search_blocks MATCH ?1 AND document_id=?2 LIMIT ?3"
        } else {
            "SELECT document_id,path,title,block_id,page,snippet(search_blocks,5,'[',']',' … ',18) FROM search_blocks WHERE search_blocks MATCH ?1 LIMIT ?2"
        };
        let Ok(mut statement) = self.connection.prepare(sql) else { return vec![] };
        let map = |row: &rusqlite::Row<'_>| Ok(SearchHit {
            document_id: row.get(0)?, path: row.get(1)?, title: row.get(2)?, block_id: nonnegative_u64(row.get(3)?),
            page: nonnegative_u32(row.get(4)?), snippet: row.get(5)?,
        });
        let result = if let Some(document_id) = document_id {
            statement.query_map(params![fts_query(query), document_id, limit as i64], map)
        } else { statement.query_map(params![fts_query(query), limit as i64], map) };
        result.map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
    }

    pub fn annotations(&self, document_id: &str) -> Vec<Annotation> {
        let Ok(mut statement) = self.connection.prepare(
            "SELECT id,document_id,kind,start_page,start_char,end_page,end_char,color,quote,note,created_at,updated_at FROM annotations WHERE document_id=?1 ORDER BY start_page,start_char"
        ) else { return vec![] };
        statement.query_map([document_id], annotation_from_row).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
    }

    pub fn save_annotation(&self, annotation: &mut Annotation) {
        let kind = annotation_kind(annotation.kind);
        let location_kind = self.location_kind(&annotation.document_id);
        if annotation.id == 0 {
            if self.connection.execute(
                "INSERT INTO annotations(document_id,kind,start_page,start_char,end_page,end_char,color,quote,note,created_at,updated_at,location_kind)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,strftime('%s','now'),strftime('%s','now'),?10)",
                params![annotation.document_id,kind,annotation.start.page,annotation.start.char_index,annotation.end.page,annotation.end.char_index,annotation.color,annotation.quote,annotation.note,location_kind],
            ).is_ok() { annotation.id = self.connection.last_insert_rowid(); }
        } else {
            self.connection.execute(
                "UPDATE annotations SET kind=?2,start_page=?3,start_char=?4,end_page=?5,end_char=?6,color=?7,quote=?8,note=?9,updated_at=strftime('%s','now') WHERE id=?1",
                params![annotation.id,kind,annotation.start.page,annotation.start.char_index,annotation.end.page,annotation.end.char_index,annotation.color,annotation.quote,annotation.note],
            ).ok();
        }
    }

    pub fn delete_annotation(&self, id: i64) { self.connection.execute("DELETE FROM annotations WHERE id=?1", [id]).ok(); }

    fn location_kind(&self, document_id: &str) -> String {
        self.connection.query_row("SELECT format FROM library_documents WHERE document_id=?1", [document_id], |row| row.get::<_, String>(0))
            .optional().ok().flatten().filter(|format| format != "pdf").map(|_| "ebook".to_owned()).unwrap_or_else(|| "pdf".to_owned())
    }

    pub fn load_document_cache(&self, id: &str) -> Option<Document> {
        let bytes = std::fs::read(self.cache_path(id)).ok()?;
        let json = decode_zstd_limited(&bytes, MAX_DOCUMENT_CACHE_BYTES)?;
        let cached: CachedDocument = serde_json::from_slice(&json).ok()?;
        (cached.version == cache_version(cached.document.format) && valid_cached_document(&cached.document, id)).then_some(cached.document)
    }

    pub fn save_document_cache(&self, document: &Document) {
        let cached = CachedDocument { version: cache_version(document.format), document: document.clone() };
        let Ok(json) = serde_json::to_vec(&cached) else { return };
        let Ok(bytes) = zstd::stream::encode_all(json.as_slice(), 5) else { return };
        let path = self.cache_path(&document.id);
        let temporary = temporary_cache_path(&path);
        if std::fs::write(&temporary, bytes).is_ok() { std::fs::rename(temporary, path).ok(); }
    }

    pub fn clear_render_cache(&self) {
        for directory in ["page-cache", "cover-cache"] {
            if let Ok(entries) = std::fs::read_dir(self.data_dir.join(directory)) { for entry in entries.flatten() { std::fs::remove_file(entry.path()).ok(); } }
        }
    }
    pub fn load_rendered_page(&self, document_id: &str, page: u32, rotation: u16) -> Option<(usize, usize, Vec<u8>)> {
        let bytes = std::fs::read(self.page_cache_path(document_id, page, rotation)).ok()?;
        let decoded = decode_zstd_limited(&bytes, MAX_PAGE_CACHE_BYTES)?;
        let cached: CachedPage = serde_json::from_slice(&decoded).ok()?;
        valid_cached_page(&cached).then_some((cached.width, cached.height, cached.rgba))
    }
    pub fn save_rendered_page(&self, document_id: &str, page: u32, rotation: u16, width: usize, height: usize, rgba: &[u8]) {
        if rgba.is_empty() { return; }
        let cached = CachedPage { version: PDF_EXTRACTION_CACHE_VERSION, width, height, rgba: rgba.to_vec() };
        let Ok(json) = serde_json::to_vec(&cached) else { return };
        let Ok(bytes) = zstd::stream::encode_all(json.as_slice(), 2) else { return };
        let path = self.page_cache_path(document_id, page, rotation);
        let temporary = temporary_cache_path(&path);
        if std::fs::write(&temporary, bytes).is_ok() { std::fs::rename(temporary, path).ok(); }
    }
    pub fn load_cover(&self, document_id: &str) -> Option<(usize, usize, Vec<u8>)> {
        let bytes = std::fs::read(self.cover_cache_path(document_id)).ok()?;
        let decoded = decode_zstd_limited(&bytes, MAX_PAGE_CACHE_BYTES)?;
        let cached: CachedPage = serde_json::from_slice(&decoded).ok()?;
        valid_cached_page(&cached).then_some((cached.width, cached.height, cached.rgba))
    }
    pub fn save_cover(&self, document_id: &str, width: usize, height: usize, rgba: &[u8]) {
        if rgba.is_empty() { return; }
        let cached = CachedPage { version: PDF_EXTRACTION_CACHE_VERSION, width, height, rgba: rgba.to_vec() };
        let Ok(json) = serde_json::to_vec(&cached) else { return };
        let Ok(bytes) = zstd::stream::encode_all(json.as_slice(), 2) else { return };
        let path = self.cover_cache_path(document_id);
        let temporary = temporary_cache_path(&path);
        if std::fs::write(&temporary, bytes).is_ok() { std::fs::rename(temporary, path).ok(); }
    }
    fn cache_path(&self, id: &str) -> PathBuf { self.data_dir.join("cache").join(format!("{id}.document.zst")) }
    fn page_cache_path(&self, id: &str, page: u32, rotation: u16) -> PathBuf { self.data_dir.join("page-cache").join(format!("{id}-{page}-{rotation}-1500.zst")) }
    fn cover_cache_path(&self, id: &str) -> PathBuf { self.data_dir.join("cover-cache").join(format!("{id}-cover-240.zst")) }
}

fn valid_cached_page(cached: &CachedPage) -> bool {
    cached.version == PDF_EXTRACTION_CACHE_VERSION
        && cached.width > 0
        && cached.height > 0
        && cached.width.checked_mul(cached.height).and_then(|pixels| pixels.checked_mul(4)) == Some(cached.rgba.len())
}

fn valid_cached_document(document: &Document, expected_id: &str) -> bool {
    let mut block_ids = std::collections::HashSet::with_capacity(document.blocks.len());
    document.id == expected_id && document.blocks.iter().all(|block| {
        block_ids.insert(block.id)
            && block.confidence.is_finite()
            && (0.0..=1.0).contains(&block.confidence)
            && block.source.iter().all(|source| {
                source.page < document.page_count && source.start_char <= source.end_char
            })
    })
}

fn temporary_cache_path(path: &Path) -> PathBuf {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("cache");
    path.with_file_name(format!("{name}.{}.part", std::process::id()))
}

fn decode_zstd_limited(bytes: &[u8], limit: u64) -> Option<Vec<u8>> {
    let decoder = zstd::stream::read::Decoder::new(bytes).ok()?;
    let mut decoded = Vec::new();
    decoder.take(limit.saturating_add(1)).read_to_end(&mut decoded).ok()?;
    (decoded.len() as u64 <= limit).then_some(decoded)
}

fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    let current_version = connection.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
    if current_version > SCHEMA_VERSION {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let transaction = connection.unchecked_transaction()?;
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS settings(id INTEGER PRIMARY KEY CHECK(id=1),json TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS library_documents(document_id TEXT PRIMARY KEY,path TEXT UNIQUE NOT NULL,title TEXT NOT NULL,author TEXT NOT NULL DEFAULT '',page_count INTEGER NOT NULL DEFAULT 0,progress REAL NOT NULL DEFAULT 0,last_opened INTEGER NOT NULL DEFAULT 0,missing INTEGER NOT NULL DEFAULT 0,extraction_version INTEGER NOT NULL DEFAULT 0,format TEXT NOT NULL DEFAULT 'pdf');
         CREATE TABLE IF NOT EXISTS reading_progress(document_id TEXT PRIMARY KEY,path TEXT NOT NULL,block_id INTEGER NOT NULL DEFAULT 0,char_offset INTEGER NOT NULL DEFAULT 0,source_page INTEGER NOT NULL DEFAULT 0,source_char INTEGER NOT NULL DEFAULT 0,quote TEXT NOT NULL DEFAULT '',page INTEGER NOT NULL DEFAULT 0,mode TEXT NOT NULL DEFAULT 'reflow',updated_at INTEGER NOT NULL DEFAULT 0,location_kind TEXT NOT NULL DEFAULT 'pdf');
         CREATE TABLE IF NOT EXISTS open_tabs(position INTEGER PRIMARY KEY,path TEXT NOT NULL,active INTEGER NOT NULL DEFAULT 0);
         CREATE TABLE IF NOT EXISTS annotations(id INTEGER PRIMARY KEY AUTOINCREMENT,document_id TEXT NOT NULL,kind TEXT NOT NULL,start_page INTEGER NOT NULL,start_char INTEGER NOT NULL,end_page INTEGER NOT NULL,end_char INTEGER NOT NULL,color INTEGER NOT NULL,quote TEXT NOT NULL DEFAULT '',note TEXT NOT NULL DEFAULT '',created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL,location_kind TEXT NOT NULL DEFAULT 'pdf');
         CREATE INDEX IF NOT EXISTS annotations_document ON annotations(document_id,start_page,start_char);
         CREATE VIRTUAL TABLE IF NOT EXISTS search_blocks USING fts5(document_id UNINDEXED,path UNINDEXED,title,block_id UNINDEXED,page UNINDEXED,content,tokenize='unicode61');"
    )?;
    if current_version > 0 && current_version < 4 {
        transaction.execute_batch(
            "ALTER TABLE library_documents ADD COLUMN format TEXT NOT NULL DEFAULT 'pdf';
             ALTER TABLE reading_progress ADD COLUMN location_kind TEXT NOT NULL DEFAULT 'pdf';
             ALTER TABLE annotations ADD COLUMN location_kind TEXT NOT NULL DEFAULT 'pdf';"
        )?;
    }
    validate_schema(&transaction)?;
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    transaction.commit()
}

fn validate_schema(connection: &Connection) -> rusqlite::Result<()> {
    for query in [
        "SELECT id,json FROM settings LIMIT 0",
        "SELECT document_id,path,title,author,page_count,progress,missing,last_opened,extraction_version,format FROM library_documents LIMIT 0",
        "SELECT document_id,path,block_id,char_offset,source_page,source_char,quote,page,mode,updated_at,location_kind FROM reading_progress LIMIT 0",
        "SELECT position,path,active FROM open_tabs LIMIT 0",
        "SELECT id,document_id,kind,start_page,start_char,end_page,end_char,color,quote,note,created_at,updated_at,location_kind FROM annotations LIMIT 0",
        "SELECT document_id,path,title,block_id,page,content FROM search_blocks LIMIT 0",
    ] {
        connection.prepare(query)?;
    }
    Ok(())
}

fn fts_query(query: &str) -> String { query.split_whitespace().map(|term| format!("\"{}\"*", term.replace('"', ""))).collect::<Vec<_>>().join(" AND ") }
fn nonnegative_u32(value: i64) -> u32 { u32::try_from(value).unwrap_or(0) }
fn nonnegative_u64(value: i64) -> u64 { u64::try_from(value).unwrap_or(0) }
fn nonnegative_usize(value: i64) -> usize { usize::try_from(value).unwrap_or(0) }
fn finite_progress(value: f32) -> f32 { if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 } }
fn parse_format(value: &str) -> DocumentFormat { match value { "epub" => DocumentFormat::Epub, "mobi" => DocumentFormat::Mobi, "azw3" => DocumentFormat::Azw3, _ => DocumentFormat::Pdf } }
fn cache_version(format: DocumentFormat) -> u32 { if format.is_ebook() { EBOOK_EXTRACTION_CACHE_VERSION } else { PDF_EXTRACTION_CACHE_VERSION } }
fn annotation_kind(kind: AnnotationKind) -> &'static str { match kind { AnnotationKind::Highlight => "highlight", AnnotationKind::Note => "note", AnnotationKind::Bookmark => "bookmark" } }
fn annotation_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Annotation> {
    let kind: String = row.get(2)?;
    Ok(Annotation { id: row.get(0)?, document_id: row.get(1)?, kind: match kind.as_str() { "note" => AnnotationKind::Note, "bookmark" => AnnotationKind::Bookmark, _ => AnnotationKind::Highlight }, start: SourcePosition { page: nonnegative_u32(row.get(3)?), char_index: nonnegative_u32(row.get(4)?) }, end: SourcePosition { page: nonnegative_u32(row.get(5)?), char_index: nonnegative_u32(row.get(6)?) }, color: nonnegative_u32(row.get(7)?), quote: row.get(8)?, note: row.get(9)?, created_at: row.get(10)?, updated_at: row.get(11)? })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Block, BlockKind, DocumentMetadata, SourceRange};

    fn storage() -> Storage {
        let connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        assert_eq!(connection.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0)).unwrap(), SCHEMA_VERSION);
        Storage { connection, data_dir: std::env::temp_dir().join("koiflow-storage-tests") }
    }

    #[test]
    fn migration_is_idempotent_and_preserves_existing_data() {
        let connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        connection.execute("INSERT INTO settings(id,json) VALUES(1,'{\"theme\":\"Sepia\"}')", []).unwrap();
        connection.execute(
            "INSERT INTO open_tabs(position,path,active) VALUES(0,'/tmp/preserved.pdf',1)",
            [],
        ).unwrap();

        migrate(&connection).unwrap();

        let settings: String = connection.query_row("SELECT json FROM settings WHERE id=1", [], |row| row.get(0)).unwrap();
        let path: String = connection.query_row("SELECT path FROM open_tabs WHERE position=0", [], |row| row.get(0)).unwrap();
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap();
        assert_eq!(settings, "{\"theme\":\"Sepia\"}");
        assert_eq!(path, "/tmp/preserved.pdf");
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn version_three_pdf_rows_migrate_to_typed_locations() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(
            "CREATE TABLE library_documents(document_id TEXT PRIMARY KEY,path TEXT UNIQUE NOT NULL,title TEXT NOT NULL,author TEXT NOT NULL DEFAULT '',page_count INTEGER NOT NULL DEFAULT 0,progress REAL NOT NULL DEFAULT 0,last_opened INTEGER NOT NULL DEFAULT 0,missing INTEGER NOT NULL DEFAULT 0,extraction_version INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE reading_progress(document_id TEXT PRIMARY KEY,path TEXT NOT NULL,block_id INTEGER NOT NULL DEFAULT 0,char_offset INTEGER NOT NULL DEFAULT 0,source_page INTEGER NOT NULL DEFAULT 0,source_char INTEGER NOT NULL DEFAULT 0,quote TEXT NOT NULL DEFAULT '',page INTEGER NOT NULL DEFAULT 0,mode TEXT NOT NULL DEFAULT 'reflow',updated_at INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE annotations(id INTEGER PRIMARY KEY AUTOINCREMENT,document_id TEXT NOT NULL,kind TEXT NOT NULL,start_page INTEGER NOT NULL,start_char INTEGER NOT NULL,end_page INTEGER NOT NULL,end_char INTEGER NOT NULL,color INTEGER NOT NULL,quote TEXT NOT NULL DEFAULT '',note TEXT NOT NULL DEFAULT '',created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL);
             INSERT INTO library_documents(document_id,path,title) VALUES('pdf-id','/tmp/legacy.pdf','Legacy');
             INSERT INTO reading_progress(document_id,path,source_page,source_char) VALUES('pdf-id','/tmp/legacy.pdf',7,42);
             INSERT INTO annotations(document_id,kind,start_page,start_char,end_page,end_char,color,created_at,updated_at) VALUES('pdf-id','highlight',7,42,7,48,0,0,0);
             PRAGMA user_version=3;"
        ).unwrap();
        migrate(&connection).unwrap();
        let format: String = connection.query_row("SELECT format FROM library_documents WHERE document_id='pdf-id'", [], |row| row.get(0)).unwrap();
        let progress_kind: String = connection.query_row("SELECT location_kind FROM reading_progress WHERE document_id='pdf-id'", [], |row| row.get(0)).unwrap();
        let annotation_kind: String = connection.query_row("SELECT location_kind FROM annotations WHERE document_id='pdf-id'", [], |row| row.get(0)).unwrap();
        assert_eq!((format.as_str(), progress_kind.as_str(), annotation_kind.as_str()), ("pdf", "pdf", "pdf"));
    }

    #[test]
    fn migration_refuses_unknown_future_schema_without_downgrading_it() {
        let connection = Connection::open_in_memory().unwrap();
        let future_version = SCHEMA_VERSION + 1;
        connection.pragma_update(None, "user_version", future_version).unwrap();

        assert!(matches!(migrate(&connection), Err(rusqlite::Error::InvalidQuery)));
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap();
        assert_eq!(version, future_version);
        let table_count: i64 = connection.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='settings'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(table_count, 0);
    }

    #[test]
    fn migration_rejects_partial_schema_and_rolls_back_all_changes() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE settings(id INTEGER PRIMARY KEY);").unwrap();

        assert!(migrate(&connection).is_err());
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap();
        let library_count: i64 = connection.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='library_documents'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(version, 0);
        assert_eq!(library_count, 0);
    }

    #[test]
    fn indexes_and_finds_semantic_blocks() {
        let mut storage = storage();
        let mut document = Document { id: "doc-1".into(), title: "A Book".into(), page_count: 2, outline: vec![], metadata: DocumentMetadata::default(), format: DocumentFormat::Pdf, blocks: vec![Block { id: 7, kind: BlockKind::Paragraph, content: "A uniquely searchable passage about rivers".into(), source: vec![SourceRange { page: 1, start_char: 0, end_char: 44 }], confidence: 1.0 }] };
        storage.index_document(&document, Path::new("/tmp/book.pdf"));
        let hits = storage.search("searchable river", Some("doc-1"), 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].page, 1);
        assert_eq!(hits[0].block_id, 7);
        assert_eq!(storage.search("searchable river", None, 10).len(), 1);

        document.blocks[0].content = "Replacement content about mountains".into();
        storage.index_document(&document, Path::new("/tmp/book.pdf"));
        assert!(storage.search("rivers", Some("doc-1"), 10).is_empty());
        assert_eq!(storage.search("mountains", Some("doc-1"), 10).len(), 1);
    }

    #[test]
    fn annotations_round_trip_without_touching_pdf() {
        let storage = storage();
        let mut annotation = Annotation { id: 0, document_id: "doc-1".into(), kind: AnnotationKind::Note, start: SourcePosition { page: 3, char_index: 10 }, end: SourcePosition { page: 3, char_index: 20 }, color: 0xFF00FF00, quote: "quoted text".into(), note: "remember this".into(), created_at: 0, updated_at: 0 };
        storage.save_annotation(&mut annotation);
        assert!(annotation.id > 0);
        let loaded = storage.annotations("doc-1");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].note, "remember this");
        annotation.note = "updated note".into();
        storage.save_annotation(&mut annotation);
        assert_eq!(storage.annotations("doc-1")[0].note, "updated note");
        storage.delete_annotation(annotation.id);
        assert!(storage.annotations("doc-1").is_empty());
    }

    #[test]
    fn rendered_page_cache_is_versioned_and_validated() {
        let mut storage = storage();
        storage.data_dir = std::env::temp_dir().join(format!("koiflow-page-cache-{}", std::process::id()));
        std::fs::create_dir_all(storage.data_dir.join("page-cache")).unwrap();
        let pixels = vec![12_u8; 4 * 3 * 4];
        storage.save_rendered_page("doc", 2, 90, 4, 3, &pixels);
        let (width, height, loaded) = storage.load_rendered_page("doc", 2, 90).unwrap();
        assert_eq!((width, height), (4, 3));
        assert_eq!(loaded, pixels);
    }

    #[test]
    fn cover_cache_is_separate_and_versioned() {
        let mut storage = storage();
        storage.data_dir = std::env::temp_dir().join(format!("koiflow-cover-cache-{}", std::process::id()));
        std::fs::create_dir_all(storage.data_dir.join("cover-cache")).unwrap();
        let pixels = vec![42_u8; 6 * 9 * 4];
        storage.save_cover("cover-doc", 6, 9, &pixels);
        let (width, height, loaded) = storage.load_cover("cover-doc").unwrap();
        assert_eq!((width, height), (6, 9));
        assert_eq!(loaded, pixels);
        assert!(storage.cover_cache_path("cover-doc").ends_with("cover-doc-cover-240.zst"));
    }

    #[test]
    fn legacy_dark_reader_setting_migrates_application_theme() {
        let storage = storage();
        storage.connection.execute("INSERT INTO settings(id,json) VALUES(1,?1)", [r#"{"theme":"Dark"}"#]).unwrap();
        let style = storage.load_style().unwrap();
        assert_eq!(style.app_theme, AppTheme::Dark);
        assert_eq!(style.theme, ReaderTheme::Dark);
    }

    #[test]
    fn explicit_application_theme_is_not_overridden_by_legacy_migration() {
        let storage = storage();
        storage.connection.execute("INSERT INTO settings(id,json) VALUES(1,?1)", [r#"{"app_theme":"Light","theme":"Dark"}"#]).unwrap();
        let style = storage.load_style().unwrap();
        assert_eq!(style.app_theme, AppTheme::Light);
        assert_eq!(style.theme, ReaderTheme::Dark);
    }

    #[test]
    fn progress_and_tab_order_round_trip() {
        let mut storage = storage();
        let anchor = ReadingAnchor { block_id: 77, char_offset: 5, source: Some(SourcePosition { page: 4, char_index: 31 }), quote: "anchor quote".into() };
        storage.save_progress("doc", Path::new("/tmp/doc.pdf"), &anchor, 4, ReadingMode::Reflow, 10);
        let (loaded, page, mode) = storage.load_progress("doc").unwrap();
        assert_eq!((loaded.block_id, loaded.char_offset, loaded.source), (77, 5, anchor.source));
        assert_eq!((page, mode), (4, ReadingMode::Reflow));

        let out_of_range = ReadingAnchor { source: Some(SourcePosition { page: 500, char_index: 31 }), ..anchor.clone() };
        storage.save_progress("doc", Path::new("/tmp/doc.pdf"), &out_of_range, 400, ReadingMode::Original, 10);
        let (loaded, page, mode) = storage.load_progress("doc").unwrap();
        assert_eq!(loaded.source.unwrap().page, 9);
        assert_eq!((page, mode), (9, ReadingMode::Original));

        storage.connection.execute(
            "UPDATE reading_progress SET block_id=-1,char_offset=-1,source_page=-1,source_char=-1,page=-1 WHERE document_id='doc'",
            [],
        ).unwrap();
        let (loaded, page, _) = storage.load_progress("doc").unwrap();
        assert_eq!((loaded.block_id, loaded.char_offset, loaded.source.unwrap().page, page), (0, 0, 0, 0));

        storage.save_tabs(&[
            SavedTab { path: PathBuf::from("/tmp/first.pdf"), active: false, position: 0 },
            SavedTab { path: PathBuf::from("/tmp/second.pdf"), active: true, position: 1 },
        ]);
        let tabs = storage.load_tabs();
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[0].path, PathBuf::from("/tmp/first.pdf"));
        assert!(tabs[1].active);
    }

    #[test]
    fn corrupt_and_mismatched_caches_are_rejected_without_panicking() {
        let mut storage = storage();
        storage.data_dir = std::env::temp_dir().join(format!("koiflow-corrupt-cache-{}", std::process::id()));
        std::fs::create_dir_all(storage.data_dir.join("cache")).unwrap();
        std::fs::create_dir_all(storage.data_dir.join("page-cache")).unwrap();

        let foreign = CachedDocument { version: PDF_EXTRACTION_CACHE_VERSION, document: Document { id: "foreign".into(), ..Document::default() } };
        let json = serde_json::to_vec(&foreign).unwrap();
        let compressed = zstd::stream::encode_all(json.as_slice(), 1).unwrap();
        std::fs::write(storage.cache_path("expected"), compressed).unwrap();
        assert!(storage.load_document_cache("expected").is_none());

        std::fs::write(storage.cache_path("truncated"), b"not a zstd stream").unwrap();
        assert!(storage.load_document_cache("truncated").is_none());

        let overflow = CachedPage { version: PDF_EXTRACTION_CACHE_VERSION, width: usize::MAX, height: 2, rgba: vec![] };
        let json = serde_json::to_vec(&overflow).unwrap();
        let compressed = zstd::stream::encode_all(json.as_slice(), 1).unwrap();
        std::fs::write(storage.page_cache_path("overflow", 0, 0), compressed).unwrap();
        assert!(storage.load_rendered_page("overflow", 0, 0).is_none());
    }

    #[test]
    fn semantic_cache_structure_is_validated_before_use() {
        let block = |id, page, start, end, confidence| Block {
            id,
            kind: BlockKind::Paragraph,
            content: "cached text".into(),
            source: vec![SourceRange { page, start_char: start, end_char: end }],
            confidence,
        };
        let valid = Document {
            id: "expected".into(),
            title: "Cached document".into(),
            page_count: 2,
            blocks: vec![block(1, 0, 3, 9, 0.9), block(2, 1, 0, 4, 0.8)],
            ..Document::default()
        };
        assert!(valid_cached_document(&valid, "expected"));

        let mut invalid = valid.clone();
        invalid.blocks[0].source[0].page = 2;
        assert!(!valid_cached_document(&invalid, "expected"));

        let mut invalid = valid.clone();
        invalid.blocks[0].source[0] = SourceRange { page: 0, start_char: 9, end_char: 3 };
        assert!(!valid_cached_document(&invalid, "expected"));

        let mut invalid = valid.clone();
        invalid.blocks[1].id = invalid.blocks[0].id;
        assert!(!valid_cached_document(&invalid, "expected"));

        let mut invalid = valid.clone();
        invalid.blocks[0].confidence = f32::NAN;
        assert!(!valid_cached_document(&invalid, "expected"));
        assert!(!valid_cached_document(&valid, "other-document"));
    }

    #[test]
    fn atomic_cache_temporary_files_are_process_scoped() {
        let final_path = Path::new("/tmp/document.zst");
        let temporary = temporary_cache_path(final_path);
        assert_ne!(temporary, final_path);
        assert_eq!(temporary.parent(), final_path.parent());
        assert!(temporary.file_name().unwrap().to_string_lossy().starts_with("document.zst."));
        assert!(temporary.file_name().unwrap().to_string_lossy().ends_with(".part"));
    }

    #[test]
    fn cache_decompression_is_bounded() {
        let expanded = vec![0_u8; 4096];
        let compressed = zstd::stream::encode_all(expanded.as_slice(), 1).unwrap();
        assert_eq!(decode_zstd_limited(&compressed, 4096).unwrap(), expanded);
        assert!(decode_zstd_limited(&compressed, 1024).is_none());
    }

    #[test]
    fn reopening_changed_content_at_the_same_path_replaces_library_identity() {
        let mut storage = storage();
        let path = PathBuf::from("/tmp/replaced-book.pdf");
        let opened = |document_id: &str, title: &str| OpenedDocument {
            path: path.clone(), document_id: document_id.into(), title: title.into(), page_count: 12,
            metadata: DocumentMetadata::default(), outline: vec![], format: DocumentFormat::Pdf,
            capabilities: crate::document::DocumentCapabilities::for_format(DocumentFormat::Pdf),
        };
        let old = opened("old-hash", "Old edition");
        storage.remember_document(&old);
        storage.index_document(&Document {
            id: "old-hash".into(), title: "Old edition".into(), page_count: 12,
            metadata: DocumentMetadata::default(), outline: vec![], format: DocumentFormat::Pdf,
            blocks: vec![Block { id: 1, kind: BlockKind::Paragraph, content: "obsolete searchable phrase".into(), source: vec![SourceRange { page: 0, start_char: 0, end_char: 26 }], confidence: 1.0 }],
        }, &path);
        assert_eq!(storage.search("obsolete", None, 10).len(), 1);
        storage.remember_document(&opened("new-hash", "New edition"));
        let entries = storage.library();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].document_id, "new-hash");
        assert_eq!(entries[0].title, "New edition");
        assert!(storage.search("obsolete", None, 10).is_empty());
    }
}
