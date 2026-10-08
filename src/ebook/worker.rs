use std::{collections::HashSet, path::PathBuf, sync::mpsc, thread};

use super::{OpenedEbook, open_ebook};

pub type EbookGeneration = u64;

#[derive(Debug)]
pub enum EbookCommand {
    Open { request_id: u64, generation: EbookGeneration, path: PathBuf },
    CancelGeneration(EbookGeneration),
}

#[derive(Debug)]
pub enum EbookResult {
    Opened { request_id: u64, generation: EbookGeneration, book: OpenedEbook },
    Failed { request_id: u64, generation: EbookGeneration, message: String },
}

pub struct EbookWorker {
    pub commands: mpsc::Sender<EbookCommand>,
    pub results: mpsc::Receiver<EbookResult>,
}

impl EbookWorker {
    pub fn spawn() -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        thread::Builder::new().name("koiflow-ebook".into()).spawn(move || {
            let mut cancelled = HashSet::new();
            while let Ok(command) = command_rx.recv() {
                match command {
                    EbookCommand::CancelGeneration(generation) => { cancelled.insert(generation); }
                    EbookCommand::Open { request_id, generation, path } => {
                        if cancelled.contains(&generation) { continue; }
                        let result = match open_ebook(&path) {
                            Ok(book) => EbookResult::Opened { request_id, generation, book },
                            Err(error) => EbookResult::Failed { request_id, generation, message: error.to_string() },
                        };
                        if !cancelled.contains(&generation) && result_tx.send(result).is_err() { break; }
                    }
                }
            }
        }).expect("ebook worker thread");
        Self { commands: command_tx, results: result_rx }
    }
}
