//! Ownership-aware automatic privacy, plus a read-only macOS external-use query.
#![allow(unexpected_cfgs)] // objc 0.2 macros contain their own feature cfgs.
use std::time::Duration;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Usage {
    Active,
    Idle,
    Unknown,
}
#[derive(Clone, Copy, Debug)]
pub struct Input {
    pub enabled: bool,
    pub mode: Option<u8>,
    pub usage: Usage,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    EnterPrivacy,
    Restore(u8),
}
/// Three seconds of positively established idle use are required.
pub const IDLE_GRACE: Duration = Duration::from_secs(3);
const RESTORE_RETRY: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Authority {
    Hint,
    Owned,
}
#[derive(Clone, Copy, Debug, Default)]
enum State {
    #[default]
    Unowned,
    Managed {
        saved: u8,
        authority: Authority,
        retry_after: Duration,
    },
    Entering {
        saved: u8,
        acknowledged: Option<(bool, Duration)>,
        baseline: Option<Authority>,
    },
    Restoring {
        saved: u8,
        acknowledged: Option<Duration>,
        authority: Authority,
    },
}
/// Caller supplies monotonic time and fresh mode. Implicit preview/producer do
/// not count as consumers; unavailable usage is Unknown. Explicit mode selection
/// MUST call user_override(), including Privacy -> Privacy.
#[derive(Default, Debug)]
pub struct AutoPrivacy {
    state: State,
    idle_since: Option<(u8, Duration)>,
}
impl AutoPrivacy {
    /// Resume only privacy authority previously persisted after our confirmed
    /// EnterPrivacy. A transport baseline must use resume_with_hint instead.
    #[cfg(test)]
    fn resume(saved: Option<u8>) -> Self {
        Self::resume_with_hint(saved, None)
    }
    pub fn resume_with_hint(owned: Option<u8>, hint: Option<u8>) -> Self {
        let state = if let Some(saved @ 0..=1) = owned {
            State::Managed {
                saved,
                authority: Authority::Owned,
                retry_after: Duration::ZERO,
            }
        } else if let Some(saved @ 0..=1) = hint {
            State::Managed {
                saved,
                authority: Authority::Hint,
                retry_after: Duration::ZERO,
            }
        } else {
            State::Unowned
        };
        Self {
            state,
            idle_since: None,
        }
    }
    /// Desired mode, for UI compatibility. This alone does NOT grant authority
    /// to open Privacy; persist owned_restore_mode and retained_mode separately.
    pub fn restore_mode(&self) -> Option<u8> {
        match self.state {
            State::Unowned => None,
            State::Managed { saved, .. }
            | State::Entering { saved, .. }
            | State::Restoring { saved, .. } => Some(saved),
        }
    }
    fn authority(&self) -> Option<Authority> {
        match self.state {
            State::Unowned => None,
            State::Managed { authority, .. } | State::Restoring { authority, .. } => {
                Some(authority)
            }
            State::Entering { baseline, .. } => Some(baseline.unwrap_or(Authority::Hint)),
        }
    }
    pub fn owned_restore_mode(&self) -> Option<u8> {
        (self.authority() == Some(Authority::Owned))
            .then(|| self.restore_mode())
            .flatten()
    }
    pub fn retained_mode(&self) -> Option<u8> {
        (self.authority() == Some(Authority::Hint))
            .then(|| self.restore_mode())
            .flatten()
    }
    /// A known user mode before our transport stops is only a baseline hint.
    /// It must never authorize opening independently selected Privacy.
    pub fn retain_idle_mode(&mut self, mode: u8) {
        if matches!(self.state, State::Unowned) && mode <= 1 {
            self.state = State::Managed {
                saved: mode,
                authority: Authority::Hint,
                retry_after: Duration::ZERO,
            };
            self.idle_since = None;
        }
    }
    pub fn update(&mut self, now: Duration, input: Input) -> Option<Action> {
        let mode = match input.mode {
            Some(3) => Some(0),
            Some(m @ 0..=2) => Some(m),
            _ => None,
        };
        if mode.is_none() || input.usage != Usage::Idle || !input.enabled {
            self.idle_since = None;
        }
        match self.state {
            State::Entering {
                saved,
                acknowledged,
                baseline,
            } => match (mode, acknowledged) {
                (Some(2), Some((true, _))) => {
                    self.state = State::Managed {
                        saved,
                        authority: Authority::Owned,
                        retry_after: now,
                    }
                }
                (Some(2), Some((false, _))) => {
                    // Only already-held authority survives a failed entry.
                    self.state = if baseline == Some(Authority::Owned) {
                        State::Managed {
                            saved,
                            authority: Authority::Owned,
                            retry_after: now,
                        }
                    } else {
                        State::Unowned
                    };
                    if baseline != Some(Authority::Owned) {
                        self.idle_since = None;
                        return None;
                    }
                }
                (Some(0 | 1), Some((false, _))) => {
                    self.state = baseline.map_or(State::Unowned, |authority| State::Managed {
                        saved,
                        authority,
                        retry_after: now.saturating_add(RESTORE_RETRY),
                    });
                    self.idle_since = None;
                    return None;
                }
                (Some(0 | 1), Some((true, deadline))) if now >= deadline => {
                    self.state = State::Managed {
                        saved,
                        authority: baseline.unwrap_or(Authority::Hint),
                        retry_after: now,
                    }
                }
                _ => return None,
            },
            State::Restoring {
                saved,
                acknowledged,
                authority,
            } => {
                if authority == Authority::Hint && mode == Some(2) {
                    // A baseline restore failure/race cannot become permission
                    // to reopen someone else's Privacy selection.
                    self.user_override();
                    return None;
                }
                if acknowledged.is_some() && mode == Some(saved) {
                    self.state = State::Unowned;
                    self.idle_since = None;
                } else if acknowledged.is_some() && mode.is_some_and(|m| m != 2) {
                    self.state = State::Managed {
                        saved,
                        authority,
                        retry_after: now.saturating_add(RESTORE_RETRY),
                    };
                    return None;
                } else if mode == Some(2) && acknowledged.is_some_and(|deadline| now >= deadline) {
                    self.state = State::Managed {
                        saved,
                        authority,
                        retry_after: now,
                    };
                } else {
                    return None;
                }
            }
            _ => {}
        }
        if let State::Managed {
            saved,
            authority,
            retry_after,
        } = self.state
        {
            let observed = mode?;
            if authority == Authority::Hint && observed == 2 {
                self.user_override();
                return None;
            }
            if !input.enabled || input.usage == Usage::Active {
                if observed == saved {
                    self.state = State::Unowned;
                    self.idle_since = None;
                } else if now >= retry_after {
                    self.state = State::Restoring {
                        saved,
                        authority,
                        acknowledged: None,
                    };
                    return Some(Action::Restore(saved));
                }
                return None;
            }
            if observed != 2 && input.usage == Usage::Idle && now >= retry_after {
                return self.idle_action(now, saved, Some(authority));
            }
            return None;
        }
        if input.enabled && input.usage == Usage::Idle {
            if let Some(mode @ 0..=1) = mode {
                return self.idle_action(now, mode, None);
            }
            self.idle_since = None;
        }
        None
    }
    fn idle_action(
        &mut self,
        now: Duration,
        saved: u8,
        baseline: Option<Authority>,
    ) -> Option<Action> {
        let (previous, since) = *self.idle_since.get_or_insert((saved, now));
        if previous != saved {
            self.idle_since = Some((saved, now));
        } else if now.saturating_sub(since) >= IDLE_GRACE {
            self.idle_since = None;
            self.state = State::Entering {
                saved,
                baseline,
                acknowledged: None,
            };
            return Some(Action::EnterPrivacy);
        }
        None
    }
    /// Complete every emitted action, including queue rejection/errors. ACK
    /// alone never grants new Privacy authority: mode 2 must also be observed.
    pub fn command_finished(&mut self, now: Duration, action: Action, success: bool) {
        match (self.state, action) {
            (
                State::Entering {
                    saved,
                    acknowledged: None,
                    baseline,
                },
                Action::EnterPrivacy,
            ) => {
                self.state = State::Entering {
                    saved,
                    baseline,
                    acknowledged: Some((success, now.saturating_add(RESTORE_RETRY))),
                }
            }
            (
                State::Restoring {
                    saved,
                    acknowledged: None,
                    authority,
                },
                Action::Restore(requested),
            ) if saved == requested => {
                self.state = if success {
                    State::Restoring {
                        saved,
                        authority,
                        acknowledged: Some(now.saturating_add(RESTORE_RETRY)),
                    }
                } else {
                    State::Managed {
                        saved,
                        authority,
                        retry_after: now.saturating_add(RESTORE_RETRY),
                    }
                }
            }
            _ => {}
        }
    }
    pub fn user_override(&mut self) {
        self.state = State::Unowned;
        self.idle_since = None;
    }
}
/// Reports another application's use of the *physical* EMEET PIXY, excluding
/// this process as documented by AVCaptureDevice.isInUseByAnotherApplication.
/// Discovery and property reads do not open an AVCaptureSession or ask consent.
/// Absence, ambiguity, disconnection, missing API, or serialization error is
/// Unknown; none is evidence that the camera is idle.
#[cfg(target_os = "macos")]
pub fn physical_usage() -> Usage {
    native_usage::read()
}
#[cfg(not(target_os = "macos"))]
pub fn physical_usage() -> Usage {
    Usage::Unknown
}

