use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{
    document::{Annotation, AnnotationKind, Document, ReadingAnchor, SearchHit, SourcePosition},
    pdf::OpenedPdf,
    reading::{ReaderStyle, ReadingMode},
};

const SCHEMA_VERSION: i64 = 3;
// Version 3 rebuilds semantic text produced before native character ordering
// was restored after geometry-based line detection.
const EXTRACTION_CACHE_VERSION: u32 = 3;

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
        let connection = Connection::open(data_dir.join("reader.sqlite"))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&connection)?;
        Ok(Self { connection, data_dir })
    }

    pub fn load_style(&self) -> Option<ReaderStyle> {
        let json: String = self.connection.query_row("SELECT json FROM settings WHERE id=1", [], |row| row.get(0)).ok()?;
        serde_json::from_str(&json).ok()
    }

    pub fn save_style(&self, style: &ReaderStyle) {
        if let Ok(json) = serde_json::to_string(style) {
            self.connection.execute("INSERT INTO settings(id,json) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET json=?1", [json]).ok();
        }
    }

    pub fn remember_document(&self, opened: &OpenedPdf) {
        self.connection.execute(
            "DELETE FROM library_documents WHERE path=?1 AND document_id<>?2",
            params![opened.path.to_string_lossy(), opened.document_id],
        ).ok();
        self.connection.execute(
            "INSERT INTO library_documents(document_id,path,title,author,page_count,last_opened,missing)
             VALUES(?1,?2,?3,?4,?5,strftime('%s','now'),0)
             ON CONFLICT(document_id) DO UPDATE SET path=?2,title=?3,author=?4,page_count=?5,last_opened=strftime('%s','now'),missing=0",
            params![opened.document_id, opened.path.to_string_lossy(), opened.title, opened.metadata.author, opened.page_count],
        ).ok();
    }

    pub fn library(&self) -> Vec<LibraryEntry> {
        let Ok(mut query) = self.connection.prepare(
            "SELECT document_id,path,title,author,page_count,progress,missing FROM library_documents ORDER BY last_opened DESC"
        ) else { return vec![] };
        query.query_map([], |row| Ok(LibraryEntry {
            document_id: row.get(0)?, path: PathBuf::from(row.get::<_, String>(1)?), title: row.get(2)?, author: row.get(3)?,
            page_count: row.get::<_, i64>(4)? as u32, progress: row.get(5)?, missing: row.get::<_, i64>(6)? != 0,
        })).map(|rows| rows.filter_map(Result::ok).map(|mut entry| { entry.missing |= !entry.path.exists(); entry }).collect()).unwrap_or_default()
    }

    pub fn save_progress(&self, document_id: &str, path: &Path, anchor: &ReadingAnchor, page: u32, mode: ReadingMode, page_count: u32) {
        let mode = if mode == ReadingMode::Reflow { "reflow" } else { "original" };
        let source_page = anchor.source.map(|source| source.page).unwrap_or(page);
        let source_char = anchor.source.map(|source| source.char_index).unwrap_or(0);
        let progress = if page_count > 1 { page as f32 / (page_count - 1) as f32 } else { 0.0 };
        self.connection.execute(
            "INSERT INTO reading_progress(document_id,path,block_id,char_offset,source_page,source_char,quote,page,mode,updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,strftime('%s','now'))
             ON CONFLICT(document_id) DO UPDATE SET path=?2,block_id=?3,char_offset=?4,source_page=?5,source_char=?6,quote=?7,page=?8,mode=?9,updated_at=strftime('%s','now')",
            params![document_id, path.to_string_lossy(), anchor.block_id as i64, anchor.char_offset as i64, source_page, source_char, anchor.quote, page, mode],
        ).ok();
        self.connection.execute("UPDATE library_documents SET progress=?2 WHERE document_id=?1", params![document_id, progress]).ok();
    }

    pub fn load_progress(&self, document_id: &str) -> Option<(ReadingAnchor, u32, ReadingMode)> {
        self.connection.query_row(
            "SELECT block_id,char_offset,source_page,source_char,quote,page,mode FROM reading_progress WHERE document_id=?1", [document_id],
            |row| {
                let mode: String = row.get(6)?;
                Ok((ReadingAnchor {
                    block_id: row.get::<_, i64>(0)? as u64,
                    char_offset: row.get::<_, i64>(1)? as usize,
                    source: Some(SourcePosition { page: row.get::<_, i64>(2)? as u32, char_index: row.get::<_, i64>(3)? as u32 }),
                    quote: row.get(4)?,
                }, row.get::<_, i64>(5)? as u32, if mode == "original" { ReadingMode::Original } else { ReadingMode::Reflow }))
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
        query.query_map([], |row| Ok(SavedTab { position: row.get::<_, i64>(0)? as u32, path: PathBuf::from(row.get::<_, String>(1)?), active: row.get(2)? }))
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
        transaction.execute("UPDATE library_documents SET extraction_version=?2 WHERE document_id=?1", params![document.id, EXTRACTION_CACHE_VERSION]).ok();
        transaction.commit().ok();
    }

    pub fn search(&self, query: &str, document_id: Option<&str>, limit: usize) -> Vec<SearchHit> {
        if query.trim().is_empty() { return vec![]; }
        let sql = if document_id.is_some() {
            "SELECT document_id,path,title,block_id,page,snippet(search_blocks,5,'[',']',' … ',18) FROM search_blocks WHERE search_blocks MATCH ?1 AND document_id=?2 LIMIT ?3"
        } else {
            "SELECT document_id,path,title,block_id,page,snippet(search_blocks,5,'[',']',' … ',18) FROM search_blocks WHERE search_blocks MATCH ?1 LIMIT ?3"
        };
        let Ok(mut statement) = self.connection.prepare(sql) else { return vec![] };
        let map = |row: &rusqlite::Row<'_>| Ok(SearchHit {
            document_id: row.get(0)?, path: row.get(1)?, title: row.get(2)?, block_id: row.get::<_, i64>(3)? as u64,
            page: row.get::<_, i64>(4)? as u32, snippet: row.get(5)?,
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
        if annotation.id == 0 {
            if self.connection.execute(
                "INSERT INTO annotations(document_id,kind,start_page,start_char,end_page,end_char,color,quote,note,created_at,updated_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,strftime('%s','now'),strftime('%s','now'))",
                params![annotation.document_id,kind,annotation.start.page,annotation.start.char_index,annotation.end.page,annotation.end.char_index,annotation.color,annotation.quote,annotation.note],
            ).is_ok() { annotation.id = self.connection.last_insert_rowid(); }
        } else {
            self.connection.execute(
                "UPDATE annotations SET kind=?2,start_page=?3,start_char=?4,end_page=?5,end_char=?6,color=?7,quote=?8,note=?9,updated_at=strftime('%s','now') WHERE id=?1",
                params![annotation.id,kind,annotation.start.page,annotation.start.char_index,annotation.end.page,annotation.end.char_index,annotation.color,annotation.quote,annotation.note],
            ).ok();
        }
    }

    pub fn delete_annotation(&self, id: i64) { self.connection.execute("DELETE FROM annotations WHERE id=?1", [id]).ok(); }

    pub fn load_document_cache(&self, id: &str) -> Option<Document> {
        let bytes = std::fs::read(self.cache_path(id)).ok()?;
        let json = zstd::stream::decode_all(bytes.as_slice()).ok()?;
        let cached: CachedDocument = serde_json::from_slice(&json).ok()?;
        (cached.version == EXTRACTION_CACHE_VERSION).then_some(cached.document)
    }

    pub fn save_document_cache(&self, document: &Document) {
        let cached = CachedDocument { version: EXTRACTION_CACHE_VERSION, document: document.clone() };
        let Ok(json) = serde_json::to_vec(&cached) else { return };
        let Ok(bytes) = zstd::stream::encode_all(json.as_slice(), 5) else { return };
        let temporary = self.cache_path(&document.id).with_extension("zst.part");
        if std::fs::write(&temporary, bytes).is_ok() { std::fs::rename(temporary, self.cache_path(&document.id)).ok(); }
    }

    pub fn clear_render_cache(&self) { if let Ok(entries) = std::fs::read_dir(self.data_dir.join("page-cache")) { for entry in entries.flatten() { std::fs::remove_file(entry.path()).ok(); } } }
    pub fn load_rendered_page(&self, document_id: &str, page: u32, rotation: u16) -> Option<(usize, usize, Vec<u8>)> {
        let bytes = std::fs::read(self.page_cache_path(document_id, page, rotation)).ok()?;
        let decoded = zstd::stream::decode_all(bytes.as_slice()).ok()?;
        let cached: CachedPage = serde_json::from_slice(&decoded).ok()?;
        (cached.version == EXTRACTION_CACHE_VERSION && cached.rgba.len() == cached.width * cached.height * 4).then_some((cached.width, cached.height, cached.rgba))
    }
    pub fn save_rendered_page(&self, document_id: &str, page: u32, rotation: u16, width: usize, height: usize, rgba: &[u8]) {
        if rgba.is_empty() { return; }
        let cached = CachedPage { version: EXTRACTION_CACHE_VERSION, width, height, rgba: rgba.to_vec() };
        let Ok(json) = serde_json::to_vec(&cached) else { return };
        let Ok(bytes) = zstd::stream::encode_all(json.as_slice(), 2) else { return };
        let path = self.page_cache_path(document_id, page, rotation);
        let temporary = path.with_extension("zst.part");
        if std::fs::write(&temporary, bytes).is_ok() { std::fs::rename(temporary, path).ok(); }
    }
    fn cache_path(&self, id: &str) -> PathBuf { self.data_dir.join("cache").join(format!("{id}.document.zst")) }
    fn page_cache_path(&self, id: &str, page: u32, rotation: u16) -> PathBuf { self.data_dir.join("page-cache").join(format!("{id}-{page}-{rotation}-1500.zst")) }
}

fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS settings(id INTEGER PRIMARY KEY CHECK(id=1),json TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS library_documents(document_id TEXT PRIMARY KEY,path TEXT UNIQUE NOT NULL,title TEXT NOT NULL,author TEXT NOT NULL DEFAULT '',page_count INTEGER NOT NULL DEFAULT 0,progress REAL NOT NULL DEFAULT 0,last_opened INTEGER NOT NULL DEFAULT 0,missing INTEGER NOT NULL DEFAULT 0,extraction_version INTEGER NOT NULL DEFAULT 0);
         CREATE TABLE IF NOT EXISTS reading_progress(document_id TEXT PRIMARY KEY,path TEXT NOT NULL,block_id INTEGER NOT NULL DEFAULT 0,char_offset INTEGER NOT NULL DEFAULT 0,source_page INTEGER NOT NULL DEFAULT 0,source_char INTEGER NOT NULL DEFAULT 0,quote TEXT NOT NULL DEFAULT '',page INTEGER NOT NULL DEFAULT 0,mode TEXT NOT NULL DEFAULT 'reflow',updated_at INTEGER NOT NULL DEFAULT 0);
         CREATE TABLE IF NOT EXISTS open_tabs(position INTEGER PRIMARY KEY,path TEXT NOT NULL,active INTEGER NOT NULL DEFAULT 0);
         CREATE TABLE IF NOT EXISTS annotations(id INTEGER PRIMARY KEY AUTOINCREMENT,document_id TEXT NOT NULL,kind TEXT NOT NULL,start_page INTEGER NOT NULL,start_char INTEGER NOT NULL,end_page INTEGER NOT NULL,end_char INTEGER NOT NULL,color INTEGER NOT NULL,quote TEXT NOT NULL DEFAULT '',note TEXT NOT NULL DEFAULT '',created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS annotations_document ON annotations(document_id,start_page,start_char);
         CREATE VIRTUAL TABLE IF NOT EXISTS search_blocks USING fts5(document_id UNINDEXED,path UNINDEXED,title,block_id UNINDEXED,page UNINDEXED,content,tokenize='unicode61');"
    )?;
    connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

fn fts_query(query: &str) -> String { query.split_whitespace().map(|term| format!("\"{}\"*", term.replace('"', ""))).collect::<Vec<_>>().join(" AND ") }
fn annotation_kind(kind: AnnotationKind) -> &'static str { match kind { AnnotationKind::Highlight => "highlight", AnnotationKind::Note => "note", AnnotationKind::Bookmark => "bookmark" } }
fn annotation_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Annotation> {
    let kind: String = row.get(2)?;
    Ok(Annotation { id: row.get(0)?, document_id: row.get(1)?, kind: match kind.as_str() { "note" => AnnotationKind::Note, "bookmark" => AnnotationKind::Bookmark, _ => AnnotationKind::Highlight }, start: SourcePosition { page: row.get::<_, i64>(3)? as u32, char_index: row.get::<_, i64>(4)? as u32 }, end: SourcePosition { page: row.get::<_, i64>(5)? as u32, char_index: row.get::<_, i64>(6)? as u32 }, color: row.get::<_, i64>(7)? as u32, quote: row.get(8)?, note: row.get(9)?, created_at: row.get(10)?, updated_at: row.get(11)? })
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
    fn indexes_and_finds_semantic_blocks() {
        let mut storage = storage();
        let document = Document { id: "doc-1".into(), title: "A Book".into(), page_count: 2, outline: vec![], metadata: DocumentMetadata::default(), blocks: vec![Block { id: 7, kind: BlockKind::Paragraph, content: "A uniquely searchable passage about rivers".into(), source: vec![SourceRange { page: 1, start_char: 0, end_char: 44 }], confidence: 1.0 }] };
        storage.index_document(&document, Path::new("/tmp/book.pdf"));
        let hits = storage.search("searchable river", Some("doc-1"), 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].page, 1);
        assert_eq!(hits[0].block_id, 7);
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
}
