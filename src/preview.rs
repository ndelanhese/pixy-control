//! Direct AVFoundation capture; only EMEET PIXY, with no network or vendor runtime.
#![allow(unexpected_cfgs)] // objc 0.2 macros contain their own feature cfgs.
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::thread;

#[derive(Clone, Copy)]
enum Target {
    PhysicalPixy,
    EmeetVirtual,
}
impl Target {
    fn matches(self, name: &str, uid: &str) -> bool {
        match self {
            Self::PhysicalPixy => {
                name.trim().eq_ignore_ascii_case("EMEET PIXY")
                    && uid.to_lowercase().ends_with("328f00c0")
            }
            Self::EmeetVirtual => {
                name.trim() == "EMEET STUDIO Virtual Camera"
                    && uid.eq_ignore_ascii_case("F3B9DD6A-3AA0-45D9-83D5-90A6FC254320")
            }
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::PhysicalPixy => "EMEET PIXY",
            Self::EmeetVirtual => "EMEET STUDIO Virtual Camera",
        }
    }
}

type Frame = (u32, u32, Vec<u8>);
/// Pixels and their native sample timing travel in one latest-frame slot.
/// Diagnostic consumers must not pair an old image with a newer global counter.
pub struct TimedFrame {
    pub image: Frame,
    pub pts_ns: Option<i64>,
    pub sequence: u64,
    pub received_at: std::time::Instant,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CaptureTiming {
    pub frames: u64,
    pub first_pts_ns: Option<i64>,
    pub last_pts_ns: Option<i64>,
    pub width: u32,
    pub height: u32,
}
struct Shared {
    frame: Mutex<Option<TimedFrame>>,
    status: Mutex<String>,
    cancelled: AtomicBool,
    finished: AtomicBool,
    frames: AtomicU64,
    last_valid_frame: Mutex<Option<std::time::Instant>>,
    last_prepared_frame: Mutex<Option<std::time::Instant>>,
    timing: Mutex<CaptureTiming>,
    diagnostics: Mutex<Vec<String>>,
    mismatched: AtomicU64,
    expected: (u32, u32),
    failure: Mutex<Option<String>>,
    prepared_request: Mutex<Option<crate::transform::FrameTransform>>,
    prepared_frame: Mutex<Option<(crate::transform::FrameTransform, Frame)>>,
    processing_error: Mutex<Option<String>>,
    recording_sink: Mutex<
        Option<(
            crate::recording::RecorderSink,
            crate::transform::FrameTransform,
        )>,
    >,
    recording_error: Mutex<Option<String>>,
    virtual_sink: Mutex<
        Option<(
            crate::virtual_camera::VirtualSink,
            crate::transform::FrameTransform,
        )>,
    >,
    virtual_error: Mutex<Option<String>>,
    blur_processor: Mutex<Option<crate::background_blur::Processor>>,
    trace_frames: bool,
    virtual_target: AtomicBool,
    live_only: AtomicBool,
    #[cfg(target_os = "macos")]
    live_processor: Mutex<Option<crate::live_blur::Processor>>,
    #[cfg(target_os = "macos")]
    incoming: crate::frame_mailbox::Mailbox<native::Sample>,
    processing: Mutex<std::collections::VecDeque<ProcessingSample>>,
}
impl Shared {
    fn new(width: u32, height: u32) -> Self {
        Self {
            frame: Mutex::new(None),
            status: Mutex::new("Aguardando permissão da câmera…".into()),
            cancelled: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            frames: AtomicU64::new(0),
            last_valid_frame: Mutex::new(None),
            last_prepared_frame: Mutex::new(None),
            timing: Mutex::new(CaptureTiming::default()),
            diagnostics: Mutex::new(Vec::new()),
            mismatched: AtomicU64::new(0),
            expected: (width, height),
            failure: Mutex::new(None),
            prepared_request: Mutex::new(None),
            prepared_frame: Mutex::new(None),
            processing_error: Mutex::new(None),
            recording_sink: Mutex::new(None),
            recording_error: Mutex::new(None),
            virtual_sink: Mutex::new(None),
            virtual_error: Mutex::new(None),
            blur_processor: Mutex::new(None),
            trace_frames: std::env::args().any(|a| a == "--trace-camera-frames"),
            virtual_target: AtomicBool::new(false),
            live_only: AtomicBool::new(false),
            #[cfg(target_os = "macos")]
            live_processor: Mutex::new(None),
            #[cfg(target_os = "macos")]
            incoming: crate::frame_mailbox::Mailbox::default(),
            processing: Mutex::new(std::collections::VecDeque::new()),
        }
    }
    fn note(&self, text: String) {
        if let Ok(mut entries) = self.diagnostics.lock() {
            if entries.len() < 80 {
                entries.push(text);
            }
        }
    }
    fn status(&self, text: impl Into<String>) {
        if let Ok(mut status) = self.status.lock() {
            *status = text.into();
        }
    }
}

#[derive(Clone, Copy, serde::Serialize)]
pub struct ProcessingSample {
    pub sequence: u64,
    pub source_fingerprint: Option<u64>,
    pub received_ns: i64,
    pub total_ms: f64,
    pub bounded: bool,
    #[cfg(target_os = "macos")]
    pub live: Option<crate::live_blur::Stats>,
    pub blur: Option<crate::background_blur::Stats>,
}
pub struct Preview {
    shared: Arc<Shared>,
}
impl Preview {
    /// Only the live 1080p output is needed while no local window is displayed.
    pub fn set_live_only(&self, value: bool) {
        self.shared.live_only.store(value, Ordering::Release);
    }
    pub fn processing_samples(&self) -> Vec<ProcessingSample> {
        self.shared
            .processing
            .lock()
            .map(|samples| samples.iter().copied().collect())
            .unwrap_or_default()
    }
    pub fn frame_fresh(&self, max_age: std::time::Duration) -> bool {
        !self.shared.cancelled.load(Ordering::Acquire)
            && !self.shared.finished.load(Ordering::Acquire)
            && self
                .shared
                .processing_error
                .lock()
                .is_ok_and(|e| e.is_none())
            && self
                .shared
                .last_prepared_frame
                .lock()
                .ok()
                .and_then(|instant| *instant)
                .is_some_and(|instant| instant.elapsed() <= max_age)
    }
    pub fn timing(&self) -> CaptureTiming {
        self.shared
            .timing
            .lock()
            .map(|timing| *timing)
            .unwrap_or_default()
    }
    pub fn processing_error(&self) -> Option<String> {
        self.shared
            .processing_error
            .lock()
            .ok()
            .and_then(|e| e.clone())
    }
    pub fn diagnostics(&self) -> Vec<String> {
        self.shared
            .diagnostics
            .lock()
            .map(|entries| entries.clone())
            .unwrap_or_default()
    }
    /// Permission and session setup run off the UI thread. No camera opens at app startup.
    pub fn start(width: u32, height: u32, fps: u32) -> Self {
        Self::start_target(width, height, fps, Target::PhysicalPixy)
    }
    /// Explicit consumer of the EMEET virtual camera; never selects a USB camera.
    pub fn start_virtual(width: u32, height: u32, fps: u32) -> Self {
        Self::start_target(width, height, fps, Target::EmeetVirtual)
    }
    fn start_target(width: u32, height: u32, fps: u32, target: Target) -> Self {
        let shared = Arc::new(Shared::new(width, height));
        shared
            .virtual_target
            .store(matches!(target, Target::EmeetVirtual), Ordering::Release);
        let worker = shared.clone();
        thread::spawn(move || {
            #[cfg(target_os = "macos")]
            let result = native::run(worker.clone(), width, height, fps, target);
            #[cfg(not(target_os = "macos"))]
            let result: Result<(), String> = Err("Prévia disponível no macOS.".into());
            match result {
                Ok(()) => worker.status("Prévia encerrada"),
                Err(error) => worker.status(format!("Prévia: {error}")),
            }
            crate::quad::release_scratch();
            worker.finished.store(true, Ordering::Release);
        });
        Self { shared }
    }
    /// A single newest-frame slot bounds memory even while the UI is busy.
    pub fn latest(&self) -> Option<Frame> {
        self.latest_timed().map(|frame| frame.image)
    }
    pub fn latest_timed(&self) -> Option<TimedFrame> {
        self.shared.frame.lock().ok()?.take()
    }
    /// Request this exact framing and take only a matching processed frame. A new
    /// view returns None until the native callback prepares it; stale geometry is
    /// never relabeled as the new view. Raw latest() remains independent.
    pub fn latest_prepared(&self, view: crate::transform::FrameTransform) -> Option<Frame> {
        if self.shared.cancelled.load(Ordering::Acquire)
            || self.shared.finished.load(Ordering::Acquire)
        {
            return None;
        }
        if let Ok(mut request) = self.shared.prepared_request.lock() {
            *request = Some(view);
        } else {
            return None;
        }
        self.shared
            .prepared_frame
            .lock()
            .ok()?
            .take()
            .and_then(|(prepared_view, frame)| (prepared_view == view).then_some(frame))
    }
    pub fn status(&self) -> String {
        let mut status = self
            .shared
            .status
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|_| "Falha na prévia".into());
        if let Some(error) = self
            .shared
            .recording_error
            .lock()
            .ok()
            .and_then(|error| error.clone())
        {
            status.push_str(&format!(" · Gravação: {error}"));
        }
        if let Some(error) = self
            .shared
            .virtual_error
            .lock()
            .ok()
            .and_then(|error| error.clone())
        {
            status.push_str(&format!(" · Câmera virtual: {error}"));
        }
        if let Some(error) = self
            .shared
            .processing_error
            .lock()
            .ok()
            .and_then(|error| error.clone())
        {
            status.push_str(&format!(" · Enquadramento: {error}"));
        }
        status
    }
    /// Captured frames feed the bounded encoder queue directly at camera cadence.
    /// Framing is frozen at recording start; UI changes cannot change video dimensions.
    pub fn set_recording_sink(
        &self,
        sink: crate::recording::RecorderSink,
        transform: crate::transform::FrameTransform,
    ) {
        if let Ok(mut error) = self.shared.recording_error.lock() {
            *error = None;
        }
        if let Ok(mut slot) = self.shared.recording_sink.lock() {
            *slot = Some((sink, transform));
        }
    }
    pub fn clear_recording_sink(&self) {
        if let Ok(mut slot) = self.shared.recording_sink.lock() {
            *slot = None;
        }
        if let Ok(mut error) = self.shared.recording_error.lock() {
            *error = None;
        }
    }
    /// The virtual output consumes its own bounded latest-frame slot. Updating
    /// framing replaces only this small handle; it never reconfigures the camera.
    pub fn set_virtual_sink(
        &self,
        sink: crate::virtual_camera::VirtualSink,
        transform: crate::transform::FrameTransform,
    ) {
        if let Ok(mut error) = self.shared.virtual_error.lock() {
            *error = None;
        }
        if let Ok(mut slot) = self.shared.virtual_sink.lock() {
            *slot = Some((sink, transform));
        }
    }
    pub fn clear_virtual_sink(&self) {
        if let Ok(mut slot) = self.shared.virtual_sink.lock() {
            *slot = None;
        }
        if let Ok(mut error) = self.shared.virtual_error.lock() {
            *error = None;
        }
    }
    pub fn stop(&mut self) {
        self.shared.cancelled.store(true, Ordering::Release);
        if let Ok(mut instant) = self.shared.last_valid_frame.lock() {
            *instant = None;
        }
        self.clear_recording_sink();
        self.clear_virtual_sink();
        if let Ok(mut request) = self.shared.prepared_request.lock() {
            *request = None;
        }
        if let Ok(mut frame) = self.shared.prepared_frame.lock() {
            *frame = None;
        }
        if let Ok(mut error) = self.shared.processing_error.lock() {
            *error = None;
        }
        if let Ok(mut frame) = self.shared.frame.lock() {
            *frame = None;
        }
    }
    pub fn suspend_processing(&self) {
        if let Ok(mut view) = self.shared.prepared_request.lock() {
            *view = None;
        }
        if let Ok(mut frame) = self.shared.prepared_frame.lock() {
            *frame = None;
        }
        if let Ok(mut time) = self.shared.last_prepared_frame.lock() {
            *time = None;
        }
    }
    pub fn is_finished(&self) -> bool {
        self.shared.finished.load(Ordering::Acquire)
    }
}
impl Drop for Preview {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use block::ConcreteBlock;
    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Protocol, Sel, BOOL, NO, YES};
    use objc::{class, msg_send, sel, sel_impl, Encode, Encoding};
    use std::{
        ffi::{c_char, c_void, CStr},
        ptr,
        sync::{mpsc, OnceLock},
        time::{Duration, Instant},
    };

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CMTime {
        value: i64,
        timescale: i32,
        flags: u32,
        epoch: i64,
    }
    unsafe impl Encode for CMTime {
        fn encode() -> Encoding {
            unsafe { Encoding::from_str("{CMTime=qiIq}") }
        }
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Dimensions {
        width: i32,
        height: i32,
    }
    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {
        static AVMediaTypeVideo: *mut Object;
        static AVCaptureDeviceTypeExternal: *mut Object;
        static AVCaptureSessionPresetInputPriority: *mut Object;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRetain(value: *const c_void) -> *const c_void;
        fn CFRelease(value: *const c_void);
    }
    #[link(name = "CoreMedia", kind = "framework")]
    extern "C" {
        fn CMVideoFormatDescriptionGetDimensions(description: *const c_void) -> Dimensions;
        fn CMSampleBufferGetImageBuffer(sample: *const c_void) -> *mut c_void;
        fn CMTimeMake(value: i64, timescale: i32) -> CMTime;
        fn CMSampleBufferGetPresentationTimeStamp(sample: *const c_void) -> CMTime;
        fn CMFormatDescriptionGetMediaSubType(description: *const c_void) -> u32;
    }
    #[link(name = "CoreVideo", kind = "framework")]
    extern "C" {
        static kCVPixelBufferPixelFormatTypeKey: *mut Object;
        fn CVPixelBufferLockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
        fn CVPixelBufferUnlockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
        fn CVPixelBufferGetWidth(buffer: *mut c_void) -> usize;
        fn CVPixelBufferGetHeight(buffer: *mut c_void) -> usize;
        fn CVPixelBufferGetBytesPerRow(buffer: *mut c_void) -> usize;
        fn CVPixelBufferGetBaseAddress(buffer: *mut c_void) -> *mut c_void;
        fn CVPixelBufferGetPixelFormatType(buffer: *mut c_void) -> u32;
    }
    #[link(name = "Foundation", kind = "framework")]
    extern "C" {}
    #[link(name = "System")]
    extern "C" {
        fn dispatch_queue_attr_make_with_qos_class(
            attribute: *const c_void,
            qos: u32,
            priority: i32,
        ) -> *const c_void;
        fn dispatch_queue_create(label: *const c_char, attribute: *const c_void) -> *mut c_void;
        fn dispatch_sync_f(
            queue: *mut c_void,
            context: *mut c_void,
            work: extern "C" fn(*mut c_void),
        );
        fn dispatch_release(object: *mut c_void);
    }
    const BGRA: u32 = u32::from_be_bytes(*b"BGRA");
    const READ_ONLY: u64 = 1;

    /// Owns one Objective-C +1 reference. Used only on the session worker thread.
    struct Owned(*mut Object);
    impl Owned {
        unsafe fn new(object: *mut Object, description: &str) -> Result<Self, String> {
            if object.is_null() {
                Err(format!("Falha ao criar {description}."))
            } else {
                Ok(Self(object))
            }
        }
    }
    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe {
                let _: () = msg_send![self.0, release];
            }
        }
    }
    /// Keep hardware format/rate ownership until the session has negotiated startup.
    /// macOS may ignore InputPriority and overwrite an unlocked device at startRunning.
    struct DeviceConfigurationLock(*mut Object);
    impl Drop for DeviceConfigurationLock {
        fn drop(&mut self) {
            unsafe {
                let _: () = msg_send![self.0, unlockForConfiguration];
            }
        }
    }
    struct Pool(*mut Object);
    impl Pool {
        unsafe fn new() -> Self {
            Self(msg_send![class!(NSAutoreleasePool), new])
        }
    }
    impl Drop for Pool {
        fn drop(&mut self) {
            unsafe {
                let _: () = msg_send![self.0, drain];
            }
        }
    }
    unsafe fn string(object: *mut Object) -> String {
        if object.is_null() {
            return String::new();
        }
        let value: *const c_char = msg_send![object, UTF8String];
        if value.is_null() {
            String::new()
        } else {
            CStr::from_ptr(value).to_string_lossy().into_owned()
        }
    }
    unsafe fn error_message(error: *mut Object) -> String {
        if error.is_null() {
            return "Erro nativo sem detalhes.".into();
        }
        let description: *mut Object = msg_send![error, localizedDescription];
        string(description)
    }

    fn authorize(shared: &Shared) -> Result<(), String> {
        unsafe {
            let status: isize = msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType:AVMediaTypeVideo];
            shared.status(format!("Permissão da câmera: {status}"));
            match status {
                3 => return Ok(()),
                1 | 2 => return Err("Permissão da câmera negada. Autorize Pixy Control em Ajustes do Sistema → Privacidade e Segurança → Câmera.".into()),
                0 => {},
                _ => return Err(format!("Estado desconhecido da permissão da câmera: {status}")),
            }
            let (tx, rx) = mpsc::sync_channel(1);
            let completion = ConcreteBlock::new(move |allowed: BOOL| {
                let _ = tx.try_send(allowed != NO);
            })
            .copy();
            let _: () = msg_send![class!(AVCaptureDevice), requestAccessForMediaType:AVMediaTypeVideo completionHandler:&*completion];
            // AVFoundation copies the completion block for its asynchronous reply. The
            // local retained block also survives until this permission wait completes.
            let deadline = Instant::now() + Duration::from_secs(120);
            loop {
                if shared.cancelled.load(Ordering::Acquire) {
                    return Ok(());
                }
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(true) => return Ok(()),
                    Ok(false) => return Err("Permissão da câmera negada pelo macOS.".into()),
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        return Err("Falha ao consultar permissão da câmera.".into())
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() >= deadline => {
                        return Err("Permissão da câmera ainda pendente.".into())
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        }
    }

    struct Selection {
        device: *mut Object,
        format: *mut Object,
        duration: CMTime,
        actual_fps: f64,
    }
    unsafe fn select(
        width: u32,
        height: u32,
        fps: u32,
        target: Target,
    ) -> Result<Selection, String> {
        let types: *mut Object =
            msg_send![class!(NSArray), arrayWithObject:AVCaptureDeviceTypeExternal];
        let discovery: *mut Object = msg_send![class!(AVCaptureDeviceDiscoverySession), discoverySessionWithDeviceTypes:types mediaType:AVMediaTypeVideo position:0isize];
        if discovery.is_null() {
            return Err("Falha ao enumerar câmeras USB.".into());
        }
        let devices: *mut Object = msg_send![discovery, devices];
        let count: usize = msg_send![devices, count];
        let mut matching = Vec::new();
        for index in 0..count {
            let device: *mut Object = msg_send![devices, objectAtIndex:index];
            let name: *mut Object = msg_send![device, localizedName];
            let unique: *mut Object = msg_send![device, uniqueID];
            if target.matches(&string(name), &string(unique)) {
                matching.push(device);
            }
        }
        let device = match matching.as_slice() {
            [only] => *only,
            [] => {
                return Err(
                    format!("{} não encontrada com a identidade esperada. Nenhuma câmera alternativa foi selecionada.",target.label()),
                )
            }
            _ => return Err(format!("Mais de uma {} conectada; seleção ambígua.",target.label())),
        };
        let formats: *mut Object = msg_send![device, formats];
        let count: usize = msg_send![formats, count];
        let mut advertised = Vec::new();
        for index in 0..count {
            let format: *mut Object = msg_send![formats, objectAtIndex:index];
            let description: *const c_void = msg_send![format, formatDescription];
            let dimensions = CMVideoFormatDescriptionGetDimensions(description);
            if dimensions.width != width as i32 || dimensions.height != height as i32 {
                continue;
            }
            let ranges: *mut Object = msg_send![format, videoSupportedFrameRateRanges];
            let range_count: usize = msg_send![ranges, count];
            for range_index in 0..range_count {
                let range: *mut Object = msg_send![ranges, objectAtIndex:range_index];
                let minimum: f64 = msg_send![range, minFrameRate];
                let maximum: f64 = msg_send![range, maxFrameRate];
                advertised.push(format!("{minimum:.3}–{maximum:.3}"));
                if minimum.is_finite()
                    && maximum.is_finite()
                    && minimum > 0.0
                    && maximum > 0.0
                    && fps as f64 >= minimum - 0.1
                    && fps as f64 <= maximum + 0.1
                {
                    // Preserve an advertised endpoint CMTime for 29.97/59.94 cameras;
                    // no integer truncation and no substitution of resolution/FPS tier.
                    let (actual_fps, duration) = if (fps as f64 - maximum).abs() <= 0.1 {
                        (maximum, msg_send![range, minFrameDuration])
                    } else if (fps as f64 - minimum).abs() <= 0.1 {
                        (minimum, msg_send![range, maxFrameDuration])
                    } else {
                        (fps as f64, CMTimeMake(1, fps as i32))
                    };
                    return Ok(Selection {
                        device,
                        format,
                        duration,
                        actual_fps,
                    });
                }
            }
        }
        Err(format!(
            "Formato {width}×{height} @ {fps} fps indisponível. Taxas nativas nessa resolução: {}",
            advertised.join(", ")
        ))
    }

    type Processed = (crate::transform::FrameTransform, Result<Frame, String>);
    #[allow(clippy::too_many_arguments)]
    fn cached_live_transform(
        cache: &mut Vec<Processed>,
        processor: &Mutex<Option<crate::background_blur::Processor>>,
        width: u32,
        height: u32,
        rgb: &[u8],
        view: crate::transform::FrameTransform,
        limit: Option<u32>,
    ) -> usize {
        // Upright rectangular live views can crop/sample directly from the 4K
        // source into their final canvas, then segment/blur only those pixels.
        // Full-resolution recording and rotated/flipped/perspective views retain
        // the original pre-transform segmentation path.
        let Some(limit) = limit.filter(|_| view.rotation == 0 && !view.flip && view.quad.is_none())
        else {
            return cached_transform(cache, processor, width, height, rgb, view);
        };
        if let Some(index) = cache.iter().position(|(key, _)| *key == view) {
            return index;
        }
        let frame = (|| -> Result<Frame, String> {
            let (w, h, mut pixels) =
                crate::transform::apply_bounded(width, height, rgb.to_vec(), &view, limit)?;
            if view.background_blur != 0.0 {
                let mut slot = processor
                    .lock()
                    .map_err(|_| "Falha no processador de desfoque")?;
                if slot.is_none() {
                    *slot = Some(crate::background_blur::Processor::new()?);
                }
                pixels = slot.as_mut().ok_or("Desfoque indisponível")?.apply(
                    w,
                    h,
                    pixels,
                    view.background_blur,
                )?;
            }
            Ok((w, h, pixels))
        })();
        cache.push((view, frame));
        cache.len() - 1
    }
    fn cached_transform(
        cache: &mut Vec<Processed>,
        processor: &Mutex<Option<crate::background_blur::Processor>>,
        width: u32,
        height: u32,
        rgb: &[u8],
        view: crate::transform::FrameTransform,
    ) -> usize {
        if let Some(index) = cache.iter().position(|(key, _)| *key == view) {
            return index;
        }
        // Vision sees the upright camera image. Rotate/crop only after person
        // segmentation, so a90/180-degree output never makes the subject vanish.
        let frame = (|| -> Result<Frame, String> {
            let mut pixels = rgb.to_vec();
            if view.background_blur != 0.0 {
                let mut slot = processor
                    .lock()
                    .map_err(|_| "Falha no processador de desfoque")?;
                if slot.is_none() {
                    *slot = Some(crate::background_blur::Processor::new()?);
                }
                pixels = slot.as_mut().ok_or("Desfoque indisponível")?.apply(
                    width,
                    height,
                    pixels,
                    view.background_blur,
                )?;
            }
            crate::transform::apply(width, height, pixels, &view)
        })();
        cache.push((view, frame));
        cache.len() - 1
    }

    // A retained CMSampleBuffer owns its immutable pixel buffer across threads.
    // Never move a borrowed AVFoundation callback pointer to another thread.
    pub(super) struct Sample {
        buffer: *mut c_void,
        received: Instant,
    }
    unsafe impl Send for Sample {}
    impl Drop for Sample {
        fn drop(&mut self) {
            unsafe {
                CFRelease(self.buffer);
            }
        }
    }
    extern "C" fn frame_callback(
        this: &Object,
        _: Sel,
        _: *mut Object,
        sample: *mut c_void,
        _: *mut Object,
    ) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            let pointer = *this.get_ivar::<usize>("rustState") as *const Shared;
            if pointer.is_null() || sample.is_null() {
                return;
            }
            let state = &*pointer;
            if state.cancelled.load(Ordering::Acquire) {
                return;
            }
            let received = Instant::now();
            if state.virtual_target.load(Ordering::Acquire) {
                process_sample(state, sample, received);
            } else {
                CFRetain(sample);
                state.incoming.submit(Sample {
                    buffer: sample,
                    received,
                });
            }
        }));
    }
    fn process_sample(state: &Shared, sample: *mut c_void, received_at: Instant) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            let _pool = Pool::new();
            if state.cancelled.load(Ordering::Acquire) {
                return;
            }
            let buffer = CMSampleBufferGetImageBuffer(sample);
            if buffer.is_null() || CVPixelBufferLockBaseAddress(buffer, READ_ONLY) != 0 {
                return;
            }
            struct Locked(*mut c_void);
            impl Drop for Locked {
                fn drop(&mut self) {
                    unsafe {
                        CVPixelBufferUnlockBaseAddress(self.0, READ_ONLY);
                    }
                }
            }
            let _locked = Locked(buffer);
            let width = CVPixelBufferGetWidth(buffer);
            let height = CVPixelBufferGetHeight(buffer);
            let pts = CMSampleBufferGetPresentationTimeStamp(sample);
            let pts_ns = if pts.timescale > 0 && pts.flags & 1 != 0 {
                i64::try_from((pts.value as i128) * 1_000_000_000 / (pts.timescale as i128)).ok()
            } else {
                None
            };
            let mut sequence = 0;
            if let Ok(mut timing) = state.timing.lock() {
                timing.frames += 1;
                sequence = timing.frames;
                timing.first_pts_ns = timing.first_pts_ns.or(pts_ns);
                timing.last_pts_ns = pts_ns;
                timing.width = width as u32;
                timing.height = height as u32;
                if timing.frames <= 12 {
                    state.note(format!(
                        "sample[{}] {}x{} pts={}/{} ns={pts_ns:?}",
                        timing.frames, width, height, pts.value, pts.timescale
                    ));
                }
            }
            if (width as u32, height as u32) != state.expected
                && !state.virtual_target.load(Ordering::Acquire)
            {
                let mismatched = state.mismatched.fetch_add(1, Ordering::Relaxed) + 1;
                if mismatched >= 30 {
                    if let Ok(mut failure) = state.failure.lock() {
                        *failure = Some(format!("A câmera entregou {mismatched} quadros consecutivos {width}×{height}, diferente do formato solicitado {}×{}.", state.expected.0, state.expected.1));
                    }
                }
                return;
            }
            state.mismatched.store(0, Ordering::Relaxed);
            let stride = CVPixelBufferGetBytesPerRow(buffer);
            let base = CVPixelBufferGetBaseAddress(buffer) as *const u8;
            if CVPixelBufferGetPixelFormatType(buffer) != BGRA
                || base.is_null()
                || width == 0
                || height == 0
                || width > 16384
                || height > 16384
                || stride < width * 4
            {
                return;
            }
            let requested = state.prepared_request.lock().ok().and_then(|view| *view);
            let recording = state
                .recording_sink
                .lock()
                .ok()
                .and_then(|slot| slot.clone());
            let virtual_output = state.virtual_sink.lock().ok().and_then(|slot| slot.clone());
            let bounded = state.live_only.load(Ordering::Acquire) && recording.is_none();
            let fast_view = requested.filter(|view| {
                bounded
                    && view.rotation == 0
                    && !view.flip
                    && view.quad.is_none()
                    && view.aspect != crate::transform::Aspect::Portrait
                    && virtual_output
                        .as_ref()
                        .is_none_or(|(_, output)| output == view)
            });
            let mut cache = Vec::with_capacity(3);
            let rgb = if let Some(view) = fast_view {
                let frame = (|| -> Result<Frame, String> {
                    let (w, h, matrix) = crate::transform::render_plan(
                        width as u32,
                        height as u32,
                        &view,
                        Some(1920),
                    )?;
                    let mut slot = state
                        .live_processor
                        .lock()
                        .map_err(|_| "Processamento de chamada indisponível")?;
                    if slot.is_none() {
                        let obsolete = state.blur_processor.lock().ok().and_then(|mut p| p.take());
                        std::thread::spawn(move || {
                            drop(obsolete);
                            crate::quad::release_scratch();
                        });
                    }
                    let processor = slot.get_or_insert_with(crate::live_blur::Processor::default);
                    let bytes = std::slice::from_raw_parts(
                        base,
                        stride.checked_mul(height).ok_or("Stride inválido")?,
                    );
                    let pixels = processor.apply(
                        bytes,
                        stride,
                        (width as u32, height as u32),
                        (w, h),
                        matrix,
                        view.background_blur,
                        received_at,
                    )?;
                    Ok((w, h, pixels))
                })();
                cache.push((view, frame));
                Vec::new()
            } else {
                if let Ok(mut processor) = state.live_processor.try_lock() {
                    *processor = None;
                }
                let mut rgb = vec![0; width * height * 3];
                for row in 0..height {
                    let source = std::slice::from_raw_parts(base.add(row * stride), width * 4);
                    let target = &mut rgb[row * width * 3..(row + 1) * width * 3];
                    for (source, target) in source.chunks_exact(4).zip(target.chunks_exact_mut(3)) {
                        target.copy_from_slice(&[source[2], source[1], source[0]]);
                    }
                }
                rgb
            };
            if !state.cancelled.load(Ordering::Acquire) {
                if let Ok(mut instant) = state.last_valid_frame.lock() {
                    *instant = Some(Instant::now());
                }
                // Snapshot only tiny handles under their respective locks. No image
                // processing or encoder submission holds a control mutex.
                // At most three distinct views: UI, frozen recording, virtual output.
                // Matching views share one expensive perspective/crop computation.
                let limit = bounded.then_some(1920);
                let prepared = requested.map(|view| {
                    (
                        view,
                        cached_live_transform(
                            &mut cache,
                            &state.blur_processor,
                            width as u32,
                            height as u32,
                            &rgb,
                            view,
                            limit,
                        ),
                    )
                });
                if let Some((sink, view)) = recording {
                    let index = cached_live_transform(
                        &mut cache,
                        &state.blur_processor,
                        width as u32,
                        height as u32,
                        &rgb,
                        view,
                        limit,
                    );
                    let submitted = match &cache[index].1 {
                        Ok((_, _, transformed)) => sink.submit_owned(transformed.clone()),
                        Err(error) => Err(error.clone()),
                    };
                    if let Err(error) = submitted {
                        if let Ok(mut status) = state.recording_error.lock() {
                            *status = Some(error);
                        }
                    }
                }
                let mut source_fingerprint = None;
                if let Some((sink, view)) = virtual_output.filter(|(sink, _)| sink.wants_frame()) {
                    let index = cached_live_transform(
                        &mut cache,
                        &state.blur_processor,
                        width as u32,
                        height as u32,
                        &rgb,
                        view,
                        limit,
                    );
                    let submitted = match &cache[index].1 {
                        Ok((w, h, transformed)) => {
                            let mut pixels = transformed.clone();
                            if state.trace_frames {
                                use std::hash::{Hash, Hasher};
                                let mut hash = std::collections::hash_map::DefaultHasher::new();
                                pixels.hash(&mut hash);
                                source_fingerprint = Some(hash.finish());
                                if let Err(error) = crate::fluency_check::stamp(
                                    &mut pixels,
                                    *w,
                                    *h,
                                    sequence as u32,
                                ) {
                                    if let Ok(mut status) = state.virtual_error.lock() {
                                        *status = Some(error);
                                    }
                                    return;
                                }
                            }
                            sink.submit_rgb_at(*w, *h, pixels, received_at)
                        }
                        Err(error) => Err(error.clone()),
                    };
                    if let Err(error) = submitted {
                        if let Ok(mut status) = state.virtual_error.lock() {
                            *status = Some(error);
                        }
                    }
                }
                if state.cancelled.load(Ordering::Acquire) {
                    return;
                }
                if let Some((view, index)) = prepared {
                    // Move the prepared allocation into its bounded latest slot;
                    // queues above copied only when they need independent ownership.
                    match cache.swap_remove(index).1 {
                        Ok(frame) => {
                            if let Ok(mut time) = state.last_prepared_frame.lock() {
                                *time = Some(Instant::now());
                            }
                            if let Ok(mut slot) = state.prepared_frame.lock() {
                                *slot = Some((view, frame));
                            }
                            if let Ok(mut error) = state.processing_error.lock() {
                                *error = None;
                            }
                        }
                        Err(error) => {
                            if let Ok(mut time) = state.last_prepared_frame.lock() {
                                *time = None;
                            }
                            if let Ok(mut slot) = state.prepared_frame.lock() {
                                *slot = None;
                            }
                            if let Ok(mut status) = state.processing_error.lock() {
                                *status = Some(error);
                            }
                        }
                    }
                }
                if let Ok(mut slot) = state.frame.lock() {
                    *slot = (!rgb.is_empty()).then_some(TimedFrame {
                        image: (width as u32, height as u32, rgb),
                        pts_ns,
                        sequence,
                        received_at,
                    });
                }
                if let Ok(mut samples) = state.processing.lock() {
                    if samples.len() == 120 {
                        samples.pop_front();
                    }
                    samples.push_back(ProcessingSample {
                        sequence,
                        source_fingerprint,
                        received_ns: pts_ns.unwrap_or(0),
                        total_ms: received_at.elapsed().as_secs_f64() * 1000.0,
                        bounded,
                        live: state
                            .live_processor
                            .try_lock()
                            .ok()
                            .and_then(|p| p.as_ref().map(|p| p.stats())),
                        blur: state
                            .blur_processor
                            .lock()
                            .ok()
                            .and_then(|p| p.as_ref().map(|p| p.last_stats())),
                    });
                }
                state.frames.fetch_add(1, Ordering::Release);
            }
        }));
    }
    extern "C" fn delegate_dealloc(this: &mut Object, _: Sel) {
        unsafe {
            let pointer = *this.get_ivar::<usize>("rustState") as *const Shared;
            this.set_ivar("rustState", 0usize);
            if !pointer.is_null() {
                drop(Arc::from_raw(pointer));
            }
            let _: () = msg_send![super(this, class!(NSObject)), dealloc];
        }
    }
    fn delegate_class() -> &'static Class {
        static CLASS: OnceLock<&'static Class> = OnceLock::new();
        CLASS.get_or_init(|| {
            let mut class = ClassDecl::new("PixyControlVideoDelegate", class!(NSObject))
                .expect("unique capture delegate class");
            class.add_ivar::<usize>("rustState");
            if let Some(protocol) = Protocol::get("AVCaptureVideoDataOutputSampleBufferDelegate") {
                class.add_protocol(protocol);
            }
            unsafe {
                class.add_method(
                    sel!(captureOutput:didOutputSampleBuffer:fromConnection:),
                    frame_callback
                        as extern "C" fn(&Object, Sel, *mut Object, *mut c_void, *mut Object),
                );
                class.add_method(
                    sel!(dealloc),
                    delegate_dealloc as extern "C" fn(&mut Object, Sel),
                );
            }
            class.register()
        })
    }
    extern "C" fn drained(_: *mut c_void) {}
    struct Session {
        session: Owned,
        output: Owned,
        _delegate: Owned,
        queue: *mut c_void,
        state: Arc<Shared>,
        worker: Option<thread::JoinHandle<()>>,
    }
    impl Drop for Session {
        fn drop(&mut self) {
            unsafe {
                let _: () = msg_send![self.session.0, stopRunning];
                let _: () = msg_send![self.output.0, setSampleBufferDelegate:ptr::null_mut::<Object>() queue:ptr::null_mut::<c_void>()];
                // Serial barrier proves all prior callbacks have returned before the
                // delegate releases its Arc. No UI thread joins or queue suspension.
                dispatch_sync_f(self.queue, ptr::null_mut(), drained);
                dispatch_release(self.queue);
                self.state.incoming.close();
                if let Some(worker) = self.worker.take() {
                    let _ = worker.join();
                }
            }
        }
    }
    unsafe fn describe(object: *mut Object) -> String {
        if object.is_null() {
            return "nil".into();
        }
        let description: *mut Object = msg_send![object, description];
        string(description).replace('\n', " ")
    }
    unsafe fn trace_configuration(
        shared: &Shared,
        phase: &str,
        device: *mut Object,
        capture: &Session,
    ) {
        let active: *mut Object = msg_send![device, activeFormat];
        let description: *const c_void = msg_send![active, formatDescription];
        let dimensions = CMVideoFormatDescriptionGetDimensions(description);
        let minimum: CMTime = msg_send![device, activeVideoMinFrameDuration];
        let maximum: CMTime = msg_send![device, activeVideoMaxFrameDuration];
        let preset: *mut Object = msg_send![capture.session.0, sessionPreset];
        let settings: *mut Object = msg_send![capture.output.0, videoSettings];
        shared.note(format!("{phase}: active={}x{} subtype={:?} min_duration={}/{} max_duration={}/{} preset={} output={}",
            dimensions.width, dimensions.height, CMFormatDescriptionGetMediaSubType(description).to_be_bytes(),
            minimum.value, minimum.timescale, maximum.value, maximum.timescale, string(preset), describe(settings)));
        let connections: *mut Object = msg_send![capture.output.0, connections];
        let count: usize = msg_send![connections, count];
        for index in 0..count {
            let connection: *mut Object = msg_send![connections, objectAtIndex:index];
            let enabled: BOOL = msg_send![connection, isEnabled];
            let active: BOOL = msg_send![connection, isActive];
            shared.note(format!(
                "{phase}: connection[{index}] enabled={} active={} {}",
                enabled != NO,
                active != NO,
                describe(connection)
            ));
        }
    }
    unsafe fn open(
        shared: Arc<Shared>,
        width: u32,
        height: u32,
        fps: u32,
        target: Target,
    ) -> Result<(Session, f64), String> {
        let selected = select(width, height, fps, target)?;
        shared.note(format!(
            "selected request={width}x{height}@{fps}, fps={} duration={}/{} format={}",
            selected.actual_fps,
            selected.duration.value,
            selected.duration.timescale,
            describe(selected.format)
        ));
        let mut error: *mut Object = ptr::null_mut();
        let input_class = class!(AVCaptureDeviceInput);
        let input: *mut Object = msg_send![input_class, alloc];
        let input: *mut Object = msg_send![input, initWithDevice:selected.device error:&mut error];
        let input = Owned::new(input, "entrada da câmera")?;
        let session = Owned::new(msg_send![class!(AVCaptureSession), new], "sessão de vídeo")?;
        let output = Owned::new(
            msg_send![class!(AVCaptureVideoDataOutput), new],
            "saída de vídeo",
        )?;
        let delegate = Owned::new(msg_send![delegate_class(), new], "delegate de vídeo")?;
        (*delegate.0).set_ivar("rustState", Arc::into_raw(shared.clone()) as usize);
        // Interactive A/V deadlines must not inherit a background QoS
        // when the window is minimized. Attribute values are system-owned.
        let attributes = dispatch_queue_attr_make_with_qos_class(ptr::null(), 0x21, 0);
        if attributes.is_null() {
            return Err("Falha na prioridade da fila de vídeo".into());
        }
        let queue = dispatch_queue_create(c"local.pixy-control.frames".as_ptr(), attributes);
        if queue.is_null() {
            return Err("Falha ao criar fila de vídeo.".into());
        }
        let worker = if matches!(target, Target::PhysicalPixy) {
            let state = shared.clone();
            Some(thread::spawn(move || {
                let _ = crate::video_activity::prioritize_current_thread();
                while let Some(sample) = state.incoming.take() {
                    process_sample(&state, sample.buffer, sample.received);
                }
            }))
        } else {
            None
        };
        let capture = Session {
            session,
            output,
            _delegate: delegate,
            queue,
            state: shared.clone(),
            worker,
        };
        let _: () = msg_send![capture.session.0, beginConfiguration];
        let can_input: BOOL = msg_send![capture.session.0, canAddInput:input.0];
        let can_output: BOOL = msg_send![capture.session.0, canAddOutput:capture.output.0];
        if can_input == NO || can_output == NO {
            let _: () = msg_send![capture.session.0, commitConfiguration];
            return Err("A sessão recusou a entrada ou saída da Pixy.".into());
        }
        let _: () = msg_send![capture.session.0, addInput:input.0];
        // Ask for input-priority only after the input exists. A session's default
        // preset can otherwise overwrite a format chosen before addInput.
        let can_preset: BOOL =
            msg_send![capture.session.0, canSetSessionPreset:AVCaptureSessionPresetInputPriority];
        if can_preset != NO {
            let _: () =
                msg_send![capture.session.0, setSessionPreset:AVCaptureSessionPresetInputPriority];
        }
        let number: *mut Object = msg_send![class!(NSNumber), numberWithUnsignedInt:BGRA];
        let settings: *mut Object = msg_send![class!(NSDictionary), dictionaryWithObject:number forKey:kCVPixelBufferPixelFormatTypeKey];
        let _: () = msg_send![capture.output.0, setVideoSettings:settings];
        let _: () = msg_send![capture.output.0, setAlwaysDiscardsLateVideoFrames:YES];
        let _: () =
            msg_send![capture.output.0, setSampleBufferDelegate:capture._delegate.0 queue:queue];
        let _: () = msg_send![capture.session.0, addOutput:capture.output.0];
        shared.note(format!("canSetInputPriority={}", can_preset != NO));
        trace_configuration(
            &shared,
            "before_device_configuration",
            selected.device,
            &capture,
        );
        let locked: BOOL = msg_send![selected.device, lockForConfiguration:&mut error];
        if locked == NO {
            let _: () = msg_send![capture.session.0, commitConfiguration];
            return Err(error_message(error));
        }
        let configuration_lock = DeviceConfigurationLock(selected.device);
        let _: () = msg_send![selected.device, setActiveFormat:selected.format];
        let _: () = msg_send![selected.device, setActiveVideoMinFrameDuration:selected.duration];
        let _: () = msg_send![selected.device, setActiveVideoMaxFrameDuration:selected.duration];
        trace_configuration(&shared, "before_commit", selected.device, &capture);
        let _: () = msg_send![capture.session.0, commitConfiguration];
        trace_configuration(&shared, "after_commit", selected.device, &capture);
        let _: () = msg_send![capture.session.0, startRunning];
        trace_configuration(&shared, "after_start", selected.device, &capture);
        drop(configuration_lock);
        trace_configuration(&shared, "after_unlock", selected.device, &capture);
        let running: BOOL = msg_send![capture.session.0, isRunning];
        if running == NO {
            return Err("AVFoundation não iniciou a sessão da Pixy.".into());
        }
        Ok((capture, selected.actual_fps))
    }
    pub(super) fn run(
        shared: Arc<Shared>,
        width: u32,
        height: u32,
        fps: u32,
        target: Target,
    ) -> Result<(), String> {
        if width == 0 || height == 0 || fps == 0 || fps > 120 {
            return Err("Formato de vídeo inválido.".into());
        }
        unsafe {
            let _pool = Pool::new();
            shared.status("Consultando permissão da câmera…");
            authorize(&shared)?;
            if shared.cancelled.load(Ordering::Acquire) {
                return Ok(());
            }
            let _activity = crate::video_activity::Activity::begin()?;
            shared.status(format!("Iniciando prévia de {}…", target.label()));
            let (_session, actual_fps) = open(shared.clone(), width, height, fps, target)?;
            let mut last_frame = Instant::now();
            let mut count = 0;
            while !shared.cancelled.load(Ordering::Acquire) {
                if let Some(error) = shared
                    .failure
                    .lock()
                    .ok()
                    .and_then(|mut error| error.take())
                {
                    return Err(error);
                }
                let frames = shared.frames.load(Ordering::Acquire);
                if frames > count {
                    count = frames;
                    last_frame = Instant::now();
                    shared.status(format!(
                        "{} · {width}×{height} · {actual_fps:.2} fps",
                        target.label()
                    ));
                } else if last_frame.elapsed() > Duration::from_secs(8) {
                    return Err(format!(
                        "{} não entregou novos frames em8segundos.",
                        target.label()
                    ));
                }
                thread::sleep(Duration::from_millis(50));
            }
            Ok(())
        }
    }
    #[cfg(test)]
    mod cache_tests {
        use super::*;
        #[test]
        fn closed_window_live_processing_bounds_both_blur_input_and_shared_frame() {
            let rgb = vec![10; 16 * 8 * 3];
            let view = crate::transform::FrameTransform {
                zoom: 1.3,
                ..Default::default()
            };
            let mut cache = Vec::new();
            let processor = Mutex::new(None);
            let i = cached_live_transform(&mut cache, &processor, 16, 8, &rgb, view, Some(8));
            let (w, h, frame) = cache[i].1.as_ref().unwrap();
            assert_eq!((*w, *h, frame.len()), (8, 4, 96));
            let allocation = frame.as_ptr();
            let j = cached_live_transform(&mut cache, &processor, 16, 8, &rgb, view, Some(8));
            assert_eq!(i, j);
            assert_eq!(allocation, cache[j].1.as_ref().unwrap().2.as_ptr());
            let mut full = Vec::new();
            let j = cached_live_transform(&mut full, &processor, 16, 8, &rgb, view, None);
            assert_eq!(
                (full[j].1.as_ref().unwrap().0, full[j].1.as_ref().unwrap().1),
                (16, 8)
            );
        }
        #[test]
        fn matching_consumers_reuse_one_processed_allocation() {
            let rgb = vec![10; 24];
            let view = crate::transform::FrameTransform::default();
            let mut cache = Vec::new();
            let first = cached_transform(&mut cache, &Mutex::new(None), 4, 2, &rgb, view);
            let allocation = cache[first].1.as_ref().unwrap().2.as_ptr();
            let second = cached_transform(&mut cache, &Mutex::new(None), 4, 2, &rgb, view);
            assert_eq!(first, second);
            assert_eq!(cache.len(), 1);
            assert_eq!(allocation, cache[second].1.as_ref().unwrap().2.as_ptr());
            let rotated = crate::transform::FrameTransform {
                rotation: 1,
                ..view
            };
            let third = cached_transform(&mut cache, &Mutex::new(None), 4, 2, &rgb, rotated);
            assert_ne!(third, first);
            let (w, h, _) = cache[third].1.as_ref().unwrap();
            assert_eq!((*w, *h), (4, 2));
        }
    }
}

