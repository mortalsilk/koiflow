mod design;
mod session;

use std::{collections::{HashMap, hash_map::DefaultHasher}, hash::{Hash, Hasher}, path::PathBuf, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};

use eframe::egui::{self, Align, Color32, FontId, Layout, RichText, Sense, Stroke, Vec2, text::{LayoutJob, TextFormat}};

use crate::{
    document::{Annotation, AnnotationKind, Block, BlockKind, Document, DocumentFormat, LinkTarget, ReadingAnchor, SearchHit, SourcePosition},
    ebook::{EbookCommand, EbookResult, EbookWorker},
    extraction::blocks_from_raw_page,
    pdf::{FailedRequest, Generation, PdfCommand, PdfResult, PdfWorker, RequestPriority},
    reading::{AppTheme, ReaderStyle, ReaderTheme, ReadingMode, TextAlignment},
    storage::{LibraryEntry, SavedTab, Storage},
};
use session::{DocumentSession, PageRequest, ReflowLayout, TabId, ZoomMode};
use design::{Icon, LayoutClass, Palette, ReaderPalette};

const TEXTURE_BUDGET: usize = 256 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SidebarView { Library, Outline, Search, Annotations }

#[derive(Clone, Copy, PartialEq, Eq)]
enum AppScreen { Home, Reader }

#[derive(Clone, Copy, PartialEq, Eq)]
enum LibrarySort { Recent, Title }

struct LibraryCover { texture: egui::TextureHandle, aspect: f32, loaded_at: Instant }

#[derive(Debug, Clone, PartialEq, Eq)]
struct DisplayMetadata {
    title: String,
    author: Option<String>,
    original_title: String,
}

impl DisplayMetadata {
    fn new(title: &str, author: &str) -> Self {
        let original_title = title.to_owned();
        let mut title = collapse_whitespace(title);
        title = strip_supported_extension(&title).to_owned();
        title = strip_isbn_prefix(&title).to_owned();
        title = strip_archive_suffix(&title).to_owned();
        let mut display_author = (!author.trim().is_empty()).then(|| collapse_whitespace(author));
        if display_author.is_none() {
            if let Some((derived_author, derived_title)) = split_filename_metadata(&title) {
                display_author = Some(derived_author);
                title = derived_title;
            }
        }
        if title.is_empty() { title = "Untitled".into(); }
        Self { title, author: display_author, original_title }
    }
}

fn collapse_whitespace(value: &str) -> String { value.split_whitespace().collect::<Vec<_>>().join(" ") }

fn strip_supported_extension(value: &str) -> &str {
    for extension in [".pdf", ".epub", ".mobi", ".azw3"] {
        if value.to_ascii_lowercase().ends_with(extension) { return &value[..value.len() - extension.len()]; }
    }
    value
}

fn strip_isbn_prefix(value: &str) -> &str {
    let Some(close) = value.strip_prefix('[').and_then(|rest| rest.find(']').map(|index| index + 1)) else { return value };
    let token = &value[1..close];
    let compact: String = token.chars().filter(|character| !matches!(character, '-' | ' ')).collect();
    if (10..=17).contains(&compact.len()) && compact.chars().all(|character| character.is_ascii_digit() || matches!(character, 'x' | 'X')) {
        value[close + 1..].trim_start()
    } else { value }
}

fn strip_archive_suffix(value: &str) -> &str {
    let Some((base, suffix)) = value.rsplit_once(" - ") else { return value };
    let looks_like_host = suffix.contains('.') && !suffix.contains(char::is_whitespace)
        && suffix.chars().all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_'));
    if looks_like_host { base.trim_end() } else { value }
}

fn split_filename_metadata(value: &str) -> Option<(String, String)> {
    let (author, remainder) = value.split_once(" - ")?;
    let author = collapse_whitespace(author);
    if author.len() < 2 || author.len() > 80 { return None; }
    let parenthesis = remainder.rfind(" (")?;
    let details = remainder.get(parenthesis + 2..)?.strip_suffix(')')?;
    let has_year = details.as_bytes().windows(4).any(|window| window.iter().all(u8::is_ascii_digit));
    if !has_year || !details.to_lowercase().contains(&author.to_lowercase()) { return None; }
    let title = collapse_whitespace(&remainder[..parenthesis]);
    (!title.is_empty()).then_some((author, title))
}

pub struct KoiFlowApp {
    worker: PdfWorker,
    ebook_worker: EbookWorker,
    storage: Option<Storage>,
    tabs: Vec<DocumentSession>,
    active: Option<usize>,
    screen: AppScreen,
    next_tab: TabId,
    next_generation: Generation,
    next_request: u64,
    style: ReaderStyle,
    library: Vec<LibraryEntry>,
    sidebar: SidebarView,
    show_sidebar: bool,
    show_settings: bool,
    distraction_free: bool,
    library_query: String,
    home_query: String,
    library_sort: LibrarySort,
    library_hits: Vec<SearchHit>,
    library_covers: HashMap<String, LibraryCover>,
    cover_requests: HashMap<String, Generation>,
    note_draft: String,
    frame_tick: u64,
    theme_transition: Option<(AppTheme, Instant)>,
}