/// System-wide physical-device I/O, INCLUDING this process. Only use this
/// result as external usage after our own physical capture has fully stopped.
/// Uses documented CoreMediaIO DeviceIsRunningSomewhere ('gone'), never 'goin'.
pub fn physical_running_anywhere() -> Usage {
    #[cfg(target_os = "macos")]
    {
        native_usage::running_anywhere()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Usage::Unknown
    }
}

#[cfg(target_os = "macos")]
mod native_usage {
    use super::Usage;
    use objc::runtime::{Class, Object, BOOL, NO};
    use objc::{msg_send, sel, sel_impl};
    use std::{ffi::CStr, sync::Mutex};
    static SERIAL: Mutex<()> = Mutex::new(());
    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {
        static AVMediaTypeVideo: *mut Object;
        static AVCaptureDeviceTypeExternal: *mut Object;
    }
    #[repr(C)]
    struct Address {
        selector: u32,
        scope: u32,
        element: u32,
    }
    #[link(name = "CoreMediaIO", kind = "framework")]
    extern "C" {
        fn CMIOObjectGetPropertyDataSize(
            object: u32,
            address: *const Address,
            qualifier_size: u32,
            qualifier: *const std::ffi::c_void,
            size: *mut u32,
        ) -> i32;
        fn CMIOObjectGetPropertyData(
            object: u32,
            address: *const Address,
            qualifier_size: u32,
            qualifier: *const std::ffi::c_void,
            size: u32,
            used: *mut u32,
            data: *mut std::ffi::c_void,
        ) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(object: *const std::ffi::c_void);
        fn CFStringGetCString(
            string: *const std::ffi::c_void,
            buffer: *mut std::ffi::c_char,
            size: isize,
            encoding: u32,
        ) -> u8;
    }
    fn address(selector: [u8; 4]) -> Address {
        Address {
            selector: u32::from_be_bytes(selector),
            scope: u32::from_be_bytes(*b"glob"),
            element: 0,
        }
    }
    unsafe fn property<T: Copy>(object: u32, selector: [u8; 4]) -> Option<T> {
        let mut value = std::mem::MaybeUninit::<T>::uninit();
        let mut used = 0;
        let size = std::mem::size_of::<T>() as u32;
        let status = CMIOObjectGetPropertyData(
            object,
            &address(selector),
            0,
            std::ptr::null(),
            size,
            &mut used,
            value.as_mut_ptr().cast(),
        );
        if status == 0 && used == size {
            Some(value.assume_init())
        } else {
            None
        }
    }
    unsafe fn cf_string(object: u32, selector: [u8; 4]) -> Option<String> {
        let value = property::<*const std::ffi::c_void>(object, selector)?;
        if value.is_null() {
            return None;
        }
        let mut buffer = vec![0i8; 4096];
        let valid = CFStringGetCString(
            value,
            buffer.as_mut_ptr(),
            buffer.len() as isize,
            0x08000100,
        );
        CFRelease(value);
        if valid == 0 {
            None
        } else {
            CStr::from_ptr(buffer.as_ptr())
                .to_str()
                .ok()
                .map(str::to_owned)
        }
    }
    unsafe fn cmio_running() -> Option<bool> {
        let mut size = 0;
        let address = address(*b"dev#");
        if CMIOObjectGetPropertyDataSize(1, &address, 0, std::ptr::null(), &mut size) != 0
            || size == 0
            || size % 4 != 0
            || size > 16384
        {
            return None;
        }
        let mut ids = vec![0u32; size as usize / 4];
        let mut used = 0;
        if CMIOObjectGetPropertyData(
            1,
            &address,
            0,
            std::ptr::null(),
            size,
            &mut used,
            ids.as_mut_ptr().cast(),
        ) != 0
            || used != size
        {
            return None;
        }
        let mut matched = None;
        for id in ids {
            let name = cf_string(id, *b"lnam")?;
            let uid = cf_string(id, *b"uid ")?;
            if name.trim().eq_ignore_ascii_case("EMEET PIXY")
                && uid.to_ascii_lowercase().ends_with("328f00c0")
            {
                if matched.is_some() {
                    return None;
                }
                matched = Some(id);
            }
        }
        let id = matched?;
        if property::<u32>(id, *b"livn")? != 1 {
            return None;
        }
        let running = property::<u32>(id, *b"gone")?;
        if property::<u32>(id, *b"livn")? != 1 {
            return None;
        }
        match running {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
    pub(super) fn running_anywhere() -> Usage {
        let Ok(_guard) = SERIAL.lock() else {
            return Usage::Unknown;
        };
        objc::rc::autoreleasepool(|| unsafe {
            match cmio_running() {
                Some(true) => Usage::Active,
                Some(false) => Usage::Idle,
                None => Usage::Unknown,
            }
        })
    }
    unsafe fn string(object: *mut Object) -> Option<String> {
        if object.is_null() {
            return None;
        }
        let ptr: *const std::ffi::c_char = msg_send![object, UTF8String];
        if ptr.is_null() {
            None
        } else {
            CStr::from_ptr(ptr).to_str().ok().map(str::to_owned)
        }
    }
    pub(super) fn read() -> Usage {
        let Ok(_guard) = SERIAL.lock() else {
            return Usage::Unknown;
        };
        objc::rc::autoreleasepool(|| unsafe {
            let (Some(array), Some(discovery)) = (
                Class::get("NSArray"),
                Class::get("AVCaptureDeviceDiscoverySession"),
            ) else {
                return Usage::Unknown;
            };
            let types: *mut Object = msg_send![array, arrayWithObject:AVCaptureDeviceTypeExternal];
            let session: *mut Object = msg_send![discovery, discoverySessionWithDeviceTypes:types mediaType:AVMediaTypeVideo position:0isize];
            if session.is_null() {
                return Usage::Unknown;
            }
            let devices: *mut Object = msg_send![session, devices];
            if devices.is_null() {
                return Usage::Unknown;
            }
            let count: usize = msg_send![devices, count];
            let mut matched = None;
            for index in 0..count {
                let device: *mut Object = msg_send![devices, objectAtIndex:index];
                let name: *mut Object = msg_send![device, localizedName];
                let uid: *mut Object = msg_send![device, uniqueID];
                if string(name).is_some_and(|name| name.trim().eq_ignore_ascii_case("EMEET PIXY"))
                    && string(uid).is_some_and(|uid| uid.to_ascii_lowercase().ends_with("328f00c0"))
                {
                    if matched.is_some() {
                        return Usage::Unknown;
                    }
                    matched = Some(device);
                }
            }
            let Some(device) = matched else {
                return Usage::Unknown;
            };
            let available: BOOL =
                msg_send![device, respondsToSelector:sel!(isInUseByAnotherApplication)];
            let connected_api: BOOL = msg_send![device, respondsToSelector:sel!(isConnected)];
            if available == NO || connected_api == NO {
                return Usage::Unknown;
            }
            let connected: BOOL = msg_send![device, isConnected];
            if connected == NO {
                return Usage::Unknown;
            }
            let external: BOOL = msg_send![device, isInUseByAnotherApplication];
            // Recheck connection, so removal racing a false usage read is unknown.
            let connected: BOOL = msg_send![device, isConnected];
            if connected == NO {
                Usage::Unknown
            } else if external != NO {
                Usage::Active
            } else {
                Usage::Idle
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input(mode: Option<u8>, usage: Usage) -> Input {
        Input {
            enabled: true,
            mode,
            usage,
        }
    }
    fn tick(p: &mut AutoPrivacy, t: u64, mode: Option<u8>, usage: Usage) -> Option<Action> {
        p.update(Duration::from_millis(t), input(mode, usage))
    }
    fn entered(saved: u8) -> AutoPrivacy {
        let mut p = AutoPrivacy::default();
        assert_eq!(tick(&mut p, 0, Some(saved), Usage::Idle), None);
        assert_eq!(
            tick(&mut p, 3000, Some(saved), Usage::Idle),
            Some(Action::EnterPrivacy)
        );
        p.command_finished(Duration::from_millis(3010), Action::EnterPrivacy, true);
        assert_eq!(tick(&mut p, 3020, Some(2), Usage::Idle), None);
        p
    }
    #[test]
    fn persisted_ownership_resumes_only_valid_manual_or_tracking_modes() {
        for saved in [0, 1] {
            let mut p = AutoPrivacy::resume(Some(saved));
            assert_eq!(p.restore_mode(), Some(saved));
            assert_eq!(
                tick(&mut p, 0, Some(2), Usage::Active),
                Some(Action::Restore(saved))
            );
            p.user_override();
            assert_eq!(p.restore_mode(), None);
        }
        for saved in [None, Some(2), Some(3), Some(255)] {
            let mut p = AutoPrivacy::resume(saved);
            assert_eq!(p.restore_mode(), None);
            assert_eq!(tick(&mut p, 0, Some(2), Usage::Active), None);
        }
    }
    #[test]
    fn idle_requires_full_continuous_grace() {
        let mut p = AutoPrivacy::default();
        assert_eq!(tick(&mut p, 0, Some(0), Usage::Idle), None);
        assert_eq!(tick(&mut p, 2999, Some(0), Usage::Idle), None);
        assert_eq!(
            tick(&mut p, 3000, Some(0), Usage::Idle),
            Some(Action::EnterPrivacy)
        );
    }
    #[test]
    fn unknown_and_disconnection_break_idle_grace() {
        for (mode, usage) in [(Some(0), Usage::Unknown), (None, Usage::Idle)] {
            let mut p = AutoPrivacy::default();
            tick(&mut p, 0, Some(0), Usage::Idle);
            tick(&mut p, 2900, mode, usage);
            assert_eq!(tick(&mut p, 3000, Some(0), Usage::Idle), None);
            assert_eq!(
                tick(&mut p, 6000, Some(0), Usage::Idle),
                Some(Action::EnterPrivacy)
            );
        }
    }
    #[test]
    fn explicit_privacy_never_auto_opens() {
        let mut p = AutoPrivacy::default();
        for t in [0, 3000, 9000] {
            assert_eq!(tick(&mut p, t, Some(2), Usage::Active), None)
        }
        let mut p = entered(0);
        p.user_override();
        assert_eq!(tick(&mut p, 9000, Some(2), Usage::Active), None);
    }
    #[test]
    fn restore_exact_prior_manual_or_tracking() {
        for saved in [0, 1, 3] {
            let mut p = entered(saved);
            assert_eq!(
                tick(&mut p, 4000, Some(2), Usage::Active),
                Some(Action::Restore(if saved == 3 { 0 } else { saved }))
            );
        }
    }
    #[test]
    fn pending_commands_wait_for_ack_and_readback() {
        let mut p = AutoPrivacy::default();
        tick(&mut p, 0, Some(1), Usage::Idle);
        assert_eq!(
            tick(&mut p, 3000, Some(1), Usage::Idle),
            Some(Action::EnterPrivacy)
        );
        assert_eq!(tick(&mut p, 4000, Some(2), Usage::Active), None);
        p.command_finished(Duration::from_millis(4010), Action::EnterPrivacy, true);
        assert_eq!(
            tick(&mut p, 4020, Some(2), Usage::Active),
            Some(Action::Restore(1))
        );
        assert_eq!(tick(&mut p, 9000, Some(2), Usage::Active), None);
        p.command_finished(Duration::from_millis(9010), Action::Restore(1), true);
        assert_eq!(tick(&mut p, 9020, Some(2), Usage::Active), None);
        assert_eq!(tick(&mut p, 9030, Some(1), Usage::Active), None);
    }
    #[test]
    fn disconnected_owned_privacy_retains_restore_intent() {
        let mut p = entered(1);
        assert_eq!(tick(&mut p, 4000, None, Usage::Active), None);
        assert_eq!(tick(&mut p, 9000, Some(2), Usage::Unknown), None);
        assert_eq!(
            tick(&mut p, 10000, Some(2), Usage::Active),
            Some(Action::Restore(1))
        );
    }
    #[test]
    fn failed_restore_retries_with_backoff_and_preserves_mode() {
        let mut p = entered(1);
        let a = Action::Restore(1);
        assert_eq!(tick(&mut p, 4000, Some(2), Usage::Active), Some(a));
        p.command_finished(Duration::from_millis(4010), a, false);
        assert_eq!(tick(&mut p, 5009, Some(2), Usage::Active), None);
        assert_eq!(tick(&mut p, 5010, Some(2), Usage::Active), Some(a));
    }
    #[test]
    fn disabling_restores_only_owned_privacy() {
        let mut p = entered(1);
        assert_eq!(
            p.update(
                Duration::from_secs(4),
                Input {
                    enabled: false,
                    mode: Some(2),
                    usage: Usage::Idle
                }
            ),
            Some(Action::Restore(1))
        );
        let mut p = AutoPrivacy::default();
        assert_eq!(
            p.update(
                Duration::from_secs(4),
                Input {
                    enabled: false,
                    mode: Some(2),
                    usage: Usage::Active
                }
            ),
            None
        );
    }
    #[test]
    fn rapid_usage_flaps_restart_grace() {
        let mut p = AutoPrivacy::default();
        tick(&mut p, 0, Some(0), Usage::Idle);
        tick(&mut p, 2500, Some(0), Usage::Active);
        assert_eq!(tick(&mut p, 2600, Some(0), Usage::Idle), None);
        assert_eq!(tick(&mut p, 5599, Some(0), Usage::Idle), None);
        assert_eq!(
            tick(&mut p, 5600, Some(0), Usage::Idle),
            Some(Action::EnterPrivacy)
        );
    }
    #[test]
    fn failed_enter_does_not_open_manual_privacy_later() {
        let mut p = AutoPrivacy::default();
        tick(&mut p, 0, Some(0), Usage::Idle);
        tick(&mut p, 3000, Some(0), Usage::Idle);
        p.command_finished(Duration::from_millis(3010), Action::EnterPrivacy, false);
        assert_eq!(tick(&mut p, 3020, Some(0), Usage::Active), None);
        assert_eq!(tick(&mut p, 4000, Some(2), Usage::Active), None);
    }
    #[test]
    fn failed_ack_with_privacy_readback_does_not_create_new_authority() {
        let mut p = AutoPrivacy::default();
        tick(&mut p, 0, Some(1), Usage::Idle);
        tick(&mut p, 3000, Some(1), Usage::Idle);
        p.command_finished(Duration::from_millis(3010), Action::EnterPrivacy, false);
        assert_eq!(tick(&mut p, 3020, Some(2), Usage::Active), None);
    }
    #[test]
    fn explicit_user_selection_relinquishes_ownership() {
        let mut p = entered(1);
        p.user_override();
        assert_eq!(tick(&mut p, 4000, Some(0), Usage::Idle), None);
        assert_eq!(tick(&mut p, 5000, Some(2), Usage::Active), None);
    }
    #[test]
    fn transport_reset_to_manual_does_not_lose_saved_tracking() {
        let mut p = AutoPrivacy::resume(Some(1));
        assert_eq!(
            tick(&mut p, 0, Some(0), Usage::Active),
            Some(Action::Restore(1))
        );
        assert_eq!(p.restore_mode(), Some(1));
    }
    #[test]
    fn transport_reset_with_unknown_usage_preserves_restore_intent() {
        let mut p = AutoPrivacy::resume(Some(1));
        assert_eq!(tick(&mut p, 0, Some(0), Usage::Unknown), None);
        assert_eq!(p.restore_mode(), Some(1));
        assert_eq!(
            tick(&mut p, 1000, Some(0), Usage::Active),
            Some(Action::Restore(1))
        );
    }
    #[test]
    fn transport_reset_while_idle_reenters_privacy_preserving_original_tracking() {
        let mut p = AutoPrivacy::resume(Some(1));
        assert_eq!(tick(&mut p, 0, Some(0), Usage::Idle), None);
        assert_eq!(tick(&mut p, 2999, Some(0), Usage::Idle), None);
        assert_eq!(
            tick(&mut p, 3000, Some(0), Usage::Idle),
            Some(Action::EnterPrivacy)
        );
        assert_eq!(p.restore_mode(), Some(1));
        p.command_finished(Duration::from_millis(3010), Action::EnterPrivacy, true);
        assert_eq!(
            tick(&mut p, 3020, Some(2), Usage::Active),
            Some(Action::Restore(1))
        );
    }
    #[test]
    fn successful_restore_ack_requires_exact_mode_and_wrong_readback_retries() {
        let mut p = AutoPrivacy::resume(Some(1));
        assert_eq!(
            tick(&mut p, 0, Some(2), Usage::Active),
            Some(Action::Restore(1))
        );
        p.command_finished(Duration::from_millis(10), Action::Restore(1), true);
        assert_eq!(tick(&mut p, 20, Some(0), Usage::Active), None);
        assert_eq!(p.restore_mode(), Some(1));
        assert_eq!(tick(&mut p, 1019, Some(0), Usage::Active), None);
        assert_eq!(
            tick(&mut p, 1020, Some(0), Usage::Active),
            Some(Action::Restore(1))
        );
        p.command_finished(Duration::from_millis(1030), Action::Restore(1), true);
        assert_eq!(tick(&mut p, 1040, Some(1), Usage::Active), None);
        assert_eq!(p.restore_mode(), None);
    }
    #[test]
    fn entering_ack_then_manual_transport_reset_has_bounded_readback_wait() {
        let mut p = AutoPrivacy::default();
        tick(&mut p, 0, Some(1), Usage::Idle);
        assert_eq!(
            tick(&mut p, 3000, Some(1), Usage::Idle),
            Some(Action::EnterPrivacy)
        );
        p.command_finished(Duration::from_millis(3010), Action::EnterPrivacy, true);
        assert_eq!(tick(&mut p, 3020, Some(0), Usage::Active), None);
        assert_eq!(p.restore_mode(), Some(1));
        assert_eq!(tick(&mut p, 4009, Some(0), Usage::Active), None);
        assert_eq!(
            tick(&mut p, 4010, Some(0), Usage::Active),
            Some(Action::Restore(1))
        );
    }
    #[test]
    fn restore_ack_reverted_privacy_retries_without_per_frame_flood() {
        let mut p = AutoPrivacy::resume(Some(1));
        assert_eq!(
            tick(&mut p, 0, Some(2), Usage::Active),
            Some(Action::Restore(1))
        );
        p.command_finished(Duration::from_millis(10), Action::Restore(1), true);
        assert_eq!(tick(&mut p, 20, Some(2), Usage::Active), None);
        assert_eq!(tick(&mut p, 1009, Some(2), Usage::Active), None);
        assert_eq!(p.restore_mode(), Some(1));
        assert_eq!(
            tick(&mut p, 1010, Some(2), Usage::Active),
            Some(Action::Restore(1))
        );
        assert_eq!(tick(&mut p, 2000, Some(2), Usage::Active), None);
        p.command_finished(Duration::from_millis(2010), Action::Restore(1), true);
        assert_eq!(tick(&mut p, 2020, Some(1), Usage::Active), None);
        assert_eq!(p.restore_mode(), None);
    }
    #[test]
    fn retain_before_transport_release_preserves_tracking_across_manual_reset() {
        let mut p = AutoPrivacy::default();
        p.retain_idle_mode(1);
        assert_eq!(p.restore_mode(), Some(1));
        assert_eq!(tick(&mut p, 0, Some(0), Usage::Idle), None);
        assert_eq!(
            tick(&mut p, 3000, Some(0), Usage::Idle),
            Some(Action::EnterPrivacy)
        );
        assert_eq!(p.restore_mode(), Some(1));
        p.user_override();
        assert_eq!(p.restore_mode(), None);
    }
    #[test]
    fn retain_never_overwrites_owned_mode_or_accepts_privacy_unknown() {
        let mut p = AutoPrivacy::resume(Some(1));
        p.retain_idle_mode(0);
        assert_eq!(p.restore_mode(), Some(1));
        for invalid in [2, 3, 255] {
            let mut p = AutoPrivacy::default();
            p.retain_idle_mode(invalid);
            assert_eq!(p.restore_mode(), None);
        }
        let mut p = AutoPrivacy::default();
        p.retain_idle_mode(0);
        assert_eq!(p.restore_mode(), Some(0));
    }
    #[test]
    fn retained_hint_never_opens_external_privacy() {
        let mut p = AutoPrivacy::default();
        p.retain_idle_mode(1);
        assert_eq!(p.owned_restore_mode(), None);
        assert_eq!(p.retained_mode(), Some(1));
        assert_eq!(tick(&mut p, 0, Some(2), Usage::Active), None);
        assert_eq!(p.restore_mode(), None);
    }
    #[test]
    fn hint_restore_failure_then_manual_privacy_never_retries_open() {
        let mut p = AutoPrivacy::resume_with_hint(None, Some(1));
        assert_eq!(
            tick(&mut p, 0, Some(0), Usage::Active),
            Some(Action::Restore(1))
        );
        p.command_finished(Duration::from_millis(10), Action::Restore(1), false);
        assert_eq!(tick(&mut p, 5000, Some(2), Usage::Active), None);
        assert_eq!(p.owned_restore_mode(), None);
        assert_eq!(p.retained_mode(), None);
    }
    #[test]
    fn hint_entry_acquires_authority_only_after_successful_ack_and_privacy_readback() {
        let mut p = AutoPrivacy::resume_with_hint(None, Some(1));
        tick(&mut p, 0, Some(0), Usage::Idle);
        assert_eq!(
            tick(&mut p, 3000, Some(0), Usage::Idle),
            Some(Action::EnterPrivacy)
        );
        assert_eq!(p.owned_restore_mode(), None);
        p.command_finished(Duration::from_millis(3010), Action::EnterPrivacy, true);
        assert_eq!(p.owned_restore_mode(), None);
        assert_eq!(tick(&mut p, 3020, Some(2), Usage::Idle), None);
        assert_eq!(p.owned_restore_mode(), Some(1));
        assert_eq!(p.retained_mode(), None);
        assert_eq!(
            tick(&mut p, 4000, Some(2), Usage::Active),
            Some(Action::Restore(1))
        );
    }
    #[test]
    fn persisted_hint_and_authority_are_separate() {
        let mut hint = AutoPrivacy::resume_with_hint(None, Some(1));
        assert_eq!(hint.owned_restore_mode(), None);
        assert_eq!(tick(&mut hint, 0, Some(2), Usage::Active), None);
        let mut owned = AutoPrivacy::resume_with_hint(Some(1), Some(0));
        assert_eq!(owned.owned_restore_mode(), Some(1));
        assert_eq!(owned.retained_mode(), None);
        assert_eq!(
            tick(&mut owned, 0, Some(2), Usage::Active),
            Some(Action::Restore(1))
        );
    }
    #[test]
    fn failed_hint_entry_with_privacy_readback_never_gains_authority() {
        let mut p = AutoPrivacy::resume_with_hint(None, Some(1));
        tick(&mut p, 0, Some(0), Usage::Idle);
        tick(&mut p, 3000, Some(0), Usage::Idle);
        p.command_finished(Duration::from_millis(3010), Action::EnterPrivacy, false);
        assert_eq!(tick(&mut p, 3020, Some(2), Usage::Active), None);
        assert_eq!(p.owned_restore_mode(), None);
        assert_eq!(p.retained_mode(), None);
    }
    #[test]
    fn pending_hint_restore_then_external_privacy_ignores_late_ack() {
        let mut p = AutoPrivacy::resume_with_hint(None, Some(1));
        assert_eq!(
            tick(&mut p, 0, Some(0), Usage::Active),
            Some(Action::Restore(1))
        );
        assert_eq!(tick(&mut p, 1, Some(2), Usage::Active), None);
        p.command_finished(Duration::from_millis(10), Action::Restore(1), true);
        assert_eq!(tick(&mut p, 5000, Some(2), Usage::Active), None);
        assert_eq!(p.restore_mode(), None);
    }
    #[test]
    fn owned_authority_survives_failed_reentry_but_hint_does_not_promote() {
        let mut p = AutoPrivacy::resume(Some(1));
        tick(&mut p, 0, Some(0), Usage::Idle);
        tick(&mut p, 3000, Some(0), Usage::Idle);
        p.command_finished(Duration::from_millis(3010), Action::EnterPrivacy, false);
        assert_eq!(
            tick(&mut p, 3020, Some(2), Usage::Active),
            Some(Action::Restore(1))
        );
        assert_eq!(p.owned_restore_mode(), Some(1));
    }
}
