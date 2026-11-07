//! Scoped user-requested video work: resist App Nap while allowing idle sleep.
//! Foundation NSProcessInfo.h defines UserInitiatedAllowingIdleSystemSleep as
//! UserInitiated &~IdleSystemSleepDisabled plus LatencyCritical for live A/V.
//! No system/display sleep prevention; the guard exists only during capture.
#![allow(unexpected_cfgs)] // objc0.2 emits its historical cargo-clippy feature cfg.
#[cfg(target_os = "macos")]
use objc::runtime::Object;
#[cfg(target_os = "macos")]
use objc::{class, msg_send, sel, sel_impl};
#[cfg(target_os = "macos")]
#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {}

/// Keep alive only for the actual physical capture session. Dropping the guard
/// ends the activity; it must not be retained by an idle virtual-camera producer.
pub struct Activity {
    #[cfg(target_os = "macos")]
    process: *mut Object,
    #[cfg(target_os = "macos")]
    token: *mut Object,
}
impl Activity {
    pub fn begin() -> Result<Self, String> {
        #[cfg(target_os = "macos")]
        {
            objc::rc::autoreleasepool(|| unsafe {
                const OPTIONS: u64 = (0x00ff_ffffu64 & !(1u64 << 20)) | 0xff_0000_0000u64;
                let process: *mut Object = msg_send![class!(NSProcessInfo), processInfo];
                if process.is_null() {
                    return Err("NSProcessInfo indisponível para captura local".into());
                }
                let reason: *mut Object = msg_send![class!(NSString),stringWithUTF8String:c"Captura e processamento local de vídeo EMEET PIXY".as_ptr()];
                let token: *mut Object =
                    msg_send![process,beginActivityWithOptions:OPTIONS reason:reason];
                if token.is_null() {
                    return Err("macOS não criou a atividade de vídeo local".into());
                }
                let _: *mut Object = msg_send![token, retain];
                let _: *mut Object = msg_send![process, retain];
                Ok(Self { process, token })
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err("Atividade de vídeo nativa disponível no macOS".into())
        }
    }
}
impl Drop for Activity {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        objc::rc::autoreleasepool(|| unsafe {
            let _: () = msg_send![self.process,endActivity:self.token];
            let _: () = msg_send![self.token, release];
            let _: () = msg_send![self.process, release];
        });
    }
}

/// CPU-bound video and diagnostic workers have user-initiated scheduling. This
/// does not change the process priority or disable sleep.
pub fn prioritize_current_thread() -> Result<(), String> {
    set_thread_qos(0x19)
}
/// Match the AVFoundation delivery queue for deadline-bound frame processing.
pub fn prioritize_delivery_thread() -> Result<(), String> {
    set_thread_qos(0x21)
}
fn set_thread_qos(qos: u32) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn pthread_set_qos_class_self_np(qos: u32, relative: i32) -> i32;
        }
        let code = unsafe { pthread_set_qos_class_self_np(qos, 0) };
        if code != 0 {
            return Err(format!("Prioridade do worker local ({code})"));
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = qos;
    Ok(())
}

/// Metadata lookups have no video deadline and must not inherit capture QoS.
pub fn prioritize_metadata_thread() -> Result<(), String> {
    set_thread_qos(0x11)
}
#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn video_and_metadata_qos_do_not_change_the_calling_thread() {
        unsafe extern "C" {
            fn pthread_self() -> *mut std::ffi::c_void;
            fn pthread_get_qos_class_np(
                thread: *mut std::ffi::c_void,
                class: *mut u32,
                relative: *mut i32,
            ) -> i32;
        }
        let class = || unsafe {
            let mut qos = 0;
            let mut relative = 0;
            assert_eq!(
                pthread_get_qos_class_np(pthread_self(), &mut qos, &mut relative),
                0
            );
            qos
        };
        let video_class = std::thread::spawn(move || {
            super::prioritize_delivery_thread().unwrap();
            class()
        })
        .join()
        .unwrap();
        assert_eq!(
            video_class, 0x21,
            "delivery must retain the original AVFoundation interactive QoS"
        );
        let before = class();
        let worker_class = std::thread::spawn(move || {
            super::prioritize_metadata_thread().unwrap();
            class()
        })
        .join()
        .unwrap();
        assert_eq!(
            worker_class, 0x11,
            "metadata must use Utility, not video QoS"
        );
        assert_eq!(class(), before, "the caller's class must be preserved");
    }
}
