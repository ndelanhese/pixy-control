//! Bounded RGB recording pipeline using a local FFmpeg executable.
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub struct Recorder {
    sender: SyncSender<Vec<u8>>,
    stop_requested: Arc<AtomicBool>,
    stopped_at: Arc<Mutex<Option<Instant>>>,
    dropped: Arc<AtomicU64>,
    status: Arc<Mutex<String>>,
    finished: Arc<AtomicBool>,
    frame_bytes: usize,
    worker: Option<JoinHandle<Result<(), String>>>,
}

#[derive(Clone)]
pub struct RecorderSink {
    sender: SyncSender<Vec<u8>>,
    frame_bytes: usize,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<String>>,
    dropped: Arc<AtomicU64>,
}

impl RecorderSink {
    /// Transfers an existing RGB allocation directly from the capture callback.
    /// A full queue drops this frame and returns immediately; it never blocks
    /// the camera or creates an unbounded backlog.
    pub fn submit_owned(&self, rgb: Vec<u8>) -> Result<(), String> {
        if rgb.len() != self.frame_bytes {
            return Err("Dimensões do frame mudaram durante a gravação.".into());
        }
        if self.stop.load(Ordering::Acquire) {
            return Err("Gravação encerrando.".into());
        }
        match self.sender.try_send(rgb) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(TrySendError::Disconnected(_)) => Err(self
                .status
                .lock()
                .map(|value| value.clone())
                .unwrap_or_else(|_| "Falha na gravação".into())),
        }
    }
}

fn drain_progress(mut input: impl Read, started: Arc<AtomicBool>) {
    // FFmpeg's machine-readable stdout is distinct from stderr diagnostics.
    // A bounded line buffer also handles keys split across pipe reads.
    let mut buffer = [0u8; 1024];
    let mut line = Vec::with_capacity(128);
    let mut oversized = false;
    loop {
        let size = match input.read(&mut buffer) {
            Ok(0) => break,
            Ok(size) => size,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        for &byte in &buffer[..size] {
            if byte == b'\n' {
                if !oversized
                    && line
                        .strip_prefix(b"frame=")
                        .and_then(|value| std::str::from_utf8(value).ok())
                        .and_then(|value| value.trim().parse::<u64>().ok())
                        .is_some_and(|value| value > 0)
                {
                    started.store(true, Ordering::Release);
                }
                line.clear();
                oversized = false;
            } else if line.len() < 128 {
                line.push(byte);
            } else {
                oversized = true;
            }
        }
    }
}

fn frame_write_timeout(encoded: bool, startup_deadline: Instant) -> Result<Duration, String> {
    if encoded {
        return Ok(Duration::from_secs(2));
    }
    startup_deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| {
            "FFmpeg não iniciou a codificação em 30 segundos; confira a permissão do microfone."
                .into()
        })
}

fn frames_for_elapsed(elapsed: Duration, fps: u32) -> u64 {
    (elapsed.as_nanos().saturating_mul(fps as u128) / 1_000_000_000).min(u64::MAX as u128) as u64
}

fn drain_write_timeout(stopped_at: Instant) -> Result<Duration, String> {
    let now = Instant::now();
    // Stop starting packets after6s, but reserve a full2s for the last raw
    // packet. Otherwise a frame begun near the drain deadline is truncated.
    // The remaining2s of the overall10s stop budget belongs to the MP4 muxer.
    if now >= stopped_at + Duration::from_secs(6) {
        return Err("Gravador não acompanhou o tempo real; faltaram frames ao finalizar.".into());
    }
    Ok((stopped_at + Duration::from_secs(8))
        .saturating_duration_since(now)
        .min(Duration::from_secs(2)))
}

