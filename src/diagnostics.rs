//! Optional diagnostics never wait for storage on the camera control thread.
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::mpsc::{self, SyncSender},
};
pub struct Writer {
    tx: SyncSender<(PathBuf, Value)>,
}
impl Writer {
    pub fn new() -> Self {
        Self::with_write(|path, value| {
            let _ = crate::model::save_json(&path, &value);
        })
    }
    fn with_write(mut write: impl FnMut(PathBuf, Value) + Send + 'static) -> Self {
        let (tx, rx) = mpsc::sync_channel::<(PathBuf, Value)>(1);
        std::thread::spawn(move || {
            while let Ok((path, value)) = rx.recv() {
                write(path, value);
            }
        });
        Self { tx }
    }
    pub fn submit(&self, path: PathBuf, value: Value) {
        let _ = self.tx.try_send((path, value));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn blocked_storage_cannot_block_control_or_accumulate_reports() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let writer = Writer::with_write(move |_, _| {
            let _ = entered_tx.send(());
            let _ = release_rx.recv();
        });
        writer.submit(PathBuf::from("unused"), Value::Null);
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::spawn(move || {
            for _ in 0..1000 {
                writer.submit(PathBuf::from("unused"), Value::Null);
            }
            let _ = done_tx.send(());
        });
        let result = done_rx.recv_timeout(Duration::from_millis(200));
        for _ in 0..1001 {
            let _ = release_tx.send(());
        }
        assert!(result.is_ok(), "storage stalled camera control");
    }
}
