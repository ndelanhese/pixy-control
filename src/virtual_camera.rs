//! Optional producer for the already installed, approved EMEET CMIO extension.
//! Does not install, activate, re-sign or modify a camera extension.
#![allow(unexpected_cfgs)]
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Condvar, Mutex, OnceLock,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;
struct InputFrame {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
    sequence: u64,
    generation: u64,
    received: std::time::Instant,
}
#[derive(Clone)]
struct PreparedFrame {
    bgra: Arc<Vec<u8>>,
    sequence: u64,
    generation: u64,
    received: std::time::Instant,
}
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct PublicationSample {
    pub tick: u64,
    pub sequence: Option<u64>,
    pub scheduled_pts_ns: i64,
    pub actual_host_ns: i64,
    pub accepted: bool,
    pub source_age_ms: Option<f64>,
    pub gap_ms: Option<f64>,
    pub lateness_ms: f64,
    pub send_ms: f64,
    pub queue_depth: u32,
    pub queue_capacity: u32,
}
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct OutputMetrics {
    pub received_frames: u64,
    pub input_replacements: u64,
    pub converted_frames: u64,
    pub obsolete_conversions: u64,
    pub publication_attempts: u64,
    pub enqueued_frames: u64,
    pub unique_enqueued_frames: u64,
    pub repeated_enqueued_frames: u64,
    pub placeholder_enqueued_frames: u64,
    pub queue_rejections: u64,
    pub skipped_ticks: u64,
    pub conversion_last_ms: f64,
    pub conversion_max_ms: f64,
    pub send_max_ms: f64,
    pub gap_max_ms: f64,
    pub lateness_max_ms: f64,
    pub recent: std::collections::VecDeque<PublicationSample>,
}
#[derive(Default)]
struct PublicationClock {
    next: u64,
    pending_skips: u64,
}
struct PublicationTick {
    number: u64,
    skipped: u64,
}
impl PublicationClock {
    fn scheduled(number: u64) -> Duration {
        Duration::from_nanos(
            ((u128::from(number) * 1_000_000_000) / 30).min(u128::from(u64::MAX)) as u64,
        )
    }
    fn due(&mut self, elapsed: Duration) -> Option<PublicationTick> {
        let number = (elapsed.as_nanos() * 30 / 1_000_000_000).min(u128::from(u64::MAX)) as u64;
        if number < self.next {
            return None;
        }
        let skipped = number - self.next + self.pending_skips;
        self.pending_skips = 0;
        self.next = number.saturating_add(1);
        // Do not emit a short catch-up burst after an oversleep. If the next
        // grid slot is less than half a frame away, omit it explicitly.
        if Self::scheduled(self.next).saturating_sub(elapsed)
            < Duration::from_nanos(1_000_000_000 / 60)
        {
            self.next = self.next.saturating_add(1);
            self.pending_skips = 1;
        }
        Some(PublicationTick { number, skipped })
    }
}
#[cfg(test)]
fn latest_converted(shared: &Shared) -> Option<PreparedFrame> {
    shared
        .converted
        .lock()
        .ok()?
        .back()
        .filter(|frame| valid_prepared(shared, frame))
        .cloned()
}
fn valid_prepared(shared: &Shared, frame: &PreparedFrame) -> bool {
    frame.generation == shared.generation.load(Ordering::Acquire)
        && !shared.placeholder.load(Ordering::Acquire)
        && frame.received.elapsed() < Duration::from_secs(1)
}
#[derive(Default)]
struct Playout {
    last: Option<PreparedFrame>,
    primed: bool,
}
impl Playout {
    fn next(&mut self, shared: &Shared) -> Option<PreparedFrame> {
        if self
            .last
            .as_ref()
            .is_some_and(|frame| !valid_prepared(shared, frame))
        {
            self.last = None;
            self.primed = false;
        }
        let mut queue = shared.converted.lock().ok()?;
        queue.retain(|frame| valid_prepared(shared, frame));
        // One-frame reserve absorbs conversion jitter at equal input/output FPS.
        // At most two ready frames; the queue cannot grow into call latency.
        if !self.primed
            && (queue.len() >= 2
                || queue
                    .front()
                    .is_some_and(|frame| frame.received.elapsed() >= Duration::from_millis(70)))
        {
            self.primed = true;
        }
        if self.primed {
            if let Some(next) = queue.pop_front() {
                self.last = Some(next);
            }
        }
        self.last.clone()
    }
}
fn conversion_loop(
    shared: Arc<Shared>,
    mut convert: impl FnMut(u32, u32, Vec<u8>) -> Result<Vec<u8>, String>,
) -> Result<(), String> {
    while !shared.stop.load(Ordering::Acquire) {
        let input = {
            let slot = shared
                .frame
                .lock()
                .map_err(|_| "Falha na fila de conversão da V-Cam")?;
            let (mut slot, _) = shared
                .wake
                .wait_timeout_while(slot, Duration::from_millis(50), |slot| {
                    slot.is_none() && !shared.stop.load(Ordering::Acquire)
                })
                .map_err(|_| "Falha ao aguardar conversão da V-Cam")?;
            slot.take()
        };
        let Some(input) = input else { continue };
        if input.generation != shared.generation.load(Ordering::Acquire)
            || shared.placeholder.load(Ordering::Acquire)
        {
            continue;
        }
        let started = std::time::Instant::now();
        let result = convert(input.width, input.height, input.rgb);
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        let bgra = match result {
            Ok(bgra) => bgra,
            Err(error) => {
                if let Ok(mut status) = shared.status.lock() {
                    *status = error.clone();
                }
                shared.stop.store(true, Ordering::Release);
                return Err(error);
            }
        };
        let current = input.generation == shared.generation.load(Ordering::Acquire)
            && !shared.placeholder.load(Ordering::Acquire);
        if let Ok(mut metrics) = shared.metrics.lock() {
            metrics.converted_frames += 1;
            metrics.conversion_last_ms = elapsed;
            metrics.conversion_max_ms = metrics.conversion_max_ms.max(elapsed);
            if !current {
                metrics.obsolete_conversions += 1;
            }
        }
        if current {
            let converted = PreparedFrame {
                bgra: Arc::new(bgra),
                sequence: input.sequence,
                generation: input.generation,
                received: input.received,
            };
            if let Ok(mut slot) = shared.converted.lock() {
                if slot.len() == 2 {
                    slot.pop_front();
                }
                slot.push_back(converted);
            }
        }
    }
    Ok(())
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub enum ConsumerState {
    Known { count: u32 },
    Unknown { reason: String },
}
impl ConsumerState {
    pub fn active(&self) -> Option<bool> {
        match self {
            Self::Known { count } => Some(*count > 0),
            Self::Unknown { .. } => None,
        }
    }
}
/// Read this only in a fresh metadata-only helper process. The installed
/// extension does not invalidate this process-local CMIO property cache.
pub fn raw_consumer_count() -> Result<u32, String> {
    #[cfg(target_os = "macos")]
    {
        native::raw_consumer_count()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("Contador EMEET disponível no macOS".into())
    }
}
/// Reads only the exact installed EMEET virtual device. Does not start capture.
pub fn consumer_state() -> ConsumerState {
    #[cfg(target_os = "macos")]
    {
        usage_cache().get(native::fresh_consumer_state)
    }
    #[cfg(not(target_os = "macos"))]
    {
        ConsumerState::Unknown {
            reason: "V-Cam disponível no macOS".into(),
        }
    }
}
struct UsageCache(Mutex<Option<(std::time::Instant, ConsumerState)>>);
impl UsageCache {
    fn get(&self, query: impl FnOnce() -> ConsumerState) -> ConsumerState {
        let Ok(mut cached) = self.0.lock() else {
            return ConsumerState::Unknown {
                reason: "Falha no cache de uso da V-Cam".into(),
            };
        };
        if let Some((updated, value)) = cached.as_ref() {
            if updated.elapsed() < Duration::from_millis(750) {
                return value.clone();
            }
        }
        // Hold the process-global lock through refresh: overlapping UI/producer
        // pollers share one child rather than launching duplicate helpers.
        let value = query();
        *cached = Some((std::time::Instant::now(), value.clone()));
        value
    }
}
fn usage_cache() -> &'static UsageCache {
    static CACHE: OnceLock<UsageCache> = OnceLock::new();
    CACHE.get_or_init(|| UsageCache(Mutex::new(None)))
}
#[cfg(target_os = "macos")]
fn read_usage_process(mut command: std::process::Command, timeout: Duration) -> ConsumerState {
    use std::{io::Read, os::fd::AsRawFd, process::Stdio, time::Instant};
    let checked = (|| -> Result<u32, String> {
        let started = Instant::now();
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("Falha na consulta de uso da V-Cam: {error}"))?;
        let mut output = match child.stdout.take() {
            Some(output) => output,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Consulta sem saída JSON".into());
            }
        };
        unsafe extern "C" {
            fn fcntl(fd: i32, cmd: i32, ...) -> i32;
        }
        let flags = unsafe { fcntl(output.as_raw_fd(), 3) };
        if flags < 0 || unsafe { fcntl(output.as_raw_fd(), 4, flags | 0x4) } < 0 {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Não foi possível limitar a leitura da consulta".into());
        }
        let mut bytes = Vec::with_capacity(128);
        let mut buffer = [0u8; 1024];
        let mut eof = false;
        let mut oversized = false;
        let mut exited = None;
        let result = loop {
            loop {
                match output.read(&mut buffer) {
                    Ok(0) => {
                        eof = true;
                        break;
                    }
                    Ok(size) => {
                        if bytes.len() + size > 4096 {
                            oversized = true;
                            break;
                        }
                        bytes.extend_from_slice(&buffer[..size]);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(error.to_string());
                    }
                }
            }
            if oversized {
                break Err("Saída da consulta excedeu4096bytes".into());
            }
            if exited.is_none() {
                match child.try_wait() {
                    Ok(status) => exited = status,
                    Err(error) => break Err(error.to_string()),
                }
            }
            if let Some(status) = exited {
                if !status.success() {
                    break Err(format!("Consulta de uso encerrou com{status}"));
                }
                if eof {
                    break serde_json::from_slice::<Result<u32, String>>(&bytes)
                        .map_err(|error| format!("JSON inválido na consulta de uso: {error}"))
                        .and_then(|result| result);
                }
            }
            if started.elapsed() >= timeout {
                break Err("Consulta de uso da V-Cam excedeu1,2segundos".into());
            }
            thread::sleep(Duration::from_millis(10));
        };
        if result.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        result
    })();
    match checked {
        Ok(count) => ConsumerState::Known { count },
        Err(reason) => ConsumerState::Unknown { reason },
    }
}
fn placeholder_bgra(width: u32, height: u32) -> Vec<u8> {
    [28, 28, 28, 255].repeat(width as usize * height as usize)
}
fn set_placeholder(shared: &Shared, paused: bool) {
    if let Ok(mut frame) = shared.frame.lock() {
        if shared.placeholder.load(Ordering::Acquire) == paused {
            return;
        }
        shared.placeholder.store(paused, Ordering::Release);
        shared.generation.fetch_add(1, Ordering::AcqRel);
        *frame = None;
        if let Ok(mut queue) = shared.converted.lock() {
            queue.clear();
        }
    }
}
#[derive(Clone)]
pub struct VirtualSink {
    shared: Arc<Shared>,
}
struct Shared {
    frame: Mutex<Option<InputFrame>>,
    wake: Condvar,
    converted: Mutex<std::collections::VecDeque<PreparedFrame>>,
    conversion_scratch: Mutex<Vec<u8>>,
    sequence: AtomicU64,
    metrics: Mutex<OutputMetrics>,
    status: Mutex<String>,
    stop: AtomicBool,
    placeholder: AtomicBool,
    generation: AtomicU64,
    consumers: Mutex<ConsumerState>,
}
impl Shared {
    fn new() -> Self {
        Self {
            frame: Mutex::new(None),
            wake: Condvar::new(),
            converted: Mutex::new(std::collections::VecDeque::new()),
            conversion_scratch: Mutex::new(Vec::new()),
            sequence: AtomicU64::new(0),
            metrics: Mutex::new(OutputMetrics::default()),
            status: Mutex::new("Iniciando V-Cam local…".into()),
            stop: AtomicBool::new(false),
            placeholder: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            consumers: Mutex::new(ConsumerState::Unknown {
                reason: "Aguardando consulta à extensão".into(),
            }),
        }
    }
}
pub struct VirtualCamera {
    sink: VirtualSink,
    worker: Option<JoinHandle<Result<(), String>>>,
}
impl VirtualSink {
    pub fn wants_frame(&self) -> bool {
        self.shared
            .consumers
            .lock()
            .map_or(true, |usage| usage.active() != Some(false))
    }
    /// Single newest frame slot; slow output replaces old frames, never blocks capture.
    pub fn submit_rgb(&self, w: u32, h: u32, rgb: Vec<u8>) -> Result<(), String> {
        validate_rgb(w, h, rgb.len())?;
        if !self.wants_frame() {
            return Ok(());
        }
        if self.shared.stop.load(Ordering::Acquire) {
            return Err("V-Cam encerrada".into());
        }
        match self.shared.frame.try_lock() {
            Ok(mut frame) => {
                if !self.shared.placeholder.load(Ordering::Acquire) {
                    let sequence = self.shared.sequence.fetch_add(1, Ordering::AcqRel) + 1;
                    if let Ok(mut metrics) = self.shared.metrics.lock() {
                        metrics.received_frames += 1;
                        if frame.is_some() {
                            metrics.input_replacements += 1;
                        }
                    }
                    *frame = Some(InputFrame {
                        width: w,
                        height: h,
                        rgb,
                        sequence,
                        generation: self.shared.generation.load(Ordering::Acquire),
                        received: std::time::Instant::now(),
                    });
                    self.shared.wake.notify_one();
                }
                Ok(())
            }
            Err(std::sync::TryLockError::WouldBlock) => Ok(()),
            Err(_) => Err("Falha na fila da V-Cam".into()),
        }
    }
    pub fn status(&self) -> String {
        self.shared
            .status
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|_| "Falha na V-Cam".into())
    }
}
impl VirtualCamera {
    pub fn start() -> Result<Self, String> {
        #[cfg(not(target_os = "macos"))]
        return Err("V-Cam disponível no macOS".into());
        #[cfg(target_os = "macos")]
        {
            let shared = Arc::new(Shared::new());
            let state = shared.clone();
            let (ready, result) = std::sync::mpsc::sync_channel(1);
            let worker = thread::spawn(move || {
                let outcome = native::run(state.clone(), ready);
                if let Ok(mut status) = state.status.lock() {
                    *status = match &outcome {
                        Ok(()) => "V-Cam encerrada".into(),
                        Err(e) => e.clone(),
                    };
                }
                state.stop.store(true, Ordering::Release);
                outcome
            });
            match result.recv_timeout(Duration::from_secs(5)) {
                Ok(Ok(())) => Ok(Self {
                    sink: VirtualSink { shared },
                    worker: Some(worker),
                }),
                Ok(Err(e)) => {
                    shared.stop.store(true, Ordering::Release);
                    let _ = worker.join();
                    Err(e)
                }
                Err(_) => {
                    shared.stop.store(true, Ordering::Release);
                    Err("V-Cam não respondeu ao iniciar; verifique a extensão EMEET nos Ajustes do Sistema".into())
                }
            }
        }
    }
    pub fn sink(&self) -> VirtualSink {
        self.sink.clone()
    }
    pub fn status(&self) -> String {
        self.sink.status()
    }
    pub fn metrics(&self) -> OutputMetrics {
        self.sink
            .shared
            .metrics
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default()
    }
    pub fn is_finished(&self) -> bool {
        self.sink.shared.stop.load(Ordering::Acquire)
    }
    /// Keep the same CMIO producer session while capture is paused/restarted.
    /// A neutral local frame replaces live output; no camera access is needed.
    pub fn set_placeholder(&self, paused: bool) {
        set_placeholder(&self.sink.shared, paused);
    }
    pub fn consumer_state(&self) -> ConsumerState {
        if self.sink.shared.stop.load(Ordering::Acquire) {
            return ConsumerState::Unknown {
                reason: "Sessão da V-Cam encerrada".into(),
            };
        }
        self.sink
            .shared
            .consumers
            .lock()
            .map(|value| value.clone())
            .unwrap_or_else(|_| ConsumerState::Unknown {
                reason: "Falha no estado da V-Cam".into(),
            })
    }
    pub fn stop(&mut self) -> Result<(), String> {
        self.sink.shared.stop.store(true, Ordering::Release);
        self.worker.take().map_or(Ok(()), |worker| {
            worker
                .join()
                .unwrap_or_else(|_| Err("Falha na thread da V-Cam".into()))
        })
    }
}
impl Drop for VirtualCamera {
    fn drop(&mut self) {
        self.sink.shared.stop.store(true, Ordering::Release);
    }
}
fn validate_rgb(w: u32, h: u32, len: usize) -> Result<(), String> {
    if w == 0
        || h == 0
        || w > 16384
        || h > 16384
        || (w as usize)
            .checked_mul(h as usize)
            .and_then(|n| n.checked_mul(3))
            != Some(len)
    {
        return Err("Frame RGB inválido para V-Cam".into());
    }
    Ok(())
}
#[cfg(test)]
fn letterbox_bgra(w: u32, h: u32, rgb: Vec<u8>, tw: u32, th: u32) -> Result<Vec<u8>, String> {
    #[cfg(target_os = "macos")]
    {
        accelerated_letterbox(w, h, &rgb, tw, th)
    }
    #[cfg(not(target_os = "macos"))]
    {
        reference_letterbox_bgra(w, h, rgb, tw, th)
    }
}
#[cfg(not(target_os = "macos"))]
fn reference_letterbox_bgra(
    w: u32,
    h: u32,
    rgb: Vec<u8>,
    tw: u32,
    th: u32,
) -> Result<Vec<u8>, String> {
    validate_rgb(w, h, rgb.len())?;
    if tw == 0 || th == 0 || tw > 16384 || th > 16384 {
        return Err("Dimensões de saída inválidas".into());
    }
    let source = image::RgbImage::from_raw(w, h, rgb).ok_or("Buffer RGB inválido")?;
    let scale = (f64::from(tw) / f64::from(w)).min(f64::from(th) / f64::from(h));
    let rw = (f64::from(w) * scale).round().max(1.0).min(f64::from(tw)) as u32;
    let rh = (f64::from(h) * scale).round().max(1.0).min(f64::from(th)) as u32;
    let resized = if (rw, rh) == (w, h) {
        source
    } else {
        image::imageops::resize(&source, rw, rh, image::imageops::FilterType::Triangle)
    };
    let mut output = vec![0; tw as usize * th as usize * 4];
    for pixel in output.chunks_exact_mut(4) {
        pixel[3] = 255;
    }
    let (left, top) = ((tw - rw) / 2, (th - rh) / 2);
    for (x, y, pixel) in resized.enumerate_pixels() {
        let offset = ((y + top) as usize * tw as usize + (x + left) as usize) * 4;
        output[offset..offset + 4].copy_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
    }
    Ok(output)
}
#[cfg(all(target_os = "macos", test))]
fn accelerated_letterbox(w: u32, h: u32, rgb: &[u8], tw: u32, th: u32) -> Result<Vec<u8>, String> {
    accelerated_letterbox_reusing(w, h, rgb, tw, th, &mut Vec::new())
}
#[cfg(target_os = "macos")]
fn accelerated_letterbox_reusing(
    w: u32,
    h: u32,
    rgb: &[u8],
    tw: u32,
    th: u32,
    converted: &mut Vec<u8>,
) -> Result<Vec<u8>, String> {
    use std::{ffi::c_void, ptr};
    #[repr(C)]
    struct Buffer {
        data: *mut c_void,
        height: usize,
        width: usize,
        row_bytes: usize,
    }
    #[link(name = "Accelerate", kind = "framework")]
    unsafe extern "C" {
        fn vImageConvert_RGB888toBGRA8888(
            src: *const Buffer,
            alpha: *const Buffer,
            value: u8,
            dest: *const Buffer,
            premultiply: bool,
            flags: u32,
        ) -> isize;
        fn vImageScale_ARGB8888(
            src: *const Buffer,
            dest: *const Buffer,
            temp: *mut c_void,
            flags: u32,
        ) -> isize;
    }
    validate_rgb(w, h, rgb.len())?;
    if tw == 0 || th == 0 || tw > 16384 || th > 16384 {
        return Err("Dimensões de saída inválidas".into());
    }
    let scale = (f64::from(tw) / f64::from(w)).min(f64::from(th) / f64::from(h));
    let rw = (f64::from(w) * scale).round().max(1.0).min(f64::from(tw)) as usize;
    let rh = (f64::from(h) * scale).round().max(1.0).min(f64::from(th)) as usize;
    let (left, top) = ((tw as usize - rw) / 2, (th as usize - rh) / 2);
    let mut output = vec![0u8; tw as usize * th as usize * 4];
    if (rw, rh) != (tw as usize, th as usize) {
        for pixel in output.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
    }
    let source = Buffer {
        data: rgb.as_ptr() as *mut c_void,
        height: h as usize,
        width: w as usize,
        row_bytes: w as usize * 3,
    };
    // Destination is a centered view directly into the final canvas, retaining
    // its full row stride. No extra resized-image allocation or row copy.
    let target = Buffer {
        data: unsafe { output.as_mut_ptr().add((top * tw as usize + left) * 4) }.cast(),
        height: rh,
        width: rw,
        row_bytes: tw as usize * 4,
    };
    let code = if (rw, rh) == (w as usize, h as usize) {
        unsafe { vImageConvert_RGB888toBGRA8888(&source, ptr::null(), 255, &target, false, 16) }
    } else {
        converted.resize(w as usize * h as usize * 4, 0);
        let bgra = Buffer {
            data: converted.as_mut_ptr().cast(),
            height: h as usize,
            width: w as usize,
            row_bytes: w as usize * 4,
        };
        let conversion_code =
            unsafe { vImageConvert_RGB888toBGRA8888(&source, ptr::null(), 255, &bgra, false, 16) };
        if conversion_code != 0 {
            return Err(format!(
                "V-Cam: conversão de cor Accelerate ({conversion_code})"
            ));
        }
        // SDK Geometry.h explicitly supports BGRA ordering with this4-channel
        // scaler. kvImageDoNotTile(16) keeps this bounded video worker serial,
        // avoiding internal worker scheduling spikes; resampling is unchanged.
        unsafe { vImageScale_ARGB8888(&bgra, &target, ptr::null_mut(), 16) }
    };
    if code != 0 {
        return Err(format!("V-Cam: redimensionamento Accelerate ({code})"));
    }
    Ok(output)
}
fn copy_rows(w: usize, h: usize, src: &[u8], stride: usize, dst: &mut [u8]) -> Result<(), String> {
    let row = w.checked_mul(4).ok_or("Linha BGRA grande demais")?;
    let size = row.checked_mul(h).ok_or("Frame BGRA grande demais")?;
    let end = h
        .checked_sub(1)
        .and_then(|n| n.checked_mul(stride))
        .and_then(|n| n.checked_add(row))
        .ok_or("Dimensões BGRA inválidas")?;
    if w == 0 || src.len() != size || stride < row || dst.len() < end {
        return Err("Stride ou buffer BGRA inválido".into());
    }
    for y in 0..h {
        dst[y * stride..y * stride + row].copy_from_slice(&src[y * row..(y + 1) * row]);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    use std::{
        ffi::{c_char, c_void},
        mem, ptr,
        time::Instant,
    };
    const WIDTH: u32 = 1920;
    const HEIGHT: u32 = 1080;
    const NAME: &str = "EMEET STUDIO Virtual Camera";
    const UID: &str = "F3B9DD6A-3AA0-45D9-83D5-90A6FC254320";
    const BGRA: u32 = u32::from_be_bytes(*b"BGRA");
    const GLOBAL: u32 = u32::from_be_bytes(*b"glob");
    const ACTIVATION:&str="V-Cam EMEET indisponível. Ative a extensão EMEET STUDIO em Ajustes do Sistema → Geral → Itens de Início e Extensões → Extensões de Câmera. O Pixy Control não instala nem ativa extensões.";
    #[repr(C)]
    struct Address {
        selector: u32,
        scope: u32,
        element: u32,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Time {
        value: i64,
        timescale: i32,
        flags: u32,
        epoch: i64,
    }
    #[repr(C)]
    struct Timing {
        duration: Time,
        presentation: Time,
        decode: Time,
    }
    #[repr(C)]
    struct Dimensions {
        width: i32,
        height: i32,
    }
    #[link(name = "CoreMediaIO", kind = "framework")]
    extern "C" {
        fn CMIOObjectGetPropertyDataSize(
            object: u32,
            address: *const Address,
            qualifier_size: u32,
            qualifier: *const c_void,
            size: *mut u32,
        ) -> i32;
        fn CMIOObjectGetPropertyData(
            object: u32,
            address: *const Address,
            qualifier_size: u32,
            qualifier: *const c_void,
            size: u32,
            used: *mut u32,
            data: *mut c_void,
        ) -> i32;
        fn CMIOStreamCopyBufferQueue(
            stream: u32,
            callback: Option<extern "C" fn(u32, *mut c_void, *mut c_void)>,
            context: *mut c_void,
            queue: *mut *mut c_void,
        ) -> i32;
        fn CMIODeviceStartStream(device: u32, stream: u32) -> i32;
        fn CMIODeviceStopStream(device: u32, stream: u32) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(object: *const c_void);
        fn CFStringGetCString(
            string: *const c_void,
            buffer: *mut c_char,
            size: isize,
            encoding: u32,
        ) -> u8;
        fn CFDictionaryCreateMutable(
            allocator: *const c_void,
            capacity: isize,
            keys: *const c_void,
            values: *const c_void,
        ) -> *mut c_void;
        fn CFDictionarySetValue(dict: *mut c_void, key: *const c_void, value: *const c_void);
        static kCFTypeDictionaryKeyCallBacks: u8;
        static kCFTypeDictionaryValueCallBacks: u8;
    }
    #[link(name = "CoreVideo", kind = "framework")]
    extern "C" {
        static kCVPixelBufferIOSurfacePropertiesKey: *const c_void;
        static kCVPixelBufferWidthKey: *const c_void;
        static kCVPixelBufferHeightKey: *const c_void;
        static kCVPixelBufferPixelFormatTypeKey: *const c_void;
        static kCVPixelBufferPoolAllocationThresholdKey: *const c_void;
        fn CVPixelBufferPoolCreate(
            allocator: *const c_void,
            pool_attributes: *const c_void,
            pixel_attributes: *const c_void,
            pool: *mut *mut c_void,
        ) -> i32;
        fn CVPixelBufferPoolFlush(pool: *mut c_void, flags: u64);
        fn CVPixelBufferPoolCreatePixelBufferWithAuxAttributes(
            allocator: *const c_void,
            pool: *mut c_void,
            aux: *const c_void,
            buffer: *mut *mut c_void,
        ) -> i32;
        fn CVPixelBufferLockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
        fn CVPixelBufferUnlockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
        fn CVPixelBufferGetBaseAddress(buffer: *mut c_void) -> *mut c_void;
        fn CVPixelBufferGetBytesPerRow(buffer: *mut c_void) -> usize;
        fn CVPixelBufferGetDataSize(buffer: *mut c_void) -> usize;
    }
    #[link(name = "CoreMedia", kind = "framework")]
    extern "C" {
        fn CMVideoFormatDescriptionGetDimensions(format: *const c_void) -> Dimensions;
        fn CMFormatDescriptionGetMediaSubType(format: *const c_void) -> u32;
        fn CMVideoFormatDescriptionCreateForImageBuffer(
            allocator: *const c_void,
            buffer: *mut c_void,
            format: *mut *mut c_void,
        ) -> i32;
        fn CMSampleBufferCreateReadyWithImageBuffer(
            allocator: *const c_void,
            buffer: *mut c_void,
            format: *const c_void,
            timing: *const Timing,
            sample: *mut *mut c_void,
        ) -> i32;
        fn CMSimpleQueueGetCount(queue: *mut c_void) -> i32;
        fn CMSimpleQueueGetCapacity(queue: *mut c_void) -> i32;
        fn CMSimpleQueueEnqueue(queue: *mut c_void, element: *const c_void) -> i32;
        fn CMClockGetHostTimeClock() -> *mut c_void;
        fn CMClockGetTime(clock: *mut c_void) -> Time;
    }
    unsafe extern "C" {
        fn pthread_set_qos_class_self_np(qos: u32, relative_priority: i32) -> i32;
    }
    fn video_qos(qos: u32) -> Result<(), String> {
        // SDK sys/qos.h: interactive=0x21, initiated=0x19, default=0x15.
        // Standard QoS only; no real-time policy or sleep assertions.
        let code = unsafe { pthread_set_qos_class_self_np(qos, 0) };
        if code == 0 {
            Ok(())
        } else {
            Err(format!("V-Cam: prioridade de vídeo ({code})"))
        }
    }
    struct Owned(*mut c_void);
    impl Drop for Owned {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0) }
            }
        }
    }
    struct Pool(*mut Object);
    impl Drop for Pool {
        fn drop(&mut self) {
            unsafe {
                let _: () = msg_send![self.0, drain];
            }
        }
    }
    fn check(code: i32, operation: &str) -> Result<(), String> {
        if code == 0 {
            Ok(())
        } else {
            Err(format!("V-Cam: {operation} (OSStatus {code})"))
        }
    }
    unsafe fn property<T: Copy>(id: u32, selector: [u8; 4]) -> Result<T, String> {
        let address = Address {
            selector: u32::from_be_bytes(selector),
            scope: GLOBAL,
            element: 0,
        };
        let mut data = mem::MaybeUninit::<T>::uninit();
        let mut used = 0;
        check(
            CMIOObjectGetPropertyData(
                id,
                &address,
                0,
                ptr::null(),
                mem::size_of::<T>() as u32,
                &mut used,
                data.as_mut_ptr().cast(),
            ),
            "ler propriedade",
        )?;
        if used != mem::size_of::<T>() as u32 {
            return Err("V-Cam: propriedade incompleta".into());
        }
        Ok(data.assume_init())
    }
    unsafe fn ids(id: u32, selector: [u8; 4]) -> Result<Vec<u32>, String> {
        let address = Address {
            selector: u32::from_be_bytes(selector),
            scope: GLOBAL,
            element: 0,
        };
        let mut size = 0;
        check(
            CMIOObjectGetPropertyDataSize(id, &address, 0, ptr::null(), &mut size),
            "enumerar dispositivos/streams",
        )?;
        if size % 4 != 0 || size > 65536 {
            return Err("V-Cam: lista de dispositivos inválida".into());
        }
        let mut result = vec![0u32; (size / 4) as usize];
        let mut used = 0;
        if size > 0 {
            check(
                CMIOObjectGetPropertyData(
                    id,
                    &address,
                    0,
                    ptr::null(),
                    size,
                    &mut used,
                    result.as_mut_ptr().cast(),
                ),
                "ler dispositivos/streams",
            )?;
            if used > size || used % 4 != 0 {
                return Err("V-Cam: lista incompleta".into());
            }
            result.truncate((used / 4) as usize);
        }
        Ok(result)
    }
    unsafe fn text(id: u32, selector: [u8; 4]) -> Result<String, String> {
        // CMIOHardwareObject/Device headers: returned CFString is caller-owned.
        let value = Owned(property::<*mut c_void>(id, selector)?);
        if value.0.is_null() {
            return Err("V-Cam: nome/UID ausente".into());
        }
        let mut bytes = vec![0u8; 1024];
        if CFStringGetCString(
            value.0,
            bytes.as_mut_ptr().cast(),
            bytes.len() as isize,
            0x08000100,
        ) == 0
        {
            return Err("V-Cam: texto inválido".into());
        }
        let end = bytes.iter().position(|v| *v == 0).unwrap_or(bytes.len());
        Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }
    unsafe fn discover_device() -> Result<u32, String> {
        let mut matches = Vec::new();
        for device in ids(1, *b"dev#")? {
            if text(device, *b"lnam").ok().as_deref() == Some(NAME)
                && text(device, *b"uid ").is_ok_and(|v| v.eq_ignore_ascii_case(UID))
            {
                matches.push(device);
            }
        }
        let device = match matches.as_slice() {
            [one] => *one,
            [] => return Err(ACTIVATION.into()),
            _ => return Err("V-Cam: mais de um dispositivo EMEET idêntico".into()),
        };
        Ok(device)
    }
    pub(super) fn fresh_consumer_state() -> ConsumerState {
        let executable = match std::env::current_exe() {
            Ok(path) => path,
            Err(error) => {
                return ConsumerState::Unknown {
                    reason: error.to_string(),
                }
            }
        };
        let mut command = std::process::Command::new(executable);
        command.arg("--read-virtual-usage");
        read_usage_process(command, Duration::from_millis(1200))
    }
    pub(super) fn raw_consumer_count() -> Result<u32, String> {
        unsafe {
            let _pool = Pool(msg_send![class!(NSAutoreleasePool), new]);
            discover_device().and_then(|device| read_consumer_count(device))
        }
    }
    unsafe fn read_consumer_count(device: u32) -> Result<u32, String> {
        // Proven installed extension device property: scnt is NSNumber(UInt32),
        // counting source consumers only; its producer sink has a separate path.
        let object = Owned(property::<*mut c_void>(device, *b"scnt")?);
        if object.0.is_null() {
            return Err("V-Cam: contador de consumidores ausente".into());
        }
        let value = object.0.cast::<Object>();
        let is_number: objc::runtime::BOOL = msg_send![value, isKindOfClass:class!(NSNumber)];
        if is_number == objc::runtime::NO {
            return Err("V-Cam: contador não é NSNumber".into());
        }
        let count: i64 = msg_send![value, longLongValue];
        u32::try_from(count).map_err(|_| "V-Cam: contador de consumidores inválido".into())
    }
    unsafe fn discover() -> Result<(u32, u32), String> {
        let device = discover_device()?;
        let streams = ids(device, *b"stm#")?;
        let mut sinks = Vec::new();
        for stream in streams {
            // SDK CMIOHardwareStream.h: direction0=output/producer sink.
            if property::<u32>(stream, *b"sdir").ok() == Some(0) {
                sinks.push(stream);
            }
        }
        let stream = match sinks.as_slice() {
            [one] => *one,
            _ => return Err("V-Cam: stream de entrada EMEET ausente ou ambíguo".into()),
        };
        let format = Owned(property::<*mut c_void>(stream, *b"pft ")?);
        if format.0.is_null() {
            return Err("V-Cam: formato de entrada indisponível".into());
        }
        let size = CMVideoFormatDescriptionGetDimensions(format.0);
        if (size.width, size.height) != (WIDTH as i32, HEIGHT as i32)
            || CMFormatDescriptionGetMediaSubType(format.0) != BGRA
        {
            return Err(format!("V-Cam: entrada atual {}×{} não é BGRA 1920×1080; configure a saída EMEET em 1080p antes de usar",size.width,size.height));
        }
        let fps = property::<f64>(stream, *b"nfrt")?;
        if !fps.is_finite() || (fps - 30.0).abs() > 0.1 {
            return Err(format!("V-Cam: entrada {fps} fps não corresponde a 30 fps"));
        }
        Ok((device, stream))
    }
    // A NULL callback unregisters the CMIO queue and this extension returns no
    // queue. No-op notification needs no context/token ownership or Rust lifetime.
    extern "C" fn queue_changed(_: u32, _: *mut c_void, _: *mut c_void) {}
    fn host_ns(time: Time) -> i64 {
        if time.timescale <= 0 {
            return 0;
        }
        ((i128::from(time.value) * 1_000_000_000) / i128::from(time.timescale))
            .clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
    }
    struct Session {
        device: u32,
        stream: u32,
        queue: Owned,
        running: bool,
        pixel_pool: Owned,
        allocation_limit: Owned,
        privacy_pixel: Option<Owned>,
        cached_pixel: Option<Owned>,
        cached_sequence: Option<u64>,
        format: Option<Owned>,
    }
    impl Session {
        unsafe fn open() -> Result<Self, String> {
            let (device, stream) = discover()?;
            let mut queue = ptr::null_mut();
            check(
                CMIOStreamCopyBufferQueue(stream, Some(queue_changed), ptr::null_mut(), &mut queue),
                "abrir fila de saída",
            )?;
            let queue = Owned(queue);
            if queue.0.is_null() || CMSimpleQueueGetCapacity(queue.0) <= 0 {
                return Err("V-Cam: fila indisponível".into());
            }
            let attributes = Owned(CFDictionaryCreateMutable(
                ptr::null(),
                1,
                ptr::addr_of!(kCFTypeDictionaryKeyCallBacks).cast(),
                ptr::addr_of!(kCFTypeDictionaryValueCallBacks).cast(),
            ));
            let surface = Owned(CFDictionaryCreateMutable(
                ptr::null(),
                0,
                ptr::addr_of!(kCFTypeDictionaryKeyCallBacks).cast(),
                ptr::addr_of!(kCFTypeDictionaryValueCallBacks).cast(),
            ));
            if attributes.0.is_null() || surface.0.is_null() {
                return Err("V-Cam: falha ao criar atributos de vídeo".into());
            }
            CFDictionarySetValue(
                attributes.0,
                kCVPixelBufferIOSurfacePropertiesKey,
                surface.0,
            );
            for (key, value) in [
                (kCVPixelBufferWidthKey, WIDTH),
                (kCVPixelBufferHeightKey, HEIGHT),
                (kCVPixelBufferPixelFormatTypeKey, BGRA),
            ] {
                let number: *mut Object = msg_send![class!(NSNumber), numberWithUnsignedInt:value];
                CFDictionarySetValue(attributes.0, key, number.cast());
            }
            let mut pixel_pool = ptr::null_mut();
            check(
                CVPixelBufferPoolCreate(ptr::null(), ptr::null(), attributes.0, &mut pixel_pool),
                "criar pool de vídeo",
            )?;
            let pixel_pool = Owned(pixel_pool);
            if pixel_pool.0.is_null() {
                return Err("V-Cam: pool de vídeo nulo".into());
            }
            let allocation_limit = Owned(CFDictionaryCreateMutable(
                ptr::null(),
                1,
                ptr::addr_of!(kCFTypeDictionaryKeyCallBacks).cast(),
                ptr::addr_of!(kCFTypeDictionaryValueCallBacks).cast(),
            ));
            if allocation_limit.0.is_null() {
                return Err("V-Cam: limite de buffers nulo".into());
            }
            // Includes the extension's10-slot queue plus the cached image and
            // next producer buffer. No unbounded allocation if a client stalls.
            let limit: *mut Object = msg_send![class!(NSNumber), numberWithUnsignedInt:12u32];
            CFDictionarySetValue(
                allocation_limit.0,
                kCVPixelBufferPoolAllocationThresholdKey,
                limit.cast(),
            );
            let mut result = Self {
                device,
                stream,
                queue,
                running: false,
                pixel_pool,
                allocation_limit,
                privacy_pixel: None,
                cached_pixel: None,
                cached_sequence: None,
                format: None,
            };
            check(
                CMIODeviceStartStream(device, stream),
                "iniciar stream; confira se a extensão EMEET está autorizada",
            )?;
            result.running = true;
            // Reserve immutable neutral pixels before accepting live input. A
            // privacy transition never requires another pool allocation.
            result.send(
                &placeholder_bgra(WIDTH, HEIGHT),
                None,
                CMClockGetTime(CMClockGetHostTimeClock()),
            )?;
            result.privacy_pixel = result.cached_pixel.take();
            Ok(result)
        }
        unsafe fn send(
            &mut self,
            bgra: &[u8],
            sequence: Option<u64>,
            presentation: Time,
        ) -> Result<bool, String> {
            if CMSimpleQueueGetCount(self.queue.0) >= CMSimpleQueueGetCapacity(self.queue.0) {
                return Ok(false);
            }
            if (sequence.is_some() || self.privacy_pixel.is_none())
                && (self.cached_pixel.is_none() || self.cached_sequence != sequence)
            {
                let mut pixel = ptr::null_mut();
                let code = CVPixelBufferPoolCreatePixelBufferWithAuxAttributes(
                    ptr::null(),
                    self.pixel_pool.0,
                    self.allocation_limit.0,
                    &mut pixel,
                );
                // kCVReturnWouldExceedAllocationThreshold: apply bounded
                // backpressure; never overwrite memory still retained downstream.
                if code == -6689 {
                    return Ok(false);
                }
                check(code, "reutilizar pixel buffer")?;
                let pixel = Owned(pixel);
                if pixel.0.is_null() {
                    return Err("V-Cam: pixel buffer nulo".into());
                }
                check(
                    CVPixelBufferLockBaseAddress(pixel.0, 0),
                    "bloquear pixel buffer",
                )?;
                let base = CVPixelBufferGetBaseAddress(pixel.0);
                let size = CVPixelBufferGetDataSize(pixel.0);
                let stride = CVPixelBufferGetBytesPerRow(pixel.0);
                let copied = if base.is_null() {
                    Err("V-Cam: pixel buffer sem memória".into())
                } else {
                    copy_rows(
                        WIDTH as usize,
                        HEIGHT as usize,
                        bgra,
                        stride,
                        std::slice::from_raw_parts_mut(base.cast(), size),
                    )
                };
                let unlocked = CVPixelBufferUnlockBaseAddress(pixel.0, 0);
                copied?;
                check(unlocked, "liberar pixel buffer")?;
                if self.format.is_none() {
                    let mut format = ptr::null_mut();
                    check(
                        CMVideoFormatDescriptionCreateForImageBuffer(
                            ptr::null(),
                            pixel.0,
                            &mut format,
                        ),
                        "descrever frame",
                    )?;
                    let format = Owned(format);
                    if format.0.is_null() {
                        return Err("V-Cam: formato de vídeo nulo".into());
                    }
                    self.format = Some(format);
                }
                // Published buffers are immutable. A repeat creates only a new
                // timed sample retaining the same pixels, including idle gray.
                self.cached_pixel = Some(pixel);
                self.cached_sequence = sequence;
            }
            if sequence.is_none() && self.privacy_pixel.is_some() {
                let was_live = self.cached_pixel.take().is_some();
                self.cached_sequence = None;
                if was_live {
                    CVPixelBufferPoolFlush(self.pixel_pool.0, 1);
                }
            }
            let pixel = if sequence.is_none() {
                self.privacy_pixel.as_ref().or(self.cached_pixel.as_ref())
            } else {
                self.cached_pixel.as_ref()
            }
            .ok_or("V-Cam: imagem ausente")?;
            let format = self.format.as_ref().ok_or("V-Cam: formato ausente")?;
            let timing = Timing {
                duration: Time {
                    value: 1,
                    timescale: 30,
                    flags: 1,
                    epoch: 0,
                },
                presentation,
                decode: Time {
                    value: 0,
                    timescale: 0,
                    flags: 0,
                    epoch: 0,
                },
            };
            let mut sample = ptr::null_mut();
            check(
                CMSampleBufferCreateReadyWithImageBuffer(
                    ptr::null(),
                    pixel.0,
                    format.0,
                    &timing,
                    &mut sample,
                ),
                "criar frame",
            )?;
            let sample = Owned(sample);
            if sample.0.is_null() {
                return Err("V-Cam: frame nulo".into());
            }
            let code = CMSimpleQueueEnqueue(self.queue.0, sample.0);
            if code != 0 {
                return Err(format!("V-Cam: envio recusado (OSStatus {code})"));
            }
            // Studio0x1006c7dd8 retains before enqueue; successful enqueue transfers
            // that +1 to the CMIO output consumer. Its queue callback0x1006c75c8
            // never releases the consumed token. Release only if enqueue fails.
            mem::forget(sample);
            Ok(true)
        }
        unsafe fn resume(&mut self) -> Result<(), String> {
            if !self.running {
                check(
                    CMIODeviceStartStream(self.device, self.stream),
                    "retomar transporte virtual",
                )?;
                self.running = true;
            }
            Ok(())
        }
        unsafe fn close(&mut self) -> Result<(), String> {
            if self.running {
                self.running = false;
                check(
                    CMIODeviceStopStream(self.device, self.stream),
                    "encerrar stream",
                )?;
            }
            Ok(())
        }
    }
    impl Drop for Session {
        fn drop(&mut self) {
            unsafe {
                let _ = self.close();
            }
        }
    }
    pub(super) fn run(
        state: Arc<Shared>,
        ready: std::sync::mpsc::SyncSender<Result<(), String>>,
    ) -> Result<(), String> {
        unsafe {
            let _pool = Pool(msg_send![class!(NSAutoreleasePool), new]);
            let mut session = match Session::open() {
                Ok(session) => session,
                Err(error) => {
                    let _ = ready.send(Err(error.clone()));
                    return Err(error);
                }
            };
            if ready.send(Ok(())).is_err() {
                return session.close();
            }
            let origin = Instant::now();
            let host = CMClockGetTime(CMClockGetHostTimeClock());
            let host_origin_ns = host_ns(host);
            let mut clock = PublicationClock::default();
            let usage_state = state.clone();
            thread::spawn(move || {
                while !usage_state.stop.load(Ordering::Acquire) {
                    let observed = super::consumer_state();
                    if let Ok(mut value) = usage_state.consumers.lock() {
                        *value = observed;
                    }
                    for _ in 0..15 {
                        if usage_state.stop.load(Ordering::Acquire) {
                            return;
                        }
                        thread::sleep(Duration::from_millis(50));
                    }
                }
            });
            let conversion_state = state.clone();
            let converter = thread::spawn(move || {
                let result = video_qos(0x19).and_then(|()| {
                    conversion_loop(conversion_state.clone(), |w, h, rgb| {
                        let mut scratch = conversion_state
                            .conversion_scratch
                            .lock()
                            .map_err(|_| "V-Cam: scratch de conversão indisponível")?;
                        accelerated_letterbox_reusing(w, h, &rgb, WIDTH, HEIGHT, &mut scratch)
                    })
                });
                if let Err(error) = &result {
                    if let Ok(mut status) = conversion_state.status.lock() {
                        *status = error.clone();
                    }
                    conversion_state.stop.store(true, Ordering::Release);
                }
                result
            });
            let placeholder = placeholder_bgra(WIDTH, HEIGHT);
            let mut playout = Playout::default();
            let mut last_sequence = None;
            let mut last_enqueue: Option<Instant> = None;
            let mut current_qos = 0;
            let mut idle = false;
            let mut idle_since = Instant::now();
            while !state.stop.load(Ordering::Acquire) {
                let known_idle = state
                    .consumers
                    .lock()
                    .is_ok_and(|usage| usage.active() == Some(false));
                if known_idle {
                    if !idle {
                        video_qos(0x15)?;
                        current_qos = 0x15;
                        // Clear old imagery and deliver neutral once before idle.
                        // Keep the SAME CMIO stream registered and running.
                        if let Ok(mut input) = state.frame.lock() {
                            state.generation.fetch_add(1, Ordering::AcqRel);
                            *input = None;
                            if let Ok(mut ready) = state.converted.lock() {
                                ready.clear();
                            }
                        }
                        playout = Playout::default();
                        if let Ok(mut scratch) = state.conversion_scratch.lock() {
                            *scratch = Vec::new();
                        }
                        session.send(
                            &placeholder,
                            None,
                            CMClockGetTime(CMClockGetHostTimeClock()),
                        )?;
                        last_sequence = None;
                        last_enqueue = None;
                        idle = true;
                        idle_since = Instant::now();
                    }
                    // No consumer exists across this entire grace. Suspending
                    // only the idle producer avoids the extension's continuous
                    // XPC pulls. Active/unknown consumers never take this path.
                    if idle_since.elapsed() >= Duration::from_secs(5) {
                        session.close()?;
                    }
                    // No one consumes this output: no continuous sample creation,
                    // IPC pixel transfer, resize, or30Hz producer wakeups.
                    clock.next = (origin.elapsed().as_nanos() * 30 / 1_000_000_000) as u64 + 1;
                    clock.pending_skips = 0;
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }
                session.resume()?;
                idle = false;
                let elapsed = origin.elapsed();
                let Some(tick) = clock.due(elapsed) else {
                    let wait = PublicationClock::scheduled(clock.next).saturating_sub(elapsed);
                    if !wait.is_zero() {
                        thread::sleep(wait);
                    }
                    continue;
                };
                let _frame_pool = Pool(msg_send![class!(NSAutoreleasePool), new]);
                let live = playout.next(&state);
                let requested_qos = if live.is_some() { 0x21 } else { 0x15 };
                if current_qos != requested_qos {
                    video_qos(requested_qos)?;
                    current_qos = requested_qos;
                }
                let sequence = live.as_ref().map(|frame| frame.sequence);
                let source_age_ms = live
                    .as_ref()
                    .map(|frame| frame.received.elapsed().as_secs_f64() * 1000.0);
                let scheduled = PublicationClock::scheduled(tick.number);
                let pts_ns = host_origin_ns
                    .saturating_add(scheduled.as_nanos().min(i64::MAX as u128) as i64);
                let presentation = Time {
                    value: pts_ns,
                    timescale: 1_000_000_000,
                    flags: 1,
                    epoch: host.epoch,
                };
                let depth = CMSimpleQueueGetCount(session.queue.0).max(0) as u32;
                let capacity = CMSimpleQueueGetCapacity(session.queue.0).max(0) as u32;
                let started = Instant::now();
                let sent = session.send(
                    live.as_ref()
                        .map_or(placeholder.as_slice(), |frame| frame.bgra.as_slice()),
                    sequence,
                    presentation,
                )?;
                let completed = Instant::now();
                let send_ms = started.elapsed().as_secs_f64() * 1000.0;
                let gap_ms =
                    last_enqueue.map(|last| completed.duration_since(last).as_secs_f64() * 1000.0);
                let lateness_ms = elapsed.saturating_sub(scheduled).as_secs_f64() * 1000.0;
                let actual_host_ns = host_ns(CMClockGetTime(CMClockGetHostTimeClock()));
                if let Ok(mut metrics) = state.metrics.lock() {
                    metrics.publication_attempts += 1;
                    metrics.skipped_ticks += tick.skipped;
                    metrics.send_max_ms = metrics.send_max_ms.max(send_ms);
                    metrics.lateness_max_ms = metrics.lateness_max_ms.max(lateness_ms);
                    if sent {
                        metrics.enqueued_frames += 1;
                        if let Some(sequence) = sequence {
                            if Some(sequence) == last_sequence {
                                metrics.repeated_enqueued_frames += 1;
                            } else {
                                metrics.unique_enqueued_frames += 1;
                            }
                        } else {
                            metrics.placeholder_enqueued_frames += 1;
                        }
                        metrics.gap_max_ms = metrics.gap_max_ms.max(gap_ms.unwrap_or_default());
                    } else {
                        metrics.queue_rejections += 1;
                    }
                    if metrics.recent.len() == 120 {
                        metrics.recent.pop_front();
                    }
                    metrics.recent.push_back(PublicationSample {
                        tick: tick.number,
                        sequence,
                        scheduled_pts_ns: pts_ns,
                        actual_host_ns,
                        accepted: sent,
                        source_age_ms,
                        gap_ms,
                        lateness_ms,
                        send_ms,
                        queue_depth: depth,
                        queue_capacity: capacity,
                    });
                }
                if sent {
                    last_enqueue = Some(completed);
                    last_sequence = sequence;
                }
                if let Ok(mut status) = state.status.lock() {
                    *status = if !sent {
                        "V-Cam EMEET · aguardando consumidor"
                    } else if live.is_some() {
                        "V-Cam EMEET · 1920×1080 · envio local ativo"
                    } else {
                        "V-Cam EMEET · imagem de privacidade local"
                    }
                    .into();
                }
            }
            let closed = session.close();
            let converted = converter
                .join()
                .map_err(|_| "Falha na conversão da V-Cam")?;
            converted?;
            closed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(target_os = "macos")]
    fn conversion_reuses_large_scratch_without_reusing_old_image_pixels() {
        let mut scratch = Vec::new();
        let first =
            accelerated_letterbox_reusing(8, 8, &[255, 0, 0].repeat(64), 4, 4, &mut scratch)
                .unwrap();
        let address = scratch.as_ptr();
        let capacity = scratch.capacity();
        let second =
            accelerated_letterbox_reusing(8, 8, &[0, 0, 255].repeat(64), 4, 4, &mut scratch)
                .unwrap();
        assert_eq!(scratch.as_ptr(), address);
        assert_eq!(scratch.capacity(), capacity);
        assert!(first.chunks_exact(4).all(|p| p == [0, 0, 255, 255]));
        assert!(second.chunks_exact(4).all(|p| p == [255, 0, 0, 255]));
    }
    #[test]
    fn no_consumer_skips_conversion_but_unknown_usage_keeps_output_available() {
        let sink = VirtualSink {
            shared: Arc::new(Shared::new()),
        };
        assert!(sink.wants_frame());
        *sink.shared.consumers.lock().unwrap() = ConsumerState::Known { count: 0 };
        assert!(!sink.wants_frame());
        sink.submit_rgb(1, 1, vec![1, 2, 3]).unwrap();
        assert!(sink.shared.frame.lock().unwrap().is_none());
        *sink.shared.consumers.lock().unwrap() = ConsumerState::Known { count: 1 };
        assert!(sink.wants_frame());
        sink.submit_rgb(1, 1, vec![1, 2, 3]).unwrap();
        assert!(sink.shared.frame.lock().unwrap().is_some());
    }
    #[test]
    fn bounded_playout_absorbs_conversion_jitter_without_reordering_or_privacy_leaks() {
        let shared = Shared::new();
        let mut playout = Playout::default();
        let frame = |sequence| PreparedFrame {
            bgra: Arc::new(vec![0; 4]),
            sequence,
            generation: 0,
            received: std::time::Instant::now(),
        };
        shared.converted.lock().unwrap().push_back(frame(1));
        assert!(playout.next(&shared).is_none());
        shared.converted.lock().unwrap().push_back(frame(2));
        assert_eq!(playout.next(&shared).unwrap().sequence, 1);
        shared.converted.lock().unwrap().push_back(frame(3));
        assert_eq!(playout.next(&shared).unwrap().sequence, 2);
        assert_eq!(playout.next(&shared).unwrap().sequence, 3);
        assert_eq!(playout.next(&shared).unwrap().sequence, 3);
        set_placeholder(&shared, true);
        assert!(playout.next(&shared).is_none());
        set_placeholder(&shared, false);
        assert!(playout.next(&shared).is_none());
    }
    #[test]
    fn publication_clock_skips_late_slots_and_never_catches_up_in_bursts() {
        let mut clock = PublicationClock::default();
        assert_eq!(clock.due(Duration::ZERO).unwrap().number, 0);
        assert!(clock.due(Duration::ZERO).is_none());
        let late = clock.due(Duration::from_millis(125)).unwrap();
        assert_eq!(late.number, 3);
        assert_eq!(late.skipped, 2);
        assert!(clock.due(Duration::from_millis(126)).is_none());
        assert!(clock.due(Duration::from_millis(133)).is_none());
        let resumed = clock.due(Duration::from_millis(167)).unwrap();
        assert_eq!(resumed.number, 5);
        assert_eq!(resumed.skipped, 1);
    }
    #[test]
    fn blocked_conversion_does_not_hold_the_publication_slot() {
        let shared = Arc::new(Shared::new());
        let sink = VirtualSink {
            shared: shared.clone(),
        };
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker = shared.clone();
        let converter = thread::spawn(move || {
            conversion_loop(worker, move |_, _, _| {
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                Ok(vec![10, 20, 30, 255])
            })
        });
        sink.submit_rgb(1, 1, vec![30, 20, 10]).unwrap();
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let mut clock = PublicationClock::default();
        for tick in 0..6 {
            assert!(clock.due(Duration::from_nanos(tick * 33_333_334)).is_some());
            assert!(latest_converted(&shared).is_none());
        }
        release_tx.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while latest_converted(&shared).is_none() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(latest_converted(&shared).unwrap().sequence, 1);
        // An in-flight conversion completed after privacy must not revive the
        // previous image, including after a later explicit resume.
        sink.submit_rgb(1, 1, vec![60, 50, 40]).unwrap();
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        set_placeholder(&shared, true);
        release_tx.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while shared.metrics.lock().unwrap().obsolete_conversions == 0
            && std::time::Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(shared.metrics.lock().unwrap().obsolete_conversions, 1);
        assert!(latest_converted(&shared).is_none());
        set_placeholder(&shared, false);
        assert!(latest_converted(&shared).is_none());
        shared.stop.store(true, Ordering::Release);
        shared.wake.notify_all();
        assert!(converter.join().unwrap().is_ok());
    }
    #[test]
    fn freshness_uses_source_receipt_not_slow_conversion_completion() {
        let shared = Shared::new();
        shared.converted.lock().unwrap().push_back(PreparedFrame {
            bgra: Arc::new(vec![0, 0, 0, 255]),
            sequence: 1,
            generation: 0,
            received: std::time::Instant::now() - Duration::from_secs(2),
        });
        assert!(latest_converted(&shared).is_none());
        shared
            .converted
            .lock()
            .unwrap()
            .back_mut()
            .unwrap()
            .received = std::time::Instant::now();
        assert!(latest_converted(&shared).is_some());
    }
    #[test]
    #[cfg(target_os = "macos")]
    fn accelerated_letterbox_preserves_rgb_order_and_opaque_padding_when_scaling() {
        let pixels = [231, 47, 89].repeat(16);
        let output = accelerated_letterbox(4, 4, &pixels, 3, 2).unwrap();
        assert_eq!(
            output,
            vec![
                89, 47, 231, 255, 89, 47, 231, 255, 0, 0, 0, 255, 89, 47, 231, 255, 89, 47, 231,
                255, 0, 0, 0, 255
            ]
        );
        let portrait = accelerated_letterbox(2, 4, &[12, 57, 193].repeat(8), 8, 4).unwrap();
        for row in portrait.chunks_exact(8 * 4) {
            for (x, pixel) in row.chunks_exact(4).enumerate() {
                assert_eq!(
                    pixel,
                    if (3..5).contains(&x) {
                        &[193, 57, 12, 255]
                    } else {
                        &[0, 0, 0, 255]
                    }
                );
            }
        }
    }
    #[test]
    #[cfg(target_os = "macos")]
    fn accelerated_letterbox_upscale_keeps_centered_aspect_and_exact_no_resize_fixture() {
        let output = accelerated_letterbox(4, 2, &[205, 31, 77].repeat(8), 5, 5).unwrap();
        for (y, row) in output.chunks_exact(5 * 4).enumerate() {
            for pixel in row.chunks_exact(4) {
                assert_eq!(
                    pixel,
                    if (1..4).contains(&y) {
                        &[77, 31, 205, 255]
                    } else {
                        &[0, 0, 0, 255]
                    }
                );
            }
        }
        assert_eq!(
            accelerated_letterbox(2, 1, &[255, 0, 0, 0, 0, 255], 2, 1).unwrap(),
            vec![0, 0, 255, 255, 255, 0, 0, 255]
        );
    }
    #[test]
    fn overlapping_usage_pollers_share_one_refresh() {
        let cache = Arc::new(UsageCache(Mutex::new(None)));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let mut workers = Vec::new();
        for _ in 0..2 {
            let (cache, calls, barrier) = (cache.clone(), calls.clone(), barrier.clone());
            workers.push(thread::spawn(move || {
                barrier.wait();
                cache.get(|| {
                    calls.fetch_add(1, Ordering::AcqRel);
                    thread::sleep(Duration::from_millis(40));
                    ConsumerState::Known { count: 2 }
                })
            }));
        }
        barrier.wait();
        for worker in workers {
            assert_eq!(worker.join().unwrap(), ConsumerState::Known { count: 2 });
        }
        assert_eq!(calls.load(Ordering::Acquire), 1);
    }
    #[test]
    #[cfg(target_os = "macos")]
    fn metadata_helper_accepts_only_valid_bounded_result_and_times_out() {
        let probe = |text: &str| {
            let mut command = std::process::Command::new("/usr/bin/printf");
            command.arg("%s").arg(text);
            read_usage_process(command, Duration::from_secs(1))
        };
        assert_eq!(probe(r#"{"Ok":0}"#), ConsumerState::Known { count: 0 });
        assert_eq!(probe(r#"{"Ok":3}"#), ConsumerState::Known { count: 3 });
        assert!(matches!(
            probe(r#"{"Err":"unavailable"}"#),
            ConsumerState::Unknown { .. }
        ));
        assert!(matches!(probe("invalid"), ConsumerState::Unknown { .. }));
        assert!(matches!(
            probe(&" ".repeat(5000)),
            ConsumerState::Unknown { .. }
        ));
        let mut command = std::process::Command::new("/bin/sleep");
        command.arg("5");
        let started = std::time::Instant::now();
        assert!(matches!(
            read_usage_process(command, Duration::from_millis(50)),
            ConsumerState::Unknown { .. }
        ));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "helper was not killed/reaped promptly"
        );
    }
    #[test]
    fn consumer_unknown_is_not_mistaken_for_idle() {
        assert_eq!(ConsumerState::Known { count: 0 }.active(), Some(false));
        assert_eq!(ConsumerState::Known { count: 2 }.active(), Some(true));
        assert_eq!(ConsumerState::Known { count: 0 }.active(), Some(false));
        assert_eq!(ConsumerState::Known { count: 1 }.active(), Some(true));
        assert_eq!(
            ConsumerState::Unknown {
                reason: "unsupported".into()
            }
            .active(),
            None
        );
    }
    #[test]
    fn placeholder_is_opaque_and_contains_no_previous_image() {
        assert_eq!(
            placeholder_bgra(2, 1),
            vec![28, 28, 28, 255, 28, 28, 28, 255]
        );
    }
    #[test]
    fn rgb_channels_and_alpha_survive_centered_letterbox() {
        let rgb = vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255];
        let bgra = letterbox_bgra(2, 2, rgb, 4, 2).unwrap();
        assert_eq!(
            bgra,
            vec![
                0, 0, 0, 255, 0, 0, 255, 255, 0, 255, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 0,
                0, 255, 255, 255, 255, 255, 0, 0, 0, 255
            ]
        );
    }
    #[test]
    fn rgb_validation_rejects_truncation_overflow_and_zero_dimensions() {
        for (w, h, rgb) in [
            (2, 2, vec![0; 11]),
            (u32::MAX, u32::MAX, vec![]),
            (0, 1, vec![]),
        ] {
            assert!(letterbox_bgra(w, h, rgb, 4, 2).is_err());
        }
    }
    #[test]
    fn bgra_rows_preserve_stride_padding_and_reject_short_buffers() {
        let mut target = vec![99; 24];
        copy_rows(2, 2, &[1; 16], 12, &mut target).unwrap();
        assert_eq!(
            target,
            [1, 1, 1, 1, 1, 1, 1, 1, 99, 99, 99, 99, 1, 1, 1, 1, 1, 1, 1, 1, 99, 99, 99, 99]
        );
        assert!(copy_rows(2, 2, &[1; 15], 12, &mut target).is_err());
        assert!(copy_rows(2, 2, &[1; 16], 7, &mut target).is_err());
        assert!(copy_rows(2, 2, &[1; 16], 12, &mut [0; 19]).is_err());
    }
    #[test]
    fn latest_slot_is_bounded_and_stop_rejects_new_frames() {
        let sink = VirtualSink {
            shared: Arc::new(Shared::new()),
        };
        sink.submit_rgb(2, 2, vec![1; 12]).unwrap();
        sink.submit_rgb(2, 2, vec![2; 12]).unwrap();
        assert_eq!(
            sink.shared.frame.lock().unwrap().take().unwrap().rgb,
            vec![2; 12]
        );
        let guard = sink.shared.frame.lock().unwrap();
        assert!(sink.submit_rgb(2, 2, vec![3; 12]).is_ok());
        drop(guard);
        sink.submit_rgb(2, 2, vec![4; 12]).unwrap();
        set_placeholder(&sink.shared, true);
        assert!(sink.shared.frame.lock().unwrap().is_none());
        assert_eq!(sink.shared.generation.load(Ordering::Acquire), 1);
        sink.submit_rgb(2, 2, vec![5; 12]).unwrap();
        assert!(sink.shared.frame.lock().unwrap().is_none());
        set_placeholder(&sink.shared, false);
        assert!(sink.shared.frame.lock().unwrap().is_none());
        sink.submit_rgb(2, 2, vec![6; 12]).unwrap();
        set_placeholder(&sink.shared, false);
        assert_eq!(
            sink.shared.frame.lock().unwrap().take().unwrap().rgb,
            vec![6; 12]
        );
        sink.shared.stop.store(true, Ordering::Release);
        assert!(sink.submit_rgb(2, 2, vec![0; 12]).is_err());
    }
}
