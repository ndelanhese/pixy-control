//! Window lifetime is independent of the camera service lifetime.
#![allow(unexpected_cfgs)]
use objc::runtime::{Object, Sel};
use objc::{class, msg_send, sel, sel_impl};
use std::sync::{
    atomic::{AtomicU8, Ordering},
    OnceLock,
};

static ACTION: AtomicU8 = AtomicU8::new(0);
static CONTEXT: OnceLock<eframe::egui::Context> = OnceLock::new();

#[derive(Default)]
pub struct Lifecycle {
    pub hidden: bool,
    pub quitting: bool,
}

impl Lifecycle {
    pub fn close_requested(&mut self) -> bool {
        if self.quitting {
            return false;
        }
        self.hidden = true;
        true
    }
    pub fn reopen(&mut self) {
        self.hidden = false;
    }
}

extern "C" fn reopen(_: &Object, _: Sel, _: *mut Object) {
    post(1);
}
extern "C" fn quit(_: &Object, _: Sel, _: *mut Object) {
    post(2);
}
fn post(action: u8) {
    ACTION.store(action, Ordering::Release);
    if let Some(ctx) = CONTEXT.get() {
        ctx.request_repaint();
    }
}

pub fn take_action() -> u8 {
    ACTION.swap(0, Ordering::AcqRel)
}

/// Called once on the AppKit main thread. AppKit owns the menu; status item and
/// its target intentionally live until process exit (one of each).
pub fn install_menu(ctx: &eframe::egui::Context) {
    let _ = CONTEXT.set(ctx.clone());
    unsafe {
        let mut decl =
            objc::declare::ClassDecl::new("PixyBackgroundMenu", class!(NSObject)).unwrap();
        decl.add_method(
            sel!(openPixy:),
            reopen as extern "C" fn(&Object, Sel, *mut Object),
        );
        decl.add_method(
            sel!(quitPixy:),
            quit as extern "C" fn(&Object, Sel, *mut Object),
        );
        let target: *mut Object = msg_send![decl.register(), new];
        let bar: *mut Object = msg_send![class!(NSStatusBar), systemStatusBar];
        let item: *mut Object = msg_send![bar, statusItemWithLength: -1.0_f64];
        let _: *mut Object = msg_send![item, retain];
        let button: *mut Object = msg_send![item, button];
        let _: () = msg_send![button, setTitle: ns("Pixy")];
        let _: () = msg_send![button, setToolTip: ns("Pixy Control — câmera em segundo plano")];
        let menu: *mut Object = msg_send![class!(NSMenu), new];
        for (title, action, key) in [
            ("Abrir Pixy Control", sel!(openPixy:), ""),
            ("Encerrar Pixy Control", sel!(quitPixy:), "q"),
        ] {
            let entry: *mut Object = msg_send![class!(NSMenuItem), alloc];
            let entry: *mut Object =
                msg_send![entry, initWithTitle: ns(title) action: action keyEquivalent: ns(key)];
            let _: () = msg_send![entry, setTarget: target];
            let _: () = msg_send![menu, addItem: entry];
            let _: () = msg_send![entry, release];
        }
        let _: () = msg_send![item, setMenu: menu];
        let _: () = msg_send![menu, release];
    }
}

unsafe fn ns(value: &str) -> *mut Object {
    let string: *mut Object = msg_send![class!(NSString), alloc];
    let string: *mut Object =
        msg_send![string, initWithBytes: value.as_ptr() length: value.len() encoding: 4_usize];
    msg_send![string, autorelease]
}

pub fn activate() {
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, activateIgnoringOtherApps: true];
    }
}

/// Query actual AppKit visibility, rather than repeatedly issuing orderOut.
/// Reissuing Visible(false) itself wakes winit and can create an idle busy loop.
pub fn window_visible() -> bool {
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let windows: *mut Object = msg_send![app, windows];
        let count: usize = msg_send![windows, count];
        for i in 0..count {
            let window: *mut Object = msg_send![windows, objectAtIndex: i];
            let title: *mut Object = msg_send![window, title];
            let main: bool = msg_send![title, isEqualToString: ns("Pixy Control")];
            let visible: bool = msg_send![window, isVisible];
            if main && visible {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_window_preserves_service() {
        let mut state = Lifecycle::default();
        assert!(state.close_requested());
        assert!(state.hidden);
        assert!(!state.quitting);
        state.reopen();
        assert!(!state.hidden);
    }

    #[test]
    fn explicit_quit_is_not_cancelled() {
        let mut state = Lifecycle {
            quitting: true,
            ..Default::default()
        };
        assert!(!state.close_requested());
    }
}