impl Recorder {
    pub fn start_with_options(
        width: u32,
        height: u32,
        fps: u32,
        destination: PathBuf,
        audio: bool,
        segment_seconds: u32,
    ) -> Result<Self, String> {
        if width == 0 || height == 0 || fps == 0 || fps > 120 {
            return Err("Formato inválido para gravação.".into());
        }
        if segment_seconds > 86400 {
            return Err("Duração do segmento deve estar entre 0 e 86400 segundos.".into());
        }
        let frame_bytes = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(3))
            .ok_or("Frame grande demais.")?;
        if let Some(parent) = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let output = if segment_seconds == 0 {
            destination.clone()
        } else {
            segmented_destination(&destination)?
        };
        if segment_seconds != 0 {
            refuse_existing_segments(&destination)?;
        }
        let ffmpeg = if std::path::Path::new("/opt/homebrew/bin/ffmpeg").is_file() {
            "/opt/homebrew/bin/ffmpeg"
        } else {
            "ffmpeg"
        };
        // -n refuses to overwrite existing recordings. No shell interpolation.
        let mut command = Command::new(ffmpeg);
        command
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostats",
                "-progress",
                "pipe:1",
                "-stats_period",
                "0.1",
                "-n",
                "-f",
                "rawvideo",
                "-pixel_format",
                "rgb24",
                "-video_size",
            ])
            .arg(format!("{width}x{height}"))
            .arg("-framerate")
            .arg(fps.to_string())
            .args(["-i", "pipe:0"]);
        if audio {
            command.args([
                "-thread_queue_size",
                "512",
                "-f",
                "avfoundation",
                "-i",
                ":EMEET PIXY",
                "-map",
                "0:v:0",
                "-map",
                "1:a:0",
                "-c:a",
                "aac",
                "-af",
                "aresample=async=1:first_pts=0",
                "-shortest",
                // Both streams are continuous, so sparse-stream buffering is
                // unnecessary. FFmpeg's default 10 s shortest sync queue held
                // mux progress at frame=0 while raw video accumulated, adding
                // those ten seconds before our recording-ready notification.
                "-shortest_buf_duration",
                "0.1",
            ]);
        } else {
            command.arg("-an");
        }
        // Require the macOS hardware encoder rather than silently using CPU
        // cycles needed by native capture and perspective correction.
        let bitrate =
            ((width as u64 * height as u64 * fps as u64) * 12 / 100).clamp(2_000_000, 60_000_000);
        command
            .args([
                "-c:v",
                "h264_videotoolbox",
                "-allow_sw",
                "0",
                "-realtime",
                "1",
                "-pix_fmt",
                "nv12",
                "-bf",
                "0",
                "-b:v",
            ])
            .arg(bitrate.to_string());
        if segment_seconds == 0 {
            command.args(["-movflags", "+faststart"]);
        } else {
            command
                .arg("-force_key_frames")
                .arg(format!("expr:gte(t,n_forced*{segment_seconds})"))
                .args(["-f", "segment", "-segment_time"])
                .arg(segment_seconds.to_string())
                .args([
                    "-reset_timestamps",
                    "1",
                    "-segment_format",
                    "mp4",
                    "-segment_format_options",
                    "movflags=+faststart",
                ]);
        }
        let mut child = command
            .arg(&output)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("FFmpeg local indisponível: {e}"))?;
        let mut input = match child.stdin.take() {
            Some(input) => input,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("FFmpeg sem entrada de vídeo.".into());
            }
        };
        if let Err(error) = make_nonblocking(&input) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("FFmpeg sem saída de diagnóstico.".into());
            }
        };
        let progress_output = match child.stdout.take() {
            Some(output) => output,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("FFmpeg sem canal de progresso.".into());
            }
        };
        let encoder_started = Arc::new(AtomicBool::new(false));
        let progress_started = encoder_started.clone();
        let progress = thread::spawn(move || drain_progress(progress_output, progress_started));
        let error_tail = Arc::new(Mutex::new(Vec::<u8>::new()));
        let tail = error_tail.clone();
        // Always drain the pipe. Retain only the most recent 8 KiB so FFmpeg's
        // diagnostics cannot block the encoder or grow memory without bound.
        let diagnostics = thread::spawn(move || drain_stderr(stderr, tail));
        let (sender, receiver) = mpsc::sync_channel::<Vec<u8>>(2);
        let stop_requested = Arc::new(AtomicBool::new(false));
        let stopped_at = Arc::new(Mutex::new(None::<Instant>));
        let stop_time = stopped_at.clone();
        let dropped = Arc::new(AtomicU64::new(0));
        let status = Arc::new(Mutex::new(if audio {
            "Iniciando gravador e microfone; aguardando permissão do macOS…".into()
        } else {
            "Aguardando primeiro frame…".into()
        }));
        let finished = Arc::new(AtomicBool::new(false));
        let stop = stop_requested.clone();
        let state = status.clone();
        let complete = finished.clone();
        let worker = thread::spawn(move || {
            let startup_deadline = Instant::now() + Duration::from_secs(30);
            let mut record = || -> Result<(), String> {
                let mut last = loop {
                    if stop.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    frame_write_timeout(encoder_started.load(Ordering::Acquire), startup_deadline)?;
                    match receiver.recv_timeout(Duration::from_millis(50)) {
                        Ok(frame) => break frame,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                    }
                    if let Some(exit) = child.try_wait().map_err(|e| e.to_string())? {
                        return Err(format!("FFmpeg encerrou antes da gravação: {exit}"));
                    }
                };
                if let Ok(mut message) = state.lock() {
                    *message = if audio {
                        "Iniciando gravador e microfone; aguardando permissão do macOS…"
                    } else {
                        "Iniciando gravador local…"
                    }
                    .into();
                }
                let interval = Duration::from_secs_f64(1.0 / fps as f64);
                let startup_interval = Duration::from_secs_f64(1.0 / fps.min(30) as f64);
                let mut deadline = Instant::now();
                let mut timing_started = false;
                let mut timing_origin = Instant::now();
                let mut steady_frames = 0u64;
                let finish_frame = AtomicBool::new(false);
                while !stop.load(Ordering::Relaxed) {
                    while let Ok(frame) = receiver.try_recv() {
                        last = frame;
                    }
                    let timeout = frame_write_timeout(
                        encoder_started.load(Ordering::Acquire),
                        startup_deadline,
                    )?;
                    // Once recording is live, finish this raw packet on stop;
                    // truncating it corrupts EOF and also skips timeline drain.
                    // The existing2s write deadline still bounds a dead reader.
                    let cancellation = if timing_started { &finish_frame } else { &stop };
                    if !write_frame(&mut input, &last, cancellation, timeout)? {
                        return Ok(());
                    }
                    if timing_started {
                        steady_frames += 1;
                    }
                    if !timing_started && encoder_started.load(Ordering::Acquire) {
                        timing_started = true;
                        // A raw stdin packet can be consumed before FFmpeg opens
                        // its microphone. Start timing only on muxed progress;
                        // never catch up permission waits with duplicate bursts.
                        deadline = Instant::now();
                        timing_origin = deadline;
                        if let Ok(mut message) = state.lock() {
                            *message = "Gravando vídeo local…".into();
                        }
                    }
                    deadline = if timing_started {
                        deadline + interval
                    } else {
                        Instant::now() + startup_interval
                    };
                    while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
                        let wait = deadline
                            .saturating_duration_since(Instant::now())
                            .min(Duration::from_millis(20));
                        match receiver.recv_timeout(wait) {
                            Ok(frame) => last = frame,
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                        }
                    }
                }
                if timing_started {
                    // A bounded input queue may replace frames, but output must
                    // still cover elapsed wall time rather than play faster.
                    // Freeze the stop target and repeat the most recent frame;
                    // do not extend the recording clock while draining it.
                    let ended = stop_time
                        .lock()
                        .ok()
                        .and_then(|time| *time)
                        .unwrap_or_else(Instant::now);
                    let target =
                        frames_for_elapsed(ended.saturating_duration_since(timing_origin), fps);
                    let draining = AtomicBool::new(false);
                    while steady_frames < target {
                        let remaining = drain_write_timeout(ended)?;
                        if !write_frame(&mut input, &last, &draining, remaining)? {
                            return Err("Gravador interrompeu a finalização dos frames.".into());
                        }
                        steady_frames += 1;
                    }
                }
                Ok(())
            };
            let recorded = record();
            drop(input); // EOF lets FFmpeg write MP4 metadata.
            let deadline = stop_time
                .lock()
                .ok()
                .and_then(|time| *time)
                .unwrap_or_else(Instant::now)
                + Duration::from_secs(10);
            let exit = loop {
                match child.try_wait() {
                    Ok(Some(exit)) => {
                        break if exit.success() {
                            Ok(())
                        } else {
                            Err(format!("FFmpeg: {exit}"))
                        }
                    }
                    Ok(None) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(25))
                    }
                    Ok(None) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break Err("FFmpeg não finalizou em 10 segundos.".into());
                    }
                    Err(error) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break Err(error.to_string());
                    }
                }
            };
            // The child has exited (or was killed and waited), closing its pipe.
            let _ = diagnostics.join();
            let _ = progress.join();
            let cancelled_startup = !encoder_started.load(Ordering::Acquire)
                && stop.load(Ordering::Acquire)
                && recorded.is_ok();
            let result = (if cancelled_startup {
                Ok(())
            } else {
                recorded.and(exit)
            })
            .map_err(|error| {
                let detail = error_tail
                    .lock()
                    .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
                    .unwrap_or_default();
                if detail.is_empty() {
                    error
                } else {
                    format!("{error}\n{detail}")
                }
            });
            if let Ok(mut message) = state.lock() {
                *message = match &result {
                    Ok(()) if cancelled_startup => {
                        "Gravação cancelada antes do primeiro frame.".into()
                    }
                    Ok(()) => format!("Vídeo salvo: {}", output.display()),
                    Err(error) => format!("Gravação: {error}"),
                };
            }
            complete.store(true, Ordering::Release);
            result
        });
        Ok(Self {
            sender,
            stop_requested,
            stopped_at,
            dropped,
            status,
            finished,
            frame_bytes,
            worker: Some(worker),
        })
    }

    pub fn sink(&self) -> RecorderSink {
        RecorderSink {
            sender: self.sender.clone(),
            frame_bytes: self.frame_bytes,
            stop: self.stop_requested.clone(),
            status: self.status.clone(),
            dropped: self.dropped.clone(),
        }
    }
    pub fn request_stop(&self) {
        if let Ok(mut ended) = self.stopped_at.lock() {
            ended.get_or_insert_with(Instant::now);
        }
        self.stop_requested.store(true, Ordering::Relaxed);
    }
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
    pub fn status(&self) -> String {
        self.status
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|_| "Falha na gravação".into())
    }
    /// Use request_stop from the UI, then stop after is_finished or on a worker.
    pub fn stop(&mut self) -> Result<(), String> {
        self.request_stop();
        self.worker.take().map_or(Ok(()), |worker| {
            worker
                .join()
                .unwrap_or_else(|_| Err("Falha na thread de gravação.".into()))
        })
    }
}
impl Drop for Recorder {
    fn drop(&mut self) {
        self.request_stop();
    }
}

