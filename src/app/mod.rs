mod session;

use std::{path::PathBuf, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};

use eframe::egui::{self, Align, Color32, FontId, Layout, RichText, Sense, Stroke, Vec2};

use crate::{
    document::{Annotation, AnnotationKind, BlockKind, Document, LinkTarget, ReadingAnchor, SearchHit, SourcePosition},
    extraction::blocks_from_raw_page,
    pdf::{Generation, PdfCommand, PdfResult, PdfWorker, RequestPriority},
    reading::{ReaderStyle, ReaderTheme, ReadingMode, TextAlignment},
    storage::{LibraryEntry, SavedTab, Storage},
};
use session::{DocumentSession, TabId, ZoomMode};

const MINT: Color32 = Color32::from_rgb(229, 255, 242);
const FOREST: Color32 = Color32::from_rgb(17, 51, 34);
const LIGHT_ACCENT: Color32 = Color32::from_rgb(36, 107, 79);
const DARK_ACCENT: Color32 = Color32::from_rgb(177, 227, 197);
const TEXTURE_BUDGET: usize = 256 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SidebarView { Library, Outline, Search, Annotations }

pub struct KoiFlowApp {
    worker: PdfWorker,
    storage: Option<Storage>,
    tabs: Vec<DocumentSession>,
    active: Option<usize>,
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
    library_hits: Vec<SearchHit>,
    note_draft: String,
    frame_tick: u64,
}