impl KoiFlowApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let storage = Storage::open().ok();
        let style = storage.as_ref().and_then(Storage::load_style).unwrap_or_default();
        let library = storage.as_ref().map(Storage::library).unwrap_or_default();
        let saved_tabs = storage.as_ref().map(Storage::load_tabs).unwrap_or_default();
        configure_fonts(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);
        design::configure(&cc.egui_ctx, style.app_theme, style.reduced_motion);
        let mut app = Self {
            worker: PdfWorker::spawn(), ebook_worker: EbookWorker::spawn(), storage, tabs: vec![], active: None, screen: AppScreen::Home, next_tab: 1,
            next_generation: 1, next_request: 1, style, library, sidebar: SidebarView::Library,
            show_sidebar: true, show_settings: false, distraction_free: false, library_query: String::new(), home_query: String::new(), library_sort: LibrarySort::Recent,
            library_hits: vec![], library_covers: HashMap::new(), cover_requests: HashMap::new(),
            note_draft: String::new(), frame_tick: 0, theme_transition: None,
        };
        let active_path = saved_tabs.iter().find(|tab| tab.active).map(|tab| tab.path.clone());
        for saved in saved_tabs { if saved.path.exists() { app.open_path(saved.path); } }
        if let Some(path) = active_path { if let Some(index) = app.tabs.iter().position(|tab| tab.path == path) { app.active = Some(index); app.screen = AppScreen::Reader; } }
        let cli_paths: Vec<_> = std::env::args_os().skip(1).map(PathBuf::from).filter(|path| DocumentFormat::from_path(path).is_some()).collect();
        for path in cli_paths { app.open_path(path); }
        app
    }

    fn open_picker(&mut self) {
        if let Some(path) = rfd::FileDialog::new().add_filter("Documents", &["pdf", "epub", "mobi", "azw3"]).pick_file() { self.open_path(path); }
    }

    fn open_path(&mut self, path: PathBuf) {
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) { self.active = Some(index); self.screen = AppScreen::Reader; if self.sidebar == SidebarView::Library { self.sidebar = SidebarView::Outline; } return; }
        let tab_id = self.next_tab; self.next_tab += 1;
        let generation = self.next_generation; self.next_generation += 1;
        self.tabs.push(DocumentSession::loading(tab_id, generation, path.clone()));
        self.active = Some(self.tabs.len() - 1);
        self.screen = AppScreen::Reader;
        if self.sidebar == SidebarView::Library { self.sidebar = SidebarView::Outline; }
        let request_id = self.take_request();
        match DocumentFormat::from_path(&path) {
            Some(DocumentFormat::Pdf) => { let _ = self.worker.commands.send(PdfCommand::Open { request_id, generation, path, password: None }); }
            Some(_) => { let _ = self.ebook_worker.commands.send(EbookCommand::Open { request_id, generation, path }); }
            None => if let Some(session) = self.tabs.last_mut() { session.status = "Unsupported document".into(); session.error = Some("koiflow opens PDF, EPUB, MOBI, and AZW3 files.".into()); },
        }
    }

    fn retry_password(&mut self, index: usize) {
        let request_id = self.take_request();
        let session = &mut self.tabs[index];
        session.password = Some(session.password_input.clone());
        session.password_required = false;
        session.status = "Unlocking document…".into();
        let _ = self.worker.commands.send(PdfCommand::Open { request_id, generation: session.generation, path: session.path.clone(), password: session.password.clone() });
    }

    fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() { return; }
        let generation = self.tabs[index].generation;
        self.persist_session(index);
        let _ = self.worker.commands.send(PdfCommand::CancelGeneration(generation));
        let _ = self.ebook_worker.commands.send(EbookCommand::CancelGeneration(generation));
        self.tabs.remove(index);
        self.active = if self.tabs.is_empty() { None } else { Some(index.min(self.tabs.len() - 1)) };
        if self.tabs.is_empty() { self.screen = AppScreen::Home; }
    }

    fn take_request(&mut self) -> u64 { let id = self.next_request; self.next_request += 1; id }

    fn palette(&self) -> Palette {
        let current = Palette::for_theme(self.style.app_theme);
        let Some((previous, started)) = self.theme_transition else { return current };
        if self.style.reduced_motion { return current; }
        let raw = (started.elapsed().as_secs_f32() / design::MOTION_DRAWER).clamp(0.0, 1.0);
        let eased = 1.0 - (1.0 - raw).powi(3);
        Palette::for_theme(previous).blend(current, eased)
    }

    fn request_page(&mut self, index: usize, page: u32, render: bool, priority: RequestPriority, force: bool) {
        let request_id = self.take_request();
        let Some(session) = self.tabs.get_mut(index) else { return };
        let Some(opened) = &session.opened else { return };
        let request = PageRequest { page, rotation: session.rotation, render };
        if page >= opened.page_count || (!force && (page_request_is_redundant(&session.requested_pages, request) || session.failed_pages.contains(&request))) { return; }
        if render && !force && session.textures.contains_key(&page) { return; }
        session.requested_pages.insert(request);
        let _ = self.worker.commands.send(PdfCommand::LoadPage {
            request_id, generation: session.generation, document_id: opened.document_id.clone(), path: opened.path.clone(),
            password: session.password.clone(), page, width: if render { 1500 } else { 0 }, rotation: session.rotation,
            render, priority,
        });
    }

    fn ensure_library_covers(&mut self, ctx: &egui::Context) {
        let entries: Vec<_> = self.library.iter().filter(|entry| !entry.missing).take(64)
            .map(|entry| (entry.document_id.clone(), entry.path.clone())).collect();
        for (document_id, path) in entries {
            if self.library_covers.contains_key(&document_id) || self.cover_requests.contains_key(&document_id) { continue; }
            if let Some((width, height, rgba)) = self.storage.as_ref().and_then(|storage| storage.load_cover(&document_id)) {
                let image = egui::ColorImage::from_rgba_unmultiplied([width, height], &rgba);
                let texture = ctx.load_texture(format!("cover-{document_id}"), image, egui::TextureOptions::LINEAR);
                self.library_covers.insert(document_id, LibraryCover { texture, aspect: height as f32 / width.max(1) as f32, loaded_at: Instant::now() });
                continue;
            }
            if DocumentFormat::from_path(&path).is_some_and(DocumentFormat::is_ebook) { continue; }
            let generation = self.next_generation; self.next_generation += 1;
            let request_id = self.take_request();
            self.cover_requests.insert(document_id.clone(), generation);
            let _ = self.worker.commands.send(PdfCommand::LoadThumbnail { request_id, generation, document_id, path, width: 240 });
        }
    }

    fn poll_worker(&mut self, ctx: &egui::Context) {
        let ebook_results: Vec<_> = self.ebook_worker.results.try_iter().collect();
        for result in ebook_results {
            match result {
                EbookResult::Opened { generation, book, .. } => {
                    let Some(index) = self.tabs.iter().position(|tab| tab.generation == generation) else { continue };
                    let opened = book.opened;
                    let progress = self.storage.as_ref().and_then(|storage| storage.load_progress(&opened.document_id));
                    let annotations = self.storage.as_ref().map(|storage| storage.annotations(&opened.document_id)).unwrap_or_default();
                    if let Some(storage) = &self.storage {
                        storage.remember_document(&opened);
                        storage.save_document_cache(&book.document);
                        if let Some(cover) = &book.cover { storage.save_cover(&opened.document_id, cover.width, cover.height, &cover.rgba); }
                    }
                    {
                        let session = &mut self.tabs[index];
                        session.document = book.document;
                        session.opened = Some(opened.clone());
                        session.mode = ReadingMode::Reflow;
                        session.annotations = annotations;
                        session.extracted_pages.extend(0..opened.page_count);
                        if let Some((anchor, section, _)) = progress { session.anchor = anchor; session.current_page = clamp_page_target(section, opened.page_count); }
                        session.scroll_to_block = (session.anchor.block_id != 0 && session.document.blocks.iter().any(|block| block.id == session.anchor.block_id)).then_some(session.anchor.block_id);
                        if session.scroll_to_block.is_none() { session.scroll_to_page = Some(session.current_page); }
                        session.status = "Book ready".into();
                        session.error = None;
                        if session.scroll_to_block.is_none() { sync_reflow_target(session); }
                    }
                    if let Some(storage) = &mut self.storage { storage.index_document(&self.tabs[index].document, &self.tabs[index].path); }
                    self.library = self.storage.as_ref().map(Storage::library).unwrap_or_default();
                }
                EbookResult::Failed { generation, message, .. } => {
                    if let Some(session) = self.tabs.iter_mut().find(|tab| tab.generation == generation) {
                        session.status = "Unable to read book".into();
                        session.error = Some(message);
                    }
                }
            }
        }
        let results: Vec<_> = self.worker.results.try_iter().collect();
        for result in results {
            match result {
                PdfResult::Opened { generation, document: opened, .. } => {
                    let Some(index) = self.tabs.iter().position(|tab| tab.generation == generation) else { continue };
                    let cached = self.storage.as_ref().and_then(|storage| storage.load_document_cache(&opened.document_id));
                    let progress = self.storage.as_ref().and_then(|storage| storage.load_progress(&opened.document_id));
                    let annotations = self.storage.as_ref().map(|storage| storage.annotations(&opened.document_id)).unwrap_or_default();
                    if let Some(storage) = &self.storage { storage.remember_document(&opened); }
                    {
                        let session = &mut self.tabs[index];
                        session.document = cached.unwrap_or_else(|| Document { id: opened.document_id.clone(), title: opened.title.clone(), page_count: opened.page_count, blocks: vec![], outline: opened.outline.clone(), metadata: opened.metadata.clone(), format: opened.format });
                        if !session.document.blocks.is_empty() { session.extracted_pages.extend(0..opened.page_count); }
                        if let Some((anchor, page, mode)) = progress.filter(|_| session.scroll_to_page.is_none()) { session.anchor = anchor; session.current_page = page.min(opened.page_count.saturating_sub(1)); session.mode = mode; }
                        session.current_page = clamp_page_target(session.current_page, opened.page_count);
                        session.scroll_to_page = session.scroll_to_page.map(|page| clamp_page_target(page, opened.page_count));
                        session.annotations = annotations; session.opened = Some(opened); session.error = None; session.status = "Preparing pages…".into();
                        sync_reflow_target(session);
                    }
                    self.library = self.storage.as_ref().map(Storage::library).unwrap_or_default();
                    let page = self.tabs[index].current_page;
                    if !self.hydrate_render_cache(index, page, ctx) { self.request_page(index, page, true, RequestPriority::Visible, true); }
                    else { self.request_page(index, page, false, RequestPriority::Visible, true); }
                    for neighbor in page.saturating_sub(2)..=(page + 2).min(self.tabs[index].page_count().saturating_sub(1)) {
                        if !self.hydrate_render_cache(index, neighbor, ctx) { self.request_page(index, neighbor, true, RequestPriority::Nearby, false); }
                        else { self.request_page(index, neighbor, false, RequestPriority::Nearby, false); }
                    }
                    if let Some(next) = self.next_unextracted(index) { self.request_page(index, next, false, RequestPriority::Background, false); }
                }
                PdfResult::Page { generation, document_id, rotation, render, data, .. } => {
                    let Some(index) = self.tabs.iter().position(|tab| tab.generation == generation && tab.document_id() == Some(document_id.as_str())) else { continue };
                    let page_number = data.page;
                    let request = PageRequest { page: page_number, rotation, render };
                    if !page_result_matches_rotation(self.tabs[index].rotation, rotation, !data.rgba.is_empty()) {
                        self.tabs[index].requested_pages.remove(&request);
                        continue;
                    }
                    let newly_extracted;
                    {
                        let session = &mut self.tabs[index];
                        session.requested_pages.remove(&request);
                        session.failed_pages.remove(&request);
                        let aspect = if data.width > 0 { data.height as f32 / data.width as f32 } else if data.raw.width > 0.0 { data.raw.height / data.raw.width } else { 1.414 };
                        session.page_aspects.insert(page_number, aspect);
                        newly_extracted = session.extracted_pages.insert(page_number);
                        if newly_extracted {
                            let first_id = ((page_number as u64) << 32) | 1;
                            session.document.blocks.extend(blocks_from_raw_page(&data.raw, first_id));
                            session.document.blocks.sort_by_key(|block| block.source.first().map(|source| (source.page, source.start_char)).unwrap_or((u32::MAX, u32::MAX)));
                            sync_reflow_target(session);
                        }
                        if !data.rgba.is_empty() || session.textures.contains_key(&page_number) {
                            session.raw_pages.insert(page_number, data.raw.clone());
                        }
                        if !data.rgba.is_empty() {
                            if let Some(storage) = &self.storage { storage.save_rendered_page(&document_id, page_number, rotation, data.width, data.height, &data.rgba); }
                            let image = egui::ColorImage::from_rgba_unmultiplied([data.width, data.height], &data.rgba);
                            let texture = ctx.load_texture(format!("{}-{page_number}-{rotation}", document_id), image, egui::TextureOptions::LINEAR);
                            session.textures.insert(page_number, session::CachedTexture { texture, bytes: data.rgba.len(), last_used: self.frame_tick });
                        }
                        let done = session.extracted_pages.len() as u32;
                        session.status = if done >= session.page_count() { "Document ready".into() } else { format!("Reading document… {done}/{}", session.page_count()) };
                    }
                    self.evict_textures(index);
                    let complete = self.tabs[index].extracted_pages.len() as u32 >= self.tabs[index].page_count();
                    if newly_extracted && complete {
                        if let Some(storage) = &mut self.storage { storage.save_document_cache(&self.tabs[index].document); storage.index_document(&self.tabs[index].document, &self.tabs[index].path); }
                    } else if let Some(next) = self.next_unextracted(index) { self.request_page(index, next, false, RequestPriority::Background, false); }
                }
                PdfResult::Thumbnail { generation, document_id, path, data, .. } => {
                    let current = self.cover_requests.get(&document_id).copied() == Some(generation);
                    let still_valid = self.library.iter().any(|entry| entry.document_id == document_id && entry.path == path && !entry.missing);
                    if !current || !still_valid || data.rgba.is_empty() { continue; }
                    if let Some(storage) = &self.storage { storage.save_cover(&document_id, data.width, data.height, &data.rgba); }
                    let image = egui::ColorImage::from_rgba_unmultiplied([data.width, data.height], &data.rgba);
                    let texture = ctx.load_texture(format!("cover-{document_id}"), image, egui::TextureOptions::LINEAR);
                    self.library_covers.insert(document_id.clone(), LibraryCover { texture, aspect: data.height as f32 / data.width.max(1) as f32, loaded_at: Instant::now() });
                    self.cover_requests.remove(&document_id);
                }
                PdfResult::PasswordRequired { generation, .. } => {
                    if let Some(session) = self.tabs.iter_mut().find(|tab| tab.generation == generation) { session.password_required = true; session.status = "Password required".into(); session.error = None; }
                }
                PdfResult::Failed { generation, request, message, .. } => match request {
                    FailedRequest::Open => {
                        if let Some(session) = self.tabs.iter_mut().find(|tab| tab.generation == generation) {
                            session.status = "Unable to read document".into();
                            session.error = Some(message);
                        }
                    }
                    FailedRequest::Page { document_id, page, rotation, render } => {
                        if let Some(index) = self.tabs.iter().position(|tab| {
                            tab.generation == generation
                                && page_failure_matches(tab.document_id(), tab.rotation, &document_id, rotation, render)
                        }) {
                            let failed = PageRequest { page, rotation, render };
                            let session = &mut self.tabs[index];
                            session.requested_pages.remove(&failed);
                            session.failed_pages.insert(failed);
                            session.status = "Unable to read page".into();
                            session.error = Some(message);
                            if !render {
                                if let Some(next) = self.next_unextracted(index) {
                                    self.request_page(index, next, false, RequestPriority::Background, false);
                                }
                            }
                        }
                    }
                    FailedRequest::Thumbnail { .. } => {}
                }
            }
        }
    }

    fn next_unextracted(&self, index: usize) -> Option<u32> {
        let session = self.tabs.get(index)?;
        next_unextracted_page(session.page_count(), &session.extracted_pages, &session.requested_pages, &session.failed_pages)
    }

    fn evict_textures(&mut self, index: usize) {
        let session = &mut self.tabs[index];
        let mut total: usize = session.textures.values().map(|entry| entry.bytes).sum();
        while total > TEXTURE_BUDGET {
            let Some((&page, _)) = session.textures.iter().filter(|(page, _)| **page != session.current_page).min_by_key(|(_, entry)| entry.last_used) else { break };
            if let Some(entry) = session.textures.remove(&page) { total = total.saturating_sub(entry.bytes); session.raw_pages.remove(&page); }
        }
    }

    fn hydrate_render_cache(&mut self, index: usize, page: u32, ctx: &egui::Context) -> bool {
        let Some(document_id) = self.tabs.get(index).and_then(|session| session.document_id()).map(str::to_owned) else { return false };
        let rotation = self.tabs[index].rotation;
        let Some((width, height, rgba)) = self.storage.as_ref().and_then(|storage| storage.load_rendered_page(&document_id, page, rotation)) else { return false };
        let image = egui::ColorImage::from_rgba_unmultiplied([width, height], &rgba);
        let texture = ctx.load_texture(format!("{document_id}-{page}-{rotation}-cached"), image, egui::TextureOptions::LINEAR);
        self.tabs[index].page_aspects.insert(page, height as f32 / width.max(1) as f32);
        self.tabs[index].textures.insert(page, session::CachedTexture { texture, bytes: rgba.len(), last_used: self.frame_tick });
        self.evict_textures(index);
        true
    }

    fn persist_session(&mut self, index: usize) {
        let Some(session) = self.tabs.get(index) else { return };
        let Some(opened) = &session.opened else { return };
        if let Some(storage) = &self.storage { storage.save_progress(&opened.document_id, &opened.path, &session.anchor, session.current_page, session.mode, opened.page_count); }
    }

    fn run_search(&mut self, current_only: bool) {
        let document_id = if current_only { self.active.and_then(|index| self.tabs[index].document_id().map(str::to_owned)) } else { None };
        let query = if current_only { self.active.map(|index| self.tabs[index].search_query.clone()).unwrap_or_default() } else { self.library_query.clone() };
        let mut hits = self.storage.as_ref().map(|storage| storage.search(&query, document_id.as_deref(), 200)).unwrap_or_default();
        if current_only && hits.is_empty() && !query.trim().is_empty() {
            if let Some(index) = self.active {
                let needle = query.to_lowercase();
                let document_id = self.tabs[index].document_id().unwrap_or_default().to_owned();
                let path = self.tabs[index].path.to_string_lossy().into_owned();
                let title = self.tabs[index].title();
                hits = self.tabs[index].document.blocks.iter()
                    .filter(|block| block.content.to_lowercase().contains(&needle))
                    .filter_map(|block| block.source.first().map(|source| SearchHit { document_id: document_id.clone(), path: path.clone(), title: title.clone(), block_id: block.id, page: source.page, snippet: block.content.chars().take(180).collect() }))
                    .take(200).collect();
            }
        }
        if current_only { if let Some(index) = self.active { self.tabs[index].search_hits = hits; self.tabs[index].selected_hit = 0; } } else { self.library_hits = hits; }
    }

    fn add_annotation(&mut self, kind: AnnotationKind) {
        let Some(index) = self.active else { return };
        let Some(document_id) = self.tabs[index].document_id().map(str::to_owned) else { return };
        let block = self.tabs[index].selected_block.and_then(|id| self.tabs[index].document.blocks.iter().find(|block| block.id == id)).cloned();
        let (start, end, quote) = if let Some((block, source)) = block.and_then(|block| block.source.first().copied().map(|source| (block, source))) {
            (SourcePosition { page: source.page, char_index: source.start_char }, SourcePosition { page: source.page, char_index: source.end_char }, block.content)
        } else {
            let page = self.tabs[index].current_page;
            (SourcePosition { page, char_index: 0 }, SourcePosition { page, char_index: 0 }, String::new())
        };
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
        let mut annotation = Annotation { id: 0, document_id, kind, start, end, color: 0xFFE8_7D7D, quote, note: if kind == AnnotationKind::Note { self.note_draft.clone() } else { String::new() }, created_at: now, updated_at: now };
        if let Some(storage) = &self.storage { storage.save_annotation(&mut annotation); }
        self.tabs[index].annotations.push(annotation);
        self.note_draft.clear();
    }

    fn keyboard(&mut self, ctx: &egui::Context) {
        let dropped: Vec<_> = ctx.input(|input| input.raw.dropped_files.iter().map(|file| file.path().to_owned()).filter(|path| DocumentFormat::from_path(path).is_some()).collect());
        for path in dropped { self.open_path(path); }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::O)) { self.open_picker(); }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::B)) { self.show_sidebar = !self.show_sidebar; }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::F)) { self.show_sidebar = true; self.sidebar = SidebarView::Search; }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::F11)) { self.distraction_free = !self.distraction_free; }
        if let Some(index) = self.active {
            let page_count = self.tabs[index].page_count();
            if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::PageDown)) && self.tabs[index].current_page + 1 < page_count { self.tabs[index].current_page += 1; self.tabs[index].scroll_to_page = Some(self.tabs[index].current_page); }
            if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::PageUp)) && self.tabs[index].current_page > 0 { self.tabs[index].current_page -= 1; self.tabs[index].scroll_to_page = Some(self.tabs[index].current_page); }
        }
    }

    fn toolbar(&mut self, root: &mut egui::Ui, layout: LayoutClass) {
        let mut open = false;
        let mut close_current = false;
        let mut theme_changed = false;
        let palette = self.palette();
        egui::Panel::top("toolbar").exact_size(design::TOOLBAR_HEIGHT)
            .frame(egui::Frame::new().fill(palette.panel).stroke(Stroke::new(1.0, palette.border)).inner_margin(egui::Margin::symmetric(10, 6)))
            .show(root, |ui| {
            let rect = ui.max_rect();
            let left_width = if layout == LayoutClass::Small { 88.0 } else { (rect.width() * 0.36).min(430.0) };
            ui.scope_builder(egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(rect.min, egui::vec2(left_width, rect.height()))), |ui| {
                ui.horizontal(|ui| {
                    if design::icon_button(ui, Icon::Panel, "Toggle navigation (Ctrl+B)").clicked() { self.show_sidebar = !self.show_sidebar; }
                    if design::icon_button(ui, Icon::Library, "Library").clicked() { self.screen = AppScreen::Home; self.sidebar = SidebarView::Library; self.show_settings = false; }
                    if layout != LayoutClass::Small {
                        let original = if self.screen == AppScreen::Home { "koiflow · Library".into() } else { self.active.map(|index| self.tabs[index].title()).unwrap_or_else(|| "koiflow".into()) };
                        let title = if self.screen == AppScreen::Home { original.clone() } else { DisplayMetadata::new(&original, "").title };
                        ui.add(egui::Label::new(RichText::new(title).font(design::display_font(15.0))).truncate()).on_hover_text(original);
                    }
                });
            });
            if self.screen == AppScreen::Reader {
                let center_width = 196.0_f32.min(rect.width() * 0.34);
                let center_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(center_width, rect.height()));
                ui.scope_builder(egui::UiBuilder::new().max_rect(center_rect).layout(Layout::top_down(Align::Center)), |ui| {
                    if let Some(index) = self.active { ui.horizontal(|ui| {
                        if self.tabs[index].opened.as_ref().is_some_and(|opened| opened.capabilities.original_pages) {
                            if ui.add(egui::Button::selectable(self.tabs[index].mode == ReadingMode::Original, "Original")).clicked() { self.tabs[index].mode = ReadingMode::Original; }
                            if ui.add(egui::Button::selectable(self.tabs[index].mode == ReadingMode::Reflow, "Reflow")).clicked() { self.tabs[index].mode = ReadingMode::Reflow; }
                        } else { ui.label(RichText::new("Reading").strong()); }
                    }); }
                });
            }
            let right_width = if layout == LayoutClass::Small { 148.0 } else { 184.0 };
            let right_rect = egui::Rect::from_min_size(egui::pos2(rect.right() - right_width, rect.top()), egui::vec2(right_width, rect.height()));
            ui.scope_builder(egui::UiBuilder::new().max_rect(right_rect).layout(Layout::right_to_left(Align::Center)), |ui| {
                    let menu = ui.menu_image_button(Icon::More.image(18.0), |ui| {
                        if ui.button("Distraction-free reading   F11").clicked() { self.distraction_free = true; ui.close(); }
                        if ui.button(if self.style.app_theme.is_dark() { "Use light interface" } else { "Use dark interface" }).clicked() {
                            let previous = self.style.app_theme;
                            self.style.app_theme = if self.style.app_theme.is_dark() { AppTheme::Light } else { AppTheme::Dark };
                            self.theme_transition = Some((previous, Instant::now()));
                            theme_changed = true; ui.close();
                        }
                        if self.active.is_some() && ui.button("Close current tab").clicked() { close_current = true; ui.close(); }
                    });
                    menu.response.on_hover_text("More options");
                    if design::icon_button(ui, Icon::Settings, "Reading appearance").clicked() { self.show_settings = !self.show_settings; if layout != LayoutClass::Large { self.show_sidebar = false; } }
                    if design::icon_button(ui, Icon::Open, "Open document (Ctrl+O)").clicked() { open = true; }
                    if design::icon_button(ui, Icon::Search, "Search (Ctrl+F)").clicked() { self.show_sidebar = true; self.show_settings = false; self.sidebar = SidebarView::Search; }
            });
        });
        if open { self.open_picker(); }
        if theme_changed { design::configure(root.ctx(), self.style.app_theme, self.style.reduced_motion); if let Some(storage) = &self.storage { storage.save_style(&self.style); } }
        if close_current { if let Some(index) = self.active { self.close_tab(index); } }
    }

    fn tab_bar(&mut self, root: &mut egui::Ui) {
        let mut close = None;
        let palette = self.palette();
        egui::Panel::top("tabs").exact_size(design::TAB_HEIGHT).frame(egui::Frame::new().fill(palette.panel).inner_margin(egui::Margin::symmetric(10, 2))).show(root, |ui| {
            egui::ScrollArea::horizontal().id_salt("document-tabs").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| { ui.horizontal(|ui| {
                for index in 0..self.tabs.len() {
                    let selected = self.active == Some(index);
                    let original = self.tabs[index].title();
                    let title = DisplayMetadata::new(&original, "").title;
                    let width = design::TAB_WIDTH;
                    let (tab_id, rect) = ui.allocate_space(egui::vec2(width, 28.0));
                    let response = ui.interact(rect, tab_id.with("tab"), Sense::click());
                    let active_strength = design::animate_bool(ui.ctx(), ui.id().with(("active-tab", self.tabs[index].tab_id)), selected, design::MOTION_SELECTION, self.style.reduced_motion);
                    let fill = if selected { palette.surface_high } else if response.hovered() { palette.surface_low } else { Color32::TRANSPARENT };
                    ui.painter().rect_filled(rect, design::CONTROL_RADIUS, fill);
                    if selected || response.hovered() { ui.painter().rect_stroke(rect, design::CONTROL_RADIUS, Stroke::new(1.0, palette.separator), egui::StrokeKind::Inside); }
                    let indicator_color = if self.tabs[index].error.is_some() { Some(palette.danger) } else if self.tabs[index].opened.is_none() { Some(palette.accent) } else { None };
                    if let Some(color) = indicator_color { ui.painter().circle_filled(rect.left_center() + egui::vec2(11.0, 0.0), 3.0, color); }
                    let title_left = rect.left() + if indicator_color.is_some() { 20.0 } else { 10.0 };
                    let title_rect = egui::Rect::from_min_max(egui::pos2(title_left, rect.top()), egui::pos2(rect.right() - 32.0, rect.bottom()));
                    let title_galley = egui::WidgetText::from(RichText::new(title).size(13.0)).into_galley(
                        ui,
                        Some(egui::TextWrapMode::Truncate),
                        title_rect.width(),
                        egui::TextStyle::Button,
                    );
                    let title_position = egui::pos2(title_rect.left(), title_rect.center().y - title_galley.size().y * 0.5);
                    ui.painter().with_clip_rect(title_rect).galley(title_position, title_galley, palette.text);
                    if response.clicked() { self.active = Some(index); self.screen = AppScreen::Reader; if self.sidebar == SidebarView::Library { self.sidebar = SidebarView::Outline; } }
                    response.clone().on_hover_text(&original);
                    let close_visible = selected || response.hovered() || response.has_focus();
                    let close_rect = egui::Rect::from_center_size(rect.right_center() - egui::vec2(15.0, 0.0), egui::vec2(26.0, 26.0));
                    if close_visible && ui.put(close_rect, egui::Button::image(Icon::Close.image(13.0)).frame(false)).on_hover_text("Close tab").clicked() { close = Some(index); }
                    let line = egui::Rect::from_min_max(egui::pos2(rect.left() + 9.0, rect.bottom() - 2.0), egui::pos2(rect.right() - 9.0, rect.bottom()));
                    ui.painter().rect_filled(line, 1.0, palette.accent.gamma_multiply(active_strength));
                }
            }); });
        });
        if let Some(index) = close { self.close_tab(index); }
    }

    fn sidebar_panel(&mut self, root: &mut egui::Ui, overlay: bool, motion: f32) {
        let mut open_path = None;
        let mut run_current_search = false;
        let mut run_library_search = false;
        let mut jump_page = None;
        let mut delete_annotation = None;
        let palette = self.palette();
        let mut content = |ui: &mut egui::Ui| {
            ui.set_opacity(motion);
            if overlay { ui.set_width(design::SIDEBAR_WIDTH - 24.0); } else { ui.set_width(ui.available_width()); }
            ui.label(RichText::new("NAVIGATE").size(11.0).strong().color(palette.muted));
            ui.add_space(4.0);
            if design::navigation_button(ui, Icon::Library, "Library", self.screen == AppScreen::Home).clicked() { self.sidebar = SidebarView::Library; self.screen = AppScreen::Home; }
            let has_reader = self.active.is_some();
            ui.add_enabled_ui(has_reader || !self.library.is_empty(), |ui| {
                if design::navigation_button(ui, Icon::Search, "Find", self.sidebar == SidebarView::Search).clicked() { self.sidebar = SidebarView::Search; }
            });
            ui.add_enabled_ui(has_reader, |ui| {
                if design::navigation_button(ui, Icon::Panel, "Outline", self.screen == AppScreen::Reader && self.sidebar == SidebarView::Outline).clicked() { self.screen = AppScreen::Reader; self.sidebar = SidebarView::Outline; }
                if design::navigation_button(ui, Icon::Note, "Notes & highlights", self.screen == AppScreen::Reader && self.sidebar == SidebarView::Annotations).clicked() { self.screen = AppScreen::Reader; self.sidebar = SidebarView::Annotations; }
            });
            if self.sidebar != SidebarView::Library { ui.add_space(6.0); ui.separator(); ui.add_space(4.0); }
            match self.sidebar {
                SidebarView::Library => {}
                SidebarView::Outline => {
                    if self.screen == AppScreen::Reader { if let Some(index) = self.active {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for item in &self.tabs[index].document.outline {
                                ui.horizontal(|ui| { ui.add_space(item.depth as f32 * 12.0); if ui.selectable_label(false, &item.title).clicked() { jump_page = item.page; } });
                            }
                            if self.tabs[index].document.outline.is_empty() {
                                for block in self.tabs[index].document.blocks.iter().filter(|block| matches!(block.kind, BlockKind::Heading { .. })) {
                                    if ui.selectable_label(false, &block.content).clicked() { jump_page = block.source.first().map(|source| source.page); }
                                }
                                if !self.tabs[index].document.blocks.iter().any(|block| matches!(block.kind, BlockKind::Heading { .. })) { ui.label("This document has no detectable outline."); }
                            }
                        });
                    } }
                }
                SidebarView::Search => {
                    if self.screen == AppScreen::Home {
                        let response = ui.add(egui::TextEdit::singleline(&mut self.library_query).hint_text("Search inside your library…"));
                        if (response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter))) || design::icon_text_button(ui, Icon::Search, "Search book contents").clicked() { run_library_search = true; }
                        ui.add_space(6.0);
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for hit in &self.library_hits {
                                let title = DisplayMetadata::new(&hit.title, "").title;
                                if ui.selectable_label(false, format!("{} · {} {}\n{}", title, if hit.page == 0 { "Start" } else { "Location" }, hit.page + 1, hit.snippet)).clicked() { open_path = Some(PathBuf::from(&hit.path)); jump_page = Some(hit.page); }
                            }
                            if !self.library_query.trim().is_empty() && self.library_hits.is_empty() { ui.label(RichText::new("No matching passages.").color(palette.muted)); }
                        });
                    } else if let Some(index) = self.active {
                        let response = ui.add(egui::TextEdit::singleline(&mut self.tabs[index].search_query).hint_text("Find in this document…"));
                        if (response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter))) || design::icon_text_button(ui, Icon::Search, "Find in document").clicked() { run_current_search = true; }
                        ui.separator();
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            let hits = self.tabs[index].search_hits.clone();
                            for (hit_index, hit) in hits.iter().enumerate() {
                                let unit = if self.tabs[index].opened.as_ref().is_some_and(|opened| opened.format.is_ebook()) { "Section" } else { "Page" };
                                if ui.selectable_label(self.tabs[index].selected_hit == hit_index, format!("{unit} {}\n{}", hit.page + 1, hit.snippet)).clicked() { self.tabs[index].selected_hit = hit_index; jump_page = Some(hit.page); }
                            }
                        });
                    }
                }
                SidebarView::Annotations => {
                    if self.screen == AppScreen::Reader { if let Some(index) = self.active {
                        ui.horizontal(|ui| { if ui.add(egui::Button::image_and_text(Icon::Highlight.image(15.0), "Highlight").image_tint_follows_text_color(true)).clicked() { self.add_annotation(AnnotationKind::Highlight); } if ui.add(egui::Button::image_and_text(Icon::Bookmark.image(15.0), "Bookmark").image_tint_follows_text_color(true)).clicked() { self.add_annotation(AnnotationKind::Bookmark); } });
                        ui.add(egui::TextEdit::multiline(&mut self.note_draft).hint_text("Write a note…").desired_rows(3));
                        if design::icon_text_button(ui, Icon::Note, "Add note to selection or page").clicked() { self.add_annotation(AnnotationKind::Note); }
                        ui.separator();
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for annotation in &self.tabs[index].annotations {
                                ui.horizontal(|ui| {
                                    let label = match annotation.kind { AnnotationKind::Highlight => "Highlight", AnnotationKind::Note => "Note", AnnotationKind::Bookmark => "Bookmark" };
                                    let location = if self.tabs[index].opened.as_ref().is_some_and(|opened| opened.format.is_ebook()) { format!("section {}", annotation.start.page + 1) } else { format!("p. {}", annotation.start.page + 1) };
                                    if ui.selectable_label(false, format!("{label} · {location}\n{}", if annotation.note.is_empty() { &annotation.quote } else { &annotation.note })).clicked() { jump_page = Some(annotation.start.page); }
                                    if design::icon_button(ui, Icon::Close, "Delete annotation").clicked() { delete_annotation = Some(annotation.id); }
                                });
                            }
                        });
                    } }
                }
            }
        };
        if overlay {
            egui::Window::new("navigation-drawer").title_bar(false).resizable(false).collapsible(false)
                .anchor(egui::Align2::LEFT_TOP, egui::vec2(-design::SIDEBAR_WIDTH * (1.0 - motion) + 8.0, design::TOOLBAR_HEIGHT + design::TAB_HEIGHT + 8.0))
                .frame(egui::Frame::new().fill(palette.panel).stroke(Stroke::new(1.0, palette.border)).corner_radius(12).inner_margin(egui::Margin::same(12)))
                .show(root.ctx(), |ui| content(ui));
        } else {
            egui::Panel::left("navigation")
                .default_size(design::SIDEBAR_WIDTH)
                .size_range(design::SIDEBAR_MIN_WIDTH..=design::SIDEBAR_MAX_WIDTH)
                .resizable(true)
                .frame(egui::Frame::new().fill(palette.panel).stroke(Stroke::new(1.0, palette.border)).inner_margin(egui::Margin::same(12)))
                .show(root, |ui| content(ui));
        }
        if run_current_search { self.run_search(true); }
        if run_library_search { self.run_search(false); }
        if let Some(path) = open_path { self.open_path(path); if let (Some(index), Some(page)) = (self.active, jump_page) { navigate_to_page(&mut self.tabs[index], page); } }
        else if let (Some(index), Some(page)) = (self.active, jump_page) { navigate_to_page(&mut self.tabs[index], page); }
        if let Some(id) = delete_annotation { if let Some(storage) = &self.storage { storage.delete_annotation(id); } if let Some(index) = self.active { self.tabs[index].annotations.retain(|annotation| annotation.id != id); } }
    }

    fn settings_panel(&mut self, root: &mut egui::Ui, overlay: bool, motion: f32) {
        let ctx = root.ctx().clone();
        let before = serde_json::to_string(&self.style).ok();
        let previous_theme = self.style.app_theme;
        let previous_reduced_motion = self.style.reduced_motion;
        let mut clear_cache = false;
        let palette = self.palette();
        let mut content = |ui: &mut egui::Ui| {
            ui.set_opacity(motion);
            ui.set_width(design::SETTINGS_WIDTH - 32.0);
            egui::ScrollArea::vertical()
                .id_salt("reading-appearance-scroll")
                .auto_shrink([false, false])
                .max_height(ui.available_height())
                .show(ui, |ui| {
            ui.set_width(design::SETTINGS_WIDTH - 32.0);
            ui.horizontal(|ui| { ui.label(RichText::new("Reading appearance").font(design::display_font(22.0))); ui.with_layout(Layout::right_to_left(Align::Center), |ui| { if ui.small_button("Reset").clicked() { self.style = ReaderStyle::default(); } }); });
            ui.label(RichText::new("Changes apply instantly").small().color(palette.muted));
            ui.add_space(12.0);
            egui::Frame::new().fill(palette.surface_low).stroke(Stroke::new(1.0, palette.separator)).corner_radius(10).inner_margin(egui::Margin::same(14)).show(ui, |ui| {
                let preview_family = if self.style.font_family == "Monospace" { egui::FontFamily::Monospace } else if self.style.font_family == "System sans-serif" { egui::FontFamily::Proportional } else { egui::FontFamily::Name("serif".into()) };
                ui.label(RichText::new("A well-set page makes room for thought.").font(FontId::new(self.style.font_size.clamp(14.0, 20.0), preview_family)).line_height(Some((self.style.font_size.clamp(14.0, 20.0) * self.style.line_height).round())).extra_letter_spacing(self.style.letter_spacing).color(palette.text));
                ui.label(RichText::new("Typography preview").small().color(palette.muted));
            });
            ui.add_space(18.0); settings_heading(ui, "TYPOGRAPHY");
            egui::ComboBox::from_id_salt("font-family").selected_text(&self.style.font_family).show_ui(ui, |ui| { for family in ["System serif", "System sans-serif", "Monospace"] { ui.selectable_value(&mut self.style.font_family, family.into(), family); } });
            slider(ui, &mut self.style.font_size, 12.0..=40.0, "Font size", " px");
            slider(ui, &mut self.style.line_height, 1.1..=2.2, "Line height", "×");
            slider(ui, &mut self.style.letter_spacing, -0.5..=3.0, "Letter spacing", " px");
            ui.horizontal(|ui| { ui.label("Alignment"); ui.selectable_value(&mut self.style.alignment, TextAlignment::Left, "Left"); ui.selectable_value(&mut self.style.alignment, TextAlignment::Center, "Center"); });
            ui.add_space(20.0); settings_heading(ui, "LAYOUT");
            slider(ui, &mut self.style.content_max_width, 440.0..=960.0, "Reading width", " px");
            slider(ui, &mut self.style.horizontal_margin, 16.0..=80.0, "Margins", " px");
            slider(ui, &mut self.style.paragraph_spacing, 6.0..=36.0, "Paragraph spacing", " px");
            ui.add_space(20.0); settings_heading(ui, "APPLICATION THEME");
            egui::Grid::new("application-theme-swatches").num_columns(2).spacing(egui::vec2(8.0, 8.0)).show(ui, |ui| {
                for (index, theme) in AppTheme::ALL.into_iter().enumerate() {
                    let colors = Palette::for_theme(theme);
                    let selected = self.style.app_theme == theme;
                    let response = ui.add_sized([124.0, 38.0], egui::Button::new(format!("●  {}", theme.label())).selected(selected));
                    ui.painter().circle_filled(response.rect.left_center() + egui::vec2(14.0, 0.0), 5.0, colors.canvas);
                    if response.clicked() { self.style.app_theme = theme; }
                    if index % 2 == 1 { ui.end_row(); }
                }
            });
            ui.add_space(12.0); settings_heading(ui, "READING SURFACE");
            ui.horizontal(|ui| { ui.selectable_value(&mut self.style.theme, ReaderTheme::Paper, "Paper"); ui.selectable_value(&mut self.style.theme, ReaderTheme::Sepia, "Sepia"); ui.selectable_value(&mut self.style.theme, ReaderTheme::Dark, "Dark"); });
            ui.add_space(20.0); settings_heading(ui, "MOTION & BEHAVIOR");
            ui.checkbox(&mut self.distraction_free, "Distraction-free reading");
            ui.checkbox(&mut self.style.reduced_motion, "Reduce interface motion");
            if ui.button("Clear page and cover cache").clicked() { clear_cache = true; }
                });
        };
        if overlay {
            let drawer_height = (ctx.content_rect().height() - design::TOOLBAR_HEIGHT - design::TAB_HEIGHT - design::STATUS_HEIGHT - 24.0).max(240.0);
            egui::Window::new("settings-drawer").title_bar(false).resizable(false).collapsible(false)
                .max_height(drawer_height)
                .anchor(egui::Align2::RIGHT_TOP, egui::vec2(design::SETTINGS_WIDTH * (1.0 - motion) - 8.0, design::TOOLBAR_HEIGHT + design::TAB_HEIGHT + 8.0))
                .frame(egui::Frame::new().fill(palette.panel).stroke(Stroke::new(1.0, palette.border)).corner_radius(12).inner_margin(egui::Margin::same(16)))
                .show(root.ctx(), |ui| content(ui));
        } else {
            egui::Panel::right("settings").exact_size(design::SETTINGS_WIDTH).resizable(false)
                .frame(egui::Frame::new().fill(palette.panel).stroke(Stroke::new(1.0, palette.border)).inner_margin(egui::Margin::same(16)))
                .show(root, |ui| content(ui));
        }
        if before != serde_json::to_string(&self.style).ok() {
            if previous_theme != self.style.app_theme { self.theme_transition = Some((previous_theme, Instant::now())); }
            if !previous_reduced_motion && self.style.reduced_motion { ctx.clear_animations(); }
            design::configure(&ctx, self.style.app_theme, self.style.reduced_motion);
            if let Some(storage) = &self.storage { storage.save_style(&self.style); }
        }
        if clear_cache { if let Some(storage) = &self.storage { storage.clear_render_cache(); } for tab in &mut self.tabs { tab.textures.clear(); } self.library_covers.clear(); self.cover_requests.clear(); }
    }

    fn central(&mut self, root: &mut egui::Ui) {
        let palette = self.palette();
        let mut requests = vec![];
        let mut followed_link = None;
        let mut home_open = false;
        let mut home_path = None;
        if self.screen == AppScreen::Home { self.ensure_library_covers(root.ctx()); }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(palette.canvas)).show(root, |ui| {
            if self.screen == AppScreen::Reader { if let Some(index) = self.active {
                if self.tabs[index].opened.is_none() {
                    if let Some(error) = self.tabs[index].error.clone() {
                        ui.centered_and_justified(|ui| { egui::Frame::new().fill(palette.surface_high).stroke(Stroke::new(1.0, palette.separator)).corner_radius(design::CARD_RADIUS).inner_margin(egui::Margin::same(24)).show(ui, |ui| {
                            ui.set_width(420.0_f32.min(ui.available_width()));
                            ui.vertical_centered(|ui| {
                                ui.label(RichText::new("This document could not be opened").font(design::display_font(21.0)).color(palette.text));
                                ui.label(RichText::new(error).color(palette.danger));
                                ui.add_space(10.0);
                                if ui.button("Choose another document").clicked() { home_open = true; }
                                if ui.button("Return to library").clicked() { self.screen = AppScreen::Home; }
                            });
                        }); });
                    } else {
                        ui.centered_and_justified(|ui| { ui.vertical_centered(|ui| { if self.style.reduced_motion { ui.label(RichText::new("Opening…").color(palette.muted)); } else { ui.spinner(); } ui.label(RichText::new(&self.tabs[index].status).color(palette.muted)); }); });
                    }
                }
                else if self.tabs[index].mode == ReadingMode::Original && self.tabs[index].opened.as_ref().is_some_and(|opened| opened.capabilities.original_pages) { (requests, followed_link) = original_view(ui, &mut self.tabs[index], self.frame_tick, palette); }
                else { reflow_view(ui, &mut self.tabs[index], &self.style); }
            } else { self.screen = AppScreen::Home; } }
            if self.screen == AppScreen::Home { (home_open, home_path) = library_home(ui, &self.library, &self.library_covers, palette, &mut self.home_query, &mut self.library_sort, self.style.reduced_motion); }
        });
        if home_open { self.open_picker(); }
        if let Some(path) = home_path { self.open_path(path); }
        if let Some(index) = self.active {
            let ctx = root.ctx().clone();
            for (page, priority) in requests {
                if !self.hydrate_render_cache(index, page, &ctx) { self.request_page(index, page, true, priority, false); }
                else if !self.tabs[index].raw_pages.contains_key(&page) { self.request_page(index, page, false, priority, false); }
            }
        }
        if let (Some(index), Some(target)) = (self.active, followed_link) {
            match target {
                LinkTarget::Page(page) => navigate_to_page(&mut self.tabs[index], page),
                LinkTarget::Uri(uri) => root.ctx().open_url(egui::OpenUrl::new_tab(uri)),
                LinkTarget::Unknown => {}
            }
        }
    }

    fn status_bar(&mut self, root: &mut egui::Ui) {
        let palette = self.palette();
        egui::Panel::bottom("status").exact_size(design::STATUS_HEIGHT).frame(egui::Frame::new().fill(palette.panel).stroke(Stroke::new(1.0, palette.border)).inner_margin(egui::Margin::symmetric(10, design::STATUS_VERTICAL_PADDING))).show(root, |ui| { ui.horizontal(|ui| {
            if self.screen == AppScreen::Home { ui.label(RichText::new(format!("Library · {} documents", self.library.len())).small().color(palette.muted)); }
            else if let Some(index) = self.active { ui.label(RichText::new(&self.tabs[index].status).small().color(palette.muted)); if let Some(error) = &self.tabs[index].error { ui.label(RichText::new(error).small().color(palette.danger)); } ui.with_layout(Layout::right_to_left(Align::Center), |ui| { let count = self.tabs[index].page_count(); let page = self.tabs[index].current_page; let progress = if count > 1 { page as f32 / (count - 1) as f32 * 100.0 } else { 0.0 }; let unit = if self.tabs[index].opened.as_ref().is_some_and(|opened| opened.format.is_ebook()) { "Chapter" } else { "Page" }; ui.label(RichText::new(format!("{unit} {} / {}  ·  {:.0}%", page + 1, count, progress)).small().color(palette.muted)); if let Some(chapter) = self.tabs[index].document.outline.iter().filter(|item| item.page.is_some_and(|chapter_page| chapter_page <= page)).next_back() { ui.add(egui::Label::new(RichText::new(&chapter.title).small().color(palette.muted)).truncate()).on_hover_text(&chapter.title); } }); }
            else { ui.label(RichText::new("Ready").small().color(ui.visuals().weak_text_color())); }
        }); });
    }

    fn password_dialog(&mut self, ctx: &egui::Context) {
        let Some(index) = self.active.filter(|index| self.tabs[*index].password_required) else { return };
        let mut unlock = false;
        egui::Window::new("Unlock PDF").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
            ui.horizontal(|ui| { ui.add(Icon::Lock.image(20.0)); ui.label(RichText::new("Password protected").strong()); });
            ui.label("Enter the document password to continue.");
            let response = ui.add(egui::TextEdit::singleline(&mut self.tabs[index].password_input).password(true).hint_text("Password"));
            if ui.button("Unlock").clicked() || (response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter))) { unlock = true; }
        });
        if unlock { self.retry_password(index); }
    }

    fn periodic_persist(&mut self) {
        let Some(index) = self.active else { return };
        if self.tabs[index].last_progress_save.elapsed() >= Duration::from_secs(1) { self.persist_session(index); self.tabs[index].last_progress_save = Instant::now(); }
    }
}

