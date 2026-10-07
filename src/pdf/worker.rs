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
    CancelGeneration(Generation),
}

impl PdfCommand {
    fn priority(&self) -> RequestPriority {
        match self {
            Self::Open { .. } | Self::CancelGeneration(_) => RequestPriority::Visible,
            Self::LoadPage { priority, .. } => *priority,
        }
    }
}

#[derive(Debug)]
pub enum PdfResult {
    Opened { request_id: RequestId, generation: Generation, document: OpenedPdf },
    Page { request_id: RequestId, generation: Generation, document_id: String, data: PageData },
    PasswordRequired { request_id: RequestId, generation: Generation, path: PathBuf },
    Failed { request_id: RequestId, generation: Generation, message: String },
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
                    pending.sort_by_key(PdfCommand::priority);
                    for command in pending {
                        if let PdfCommand::CancelGeneration(generation) = command {
                            cancelled.insert(generation);
                            continue;
                        }
                        let generation = match &command {
                            PdfCommand::Open { generation, .. } | PdfCommand::LoadPage { generation, .. } => *generation,
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
            Err(error) => PdfResult::Failed { request_id, generation, message: error.to_string() },
        },
        PdfCommand::LoadPage { request_id, generation, document_id, path, password, page, width, rotation, render, .. } => {
            match backend.load_page(&path, password.as_deref(), page, width, rotation, render) {
                Ok(data) => PdfResult::Page { request_id, generation, document_id, data },
                Err(error) => PdfResult::Failed { request_id, generation, message: error.to_string() },
            }
        }
        PdfCommand::CancelGeneration(_) => unreachable!(),
    }
}

fn command_failure(command: PdfCommand, message: String) -> PdfResult {
    match command {
        PdfCommand::Open { request_id, generation, .. } | PdfCommand::LoadPage { request_id, generation, .. } => {
            PdfResult::Failed { request_id, generation, message }
        }
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
            PdfCommand::CancelGeneration(0),
        ];
        commands.sort_by_key(PdfCommand::priority);
        assert_eq!(commands[0].priority(), RequestPriority::Visible);
        assert_eq!(commands[1].priority(), RequestPriority::Visible);
        assert_eq!(commands[2].priority(), RequestPriority::Background);
    }
}
