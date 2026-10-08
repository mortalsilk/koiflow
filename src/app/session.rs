use std::{collections::{HashMap, HashSet}, path::PathBuf, sync::Arc, time::Instant};

use eframe::egui::{Galley, TextureHandle};

use crate::{
    document::{Annotation, Document, OpenedDocument, RawPage, ReadingAnchor, SearchHit},
    reading::ReadingMode,
};

pub type TabId = u64;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ZoomMode { FitWidth, FitPage, Custom(f32) }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PageRequest {
    pub page: u32,
    pub rotation: u16,
    pub render: bool,
}

pub struct CachedTexture {
    pub texture: TextureHandle,
    pub bytes: usize,
    pub last_used: u64,
}

pub struct ReflowLayout {
    pub galley: Arc<Galley>,
    pub content_height: f32,
    pub before_spacing: f32,
    pub after_spacing: f32,
}

impl ReflowLayout {
    pub fn total_height(&self) -> f32 { self.before_spacing + self.content_height + self.after_spacing }
}

pub struct DocumentSession {
    pub tab_id: TabId,
    pub generation: u64,
    pub path: PathBuf,
    pub password: Option<String>,
    pub opened: Option<OpenedDocument>,
    pub document: Document,
    pub mode: ReadingMode,
    pub zoom: ZoomMode,
    pub rotation: u16,
    pub current_page: u32,
    pub scroll_to_page: Option<u32>,
    pub anchor: ReadingAnchor,
    pub selected_block: Option<u64>,
    pub scroll_to_block: Option<u64>,
    pub extracted_pages: HashSet<u32>,
    pub requested_pages: HashSet<PageRequest>,
    pub failed_pages: HashSet<PageRequest>,
    pub textures: HashMap<u32, CachedTexture>,
    pub page_aspects: HashMap<u32, f32>,
    pub raw_pages: HashMap<u32, RawPage>,
    pub reflow_layouts: HashMap<u64, ReflowLayout>,
    pub reflow_layout_signature: u64,
    pub reflow_scroll_restore: Option<(u64, f32)>,
    pub annotations: Vec<Annotation>,
    pub search_query: String,
    pub search_hits: Vec<SearchHit>,
    pub selected_hit: usize,
    pub status: String,
    pub error: Option<String>,
    pub password_required: bool,
    pub password_input: String,
    pub last_progress_save: Instant,
}

impl DocumentSession {
    pub fn loading(tab_id: TabId, generation: u64, path: PathBuf) -> Self {
        Self {
            tab_id, generation, path, password: None, opened: None, document: Document::default(),
            mode: ReadingMode::Reflow, zoom: ZoomMode::FitWidth, rotation: 0, current_page: 0, scroll_to_page: None,
            anchor: ReadingAnchor::default(), selected_block: None, scroll_to_block: None, extracted_pages: HashSet::new(), requested_pages: HashSet::new(), failed_pages: HashSet::new(),
            textures: HashMap::new(), page_aspects: HashMap::new(), raw_pages: HashMap::new(), reflow_layouts: HashMap::new(), reflow_layout_signature: 0, reflow_scroll_restore: None, annotations: vec![], search_query: String::new(),
            search_hits: vec![], selected_hit: 0, status: "Opening document…".into(), error: None,
            password_required: false, password_input: String::new(), last_progress_save: Instant::now(),
        }
    }

    pub fn title(&self) -> String {
        self.opened.as_ref().map(|opened| opened.title.clone()).or_else(|| self.path.file_stem().and_then(|name| name.to_str()).map(str::to_owned)).unwrap_or_else(|| "Untitled".into())
    }

    pub fn page_count(&self) -> u32 { self.opened.as_ref().map(|opened| opened.page_count).unwrap_or(0) }
    pub fn document_id(&self) -> Option<&str> { self.opened.as_ref().map(|opened| opened.document_id.as_str()) }
}