impl eframe::App for KoiFlowApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frame_tick = self.frame_tick.wrapping_add(1);
        let ctx = ui.ctx().clone();
        self.poll_worker(&ctx); self.keyboard(&ctx); self.periodic_persist();
        let background_work = !self.cover_requests.is_empty() || self.tabs.iter().any(|tab| tab.opened.is_none() || !tab.requested_pages.is_empty());
        if background_work && !self.style.reduced_motion { ctx.request_repaint_after(Duration::from_millis(80)); }
        if let Some(index) = self.active {
            ctx.request_repaint_after(Duration::from_secs(1).saturating_sub(self.tabs[index].last_progress_save.elapsed()));
        }
        let layout = LayoutClass::for_width(ui.available_width());
        if layout != LayoutClass::Large && self.show_settings { self.show_sidebar = false; }
        let chrome = design::animate_bool(&ctx, egui::Id::new("application-chrome"), !self.distraction_free, design::MOTION_SELECTION, self.style.reduced_motion);
        if chrome > 0.0 { self.toolbar(ui, layout); if !self.tabs.is_empty() { self.tab_bar(ui); } self.status_bar(ui); }
        if self.theme_transition.is_some_and(|(_, started)| started.elapsed().as_secs_f32() < design::MOTION_DRAWER) { ctx.request_repaint(); }
        let settings_motion = design::animate_bool(&ctx, egui::Id::new("settings-drawer-motion"), !self.distraction_free && self.show_settings, design::MOTION_DRAWER, self.style.reduced_motion);
        let sidebar_motion = design::animate_bool(&ctx, egui::Id::new("navigation-drawer-motion"), !self.distraction_free && self.show_sidebar, design::MOTION_DRAWER, self.style.reduced_motion);
        if settings_motion > 0.0 { self.settings_panel(ui, layout == LayoutClass::Small, settings_motion); }
        if sidebar_motion > 0.0 { self.sidebar_panel(ui, layout == LayoutClass::Small, sidebar_motion); }
        self.central(ui); self.password_dialog(&ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        for index in 0..self.tabs.len() { self.persist_session(index); }
        if let Some(storage) = &mut self.storage {
            storage.save_style(&self.style);
            let tabs: Vec<_> = self.tabs.iter().enumerate().map(|(index, tab)| SavedTab { path: tab.path.clone(), active: self.active == Some(index), position: index as u32 }).collect();
            storage.save_tabs(&tabs);
        }
    }
}