#[cfg(test)]
mod target_tests {
    use super::*;
    #[test]
    fn virtual_and_physical_targets_never_cross_or_fall_back() {
        let uid = "F3B9DD6A-3AA0-45D9-83D5-90A6FC254320";
        assert!(Target::PhysicalPixy.matches("EMEET PIXY", "0x1100000328f00c0"));
        assert!(Target::EmeetVirtual.matches("EMEET STUDIO Virtual Camera", uid));
        assert!(!Target::PhysicalPixy.matches("EMEET STUDIO Virtual Camera", uid));
        assert!(!Target::EmeetVirtual.matches("EMEET PIXY", "0x1100000328f00c0"));
        assert!(!Target::EmeetVirtual.matches("OBS Virtual Camera", uid));
        assert!(!Target::EmeetVirtual.matches("EMEET STUDIO Virtual Camera", "wrong-uid"));
        assert!(!Target::PhysicalPixy.matches("FaceTime HD Camera", "0x1100000328f00c0"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    fn preview() -> Preview {
        Preview {
            shared: Arc::new(Shared::new(4, 2)),
        }
    }
    #[test]
    fn taking_an_old_frame_does_not_make_it_fresh_and_stop_invalidates() {
        let mut preview = preview();
        *preview.shared.last_valid_frame.lock().unwrap() =
            Some(Instant::now() - Duration::from_secs(2));
        *preview.shared.frame.lock().unwrap() = Some(TimedFrame {
            image: (4, 2, vec![0; 24]),
            pts_ns: Some(42),
            sequence: 1,
            received_at: Instant::now(),
        });
        assert!(preview.latest().is_some());
        assert!(!preview.frame_fresh(Duration::from_secs(1)));
        *preview.shared.last_prepared_frame.lock().unwrap() = Some(Instant::now());
        assert!(preview.frame_fresh(Duration::from_secs(1)));
        preview.stop();
        assert!(!preview.frame_fresh(Duration::from_secs(1)));
    }
    #[test]
    fn prepared_frames_are_never_relabelled_with_new_framing() {
        let preview = preview();
        let old = crate::transform::FrameTransform::default();
        let new = crate::transform::FrameTransform { rotation: 1, ..old };
        *preview.shared.prepared_frame.lock().unwrap() = Some((old, (4, 2, vec![1; 24])));
        assert!(preview.latest_prepared(new).is_none());
        assert_eq!(*preview.shared.prepared_request.lock().unwrap(), Some(new));
        *preview.shared.prepared_frame.lock().unwrap() = Some((new, (2, 4, vec![2; 24])));
        assert_eq!(preview.latest_prepared(new), Some((2, 4, vec![2; 24])));
        assert!(preview.latest_prepared(new).is_none());
    }
}

#[cfg(test)]
mod processed_freshness_tests {
    use super::*;
    #[test]
    fn fresh_raw_frames_cannot_make_failed_blur_output_ready() {
        let shared = Arc::new(Shared::new(2, 2));
        *shared.last_valid_frame.lock().unwrap() = Some(std::time::Instant::now());
        *shared.processing_error.lock().unwrap() = Some("Vision unavailable".into());
        let preview = Preview { shared };
        assert!(!preview.frame_fresh(std::time::Duration::from_secs(1)));
    }
}

#[cfg(test)]
mod timed_frame_tests {
    use super::*;
    #[test]
    fn pixels_keep_their_own_pts_even_when_global_timing_has_advanced() {
        let shared = Arc::new(Shared::new(2, 2));
        *shared.frame.lock().unwrap() = Some(TimedFrame {
            image: (2, 2, vec![7; 12]),
            pts_ns: Some(100),
            sequence: 3,
            received_at: std::time::Instant::now(),
        });
        shared.timing.lock().unwrap().last_pts_ns = Some(200);
        let preview = Preview { shared };
        let packet = preview.latest_timed().unwrap();
        assert_eq!(
            (packet.pts_ns, packet.sequence, packet.image.2),
            (Some(100), 3, vec![7; 12])
        );
        assert!(preview.latest_timed().is_none());
    }
}