impl KoiFlowApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let storage = Storage::open().ok();
        let style = storage.as_ref().and_then(Storage::load_style).unwrap_or_default();
        let library = storage.as_ref().map(Storage::library).unwrap_or_default();
        let saved_tabs = storage.as_ref().map(Storage::load_tabs).unwrap_or_default();
        configure_fonts(&cc.egui_ctx);
        configure_visuals(&cc.egui_ctx, style.theme == ReaderTheme::Dark);
        let mut app = Self {
            worker: PdfWorker::spawn(), storage, tabs: vec![], active: None, next_tab: 1,
            next_generation: 1, next_request: 1, style, library, sidebar: SidebarView::Library,
            show_sidebar: true, show_settings: false, distraction_free: false, library_query: String::new(),
            library_hits: vec![], note_draft: String::new(), frame_tick: 0,
        };
        let active_path = saved_tabs.iter().find(|tab| tab.active).map(|tab| tab.path.clone());
        for saved in saved_tabs { if saved.path.exists() { app.open_path(saved.path); } }
        if let Some(path) = active_path { if let Some(index) = app.tabs.iter().position(|tab| tab.path == path) { app.active = Some(index); } }
        let cli_paths: Vec<_> = std::env::args_os().skip(1).map(PathBuf::from).filter(|path| path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))).collect();
        for path in cli_paths { app.open_path(path); }
        app
    }

    fn open_picker(&mut self) {
        if let Some(path) = rfd::FileDialog::new().add_filter("PDF document", &["pdf"]).pick_file() { self.open_path(path); }
    }

    fn open_path(&mut self, path: PathBuf) {
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) { self.active = Some(index); return; }
        let tab_id = self.next_tab; self.next_tab += 1;
        let generation = self.next_generation; self.next_generation += 1;
        self.tabs.push(DocumentSession::loading(tab_id, generation, path.clone()));
        self.active = Some(self.tabs.len() - 1);
        let request_id = self.take_request();
        let _ = self.worker.commands.send(PdfCommand::Open { request_id, generation, path, password: None });
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
        self.tabs.remove(index);
        self.active = if self.tabs.is_empty() { None } else { Some(index.min(self.tabs.len() - 1)) };
    }

    fn take_request(&mut self) -> u64 { let id = self.next_request; self.next_request += 1; id }

    fn request_page(&mut self, index: usize, page: u32, render: bool, priority: RequestPriority, force: bool) {
        let request_id = self.take_request();
        let Some(session) = self.tabs.get_mut(index) else { return };
        let Some(opened) = &session.opened else { return };
        if page >= opened.page_count || (!force && session.requested_pages.contains(&page)) { return; }
        if render && !force && session.textures.contains_key(&page) { return; }
        session.requested_pages.insert(page);
        let _ = self.worker.commands.send(PdfCommand::LoadPage {
            request_id, generation: session.generation, document_id: opened.document_id.clone(), path: opened.path.clone(),
            password: session.password.clone(), page, width: if render { 1500 } else { 0 }, rotation: session.rotation,
            render, priority,
        });
    }

    fn poll_worker(&mut self, ctx: &egui::Context) {
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
                        session.document = cached.unwrap_or_else(|| Document { id: opened.document_id.clone(), title: opened.title.clone(), page_count: opened.page_count, blocks: vec![], outline: opened.outline.clone(), metadata: opened.metadata.clone() });
                        if !session.document.blocks.is_empty() { session.extracted_pages.extend(0..opened.page_count); }
                        if let Some((anchor, page, mode)) = progress.filter(|_| session.scroll_to_page.is_none()) { session.anchor = anchor; session.current_page = page.min(opened.page_count.saturating_sub(1)); session.mode = mode; }
                        session.annotations = annotations; session.opened = Some(opened); session.error = None; session.status = "Preparing pages…".into();
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
                PdfResult::Page { generation, document_id, data, .. } => {
                    let Some(index) = self.tabs.iter().position(|tab| tab.generation == generation && tab.document_id() == Some(document_id.as_str())) else { continue };
                    let page_number = data.page;
                    let newly_extracted;
                    {
                        let session = &mut self.tabs[index];
                        session.requested_pages.remove(&page_number);
                        let aspect = if data.width > 0 { data.height as f32 / data.width as f32 } else if data.raw.width > 0.0 { data.raw.height / data.raw.width } else { 1.414 };
                        session.page_aspects.insert(page_number, aspect);
                        newly_extracted = session.extracted_pages.insert(page_number);
                        if newly_extracted {
                            let first_id = ((page_number as u64) << 32) | 1;
                            session.document.blocks.extend(blocks_from_raw_page(&data.raw, first_id));
                            session.document.blocks.sort_by_key(|block| (block.source[0].page, block.source[0].start_char));
                        }
                        if !data.rgba.is_empty() || session.textures.contains_key(&page_number) {
                            session.raw_pages.insert(page_number, data.raw.clone());
                        }
                        if !data.rgba.is_empty() {
                            if let Some(storage) = &self.storage { storage.save_rendered_page(&document_id, page_number, session.rotation, data.width, data.height, &data.rgba); }
                            let image = egui::ColorImage::from_rgba_unmultiplied([data.width, data.height], &data.rgba);
                            let texture = ctx.load_texture(format!("{}-{page_number}-{}", document_id, session.rotation), image, egui::TextureOptions::LINEAR);
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
                PdfResult::PasswordRequired { generation, .. } => {
                    if let Some(session) = self.tabs.iter_mut().find(|tab| tab.generation == generation) { session.password_required = true; session.status = "Password required".into(); session.error = None; }
                }
                PdfResult::Failed { generation, message, .. } => {
                    if let Some(session) = self.tabs.iter_mut().find(|tab| tab.generation == generation) { session.status = "Unable to read document".into(); session.error = Some(message); }
                }
            }
        }
    }

    fn next_unextracted(&self, index: usize) -> Option<u32> {
        let session = self.tabs.get(index)?;
        (0..session.page_count()).find(|page| !session.extracted_pages.contains(page) && !session.requested_pages.contains(page))
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
                hits = self.tabs[index].document.blocks.iter().filter(|block| block.content.to_lowercase().contains(&needle)).take(200).map(|block| SearchHit { document_id: document_id.clone(), path: path.clone(), title: title.clone(), block_id: block.id, page: block.source[0].page, snippet: block.content.chars().take(180).collect() }).collect();
            }
        }
        if current_only { if let Some(index) = self.active { self.tabs[index].search_hits = hits; self.tabs[index].selected_hit = 0; } } else { self.library_hits = hits; }
    }

    fn add_annotation(&mut self, kind: AnnotationKind) {
        let Some(index) = self.active else { return };
        let Some(document_id) = self.tabs[index].document_id().map(str::to_owned) else { return };
        let block = self.tabs[index].selected_block.and_then(|id| self.tabs[index].document.blocks.iter().find(|block| block.id == id)).cloned();
        let (start, end, quote) = if let Some(block) = block {
            let source = block.source.first().copied().unwrap();
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
        let dropped: Vec<_> = ctx.input(|input| input.raw.dropped_files.iter().map(|file| file.path().to_owned()).filter(|path| path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))).collect());
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

    fn toolbar(&mut self, root: &mut egui::Ui) {
        let mut open = false;
        egui::Panel::top("toolbar").exact_size(54.0).show(root, |ui| {
            ui.add_space(7.0);
            ui.horizontal(|ui| {
                if ui.button(if self.show_sidebar { "◧" } else { "▤" }).on_hover_text("Toggle navigation (Ctrl+B)").clicked() { self.show_sidebar = !self.show_sidebar; }
                if ui.button("Library").clicked() { self.show_sidebar = true; self.sidebar = SidebarView::Library; }
                let title = self.active.map(|index| self.tabs[index].title()).unwrap_or_else(|| "KoiFlow".into());
                ui.label(RichText::new(title).strong().size(15.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Aa").on_hover_text("Reading appearance").clicked() { self.show_settings = !self.show_settings; }
                    if ui.button("Open").on_hover_text("Open PDF (Ctrl+O)").clicked() { open = true; }
                    if ui.button("Search").on_hover_text("Search (Ctrl+F)").clicked() { self.show_sidebar = true; self.sidebar = SidebarView::Search; }
                    if let Some(index) = self.active {
                        ui.selectable_value(&mut self.tabs[index].mode, ReadingMode::Reflow, "Reflow");
                        ui.selectable_value(&mut self.tabs[index].mode, ReadingMode::Original, "Original");
                    }
                });
            });
        });
        if open { self.open_picker(); }
    }

    fn tab_bar(&mut self, root: &mut egui::Ui) {
        let mut close = None;
        egui::Panel::top("tabs").exact_size(38.0).show(root, |ui| {
            ui.horizontal(|ui| {
                for index in 0..self.tabs.len() {
                    let selected = self.active == Some(index);
                    if ui.selectable_label(selected, self.tabs[index].title()).clicked() { self.active = Some(index); }
                    if selected && ui.small_button("×").on_hover_text("Close tab").clicked() { close = Some(index); }
                    ui.separator();
                }
            });
        });
        if let Some(index) = close { self.close_tab(index); }
    }

    fn sidebar_panel(&mut self, root: &mut egui::Ui) {
        let mut open_path = None;
        let mut run_current_search = false;
        let mut run_library_search = false;
        let mut jump_page = None;
        let mut delete_annotation = None;
        egui::Panel::left("navigation").exact_size(285.0).resizable(false).show(root, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.sidebar, SidebarView::Library, "Library");
                ui.selectable_value(&mut self.sidebar, SidebarView::Outline, "Outline");
                ui.selectable_value(&mut self.sidebar, SidebarView::Search, "Find");
                ui.selectable_value(&mut self.sidebar, SidebarView::Annotations, "Notes");
            });
            ui.separator();
            match self.sidebar {
                SidebarView::Library => {
                    if ui.add_sized([ui.available_width(), 36.0], egui::Button::new("+ Open PDF")).clicked() { self.open_picker(); }
                    ui.add_space(12.0);
                    ui.horizontal(|ui| { let response = ui.text_edit_singleline(&mut self.library_query); if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) { run_library_search = true; } if ui.button("Search all").clicked() { run_library_search = true; } });
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        if self.library_query.trim().is_empty() || self.library_hits.is_empty() {
                            for entry in &self.library {
                                ui.add_enabled_ui(!entry.missing, |ui| {
                                    if ui.selectable_label(false, format!("{}\n{}  ·  {:.0}%", entry.title, entry.author, entry.progress * 100.0)).clicked() { open_path = Some(entry.path.clone()); }
                                });
                                if entry.missing { ui.label(RichText::new("File missing — relink by opening it again").small().color(Color32::from_rgb(190, 90, 70))); }
                                ui.separator();
                            }
                        } else {
                            for hit in &self.library_hits { if ui.selectable_label(false, format!("{} · p. {}\n{}", hit.title, hit.page + 1, hit.snippet)).clicked() { open_path = Some(PathBuf::from(&hit.path)); jump_page = Some(hit.page); } }
                        }
                    });
                }
                SidebarView::Outline => {
                    if let Some(index) = self.active {
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
                    }
                }
                SidebarView::Search => {
                    if let Some(index) = self.active {
                        let response = ui.text_edit_singleline(&mut self.tabs[index].search_query);
                        if (response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter))) || ui.button("Find in document").clicked() { run_current_search = true; }
                        ui.separator();
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            let hits = self.tabs[index].search_hits.clone();
                            for (hit_index, hit) in hits.iter().enumerate() {
                                if ui.selectable_label(self.tabs[index].selected_hit == hit_index, format!("Page {}\n{}", hit.page + 1, hit.snippet)).clicked() { self.tabs[index].selected_hit = hit_index; jump_page = Some(hit.page); }
                            }
                        });
                    }
                }
                SidebarView::Annotations => {
                    if let Some(index) = self.active {
                        ui.horizontal(|ui| { if ui.button("Highlight").clicked() { self.add_annotation(AnnotationKind::Highlight); } if ui.button("Bookmark").clicked() { self.add_annotation(AnnotationKind::Bookmark); } });
                        ui.text_edit_multiline(&mut self.note_draft);
                        if ui.button("Add note to selection/page").clicked() { self.add_annotation(AnnotationKind::Note); }
                        ui.separator();
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for annotation in &self.tabs[index].annotations {
                                ui.horizontal(|ui| {
                                    let label = match annotation.kind { AnnotationKind::Highlight => "Highlight", AnnotationKind::Note => "Note", AnnotationKind::Bookmark => "Bookmark" };
                                    if ui.selectable_label(false, format!("{label} · p. {}\n{}", annotation.start.page + 1, if annotation.note.is_empty() { &annotation.quote } else { &annotation.note })).clicked() { jump_page = Some(annotation.start.page); }
                                    if ui.small_button("×").clicked() { delete_annotation = Some(annotation.id); }
                                });
                            }
                        });
                    }
                }
            }
        });
        if run_current_search { self.run_search(true); }
        if run_library_search { self.run_search(false); }
        if let Some(path) = open_path { self.open_path(path); if let (Some(index), Some(page)) = (self.active, jump_page) { navigate_to_page(&mut self.tabs[index], page); } }
        else if let (Some(index), Some(page)) = (self.active, jump_page) { navigate_to_page(&mut self.tabs[index], page); }
        if let Some(id) = delete_annotation { if let Some(storage) = &self.storage { storage.delete_annotation(id); } if let Some(index) = self.active { self.tabs[index].annotations.retain(|annotation| annotation.id != id); } }
    }

    fn settings_panel(&mut self, root: &mut egui::Ui) {
        let ctx = root.ctx().clone();
        let before = serde_json::to_string(&self.style).ok();
        let mut clear_cache = false;
        egui::Panel::right("settings").exact_size(310.0).resizable(false).show(root, |ui| {
            ui.horizontal(|ui| { ui.heading("Reading settings"); if ui.small_button("Reset").clicked() { self.style = ReaderStyle::default(); } });
            ui.add_space(14.0); settings_heading(ui, "TYPOGRAPHY");
            egui::ComboBox::from_id_salt("font-family").selected_text(&self.style.font_family).show_ui(ui, |ui| { for family in ["System serif", "System sans-serif", "Monospace"] { ui.selectable_value(&mut self.style.font_family, family.into(), family); } });
            slider(ui, &mut self.style.font_size, 12.0..=40.0, "Font size", " px");
            slider(ui, &mut self.style.line_height, 1.1..=2.2, "Line height", "×");
            slider(ui, &mut self.style.letter_spacing, -0.5..=3.0, "Letter spacing", " px");
            ui.horizontal(|ui| { ui.label("Alignment"); ui.selectable_value(&mut self.style.alignment, TextAlignment::Left, "Left"); ui.selectable_value(&mut self.style.alignment, TextAlignment::Center, "Center"); });
            ui.add_space(16.0); settings_heading(ui, "LAYOUT");
            slider(ui, &mut self.style.content_max_width, 440.0..=960.0, "Reading width", " px");
            slider(ui, &mut self.style.horizontal_margin, 16.0..=80.0, "Margins", " px");
            slider(ui, &mut self.style.paragraph_spacing, 6.0..=36.0, "Paragraph spacing", " px");
            ui.add_space(16.0); settings_heading(ui, "APPEARANCE");
            ui.horizontal(|ui| { ui.selectable_value(&mut self.style.theme, ReaderTheme::Paper, "Paper"); ui.selectable_value(&mut self.style.theme, ReaderTheme::Sepia, "Sepia"); ui.selectable_value(&mut self.style.theme, ReaderTheme::Dark, "Dark"); });
            ui.checkbox(&mut self.distraction_free, "Distraction-free reading");
            if ui.button("Clear rendered-page cache").clicked() { clear_cache = true; }
        });
        if before != serde_json::to_string(&self.style).ok() { configure_visuals(&ctx, self.style.theme == ReaderTheme::Dark); if let Some(storage) = &self.storage { storage.save_style(&self.style); } }
        if clear_cache { if let Some(storage) = &self.storage { storage.clear_render_cache(); } for tab in &mut self.tabs { tab.textures.clear(); } }
    }

    fn central(&mut self, root: &mut egui::Ui) {
        let canvas = match self.style.theme { ReaderTheme::Paper => Color32::WHITE, ReaderTheme::Sepia => Color32::from_rgb(248, 241, 221), ReaderTheme::Dark => Color32::from_rgb(25, 60, 43) };
        let mut requests = vec![];
        let mut followed_link = None;
        egui::CentralPanel::default().frame(egui::Frame::new().fill(canvas)).show(root, |ui| {
            if let Some(index) = self.active {
                if self.tabs[index].opened.is_none() { ui.centered_and_justified(|ui| { ui.spinner(); ui.label(&self.tabs[index].status); }); }
                else if self.tabs[index].mode == ReadingMode::Original { (requests, followed_link) = original_view(ui, &mut self.tabs[index], self.frame_tick); }
                else { reflow_view(ui, &mut self.tabs[index], &self.style); }
            } else { welcome(ui); }
        });
        if let Some(index) = self.active {
            let ctx = root.ctx().clone();
            for (page, priority) in requests {
                if !self.hydrate_render_cache(index, page, &ctx) { self.request_page(index, page, true, priority, false); }
                else if !self.tabs[index].raw_pages.contains_key(&page) { self.request_page(index, page, false, priority, false); }
            }
        }
        if let (Some(index), Some(target)) = (self.active, followed_link) {
            match target {
                LinkTarget::Page(page) => { self.tabs[index].current_page = page; self.tabs[index].scroll_to_page = Some(page); }
                LinkTarget::Uri(uri) => root.ctx().open_url(egui::OpenUrl::new_tab(uri)),
                LinkTarget::Unknown => {}
            }
        }
    }

    fn status_bar(&mut self, root: &mut egui::Ui) {
        egui::Panel::bottom("status").exact_size(28.0).show(root, |ui| { ui.horizontal(|ui| {
            if let Some(index) = self.active { ui.label(RichText::new(&self.tabs[index].status).small().color(ui.visuals().weak_text_color())); if let Some(error) = &self.tabs[index].error { ui.colored_label(Color32::from_rgb(200, 80, 70), error); } ui.with_layout(Layout::right_to_left(Align::Center), |ui| { ui.label(format!("Page {} / {}", self.tabs[index].current_page + 1, self.tabs[index].page_count())); }); }
            else { ui.label(RichText::new("Ready").small().color(ui.visuals().weak_text_color())); }
        }); });
    }

    fn password_dialog(&mut self, ctx: &egui::Context) {
        let Some(index) = self.active.filter(|index| self.tabs[*index].password_required) else { return };
        let mut unlock = false;
        egui::Window::new("Unlock PDF").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
            ui.label("This document is password protected.");
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
        ctx.request_repaint_after(Duration::from_millis(50));
        if !self.distraction_free { self.toolbar(ui); if !self.tabs.is_empty() { self.tab_bar(ui); } self.status_bar(ui); }
        if !self.distraction_free && self.show_settings { self.settings_panel(ui); }
        if !self.distraction_free && self.show_sidebar { self.sidebar_panel(ui); }
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

fn original_view(ui: &mut egui::Ui, session: &mut DocumentSession, tick: u64) -> (Vec<(u32, RequestPriority)>, Option<LinkTarget>) {
    let mut requests = vec![];
    let mut followed_link = None;
    ui.horizontal(|ui| {
        if ui.button("‹").clicked() && session.current_page > 0 { session.current_page -= 1; session.scroll_to_page = Some(session.current_page); }
        let mut page = session.current_page + 1;
        if ui.add(egui::DragValue::new(&mut page).range(1..=session.page_count().max(1))).changed() { session.current_page = page.saturating_sub(1); session.scroll_to_page = Some(session.current_page); }
        ui.label(format!("of {}", session.page_count()));
        if ui.button("›").clicked() && session.current_page + 1 < session.page_count() { session.current_page += 1; session.scroll_to_page = Some(session.current_page); }
        ui.separator();
        ui.selectable_value(&mut session.zoom, ZoomMode::FitWidth, "Fit width");
        ui.selectable_value(&mut session.zoom, ZoomMode::FitPage, "Fit page");
        if ui.button("−").clicked() { let value = match session.zoom { ZoomMode::Custom(value) => value, _ => 1.0 }; session.zoom = ZoomMode::Custom((value - 0.1).max(0.35)); }
        if ui.button("+").clicked() { let value = match session.zoom { ZoomMode::Custom(value) => value, _ => 1.0 }; session.zoom = ZoomMode::Custom((value + 0.1).min(3.0)); }
        if ui.button("Rotate").clicked() { session.rotation = (session.rotation + 90) % 360; session.textures.clear(); }
    });
    ui.separator();
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
        let first = offsets.partition_point(|offset| *offset < viewport.min.y).saturating_sub(3) as u32;
        let last = (offsets.partition_point(|offset| *offset <= viewport.max.y) + 2).min(page_count as usize).saturating_sub(1) as u32;
        session.current_page = offsets.partition_point(|offset| *offset <= viewport.center().y).saturating_sub(1).min(page_count.saturating_sub(1) as usize) as u32;
        for page in first..=last {
            let aspect = session.page_aspects.get(&page).copied().unwrap_or(1.414);
            let height = page_width * aspect;
            let x = ui.max_rect().left() + ((ui.available_width() - page_width) * 0.5).max(12.0);
            let y = ui.max_rect().top() + 20.0 + offsets[page as usize];
            let rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(page_width, height));
            ui.painter().rect_filled(rect, 2.0, Color32::WHITE);
            ui.painter().rect_stroke(rect, 2.0, Stroke::new(1.0, Color32::from_gray(170)), egui::StrokeKind::Outside);
            if let Some(entry) = session.textures.get_mut(&page) {
                entry.last_used = tick;
                ui.put(rect, egui::Image::new(&entry.texture).fit_to_exact_size(rect.size()));
            } else { ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, format!("Rendering page {}…", page + 1), FontId::proportional(13.0), Color32::GRAY); requests.push((page, if page == session.current_page { RequestPriority::Visible } else { RequestPriority::Nearby })); }
            if session.rotation == 0 {
                if let Some(raw) = session.raw_pages.get(&page) {
                    for annotation in session.annotations.iter().filter(|annotation| annotation.start.page == page && annotation.kind == AnnotationKind::Highlight) {
                        for glyph in raw.glyphs.iter().filter(|glyph| glyph.source.char_index >= annotation.start.char_index && glyph.source.char_index <= annotation.end.char_index) {
                            let glyph_rect = map_page_rect(rect, raw.width, raw.height, glyph.bounds);
                            ui.painter().rect_filled(glyph_rect, 1.0, Color32::from_rgba_unmultiplied(255, 220, 80, 75));
                        }
                    }
                    for link in &raw.links {
                        let link_rect = map_page_rect(rect, raw.width, raw.height, link.bounds);
                        let response = ui.put(link_rect, egui::Button::new("").frame(false));
                        if response.clicked() { followed_link = Some(link.target.clone()); }
                        response.on_hover_cursor(egui::CursorIcon::PointingHand);
                    }
                }
            }
            if session.scroll_to_page == Some(page) { ui.scroll_to_rect(rect, Some(Align::Center)); session.scroll_to_page = None; }
        }
    });
    requests.sort_by_key(|(_, priority)| *priority); requests.dedup_by_key(|(page, _)| *page); (requests, followed_link)
}