fn original_view(ui: &mut egui::Ui, session: &mut DocumentSession, tick: u64, palette: Palette) -> (Vec<(u32, RequestPriority)>, Option<LinkTarget>) {
    let mut requests = vec![];
    let mut followed_link = None;
    egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(12, 3)).show(ui, |ui| { ui.horizontal(|ui| {
        if design::icon_button(ui, Icon::ChevronLeft, "Previous page").clicked() && session.current_page > 0 { session.current_page -= 1; session.scroll_to_page = Some(session.current_page); }
        let mut page = session.current_page + 1;
        if ui.add(egui::DragValue::new(&mut page).range(1..=session.page_count().max(1))).changed() { session.current_page = page.saturating_sub(1); session.scroll_to_page = Some(session.current_page); }
        ui.label(format!("of {}", session.page_count()));
        if design::icon_button(ui, Icon::ChevronRight, "Next page").clicked() && session.current_page + 1 < session.page_count() { session.current_page += 1; session.scroll_to_page = Some(session.current_page); }
        ui.separator();
        ui.selectable_value(&mut session.zoom, ZoomMode::FitWidth, "Fit width");
        ui.selectable_value(&mut session.zoom, ZoomMode::FitPage, "Fit page");
        if design::icon_button(ui, Icon::ZoomOut, "Zoom out").clicked() { let value = match session.zoom { ZoomMode::Custom(value) => value, _ => 1.0 }; session.zoom = ZoomMode::Custom((value - 0.1).max(0.35)); }
        if design::icon_button(ui, Icon::ZoomIn, "Zoom in").clicked() { let value = match session.zoom { ZoomMode::Custom(value) => value, _ => 1.0 }; session.zoom = ZoomMode::Custom((value + 0.1).min(3.0)); }
        if design::icon_button(ui, Icon::Rotate, "Rotate clockwise").clicked() { session.rotation = (session.rotation + 90) % 360; session.textures.clear(); }
    }); });
    let page_count = session.page_count();
    if page_count == 0 { ui.centered_and_justified(|ui| { ui.label("This PDF contains no pages."); }); return (requests, followed_link); }
    egui::ScrollArea::vertical().id_salt(("original-scroll", session.tab_id)).show_viewport(ui, |ui, viewport| {
        let available = (ui.available_width() - 56.0).max(320.0);
        let zoom = match session.zoom { ZoomMode::FitWidth => 1.0, ZoomMode::FitPage => (ui.clip_rect().height() / (available * 1.414)).min(1.0), ZoomMode::Custom(value) => value };
        let page_width = available * zoom;
        let heights: Vec<_> = (0..page_count).map(|page| page_width * session.page_aspects.get(&page).copied().unwrap_or(1.414) + 28.0).collect();
        let mut offsets = Vec::with_capacity(heights.len() + 1); offsets.push(0.0);
        for height in &heights { offsets.push(offsets.last().copied().unwrap_or(0.0) + height); }
        ui.set_min_height(offsets.last().copied().unwrap_or(0.0) + 40.0);
        let Some((first, last)) = visible_page_window(&offsets, viewport.min.y, viewport.max.y, page_count) else { return };
        session.current_page = offsets.partition_point(|offset| *offset <= viewport.center().y).saturating_sub(1).min(page_count.saturating_sub(1) as usize) as u32;
        for page in first..=last {
            let aspect = session.page_aspects.get(&page).copied().unwrap_or(1.414);
            let height = page_width * aspect;
            let x = ui.max_rect().left() + ((ui.available_width() - page_width) * 0.5).max(12.0);
            let y = ui.max_rect().top() + 20.0 + offsets[page as usize];
            let rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(page_width, height));
            ui.painter().add(design::card_shadow(palette.shadow, false).as_shape(rect, 3));
            ui.painter().rect_filled(rect, 2.0, Color32::WHITE);
            ui.painter().rect_stroke(rect, 2.0, Stroke::new(1.0, palette.separator), egui::StrokeKind::Outside);
            if let Some(entry) = session.textures.get_mut(&page) {
                entry.last_used = tick;
                ui.put(rect, egui::Image::new(&entry.texture).fit_to_exact_size(rect.size()));
            } else { ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, format!("Rendering page {}…", page + 1), FontId::proportional(13.0), Color32::GRAY); requests.push((page, if page == session.current_page { RequestPriority::Visible } else { RequestPriority::Nearby })); }
            if session.rotation == 0 {
                if let Some(raw) = session.raw_pages.get(&page) {
                    for annotation in session.annotations.iter().filter(|annotation| annotation.start.page == page && annotation.kind == AnnotationKind::Highlight) {
                        for glyph in raw.glyphs.iter().filter(|glyph| char_range_contains(annotation.start.char_index, annotation.end.char_index, glyph.source.char_index)) {
                            if let Some(glyph_rect) = map_page_rect(rect, raw.width, raw.height, glyph.bounds) {
                                ui.painter().rect_filled(glyph_rect, 1.0, Color32::from_rgba_unmultiplied(255, 220, 80, 75));
                            }
                        }
                    }
                    for link in &raw.links {
                        if let Some(link_rect) = map_page_rect(rect, raw.width, raw.height, link.bounds) {
                            let response = ui.put(link_rect, egui::Button::new("").frame(false));
                            if response.clicked() { followed_link = Some(link.target.clone()); }
                            response.on_hover_cursor(egui::CursorIcon::PointingHand);
                        }
                    }
                }
            }
            if session.scroll_to_page == Some(page) { ui.scroll_to_rect(rect, Some(Align::Center)); session.scroll_to_page = None; }
        }
    });
    requests.sort_by_key(|(_, priority)| *priority); requests.dedup_by_key(|(page, _)| *page); (requests, followed_link)
}

