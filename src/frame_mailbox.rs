//! One pending frame, independently owned by the processing worker.
use std::sync::{Condvar, Mutex};

pub struct Mailbox<T> {
    state: Mutex<(bool, Option<T>)>,
    ready: Condvar,
}
impl<T> Default for Mailbox<T> {
    fn default() -> Self {
        Self {
            state: Mutex::new((false, None)),
            ready: Condvar::new(),
        }
    }
}
impl<T> Mailbox<T> {
    pub fn submit(&self, value: T) {
        let obsolete = {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if state.0 {
                return;
            }
            let old = state.1.replace(value);
            self.ready.notify_one();
            old
        };
        // Native buffer destruction must never hold the mailbox lock.
        drop(obsolete);
    }
    pub fn take(&self) -> Option<T> {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut state = self
            .ready
            .wait_while(state, |s| !s.0 && s.1.is_none())
            .unwrap_or_else(|p| p.into_inner());
        if state.0 {
            None
        } else {
            state.1.take()
        }
    }
    pub fn close(&self) {
        let pending = {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state.0 = true;
            let pending = state.1.take();
            self.ready.notify_all();
            pending
        };
        drop(pending);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            mpsc, Arc,
        },
        thread,
        time::Duration,
    };
    struct Frame(usize, Arc<AtomicUsize>);
    impl Drop for Frame {
        fn drop(&mut self) {
            self.1.fetch_add(1, Ordering::SeqCst);
        }
    }
    #[test]
    fn blocked_processor_does_not_block_capture_or_accumulate_old_frames() {
        let mailbox = Arc::new(Mailbox::<Frame>::default());
        let drops = Arc::new(AtomicUsize::new(0));
        let (begun_tx, begun_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let worker_box = mailbox.clone();
        let worker = thread::spawn(move || {
            let first = worker_box.take().expect("first frame");
            begun_tx.send(first.0).unwrap();
            resume_rx.recv().unwrap();
            drop(first);
            let last = worker_box.take().expect("latest frame");
            assert_eq!(last.0, 1000);
            drop(last);
        });
        mailbox.submit(Frame(1, drops.clone()));
        assert_eq!(begun_rx.recv_timeout(Duration::from_secs(2)).unwrap(), 1);
        let (submitted_tx, submitted_rx) = mpsc::channel();
        let capture_box = mailbox.clone();
        let capture_drops = drops.clone();
        let capture = thread::spawn(move || {
            for id in 2..=1000 {
                capture_box.submit(Frame(id, capture_drops.clone()));
            }
            submitted_tx.send(()).unwrap();
        });
        submitted_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("capture must finish while processor is blocked");
        assert_eq!(
            drops.load(Ordering::SeqCst),
            998,
            "only in-flight and newest frames retained"
        );
        resume_tx.send(()).unwrap();
        capture.join().unwrap();
        worker.join().unwrap();
        assert_eq!(drops.load(Ordering::SeqCst), 1000);
        mailbox.close();
        assert!(mailbox.take().is_none());
    }
    #[test]
    fn close_drops_pending_frame_wakes_reader_and_rejects_later_frames() {
        let mailbox = Arc::new(Mailbox::<usize>::default());
        let reader_box = mailbox.clone();
        let (tx, rx) = mpsc::channel();
        let reader = thread::spawn(move || tx.send(reader_box.take()).unwrap());
        mailbox.close();
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), None);
        reader.join().unwrap();
        mailbox.submit(5);
        assert!(mailbox.take().is_none());
        let drops = Arc::new(AtomicUsize::new(0));
        let pending = Mailbox::default();
        pending.submit(Frame(1, drops.clone()));
        pending.close();
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        pending.submit(Frame(2, drops.clone()));
        assert_eq!(drops.load(Ordering::SeqCst), 2);
        assert!(pending.take().is_none());
    }
}