fn segmented_destination(destination: &Path) -> Result<PathBuf, String> {
    let stem = destination
        .file_stem()
        .filter(|stem| !stem.is_empty())
        .ok_or("Nome de arquivo de vídeo ausente.")?;
    let mut name = stem.to_os_string();
    name.push("-%03d.mp4");
    Ok(destination.with_file_name(name))
}

fn refuse_existing_segments(destination: &Path) -> Result<(), String> {
    // The segment muxer opens its own children; -n alone does not reliably
    // protect them. Refuse a reused numbered sequence before starting FFmpeg.
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let stem = destination
        .file_stem()
        .ok_or("Nome de arquivo de vídeo ausente.")?
        .to_string_lossy();
    let prefix = format!("{stem}-");
    for entry in std::fs::read_dir(parent).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name
            .strip_prefix(&prefix)
            .and_then(|suffix| suffix.strip_suffix(".mp4"))
            .is_some_and(|digits| {
                digits.len() >= 3 && digits.bytes().all(|byte| byte.is_ascii_digit())
            })
        {
            return Err(format!(
                "Segmento já existe; escolha outro nome: {}",
                entry.path().display()
            ));
        }
    }
    Ok(())
}

fn append_tail(tail: &mut Vec<u8>, bytes: &[u8]) {
    const LIMIT: usize = 8192;
    if bytes.len() >= LIMIT {
        tail.clear();
        tail.extend_from_slice(&bytes[bytes.len() - LIMIT..]);
    } else {
        let overflow = (tail.len() + bytes.len()).saturating_sub(LIMIT);
        if overflow != 0 {
            tail.drain(..overflow);
        }
        tail.extend_from_slice(bytes);
    }
}