fn map_page_rect(screen: egui::Rect, page_width: f32, page_height: f32, bounds: crate::document::PageRect) -> Option<egui::Rect> {
    if page_width <= 0.0 || page_height <= 0.0 || !page_width.is_finite() || !page_height.is_finite()
        || ![bounds.left, bounds.top, bounds.right, bounds.bottom].into_iter().all(f32::is_finite) {
        return None;
    }
    let left = screen.left() + bounds.left / page_width * screen.width();
    let right = screen.left() + bounds.right / page_width * screen.width();
    let top = screen.top() + (page_height - bounds.top) / page_height * screen.height();
    let bottom = screen.top() + (page_height - bounds.bottom) / page_height * screen.height();
    Some(egui::Rect::from_min_max(egui::pos2(left.min(right), top.min(bottom)), egui::pos2(left.max(right), top.max(bottom))))
}

fn reflow_view(ui: &mut egui::Ui, session: &mut DocumentSession, style: &ReaderStyle) {
    let paper_padding = 32.0;
    let workspace_gutter = style.horizontal_margin.max(20.0);
    let max_width = style.content_max_width.min((ui.available_width() - workspace_gutter * 2.0 - paper_padding * 2.0).max(280.0));
    let colors = ReaderPalette::for_theme(style.theme);
    let font = if style.font_family == "Monospace" { egui::FontFamily::Monospace } else if style.font_family == "System sans-serif" { egui::FontFamily::Proportional } else { egui::FontFamily::Name("serif".into()) };
    let mut hasher = DefaultHasher::new();
    max_width.to_bits().hash(&mut hasher); style.font_family.hash(&mut hasher); style.font_size.to_bits().hash(&mut hasher);
    style.line_height.to_bits().hash(&mut hasher); style.letter_spacing.to_bits().hash(&mut hasher); style.paragraph_spacing.to_bits().hash(&mut hasher);
    (style.alignment == TextAlignment::Center).hash(&mut hasher);
    (style.theme as u8).hash(&mut hasher);
    let signature = hasher.finish();
    if session.reflow_layout_signature != signature {
        session.reflow_layout_signature = signature;
        session.reflow_layouts.clear();
        if session.anchor.block_id != 0 { session.scroll_to_block = Some(session.anchor.block_id); }
    }
    let blocks = &session.document.blocks;
    let heights: Vec<f32> = blocks.iter().map(|block| {
        session.reflow_layouts.get(&block.id).map(ReflowLayout::total_height).unwrap_or_else(|| estimate_reflow_height(block, style, max_width))
    }).collect();
    let mut offsets = Vec::with_capacity(heights.len() + 1);
    offsets.push(0.0);
    for height in &heights { offsets.push(offsets.last().copied().unwrap_or(0.0) + height); }
    let document_inset = 56.0;
    let target_offset = session.scroll_to_block
        .and_then(|id| reflow_restore_offset(blocks, &offsets, (id, 0.0)))
        .or_else(|| session.reflow_scroll_restore.and_then(|restore| reflow_restore_offset(blocks, &offsets, restore)))
        .map(|offset| offset + document_inset);
    if target_offset.is_some() { session.scroll_to_block = None; session.reflow_scroll_restore = None; }
    let scroll = egui::ScrollArea::vertical().id_salt(("reflow-scroll", session.tab_id));
    let scroll = if let Some(offset) = target_offset { scroll.vertical_scroll_offset(offset) } else { scroll };
    scroll.show_viewport(ui, |ui, viewport| {
        let top_gutter = 24.0;
        let bottom_gutter = 24.0;
        let document_height = offsets.last().copied().unwrap_or(0.0) + paper_padding * 2.0;
        ui.set_min_height(document_height + top_gutter + bottom_gutter);
        let paper_width = max_width + paper_padding * 2.0;
        let paper_left = ui.max_rect().left() + ((ui.available_width() - paper_width) * 0.5).max(20.0);
        let paper = egui::Rect::from_min_size(egui::pos2(paper_left, ui.max_rect().top() + top_gutter), egui::vec2(paper_width, document_height));
        ui.painter().add(design::card_shadow(Color32::from_black_alpha(24), false).as_shape(paper, 10));
        ui.painter().rect_filled(paper, 10.0, colors.surface);
        ui.painter().rect_stroke(paper, 10.0, Stroke::new(1.0, colors.muted.gamma_multiply(0.18)), egui::StrokeKind::Inside);
        let content_top = paper.top() + paper_padding;
        let relative_view_min = (viewport.min.y - top_gutter - paper_padding).max(0.0);
        let relative_view_max = (viewport.max.y - top_gutter - paper_padding).max(0.0);
        let start = offsets.partition_point(|offset| *offset < relative_view_min).saturating_sub(1);
        let end = offsets.partition_point(|offset| *offset <= relative_view_max + 240.0).min(blocks.len());
        let measure_start = start.saturating_sub(3);
        let measure_end = (end + 3).min(blocks.len());
        let mut measurement_changed = false;
        for block in &blocks[measure_start..measure_end] {
            if session.reflow_layouts.contains_key(&block.id) { continue; }
            session.reflow_layouts.insert(block.id, build_reflow_layout(ui.ctx(), block, style, &colors, max_width, &font));
            measurement_changed = true;
        }
        if measurement_changed {
            let top_index = offsets.partition_point(|offset| *offset <= relative_view_min).saturating_sub(1).min(blocks.len().saturating_sub(1));
            if let Some(block) = blocks.get(top_index) {
                session.reflow_scroll_restore = Some((block.id, (relative_view_min - offsets[top_index]).max(0.0)));
            }
            ui.ctx().request_repaint();
        }
        if !blocks.is_empty() {
            let center = (viewport.center().y - top_gutter - paper_padding).max(0.0);
            let visible_index = offsets.partition_point(|offset| *offset <= center).saturating_sub(1).min(blocks.len() - 1);
            if let Some(source) = blocks[visible_index].source.first() {
                session.current_page = source.page;
                session.anchor = ReadingAnchor { block_id: blocks[visible_index].id, char_offset: 0, source: Some(SourcePosition { page: source.page, char_index: source.start_char }), quote: blocks[visible_index].content.chars().take(64).collect() };
            }
        }
        for index in start..end {
            let block = &blocks[index];
            let Some(layout) = session.reflow_layouts.get(&block.id) else { continue };
            let highlighted = session.annotations.iter().any(|annotation| annotation.kind == AnnotationKind::Highlight && block.source.iter().any(|source| {
                source.page == annotation.start.page
                    && half_open_ranges_overlap(source.start_char, source.end_char, annotation.start.char_index, annotation.end.char_index)
            }));
            let rect = egui::Rect::from_min_size(egui::pos2(paper.left() + paper_padding, content_top + offsets[index] + layout.before_spacing), egui::vec2(max_width, layout.content_height));
            if highlighted { ui.painter().rect_filled(rect, 4.0, colors.highlight); }
            if matches!(block.kind, BlockKind::Code) { ui.painter().rect_filled(rect.expand2(egui::vec2(10.0, 4.0)), 7.0, colors.code); }
            if matches!(block.kind, BlockKind::Quote) {
                ui.painter().line_segment([rect.left_top() - egui::vec2(10.0, 0.0), rect.left_bottom() - egui::vec2(10.0, 0.0)], Stroke::new(3.0, colors.quote));
            }
            if matches!(block.kind, BlockKind::ListItem) { ui.painter().circle_filled(rect.left_top() + egui::vec2(-10.0, style.font_size * style.line_height * 0.5), 2.5, colors.quote); }
            let response = ui.put(rect, egui::Label::new(layout.galley.clone()).selectable(true).sense(Sense::click()));
            if response.clicked() {
                session.selected_block = Some(block.id);
                if let Some(source) = block.source.first() {
                    session.current_page = source.page;
                    session.anchor = ReadingAnchor { block_id: block.id, char_offset: 0, source: Some(SourcePosition { page: source.page, char_index: source.start_char }), quote: block.content.chars().take(64).collect() };
                }
            }
        }
    });
}

