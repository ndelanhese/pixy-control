//! Scoped user-requested video work: resist App Nap while allowing idle sleep.
//! Foundation NSProcessInfo.h defines UserInitiatedAllowingIdleSystemSleep as
//! UserInitiated &~IdleSystemSleepDisabled. No display-sleep or latency-critical flag.
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
                const OPTIONS: u64 = 0x00ff_ffffu64 & !(1u64 << 20);
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
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn pthread_set_qos_class_self_np(qos: u32, relative: i32) -> i32;
        }
        let code = unsafe { pthread_set_qos_class_self_np(0x19, 0) };
        if code != 0 {
            return Err(format!("Prioridade do worker de vídeo ({code})"));
        }
    }
    Ok(())
}