fn drain_stderr(mut stderr: std::process::ChildStderr, tail: Arc<Mutex<Vec<u8>>>) {
    let mut buffer = [0u8; 4096];
    loop {
        match stderr.read(&mut buffer) {
            Ok(0) => break,
            Ok(size) => {
                if let Ok(mut tail) = tail.lock() {
                    append_tail(&mut tail, &buffer[..size]);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
}

#[cfg(unix)]
fn make_nonblocking(input: &std::process::ChildStdin) -> Result<(), String> {
    use std::os::fd::AsRawFd;
    unsafe extern "C" {
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    }
    #[cfg(target_os = "macos")]
    const NONBLOCK: i32 = 0x4;
    #[cfg(not(target_os = "macos"))]
    const NONBLOCK: i32 = 0x800;
    let fd = input.as_raw_fd();
    let flags = unsafe { fcntl(fd, 3) };
    if flags < 0 || unsafe { fcntl(fd, 4, flags | NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}
#[cfg(not(unix))]
fn make_nonblocking(_: &std::process::ChildStdin) -> Result<(), String> {
    Err("Gravação disponível no macOS.".into())
}
fn write_frame(
    input: &mut std::process::ChildStdin,
    frame: &[u8],
    stop: &AtomicBool,
    timeout: Duration,
) -> Result<bool, String> {
    let deadline = Instant::now() + timeout;
    let mut remaining = frame;
    let mut stopping_deadline = None;
    while !remaining.is_empty() {
        if stop.load(Ordering::Acquire) {
            if remaining.len() == frame.len() {
                return Ok(false);
            }
            // EOF in the middle of rawvideo would corrupt the last packet.
            // Give an already-started frame a short chance to drain while the
            // reader is ready; a stuck/TCC-blocked reader still cancels quickly.
            let until = *stopping_deadline
                .get_or_insert_with(|| Instant::now() + Duration::from_millis(50));
            if Instant::now() >= until {
                return Ok(false);
            }
        }
        match input.write(remaining) {
            Ok(0) => return Err("FFmpeg fechou a entrada de vídeo.".into()),
            Ok(count) => remaining = &remaining[count..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                let until = stopping_deadline.unwrap_or(deadline);
                let wait = until.saturating_duration_since(Instant::now());
                if wait.is_zero() {
                    if stopping_deadline.is_some() {
                        return Ok(false);
                    }
                    return Err(format!(
                        "FFmpeg não aceitou um frame em {} segundos.",
                        timeout.as_secs_f64()
                    ));
                }
                // A 1080p RGB frame is6.2MB, much larger than a kernel pipe.
                // Fixed sleeps per16/64KiB chunk limited throughput below4fps.
                // Poll wakes as soon as the reader frees space, with a20ms cap
                // solely to recheck stop/deadline when no reader makes progress.
                let _ = wait_pipe_writable(input, wait.min(Duration::from_millis(20)))?;
            }
            Err(error) => return Err(format!("Erro ao gravar frame: {error}")),
        }
    }
    Ok(true)
}

#[cfg(unix)]
fn wait_pipe_writable(input: &std::process::ChildStdin, timeout: Duration) -> Result<bool, String> {
    use std::os::fd::AsRawFd;
    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }
    #[cfg(target_os = "macos")]
    type Count = u32;
    #[cfg(not(target_os = "macos"))]
    type Count = usize;
    unsafe extern "C" {
        fn poll(descriptors: *mut PollFd, count: Count, timeout_ms: i32) -> i32;
    }
    let mut descriptor = PollFd {
        fd: input.as_raw_fd(),
        events: 0x0004,
        revents: 0,
    };
    let milliseconds = timeout.as_millis().clamp(1, i32::MAX as u128) as i32;
    let result = unsafe { poll(&mut descriptor, 1, milliseconds) };
    if result < 0 {
        let error = std::io::Error::last_os_error();
        return if error.kind() == std::io::ErrorKind::Interrupted {
            Ok(false)
        } else {
            Err(format!("Aguardar entrada do FFmpeg: {error}"))
        };
    }
    if descriptor.revents & (0x0008 | 0x0010 | 0x0020) != 0 {
        return Err("FFmpeg fechou a entrada de vídeo.".into());
    }
    Ok(result > 0 && descriptor.revents & 0x0004 != 0)
}

#[cfg(not(unix))]
fn wait_pipe_writable(_: &std::process::ChildStdin, _: Duration) -> Result<bool, String> {
    Err("Gravação disponível no macOS.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(unix)]
    fn last_drain_packet_retains_write_reserve_before_mux_finalization() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "sleep 0.15; wc -c"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        make_nonblocking(&input).unwrap();
        // Only100ms remain to START another packet. A real reader pauses150ms,
        // so reusing that deadline for its write would truncate the raw frame.
        let ended = Instant::now() - Duration::from_millis(5900);
        let timeout = drain_write_timeout(ended).unwrap();
        assert!(timeout >= Duration::from_millis(1900));
        assert_eq!(
            write_frame(
                &mut input,
                &vec![80; 256 * 1024],
                &AtomicBool::new(false),
                timeout
            ),
            Ok(true)
        );
        drop(input);
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "262144");
        assert!(drain_write_timeout(Instant::now() - Duration::from_secs(6)).is_err());
    }
    #[test]
    fn elapsed_frame_target_preserves_realtime_after_backpressure() {
        assert_eq!(frames_for_elapsed(Duration::from_secs(3), 60), 180);
        assert_eq!(frames_for_elapsed(Duration::from_millis(3010), 30), 90);
        assert_eq!(frames_for_elapsed(Duration::from_nanos(16_666_666), 60), 0);
        assert_eq!(frames_for_elapsed(Duration::from_nanos(16_666_667), 60), 1);
        // A slow writer with only136 frames must drain44, not finalize a
        // 2.267s file representing a3s capture.
        assert_eq!(frames_for_elapsed(Duration::from_secs(3), 60) - 136, 44);
    }

    #[test]
    fn sink_is_bounded_nonblocking_and_preserves_frame_ownership() {
        let (sender, receiver) = mpsc::sync_channel(2);
        let sink = RecorderSink {
            sender,
            frame_bytes: 3,
            stop: Arc::new(AtomicBool::new(false)),
            status: Arc::new(Mutex::new("test".into())),
            dropped: Arc::new(AtomicU64::new(0)),
        };
        assert!(sink.submit_owned(vec![1, 2, 3]).is_ok());
        assert!(sink.submit_owned(vec![4, 5, 6]).is_ok());
        assert!(sink.clone().submit_owned(vec![7, 8, 9]).is_ok());
        assert_eq!(sink.dropped.load(Ordering::Relaxed), 1);
        assert_eq!(receiver.try_recv().unwrap(), vec![1, 2, 3]);
        assert_eq!(receiver.try_recv().unwrap(), vec![4, 5, 6]);
        assert!(receiver.try_recv().is_err());
        assert!(sink.submit_owned(vec![1, 2]).is_err());
        sink.stop.store(true, Ordering::Release);
        assert!(sink.submit_owned(vec![1, 2, 3]).is_err());
    }

    #[test]
    fn diagnostics_keep_exact_newest_eight_kib() {
        let mut tail = Vec::new();
        append_tail(&mut tail, &vec![1; 8000]);
        append_tail(&mut tail, &vec![2; 4000]);
        assert_eq!(tail.len(), 8192);
        assert!(tail[..4192].iter().all(|byte| *byte == 1));
        assert!(tail[4192..].iter().all(|byte| *byte == 2));
        append_tail(&mut tail, &vec![3; 9000]);
        assert_eq!(tail, vec![3; 8192]);
    }

    #[test]
    fn segment_duration_outside_bound_fails_before_file_creation() {
        assert!(Recorder::start_with_options(
            160,
            120,
            30,
            PathBuf::from("unused.mp4"),
            false,
            86401
        )
        .is_err());
        assert_eq!(
            segmented_destination(Path::new("/tmp/example.mp4")).unwrap(),
            Path::new("/tmp/example-%03d.mp4")
        );
    }

    #[test]
    #[cfg(unix)]
    fn cancellation_interrupts_real_full_nonblocking_pipe() {
        // This child deliberately never reads its stdin, creating real kernel
        // pipe backpressure rather than a mocked WouldBlock implementation.
        let mut child = Command::new("/bin/sleep")
            .arg("5")
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        make_nonblocking(&input).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let signal = stop.clone();
        let cancel = thread::spawn(move || {
            thread::sleep(Duration::from_millis(40));
            let requested = Instant::now();
            signal.store(true, Ordering::Release);
            requested
        });
        let result = write_frame(
            &mut input,
            &vec![0u8; 1024 * 1024],
            &stop,
            Duration::from_millis(300),
        );
        let finished = Instant::now();
        let _ = child.kill();
        let _ = child.wait();
        let requested = cancel.join().unwrap();
        // Measure response to the actual signal, excluding scheduler delay
        // before the separate cancellation thread was able to request it.
        let elapsed = finished.saturating_duration_since(requested);
        assert_eq!(result, Ok(false));
        assert!(
            elapsed < Duration::from_millis(200),
            "cancel took {elapsed:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn normal_stop_finishes_partial_frame_when_reader_resumes_promptly() {
        use std::io::BufRead;
        let mut child=Command::new("/bin/sh").args(["-c","dd bs=4096 count=1 of=/dev/null 2>/dev/null; printf 'started\\n'; sleep 0.08; wc -c"])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let mut input = child.stdin.take().unwrap();
        make_nonblocking(&input).unwrap();
        let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
        let stop_requested = AtomicBool::new(false);
        let bytes = 256 * 1024;
        let writer = thread::spawn(move || {
            // Live recording finishes the current packet under its2s deadline;
            // the outer recording loop handles stop before starting the next.
            // Startup's separate50ms cancellation grace does not apply here.
            let finish_frame = AtomicBool::new(false);
            write_frame(
                &mut input,
                &vec![80; bytes],
                &finish_frame,
                Duration::from_secs(2),
            )
        });
        let mut started = String::new();
        output.read_line(&mut started).unwrap();
        stop_requested.store(true, Ordering::Release);
        let result = writer.join().unwrap();
        let mut remaining = String::new();
        output.read_to_string(&mut remaining).unwrap();
        assert!(child.wait().unwrap().success());
        assert!(stop_requested.load(Ordering::Acquire));
        assert_eq!(started.trim(), "started");
        assert_eq!(result, Ok(true));
        assert_eq!(remaining.trim().parse::<usize>().unwrap() + 4096, bytes);
    }

    #[test]
    fn ffmpeg_progress_not_pipe_acceptance_confirms_encoder_readiness() {
        let started = Arc::new(AtomicBool::new(false));
        drain_progress(
            std::io::Cursor::new(b"frame=0\nout_time_us=0\nprogress=continue\n"),
            started.clone(),
        );
        assert!(!started.load(Ordering::Acquire));
        assert!(
            frame_write_timeout(false, Instant::now() + Duration::from_secs(30)).unwrap()
                > Duration::from_secs(29)
        );
        drain_progress(
            std::io::Cursor::new(b"frame=  2\nout_time_us=33333\nprogress=continue\n"),
            started.clone(),
        );
        assert!(started.load(Ordering::Acquire));
        assert_eq!(
            frame_write_timeout(true, Instant::now() - Duration::from_secs(1)).unwrap(),
            Duration::from_secs(2)
        );
        assert!(frame_write_timeout(false, Instant::now() - Duration::from_secs(1)).is_err());
    }
}