fn block_spacing(kind: BlockKind, paragraph_spacing: f32) -> (f32, f32) {
    match kind {
        BlockKind::Heading { level: 1 } => (paragraph_spacing * 1.1, paragraph_spacing * 0.65),
        BlockKind::Heading { .. } => (paragraph_spacing * 0.8, paragraph_spacing * 0.45),
        BlockKind::ListItem => (0.0, paragraph_spacing * 0.5),
        BlockKind::Quote => (paragraph_spacing * 0.2, paragraph_spacing * 0.8),
        BlockKind::Code | BlockKind::Figure => (paragraph_spacing * 0.35, paragraph_spacing * 0.85),
        BlockKind::Paragraph | BlockKind::Unknown => (0.0, paragraph_spacing),
    }
}

fn block_font_size(kind: BlockKind, base: f32) -> f32 {
    match kind { BlockKind::Heading { level: 1 } => base * 1.6, BlockKind::Heading { .. } => base * 1.26, BlockKind::Code => base * 0.92, _ => base }
}

fn estimate_reflow_height(block: &Block, style: &ReaderStyle, width: f32) -> f32 {
    let size = block_font_size(block.kind, style.font_size);
    let characters_per_line = (width / (size * 0.54 + style.letter_spacing.max(0.0))).max(8.0);
    let lines = (block.content.chars().count() as f32 / characters_per_line).ceil().max(1.0);
    let (before, after) = block_spacing(block.kind, style.paragraph_spacing);
    before + lines * (size * style.line_height).round() + after
}

fn build_reflow_layout(ctx: &egui::Context, block: &Block, style: &ReaderStyle, colors: &ReaderPalette, width: f32, family: &egui::FontFamily) -> ReflowLayout {
    let size = block_font_size(block.kind, style.font_size);
    let family = if matches!(block.kind, BlockKind::Code) { egui::FontFamily::Monospace } else { family.clone() };
    let color = if matches!(block.kind, BlockKind::Quote) { colors.quote } else if matches!(block.kind, BlockKind::Figure) { colors.muted } else { colors.text };
    let mut job = LayoutJob::simple_format(block.content.clone(), TextFormat {
        font_id: FontId::new(size, family),
        extra_letter_spacing: style.letter_spacing,
        line_height: Some((size * style.line_height).round()),
        color,
        ..Default::default()
    });
    job.wrap.max_width = width;
    job.halign = if style.alignment == TextAlignment::Center { Align::Center } else { Align::LEFT };
    let galley = ctx.fonts_mut(|fonts| fonts.layout_job(job));
    let (before_spacing, after_spacing) = block_spacing(block.kind, style.paragraph_spacing);
    ReflowLayout { content_height: galley.size().y, galley, before_spacing, after_spacing }
}

fn reflow_restore_offset(blocks: &[Block], offsets: &[f32], restore: (u64, f32)) -> Option<f32> {
    blocks.iter().position(|block| block.id == restore.0).and_then(|index| offsets.get(index).map(|offset| offset + restore.1.max(0.0)))
}

