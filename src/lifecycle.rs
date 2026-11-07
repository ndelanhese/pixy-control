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
extern "C" fn react(_: &Object, _: Sel, sender: *mut Object) {
    let tag: isize = unsafe { msg_send![sender, tag] };
    if (3..11).contains(&tag) {
        post(tag as u8);
    }
}

fn post(action: u8) {
    ACTION.store(action, Ordering::Release);
    if let Some(ctx) = CONTEXT.get() {
        ctx.request_repaint();
    }
}

pub fn show_camera_consent() {
    post(11);
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
        decl.add_method(
            sel!(reactPixy:),
            react as extern "C" fn(&Object, Sel, *mut Object),
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
        // The parent is a submenu container, so it must have a nil action.
        let submenu: *mut Object = msg_send![class!(NSMenu), new];
        let parent: *mut Object = msg_send![class!(NSMenuItem), alloc];
        let parent: *mut Object = msg_send![parent,
            initWithTitle:ns("Enviar reação") action:std::ptr::null::<std::ffi::c_void>() keyEquivalent:ns("")];
        for (i, (_, label)) in crate::reactions::Reaction::ALL.into_iter().enumerate() {
            let entry: *mut Object = msg_send![class!(NSMenuItem), alloc];
            let entry: *mut Object = msg_send![entry,
                initWithTitle:ns(label) action:sel!(reactPixy:) keyEquivalent:ns("")];
            let _: () = msg_send![entry, setTag:(i as isize + 3)];
            let _: () = msg_send![entry, setTarget:target];
            let _: () = msg_send![submenu, addItem:entry];
            let _: () = msg_send![entry, release];
        }
        let _: () = msg_send![parent, setSubmenu:submenu];
        let _: () = msg_send![menu, insertItem:parent atIndex:1isize];
        let _: () = msg_send![submenu, release];
        let _: () = msg_send![parent, release];
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
/// Static, click-through screen light. All calls stay on the AppKit main thread.
#[derive(Default)]
pub struct EdgeLight {
    panels: Vec<*mut Object>,
}
impl EdgeLight {
    pub fn visible(&self) -> bool {
        !self.panels.is_empty()
            && self
                .panels
                .iter()
                .all(|&panel| unsafe { msg_send![panel, isVisible] })
    }
    pub fn update(&mut self, enabled: bool) {
        unsafe {
            if !enabled {
                for panel in self.panels.drain(..) {
                    let _: () = msg_send![panel, close];
                    let _: () = msg_send![panel, release];
                }
                return;
            }
            if !self.panels.is_empty() {
                return;
            }
            let screen: *mut Object = msg_send![class!(NSScreen), mainScreen];
            if screen.is_null() {
                return;
            }
            let screen: ScreenRect = msg_send![screen, frame];
            let white: *mut Object = msg_send![class!(NSColor), colorWithWhite:1.0f64 alpha:0.9f64];
            // Thin windows avoid a full-screen transparent backing surface.
            for frame in screen.borders() {
                let panel: *mut Object = msg_send![class!(NSPanel), alloc];
                let panel: *mut Object = msg_send![panel,
                    initWithContentRect:frame styleMask:128usize backing:2usize defer:false];
                if panel.is_null() {
                    continue;
                }
                let _: () = msg_send![panel, setReleasedWhenClosed:false];
                let _: () = msg_send![panel, setOpaque:false];
                let _: () = msg_send![panel, setHasShadow:false];
                let _: () = msg_send![panel, setIgnoresMouseEvents:true];
                let _: () = msg_send![panel, setHidesOnDeactivate:false];
                let _: () = msg_send![panel, setLevel:3isize];
                let _: () = msg_send![panel, setCollectionBehavior:145usize];
                let _: () = msg_send![panel, setBackgroundColor:white];
                let _: () = msg_send![panel, orderFrontRegardless];
                self.panels.push(panel);
            }
        }
    }
}
impl Drop for EdgeLight {
    fn drop(&mut self) {
        self.update(false);
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ScreenRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}
impl ScreenRect {
    fn borders(self) -> [Self; 4] {
        let t = 36.0f64.min(self.width / 2.0).min(self.height / 2.0);
        [
            Self { height: t, ..self },
            Self {
                y: self.y + self.height - t,
                height: t,
                ..self
            },
            Self {
                y: self.y + t,
                width: t,
                height: self.height - 2.0 * t,
                ..self
            },
            Self {
                x: self.x + self.width - t,
                y: self.y + t,
                width: t,
                height: self.height - 2.0 * t,
            },
        ]
    }
}
unsafe impl objc::Encode for ScreenRect {
    fn encode() -> objc::Encoding {
        unsafe { objc::Encoding::from_str("{CGRect={CGPoint=dd}{CGSize=dd}}") }
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

    #[test]
    fn screen_light_covers_edges_without_allocating_a_center_surface() {
        let screen = ScreenRect {
            x: -1512.0,
            y: 100.0,
            width: 1512.0,
            height: 982.0,
        };
        let borders = screen.borders();
        let area: f64 = borders.iter().map(|r| r.width * r.height).sum();
        assert!((area - (2.0 * 36.0 * (1512.0 + 982.0) - 4.0 * 36.0 * 36.0)).abs() < 0.01);
        let center = (
            screen.x + screen.width / 2.0,
            screen.y + screen.height / 2.0,
        );
        assert!(borders.iter().all(|r| center.0 < r.x
            || center.0 > r.x + r.width
            || center.1 < r.y
            || center.1 > r.y + r.height));
        assert!(borders.iter().all(|r| r.x >= screen.x
            && r.y >= screen.y
            && r.x + r.width <= screen.x + screen.width
            && r.y + r.height <= screen.y + screen.height));
    }
}
