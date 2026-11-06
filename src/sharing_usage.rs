//! Bounded, read-only background usage sampling. No capture sessions are opened.
use crate::auto_privacy::Usage;
use crate::virtual_camera::ConsumerState;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct UsageSample {
    pub physical: Usage,
    pub physical_running: Usage,
    pub virtual_consumers: ConsumerState,
    pub sampled_at: Instant,
}
struct Shared {
    stopped: AtomicBool,
    latest: Mutex<Option<UsageSample>>,
}
pub struct UsageMonitor {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}
impl UsageMonitor {
    pub fn start() -> Self {
        Self::start_with_sampler(Duration::from_millis(500), || {
            // Timestamp the oldest read, so a slow native property query cannot
            // publish an old physical reading disguised as a fresh sample.
            let sampled_at = Instant::now();
            UsageSample {
                physical: crate::auto_privacy::physical_usage(),
                physical_running: crate::auto_privacy::physical_running_anywhere(),
                virtual_consumers: crate::virtual_camera::consumer_state(),
                sampled_at,
            }
        })
    }
    fn start_with_sampler(
        interval: Duration,
        mut sample: impl FnMut() -> UsageSample + Send + 'static,
    ) -> Self {
        let shared = Arc::new(Shared {
            stopped: AtomicBool::new(false),
            latest: Mutex::new(None),
        });
        let state = shared.clone();
        let worker = thread::spawn(move || {
            while !state.stopped.load(Ordering::Acquire) {
                let observed = sample();
                if state.stopped.load(Ordering::Acquire) {
                    break;
                }
                if let Ok(mut slot) = state.latest.lock() {
                    *slot = Some(observed);
                }
                // unpark makes Drop wake this wait without blocking the UI on an
                // OS property query already in progress. The worker then exits.
                thread::park_timeout(interval);
            }
        });
        Self {
            shared,
            worker: Some(worker),
        }
    }
    /// Returns the most recent sample without consuming it. Callers must use
    /// sampled_at to reject stale samples before making camera-state decisions.
    pub fn latest(&self) -> Option<UsageSample> {
        self.shared.latest.lock().ok().and_then(|slot| slot.clone())
    }
}
impl Drop for UsageMonitor {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
        }
    }
}

/// An explicit local preview or recording counts as use. Implicit preview and
/// the virtual producer itself must not be passed as local_preview=true.
pub fn aggregate(
    physical: Usage,
    virtual_active: Option<bool>,
    local_preview: bool,
    recording: bool,
) -> Usage {
    if local_preview || recording || physical == Usage::Active || virtual_active == Some(true) {
        Usage::Active
    } else if physical == Usage::Idle && virtual_active == Some(false) {
        Usage::Idle
    } else {
        Usage::Unknown
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn any_positive_consumer_wins_over_unknown() {
        for physical in [Usage::Active, Usage::Idle, Usage::Unknown] {
            for virtual_active in [Some(true), Some(false), None] {
                assert_eq!(
                    aggregate(physical, virtual_active, true, false),
                    Usage::Active
                );
                assert_eq!(
                    aggregate(physical, virtual_active, false, true),
                    Usage::Active
                );
            }
        }
        assert_eq!(aggregate(Usage::Active, None, false, false), Usage::Active);
        assert_eq!(
            aggregate(Usage::Unknown, Some(true), false, false),
            Usage::Active
        );
    }
    #[test]
    fn idle_requires_both_external_sources_known_idle() {
        assert_eq!(
            aggregate(Usage::Idle, Some(false), false, false),
            Usage::Idle
        );
        assert_eq!(
            aggregate(Usage::Unknown, Some(false), false, false),
            Usage::Unknown
        );
        assert_eq!(aggregate(Usage::Idle, None, false, false), Usage::Unknown);
        assert_eq!(
            aggregate(Usage::Unknown, None, false, false),
            Usage::Unknown
        );
    }
    #[test]
    fn all_external_signal_combinations_are_conservative() {
        for physical in [Usage::Active, Usage::Idle, Usage::Unknown] {
            for virtual_active in [Some(true), Some(false), None] {
                let expected = if physical == Usage::Active || virtual_active == Some(true) {
                    Usage::Active
                } else if physical == Usage::Idle && virtual_active == Some(false) {
                    Usage::Idle
                } else {
                    Usage::Unknown
                };
                assert_eq!(aggregate(physical, virtual_active, false, false), expected);
            }
        }
    }
    #[test]
    fn monitor_keeps_single_latest_slot_and_drop_wakes_worker() {
        let (tx, rx) = std::sync::mpsc::sync_channel(4);
        let mut count = 0;
        let monitor = UsageMonitor::start_with_sampler(Duration::from_millis(1), move || {
            count += 1;
            let _ = tx.try_send(count);
            UsageSample {
                physical: Usage::Idle,
                physical_running: Usage::Idle,
                virtual_consumers: ConsumerState::Known { count },
                sampled_at: Instant::now(),
            }
        });
        for _ in 0..3 {
            rx.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        // Sampling announces before publishing, so wait for a further sample
        // to prove the third has reached the bounded slot.
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let sample = monitor.latest().expect("sample published");
        assert!(matches!(
            sample.virtual_consumers,
            ConsumerState::Known { count: 3.. }
        ));
        assert!(
            monitor.latest().is_some(),
            "reading does not consume latest state"
        );
        drop(monitor);
        while rx.recv_timeout(Duration::from_secs(2)).is_ok() {}
        assert!(matches!(
            rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ));
    }
}