fn library_home(
    ui: &mut egui::Ui,
    library: &[LibraryEntry],
    covers: &HashMap<String, LibraryCover>,
    palette: Palette,
    query: &mut String,
    sort: &mut LibrarySort,
    reduced_motion: bool,
) -> (bool, Option<PathBuf>) {
    let mut open = false;
    let mut selected = None;
    egui::ScrollArea::vertical().id_salt("library-home").show(ui, |ui| {
        let outer_width = ui.available_width();
        let content_width = (outer_width - 48.0).clamp(320.0, 1320.0);
        let side = ((outer_width - content_width) * 0.5).max(24.0);
        ui.horizontal(|ui| {
            ui.add_space(side);
            ui.vertical(|ui| {
                ui.set_width(content_width);
                ui.add_space(28.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Library").font(design::display_font(32.0)).color(palette.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add_sized([148.0, 38.0], egui::Button::image_and_text(Icon::Open.image(17.0), "Open document").image_tint_follows_text_color(true)).clicked() { open = true; }
                    });
                });
                ui.add_space(24.0);
                if library.is_empty() {
                    let width = content_width.min(560.0);
                    ui.horizontal(|ui| { ui.add_space(((content_width - width) * 0.5).max(0.0));
                        egui::Frame::new().fill(palette.surface_high).stroke(Stroke::new(1.0, palette.separator)).corner_radius(12).inner_margin(egui::Margin::same(28)).show(ui, |ui| {
                            ui.set_width(width - 56.0);
                            ui.with_layout(Layout::top_down(Align::Center), |ui| {
                                ui.add(Icon::Library.image(34.0).tint(palette.accent));
                                ui.label(RichText::new("A quiet place for your books").font(design::display_font(20.0)));
                                ui.label(RichText::new("Open a document or drop one anywhere in this window.").color(palette.muted));
                                ui.add_space(8.0);
                                if ui.add(egui::Button::image_and_text(Icon::Open.image(17.0), "Choose a document").image_tint_follows_text_color(true)).clicked() { open = true; }
                            });
                        });
                    });
                } else {
                    if let Some(entry) = library.iter().find(|entry| !entry.missing && entry.progress > 0.0).or_else(|| library.iter().find(|entry| !entry.missing)) {
                        section_label(ui, "Continue reading", palette);
                        ui.add_space(8.0);
                        let rect = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(content_width.min(1080.0), 164.0));
                        let response = ui.scope_builder(egui::UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)), |card_ui| {
                            continue_reading_card(card_ui, rect, entry, covers.get(&entry.document_id), palette, reduced_motion)
                        }).inner;
                        if response.clicked() { selected = Some(entry.path.clone()); }
                        ui.add_space(28.0);
                    }

                    ui.horizontal(|ui| {
                        section_label(ui, "Collection", palette);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.add_sized([62.0, 34.0], egui::Button::selectable(*sort == LibrarySort::Title, "Title")).clicked() { *sort = LibrarySort::Title; }
                            if ui.add_sized([70.0, 34.0], egui::Button::selectable(*sort == LibrarySort::Recent, "Recent")).clicked() { *sort = LibrarySort::Recent; }
                        });
                    });
                    ui.add_space(10.0);
                    ui.add_sized([content_width, 36.0], egui::TextEdit::singleline(query).hint_text("Filter by title or author…"));
                    ui.add_space(20.0);

                    let needle = query.trim().to_lowercase();
                    let mut entries: Vec<_> = library.iter().filter(|entry| {
                        let display = DisplayMetadata::new(&entry.title, &entry.author);
                        needle.is_empty() || display.title.to_lowercase().contains(&needle) || display.author.as_deref().unwrap_or_default().to_lowercase().contains(&needle)
                    }).collect();
                    if *sort == LibrarySort::Title { entries.sort_by_key(|entry| DisplayMetadata::new(&entry.title, &entry.author).title.to_lowercase()); }
                    if entries.is_empty() {
                        ui.add_space(40.0);
                        ui.vertical_centered(|ui| { ui.label(RichText::new("No books match that filter.").font(design::display_font(18.0))); ui.label(RichText::new("Try a shorter title or author name.").color(palette.muted)); });
                    } else {
                        let card_width = design::LIBRARY_CARD_WIDTH;
                        let card_height = design::LIBRARY_CARD_HEIGHT;
                        let column_gap = 24.0;
                        let row_gap = 24.0;
                        let row_height = card_height + row_gap;
                        let columns = library_column_count(content_width, card_width, column_gap);
                        let rows = entries.len().div_ceil(columns);
                        let (grid_rect, _) = ui.allocate_exact_size(egui::vec2(content_width, rows as f32 * row_height), Sense::hover());
                        let first_row = (((ui.clip_rect().top() - grid_rect.top()) / row_height).floor() as isize).max(0) as usize;
                        let last_row = (((ui.clip_rect().bottom() - grid_rect.top()) / row_height).ceil() as isize).max(0) as usize;
                        for row in first_row..last_row.min(rows) {
                            for column in 0..columns {
                                let index = row * columns + column;
                                let Some(entry) = entries.get(index) else { break };
                                let rect = egui::Rect::from_min_size(
                                    grid_rect.min + egui::vec2(column as f32 * (card_width + column_gap), row as f32 * row_height),
                                    egui::vec2(card_width, card_height),
                                );
                                if library_card(ui, rect, entry, covers.get(&entry.document_id), palette, reduced_motion).clicked() && !entry.missing { selected = Some(entry.path.clone()); }
                            }
                        }
                    }
                }
                ui.add_space(36.0);
            });
        });
    });
    (open, selected)
}

fn library_column_count(width: f32, card_width: f32, gap: f32) -> usize {
    ((width + gap) / (card_width + gap)).floor().max(1.0) as usize
}

fn section_label(ui: &mut egui::Ui, text: &str, palette: Palette) {
    ui.label(RichText::new(text.to_uppercase()).size(11.0).strong().extra_letter_spacing(0.8).color(palette.muted));
}

fn continue_reading_card(ui: &mut egui::Ui, rect: egui::Rect, entry: &LibraryEntry, cover: Option<&LibraryCover>, palette: Palette, reduced_motion: bool) -> egui::Response {
    let response = ui.interact(rect, ui.id().with(("continue-reading", &entry.document_id)), Sense::click());
    ui.painter().add(design::card_shadow(palette.shadow, response.hovered()).as_shape(rect, design::CARD_RADIUS));
    ui.painter().rect_filled(rect, design::CARD_RADIUS, palette.surface_high);
    ui.painter().rect_stroke(rect, design::CARD_RADIUS, Stroke::new(1.0, if response.hovered() { palette.accent } else { palette.separator }), egui::StrokeKind::Inside);
    let cover_rect = egui::Rect::from_min_size(rect.min + egui::vec2(16.0, 16.0), egui::vec2(88.0, 132.0));
    paint_cover(ui, entry, cover, cover_rect, palette, reduced_motion);
    let display = DisplayMetadata::new(&entry.title, &entry.author);
    let text_left = cover_rect.right() + 22.0;
    let title_rect = egui::Rect::from_min_max(egui::pos2(text_left, rect.top() + 24.0), egui::pos2(rect.right() - 24.0, rect.top() + 78.0));
    let title_galley = ui.painter().layout(display.title.clone(), design::display_font(21.0), palette.text, title_rect.width());
    ui.painter().with_clip_rect(title_rect).galley(title_rect.min, title_galley, palette.text);
    let title_hover = ui.interact(title_rect, ui.id().with(("continue-title", &entry.document_id)), Sense::hover());
    title_hover.on_hover_text(&display.original_title);
    if let Some(author) = &display.author {
        let author_rect = egui::Rect::from_min_max(egui::pos2(text_left, rect.top() + 78.0), egui::pos2(rect.right() - 24.0, rect.top() + 101.0));
        ui.painter().with_clip_rect(author_rect).text(author_rect.left_top(), egui::Align2::LEFT_TOP, author, design::ui_font(13.0), palette.muted);
    }
    let meta = format!("{}  ·  {}% complete", entry.format.as_str().to_ascii_uppercase(), (entry.progress.clamp(0.0, 1.0) * 100.0).round() as u32);
    ui.painter().text(egui::pos2(text_left, rect.bottom() - 39.0), egui::Align2::LEFT_BOTTOM, meta, design::ui_font(11.0), palette.muted);
    paint_progress(ui, egui::Rect::from_min_size(egui::pos2(text_left, rect.bottom() - 25.0), egui::vec2((rect.right() - text_left - 24.0).min(460.0), 5.0)), entry.progress, palette);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn library_card(ui: &mut egui::Ui, rect: egui::Rect, entry: &LibraryEntry, cover: Option<&LibraryCover>, palette: Palette, reduced_motion: bool) -> egui::Response {
    let response = ui.interact(rect, ui.id().with(("library-card-hit", &entry.document_id)), Sense::click());
    let hover = design::animate_bool(ui.ctx(), ui.id().with(("library-card", &entry.document_id)), response.hovered(), design::MOTION_CARD, reduced_motion);
    let card = rect.translate(egui::vec2(0.0, -2.0 * hover));
    ui.painter().add(design::card_shadow(palette.shadow.gamma_multiply(0.65 + hover * 0.35), response.hovered()).as_shape(card, design::CARD_RADIUS));
    ui.painter().rect_filled(card, design::CARD_RADIUS, palette.surface_high);
    ui.painter().rect_stroke(card, design::CARD_RADIUS, Stroke::new(1.0, if response.hovered() { palette.accent } else { palette.separator }), egui::StrokeKind::Inside);
    let cover_rect = egui::Rect::from_min_size(card.min + egui::vec2(12.0, 12.0), egui::vec2(160.0, 240.0));
    paint_cover(ui, entry, cover, cover_rect, palette, reduced_motion);
    let display = DisplayMetadata::new(&entry.title, &entry.author);
    let title_rect = egui::Rect::from_min_max(egui::pos2(card.left() + 12.0, cover_rect.bottom() + 9.0), egui::pos2(card.right() - 12.0, cover_rect.bottom() + 49.0));
    ui.put(title_rect, egui::Label::new(RichText::new(&display.title).font(design::display_font(15.0)).color(palette.text)).wrap().truncate()).on_hover_text(&display.original_title);
    if let Some(author) = &display.author {
        let author_rect = egui::Rect::from_min_max(egui::pos2(card.left() + 12.0, cover_rect.bottom() + 51.0), egui::pos2(card.right() - 12.0, cover_rect.bottom() + 70.0));
        ui.put(author_rect, egui::Label::new(RichText::new(author).size(11.0).color(palette.muted)).truncate());
    }
    let badge = if entry.missing { format!("{}  ·  FILE MISSING", entry.format.as_str().to_ascii_uppercase()) } else { entry.format.as_str().to_ascii_uppercase() };
    ui.painter().text(egui::pos2(card.left() + 12.0, card.bottom() - 18.0), egui::Align2::LEFT_BOTTOM, badge, design::ui_font(10.0), if entry.missing { palette.danger } else { palette.accent });
    paint_progress(ui, egui::Rect::from_min_size(egui::pos2(card.left() + 12.0, card.bottom() - 10.0), egui::vec2(160.0, 4.0)), entry.progress, palette);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn paint_progress(ui: &egui::Ui, rect: egui::Rect, progress: f32, palette: Palette) {
    ui.painter().rect_filled(rect, 2.0, palette.separator);
    let filled = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * progress.clamp(0.0, 1.0), rect.height()));
    ui.painter().rect_filled(filled, 2.0, palette.accent);
}

fn paint_cover(ui: &mut egui::Ui, entry: &LibraryEntry, cover: Option<&LibraryCover>, rect: egui::Rect, palette: Palette, reduced_motion: bool) {
    ui.painter().rect_filled(rect, 7.0, palette.accent_soft);
    if let Some(cover) = cover {
        let target_aspect = rect.height() / rect.width();
        let size = if cover.aspect > target_aspect { egui::vec2(rect.height() / cover.aspect, rect.height()) } else { egui::vec2(rect.width(), rect.width() * cover.aspect) };
        let image_rect = egui::Rect::from_center_size(rect.center(), size);
        let alpha = if reduced_motion { 1.0 } else { (cover.loaded_at.elapsed().as_secs_f32() / design::MOTION_SELECTION).clamp(0.0, 1.0) };
        if alpha < 1.0 { ui.ctx().request_repaint(); }
        ui.put(image_rect, egui::Image::new(&cover.texture).fit_to_exact_size(size).tint(Color32::WHITE.gamma_multiply(alpha)));
    } else {
        let display = DisplayMetadata::new(&entry.title, &entry.author);
        let seed = entry.document_id.bytes().fold(0_u32, |value, byte| value.wrapping_mul(33).wrapping_add(u32::from(byte)));
        let ink = if seed % 2 == 0 { palette.text } else { palette.accent };
        ui.painter().line_segment([rect.left_top() + egui::vec2(14.0, 18.0), rect.right_top() + egui::vec2(-14.0, 18.0)], Stroke::new(2.0, ink.gamma_multiply(0.55)));
        let title_rect = rect.shrink2(egui::vec2(16.0, 34.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(title_rect).layout(Layout::top_down(Align::Center)), |ui| {
            ui.add_space(18.0);
            ui.add(egui::Label::new(RichText::new(display.title.chars().take(54).collect::<String>()).font(design::display_font(17.0)).color(ink)).wrap().halign(Align::Center));
            ui.add_space(10.0);
            ui.label(RichText::new(display.author.as_deref().unwrap_or("KOIFLOW EDITION")).size(10.0).color(palette.muted));
        });
    }
    ui.painter().rect_stroke(rect, 7.0, Stroke::new(1.0, palette.separator), egui::StrokeKind::Inside);
}

fn navigate_to_page(session: &mut DocumentSession, page: u32) {
    session.current_page = clamp_page_target(page, session.page_count());
    session.scroll_to_page = Some(session.current_page);
    if session.mode == ReadingMode::Reflow {
        session.scroll_to_block = session.document.blocks.iter().find(|block| block.source.first().is_some_and(|source| source.page >= session.current_page)).map(|block| block.id);
    }
}

fn sync_reflow_target(session: &mut DocumentSession) {
    if session.mode != ReadingMode::Reflow { return; }
    let Some(page) = session.scroll_to_page else { return };
    if let Some(block_id) = block_for_page(&session.document, page) {
        session.scroll_to_block = Some(block_id);
        session.scroll_to_page = None;
    }
}

fn block_for_page(document: &Document, page: u32) -> Option<u64> {
    document.blocks.iter()
        .filter_map(|block| block.source.first().map(|source| (block.id, source.page)))
        .find(|(_, source_page)| *source_page >= page)
        .map(|(block_id, _)| block_id)
}

fn clamp_page_target(page: u32, page_count: u32) -> u32 {
    if page_count == 0 { page } else { page.min(page_count - 1) }
}

fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("koiflow-sans".into(), egui::FontData::from_static(dejavu::sans::regular()).into());
    fonts.font_data.insert("koiflow-serif".into(), egui::FontData::from_static(dejavu::serif::regular()).into());
    fonts.families.insert(egui::FontFamily::Name("koiflow-sans".into()), vec!["koiflow-sans".into()]);
    fonts.families.insert(egui::FontFamily::Name("koiflow-serif".into()), vec!["koiflow-serif".into(), "koiflow-sans".into()]);
    fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "koiflow-sans".into());
    fonts.families.insert(egui::FontFamily::Name("serif".into()), vec!["koiflow-serif".into(), "koiflow-sans".into()]);
    ctx.set_fonts(fonts);
}