fn map_page_rect(screen: egui::Rect, page_width: f32, page_height: f32, bounds: crate::document::PageRect) -> egui::Rect {
    let left = screen.left() + bounds.left / page_width * screen.width();
    let right = screen.left() + bounds.right / page_width * screen.width();
    let top = screen.top() + (page_height - bounds.top) / page_height * screen.height();
    let bottom = screen.top() + (page_height - bounds.bottom) / page_height * screen.height();
    egui::Rect::from_min_max(egui::pos2(left.min(right), top.min(bottom)), egui::pos2(left.max(right), top.max(bottom)))
}

fn reflow_view(ui: &mut egui::Ui, session: &mut DocumentSession, style: &ReaderStyle) {
    let max_width = style.content_max_width.min((ui.available_width() - style.horizontal_margin * 2.0).max(280.0));
    let text_color = if style.theme == ReaderTheme::Dark { Color32::from_rgb(239, 249, 242) } else { Color32::from_rgb(24, 53, 41) };
    let font = if style.font_family == "Monospace" { egui::FontFamily::Monospace } else if style.font_family == "System sans-serif" { egui::FontFamily::Proportional } else { egui::FontFamily::Name("serif".into()) };
    let blocks = &session.document.blocks;
    let heights: Vec<f32> = blocks.iter().map(|block| {
        let size = match block.kind { BlockKind::Heading { level: 1 } => style.font_size * 1.65, BlockKind::Heading { .. } => style.font_size * 1.28, _ => style.font_size };
        let characters_per_line = (max_width / (size * 0.52 + style.letter_spacing.max(0.0))).max(8.0);
        let lines = (block.content.chars().count() as f32 / characters_per_line).ceil().max(1.0);
        (lines * size * style.line_height + style.paragraph_spacing).max(size * style.line_height + style.paragraph_spacing)
    }).collect();
    let mut offsets = Vec::with_capacity(heights.len() + 1);
    offsets.push(0.0);
    for height in &heights { offsets.push(offsets.last().copied().unwrap_or(0.0) + height); }
    let target_offset = session.scroll_to_block.and_then(|id| blocks.iter().position(|block| block.id == id)).map(|index| offsets[index]);
    if target_offset.is_some() { session.scroll_to_block = None; }
    let scroll = egui::ScrollArea::vertical().id_salt(("reflow-scroll", session.tab_id));
    let scroll = if let Some(offset) = target_offset { scroll.vertical_scroll_offset(offset) } else { scroll };
    scroll.show_viewport(ui, |ui, viewport| {
        ui.set_min_height(offsets.last().copied().unwrap_or(0.0) + 48.0);
        let side = ((ui.available_width() - max_width) / 2.0).max(style.horizontal_margin);
        let start = offsets.partition_point(|offset| *offset < viewport.min.y).saturating_sub(1);
        let end = offsets.partition_point(|offset| *offset <= viewport.max.y + 200.0).min(blocks.len());
        for index in start..end {
            let block = &blocks[index];
            let highlighted = session.annotations.iter().any(|annotation| annotation.kind == AnnotationKind::Highlight && block.source.iter().any(|source| source.page == annotation.start.page && source.end_char >= annotation.start.char_index && source.start_char <= annotation.end.char_index));
            let rect = egui::Rect::from_min_size(egui::pos2(ui.max_rect().left() + side, ui.max_rect().top() + offsets[index] + 16.0), egui::vec2(max_width, heights[index] - style.paragraph_spacing * 0.4));
            if highlighted { ui.painter().rect_filled(rect, 4.0, Color32::from_rgba_unmultiplied(255, 226, 120, 70)); }
            let (size, strong) = match block.kind { BlockKind::Heading { level: 1 } => (style.font_size * 1.65, true), BlockKind::Heading { .. } => (style.font_size * 1.28, true), _ => (style.font_size, false) };
            let mut text = RichText::new(&block.content).font(FontId::new(size, font.clone())).color(text_color).line_height(Some(size * style.line_height)).extra_letter_spacing(style.letter_spacing);
            if strong { text = text.strong(); }
            let align = if style.alignment == TextAlignment::Center { Align::Center } else { Align::Min };
            let response = ui.put(rect, egui::Label::new(text).wrap().selectable(true).halign(align).sense(Sense::click()));
            if response.clicked() {
                session.selected_block = Some(block.id);
                let source = block.source[0];
                session.current_page = source.page;
                session.anchor = ReadingAnchor { block_id: block.id, char_offset: 0, source: Some(SourcePosition { page: source.page, char_index: source.start_char }), quote: block.content.chars().take(64).collect() };
            }
        }
    });
}

