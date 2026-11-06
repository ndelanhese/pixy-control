//! Debounce release of our USB session independently of physical privacy.
use std::time::Duration;
const HOLD: Duration = Duration::from_millis(1500);
#[derive(Default)]
pub struct IdleProbe {
    since: Option<Duration>,
}
impl IdleProbe {
    pub fn ready(&mut self, now: Duration, idle: bool) -> bool {
        if !idle {
            self.since = None;
            return false;
        }
        let since = *self.since.get_or_insert(now);
        now.saturating_sub(since) >= HOLD
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn short_client_renegotiations_do_not_release_the_usb_capture() {
        let mut probe = IdleProbe::default();
        assert!(!probe.ready(Duration::ZERO, true));
        assert!(!probe.ready(Duration::from_millis(1000), true));
        assert!(!probe.ready(Duration::from_millis(1100), false));
        assert!(!probe.ready(Duration::from_millis(2000), true));
        assert!(!probe.ready(Duration::from_millis(3499), true));
        assert!(probe.ready(Duration::from_millis(3500), true));
    }
    #[test]
    fn unknown_usage_or_local_preview_restarts_the_idle_confirmation() {
        let mut probe = IdleProbe::default();
        probe.ready(Duration::ZERO, true);
        probe.ready(Duration::from_millis(1400), false);
        assert!(!probe.ready(Duration::from_millis(1500), true));
        assert!(!probe.ready(Duration::from_millis(2999), true));
        assert!(probe.ready(Duration::from_millis(3000), true));
    }
}