fn settings_heading(ui: &mut egui::Ui, text: &str) { ui.label(RichText::new(text).size(11.0).strong().color(ui.visuals().weak_text_color())); }
fn slider(ui: &mut egui::Ui, value: &mut f32, range: std::ops::RangeInclusive<f32>, label: &str, suffix: &str) { ui.label(label); ui.add(egui::Slider::new(value, range).suffix(suffix)); }

fn page_result_matches_rotation(current_rotation: u16, result_rotation: u16, has_bitmap: bool) -> bool {
    !has_bitmap || current_rotation % 360 == result_rotation % 360
}

fn char_range_contains(start: u32, end: u32, index: u32) -> bool {
    start <= index && index < end
}

fn half_open_ranges_overlap(first_start: u32, first_end: u32, second_start: u32, second_end: u32) -> bool {
    first_start < first_end && second_start < second_end && first_start < second_end && second_start < first_end
}

fn visible_page_window(offsets: &[f32], viewport_min: f32, viewport_max: f32, page_count: u32) -> Option<(u32, u32)> {
    if page_count == 0 || offsets.len() < page_count as usize + 1 || !viewport_min.is_finite() || !viewport_max.is_finite() {
        return None;
    }
    let first = offsets.partition_point(|offset| *offset < viewport_min).saturating_sub(3).min(page_count as usize - 1) as u32;
    let last = (offsets.partition_point(|offset| *offset <= viewport_max) + 2).min(page_count as usize).saturating_sub(1) as u32;
    (first <= last).then_some((first, last))
}

fn page_failure_matches(current_document_id: Option<&str>, current_rotation: u16, document_id: &str, result_rotation: u16, render: bool) -> bool {
    current_document_id == Some(document_id) && page_result_matches_rotation(current_rotation, result_rotation, render)
}

fn page_request_is_redundant(pending: &std::collections::HashSet<PageRequest>, request: PageRequest) -> bool {
    pending.contains(&request) || (!request.render && pending.iter().any(|existing| existing.page == request.page))
}

fn next_unextracted_page(
    page_count: u32,
    extracted: &std::collections::HashSet<u32>,
    pending: &std::collections::HashSet<PageRequest>,
    failed: &std::collections::HashSet<PageRequest>,
) -> Option<u32> {
    (0..page_count).find(|page| {
        !extracted.contains(page)
            && !pending.iter().any(|request| request.page == *page)
            && !failed.iter().any(|request| !request.render && request.page == *page)
    })
}

#[cfg(test)]
mod tests {
    use super::{DisplayMetadata, block_for_page, build_reflow_layout, char_range_contains, clamp_page_target, configure_fonts, half_open_ranges_overlap, library_column_count, map_page_rect, next_unextracted_page, page_failure_matches, page_request_is_redundant, page_result_matches_rotation, reflow_restore_offset, visible_page_window};
    use super::session::PageRequest;
    use crate::document::{Block, BlockKind, Document, SourceRange};
    use crate::reading::ReaderStyle;
    use super::ReaderPalette;
    use std::collections::HashSet;

    #[test]
    fn stale_rotated_bitmaps_are_rejected_but_geometry_is_rotation_independent() {
        assert!(page_result_matches_rotation(90, 450, true));
        assert!(!page_result_matches_rotation(90, 0, true));
        assert!(page_result_matches_rotation(90, 0, false));
        assert!(!page_failure_matches(Some("doc"), 90, "doc", 0, true));
        assert!(page_failure_matches(Some("doc"), 90, "doc", 0, false));
        assert!(!page_failure_matches(Some("other"), 90, "doc", 90, true));
    }

    #[test]
    fn pending_search_page_survives_open_then_clamps_to_document_bounds() {
        assert_eq!(clamp_page_target(37, 0), 37);
        assert_eq!(clamp_page_target(37, 100), 37);
        assert_eq!(clamp_page_target(137, 100), 99);
    }

    #[test]
    fn reflow_navigation_ignores_unmapped_blocks_and_finds_target_page() {
        let document = Document {
            blocks: vec![
                Block { id: 1, kind: BlockKind::Paragraph, content: "unmapped".into(), source: vec![], confidence: 0.0 },
                Block { id: 2, kind: BlockKind::Paragraph, content: "page two".into(), source: vec![SourceRange { page: 2, start_char: 0, end_char: 8 }], confidence: 1.0 },
                Block { id: 3, kind: BlockKind::Paragraph, content: "page four".into(), source: vec![SourceRange { page: 4, start_char: 0, end_char: 9 }], confidence: 1.0 },
            ],
            ..Document::default()
        };
        assert_eq!(block_for_page(&document, 3), Some(3));
        assert_eq!(block_for_page(&document, 5), None);
    }

    #[test]
    fn measured_reflow_restores_the_same_intra_block_position() {
        let blocks = vec![
            Block { id: 11, kind: BlockKind::Paragraph, content: "first".into(), source: vec![], confidence: 1.0 },
            Block { id: 12, kind: BlockKind::Paragraph, content: "second".into(), source: vec![], confidence: 1.0 },
        ];
        assert_eq!(reflow_restore_offset(&blocks, &[0.0, 180.0, 400.0], (12, 37.5)), Some(217.5));
        assert_eq!(reflow_restore_offset(&blocks, &[0.0, 180.0, 400.0], (99, 10.0)), None);
    }

    #[test]
    fn display_metadata_cleans_download_names_without_mutating_genuine_titles() {
        let isbn = DisplayMetadata::new("[9780262270830] Introduction to Algorithms, third edition.pdf", "Thomas H. Cormen");
        assert_eq!(isbn.title, "Introduction to Algorithms, third edition");
        assert_eq!(isbn.author.as_deref(), Some("Thomas H. Cormen"));
        assert_eq!(isbn.original_title, "[9780262270830] Introduction to Algorithms, third edition.pdf");

        let archive = DisplayMetadata::new("Mike Ma - Harassment Architecture (2019, Mike Ma) - libgen.li", "");
        assert_eq!(archive.title, "Harassment Architecture");
        assert_eq!(archive.author.as_deref(), Some("Mike Ma"));

        assert_eq!(DisplayMetadata::new("War - Peace", "").title, "War - Peace");
        assert_eq!(DisplayMetadata::new("  百年孤独  ", "").title, "百年孤独");
        assert_eq!(DisplayMetadata::new("", "").title, "Untitled");
    }

    #[test]
    fn library_columns_are_adaptive_and_never_zero() {
        assert_eq!(library_column_count(1320.0, 184.0, 24.0), 6);
        assert_eq!(library_column_count(1000.0, 184.0, 24.0), 4);
        assert_eq!(library_column_count(120.0, 184.0, 24.0), 1);
    }

    #[test]
    fn shaped_reflow_layout_applies_block_spacing_once() {
        let ctx = eframe::egui::Context::default();
        configure_fonts(&ctx);
        let style = ReaderStyle::default();
        let colors = ReaderPalette::for_theme(style.theme);
        let block = Block { id: 1, kind: BlockKind::Paragraph, content: "A paragraph that wraps across the available measure and remains readable.".into(), source: vec![], confidence: 1.0 };
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let layout = build_reflow_layout(ui.ctx(), &block, &style, &colors, 240.0, &eframe::egui::FontFamily::Name("serif".into()));
            assert!((layout.content_height - layout.galley.size().y).abs() < f32::EPSILON);
            assert!((layout.total_height() - layout.content_height - layout.before_spacing - layout.after_spacing).abs() < f32::EPSILON);
            assert_eq!(layout.before_spacing, 0.0);
            assert_eq!(layout.after_spacing, style.paragraph_spacing);
        });
        output.textures_delta.clear();
    }

    #[test]
    fn background_extraction_does_not_suppress_visible_rendering() {
        let extraction = PageRequest { page: 8, rotation: 0, render: false };
        let render = PageRequest { page: 8, rotation: 0, render: true };
        let rotated_render = PageRequest { page: 8, rotation: 90, render: true };
        let pending = HashSet::from([extraction]);
        assert!(!page_request_is_redundant(&pending, render));
        assert!(!page_request_is_redundant(&pending, rotated_render));

        let pending = HashSet::from([render]);
        assert!(page_request_is_redundant(&pending, render));
        assert!(page_request_is_redundant(&pending, extraction));
    }

    #[test]
    fn failed_extraction_does_not_stall_later_pages_or_retry_forever() {
        let extracted = HashSet::from([0]);
        let pending = HashSet::from([PageRequest { page: 2, rotation: 0, render: false }]);
        let failed = HashSet::from([
            PageRequest { page: 1, rotation: 0, render: false },
            PageRequest { page: 3, rotation: 0, render: true },
        ]);
        assert_eq!(next_unextracted_page(5, &extracted, &pending, &failed), Some(3));

        let pending = HashSet::new();
        assert_eq!(next_unextracted_page(3, &extracted, &pending, &failed), Some(2));
        let extracted = HashSet::from([0, 2]);
        assert_eq!(next_unextracted_page(3, &extracted, &pending, &failed), None);
    }

    #[test]
    fn page_geometry_mapping_rejects_invalid_dimensions() {
        let screen = eframe::egui::Rect::from_min_size(eframe::egui::pos2(10.0, 20.0), eframe::egui::vec2(200.0, 400.0));
        let bounds = crate::document::PageRect { left: 25.0, right: 75.0, bottom: 20.0, top: 80.0 };
        let mapped = map_page_rect(screen, 100.0, 100.0, bounds).unwrap();
        assert_eq!(mapped.min, eframe::egui::pos2(60.0, 100.0));
        assert_eq!(mapped.max, eframe::egui::pos2(160.0, 340.0));
        assert!(map_page_rect(screen, 0.0, 100.0, bounds).is_none());
        assert!(map_page_rect(screen, f32::NAN, 100.0, bounds).is_none());
    }

    #[test]
    fn annotation_ranges_are_half_open_at_block_and_glyph_boundaries() {
        assert!(!char_range_contains(10, 20, 9));
        assert!(char_range_contains(10, 20, 10));
        assert!(char_range_contains(10, 20, 19));
        assert!(!char_range_contains(10, 20, 20));
        assert!(!char_range_contains(10, 10, 10));

        assert!(half_open_ranges_overlap(0, 10, 9, 12));
        assert!(!half_open_ranges_overlap(0, 10, 10, 12));
        assert!(!half_open_ranges_overlap(10, 20, 0, 10));
        assert!(!half_open_ranges_overlap(4, 4, 0, 10));
        assert!(!half_open_ranges_overlap(8, 4, 0, 10));
        assert!(!half_open_ranges_overlap(0, 10, 8, 4));
    }

    #[test]
    fn thousand_page_viewport_only_materializes_a_small_window() {
        let offsets: Vec<_> = (0..=1000).map(|page| page as f32 * 1028.0).collect();
        let (first, last) = visible_page_window(&offsets, 500_000.0, 500_900.0, 1000).unwrap();
        assert!(first <= 486 && last >= 486);
        assert!(last - first + 1 <= 8);
        assert_eq!(visible_page_window(&offsets, f32::NAN, 900.0, 1000), None);
        assert_eq!(visible_page_window(&[0.0], 0.0, 900.0, 0), None);
        assert_eq!(visible_page_window(&[0.0, 1028.0], 0.0, 900.0, 1000), None);
    }
}