fn welcome(ui: &mut egui::Ui) {
    ui.with_layout(Layout::top_down(Align::Center), |ui| { ui.add_space((ui.available_height() * 0.22).max(48.0)); ui.label(RichText::new("◌").size(52.0).color(LIGHT_ACCENT)); ui.label(RichText::new("Read PDFs your way").size(28.0).strong()); ui.label("Open a PDF or choose one from your library."); });
}

fn navigate_to_page(session: &mut DocumentSession, page: u32) {
    session.current_page = page.min(session.page_count().saturating_sub(1));
    session.scroll_to_page = Some(session.current_page);
    if session.mode == ReadingMode::Reflow {
        session.scroll_to_block = session.document.blocks.iter().find(|block| block.source.first().is_some_and(|source| source.page >= session.current_page)).map(|block| block.id);
    }
}

fn configure_visuals(ctx: &egui::Context, dark: bool) {
    let mut visuals = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    visuals.panel_fill = if dark { FOREST } else { MINT };
    visuals.window_fill = if dark { Color32::from_rgb(33, 72, 51) } else { Color32::from_rgb(247, 252, 249) };
    visuals.selection.bg_fill = if dark { Color32::from_rgb(49, 91, 68) } else { Color32::from_rgb(200, 241, 219) };
    visuals.selection.stroke = Stroke::new(1.0, if dark { DARK_ACCENT } else { LIGHT_ACCENT });
    ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light }); ctx.set_visuals(visuals);
    ctx.all_styles_mut(|style| { style.spacing.item_spacing = Vec2::new(8.0, 8.0); style.spacing.button_padding = Vec2::new(10.0, 6.0); });
}

fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for (name, family, candidates) in [
        ("system-ui", egui::FontFamily::Proportional, &["/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/usr/share/fonts/truetype/liberation2/LiberationSans-Regular.ttf"][..]),
        ("system-serif", egui::FontFamily::Name("serif".into()), &["/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf", "/usr/share/fonts/truetype/liberation2/LiberationSerif-Regular.ttf"][..]),
    ] {
        if let Some(bytes) = candidates.iter().find_map(|path| std::fs::read(path).ok()) { fonts.font_data.insert(name.into(), egui::FontData::from_owned(bytes).into()); fonts.families.entry(family).or_default().insert(0, name.into()); }
    }
    ctx.set_fonts(fonts);
}

fn settings_heading(ui: &mut egui::Ui, text: &str) { ui.label(RichText::new(text).size(11.0).strong().color(ui.visuals().weak_text_color())); }
fn slider(ui: &mut egui::Ui, value: &mut f32, range: std::ops::RangeInclusive<f32>, label: &str, suffix: &str) { ui.label(label); ui.add(egui::Slider::new(value, range).suffix(suffix)); }
