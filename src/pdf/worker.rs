use std::{collections::HashSet, path::PathBuf, sync::mpsc, thread, time::Instant};

use super::{OpenedPdf, PageData, PdfBackend, PdfError, PdfiumBackend};

pub type RequestId = u64;
pub type Generation = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RequestPriority {
    Visible,
    Nearby,
    Background,
}

#[derive(Debug)]
pub enum PdfCommand {
    Open { request_id: RequestId, generation: Generation, path: PathBuf, password: Option<String> },
    LoadPage {
        request_id: RequestId,
        generation: Generation,
        document_id: String,
        path: PathBuf,
        password: Option<String>,
        page: u32,
        width: i32,
        rotation: u16,
        render: bool,
        priority: RequestPriority,
    },
    LoadThumbnail {
        request_id: RequestId,
        generation: Generation,
        document_id: String,
        path: PathBuf,
        width: i32,
    },
    CancelGeneration(Generation),
}

impl PdfCommand {
    fn priority(&self) -> RequestPriority {
        match self {
            Self::Open { .. } | Self::CancelGeneration(_) => RequestPriority::Visible,
            Self::LoadPage { priority, .. } => *priority,
            Self::LoadThumbnail { .. } => RequestPriority::Background,
        }
    }

    fn queue_rank(&self) -> (u8, RequestPriority) {
        match self {
            Self::CancelGeneration(_) => (0, RequestPriority::Visible),
            _ => (1, self.priority()),
        }
    }
}

#[derive(Debug)]
pub enum PdfResult {
    Opened { request_id: RequestId, generation: Generation, document: OpenedPdf },
    Page { request_id: RequestId, generation: Generation, document_id: String, rotation: u16, render: bool, data: PageData },
    Thumbnail { request_id: RequestId, generation: Generation, document_id: String, path: PathBuf, data: PageData },
    PasswordRequired { request_id: RequestId, generation: Generation, path: PathBuf },
    Failed { request_id: RequestId, generation: Generation, request: FailedRequest, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailedRequest {
    Open,
    Page { document_id: String, page: u32, rotation: u16, render: bool },
    Thumbnail { document_id: String, path: PathBuf },
}

pub struct PdfWorker {
    pub commands: mpsc::Sender<PdfCommand>,
    pub results: mpsc::Receiver<PdfResult>,
}

impl PdfWorker {
    pub fn spawn() -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        thread::Builder::new()
            .name("pdfium-worker".into())
            .spawn(move || {
                let backend = PdfiumBackend::new();
                let mut cancelled = HashSet::new();
                while let Ok(first) = command_rx.recv() {
                    let mut pending = vec![first];
                    pending.extend(command_rx.try_iter());
                    pending.sort_by_key(PdfCommand::queue_rank);
                    for command in pending {
                        if let PdfCommand::CancelGeneration(generation) = command {
                            cancelled.insert(generation);
                            continue;
                        }
                        let generation = match &command {
                            PdfCommand::Open { generation, .. } | PdfCommand::LoadPage { generation, .. } | PdfCommand::LoadThumbnail { generation, .. } => *generation,
                            PdfCommand::CancelGeneration(_) => unreachable!(),
                        };
                        if cancelled.contains(&generation) { continue; }
                        let started = Instant::now();
                        let result = match &backend {
                            Ok(backend) => execute(backend, command),
                            Err(error) => command_failure(command, error.to_string()),
                        };
                        tracing::debug!(generation, elapsed_ms = started.elapsed().as_millis(), "completed PDF worker request");
                        if result_tx.send(result).is_err() { return; }
                    }
                }
            })
            .expect("failed to create PDF worker");
        Self { commands: command_tx, results: result_rx }
    }
}

fn execute(backend: &PdfiumBackend, command: PdfCommand) -> PdfResult {
    match command {
        PdfCommand::Open { request_id, generation, path, password } => match backend.inspect(&path, password.as_deref()) {
            Ok(document) => PdfResult::Opened { request_id, generation, document },
            Err(PdfError::PasswordRequired) => PdfResult::PasswordRequired { request_id, generation, path },
            Err(error) => PdfResult::Failed { request_id, generation, request: FailedRequest::Open, message: error.to_string() },
        },
        PdfCommand::LoadPage { request_id, generation, document_id, path, password, page, width, rotation, render, .. } => {
            match backend.load_page(&path, password.as_deref(), page, width, rotation, render) {
                Ok(data) => PdfResult::Page { request_id, generation, document_id, rotation, render, data },
                Err(error) => PdfResult::Failed {
                    request_id,
                    generation,
                    request: FailedRequest::Page { document_id, page, rotation, render },
                    message: error.to_string(),
                },
            }
        }
        PdfCommand::LoadThumbnail { request_id, generation, document_id, path, width } => {
            match backend.load_page(&path, None, 0, width, 0, true) {
                Ok(data) => PdfResult::Thumbnail { request_id, generation, document_id, path, data },
                Err(error) => PdfResult::Failed {
                    request_id,
                    generation,
                    request: FailedRequest::Thumbnail { document_id, path },
                    message: error.to_string(),
                },
            }
        }
        PdfCommand::CancelGeneration(_) => unreachable!(),
    }
}

fn command_failure(command: PdfCommand, message: String) -> PdfResult {
    match command {
        PdfCommand::Open { request_id, generation, .. } => PdfResult::Failed { request_id, generation, request: FailedRequest::Open, message },
        PdfCommand::LoadPage { request_id, generation, document_id, page, rotation, render, .. } => PdfResult::Failed {
            request_id, generation, request: FailedRequest::Page { document_id, page, rotation, render }, message,
        },
        PdfCommand::LoadThumbnail { request_id, generation, document_id, path, .. } => PdfResult::Failed {
            request_id, generation, request: FailedRequest::Thumbnail { document_id, path }, message,
        },
        PdfCommand::CancelGeneration(_) => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_and_cancellation_work_sort_before_background_extraction() {
        let mut commands = vec![
            PdfCommand::LoadPage { request_id: 1, generation: 1, document_id: "d".into(), path: PathBuf::from("d.pdf"), password: None, page: 20, width: 0, rotation: 0, render: false, priority: RequestPriority::Background },
            PdfCommand::LoadPage { request_id: 2, generation: 1, document_id: "d".into(), path: PathBuf::from("d.pdf"), password: None, page: 3, width: 1500, rotation: 0, render: true, priority: RequestPriority::Visible },
            PdfCommand::LoadThumbnail { request_id: 3, generation: 2, document_id: "cover".into(), path: PathBuf::from("cover.pdf"), width: 240 },
            PdfCommand::CancelGeneration(1),
        ];
        commands.sort_by_key(PdfCommand::queue_rank);
        assert!(matches!(commands[0], PdfCommand::CancelGeneration(1)));
        assert_eq!(commands[1].priority(), RequestPriority::Visible);
        assert_eq!(commands[2].priority(), RequestPriority::Background);
        assert_eq!(commands[3].priority(), RequestPriority::Background);
    }

    #[test]
    fn failures_preserve_request_identity_for_stale_result_rejection() {
        let failure = command_failure(
            PdfCommand::LoadPage {
                request_id: 9,
                generation: 4,
                document_id: "document-hash".into(),
                path: PathBuf::from("book.pdf"),
                password: None,
                page: 12,
                width: 1500,
                rotation: 270,
                render: true,
                priority: RequestPriority::Visible,
            },
            "render failed".into(),
        );
        assert!(matches!(
            failure,
            PdfResult::Failed {
                request_id: 9,
                generation: 4,
                request: FailedRequest::Page { ref document_id, page: 12, rotation: 270, render: true },
                ref message,
            } if document_id == "document-hash" && message == "render failed"
        ));
    }
}
