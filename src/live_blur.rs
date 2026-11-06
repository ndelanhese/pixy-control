//! Bounded live-call renderer. Vision never runs on the frame delivery queue.
//! A late/absent mask means full blur, never an unprocessed camera frame.
use crate::background_blur::PersonMask;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Condvar, Mutex,
};
use std::time::{Duration, Instant};

struct Input {
    rgb: Vec<u8>,
    size: (u32, u32),
    received: Instant,
    generation: u64,
}
struct Mask {
    image: PersonMask,
    received: Instant,
    generation: u64,
    work_ms: f64,
}
struct Shared {
    input: Mutex<Option<Input>>,
    wake: Condvar,
    mask: Mutex<Option<Arc<Mask>>>,
    stop: AtomicBool,
}
struct Worker {
    shared: Arc<Shared>,
}
impl Worker {
    fn start() -> Self {
        Self::spawn({
            let mut segmenter = None;
            let mut generation = None;
            move |input: &Input| {
                if segmenter.is_none() {
                    segmenter = Some(crate::background_blur::Segmenter::new()?);
                }
                let segmenter = segmenter.as_mut().ok_or("Segmentação indisponível")?;
                if generation != Some(input.generation) {
                    segmenter.reset()?;
                    generation = Some(input.generation);
                }
                segmenter.generate(input.size.0, input.size.1, &input.rgb)
            }
        })
    }
    fn spawn(
        mut generate: impl FnMut(&Input) -> Result<PersonMask, String> + Send + 'static,
    ) -> Self {
        let shared = Arc::new(Shared {
            input: Mutex::new(None),
            wake: Condvar::new(),
            mask: Mutex::new(None),
            stop: AtomicBool::new(false),
        });
        let state = shared.clone();
        std::thread::spawn(move || {
            let _ = crate::video_activity::prioritize_current_thread();
            while !state.stop.load(Ordering::Acquire) {
                let input = {
                    let Ok(slot) = state.input.lock() else { break };
                    let Ok((mut slot, _)) =
                        state
                            .wake
                            .wait_timeout_while(slot, Duration::from_millis(100), |s| {
                                s.is_none() && !state.stop.load(Ordering::Acquire)
                            })
                    else {
                        break;
                    };
                    slot.take()
                };
                let Some(input) = input else { continue };
                let start = Instant::now();
                let mask = generate(&input).ok().map(|image| {
                    Arc::new(Mask {
                        image,
                        received: input.received,
                        generation: input.generation,
                        work_ms: start.elapsed().as_secs_f64() * 1000.0,
                    })
                });
                if state.stop.load(Ordering::Acquire) {
                    break;
                }
                if let Ok(mut slot) = state.mask.lock() {
                    *slot = mask;
                }
            }
        });
        Self { shared }
    }
    fn submit(&self, input: Input) {
        if let Ok(mut slot) = self.shared.input.try_lock() {
            *slot = Some(input);
            self.shared.wake.notify_one();
        }
    }
    fn current(&self, generation: u64) -> Option<Arc<Mask>> {
        self.shared
            .mask
            .try_lock()
            .ok()?
            .as_ref()
            .filter(|m| {
                m.generation == generation && m.received.elapsed() <= Duration::from_millis(150)
            })
            .cloned()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.wake.notify_one();
    }
}

fn thread_cpu_seconds() -> f64 {
    #[repr(C)]
    struct Time {
        seconds: i64,
        nanos: i64,
    }
    unsafe extern "C" {
        fn clock_gettime(clock: i32, time: *mut Time) -> i32;
    }
    let mut time = Time {
        seconds: 0,
        nanos: 0,
    };
    if unsafe { clock_gettime(16, &mut time) } == 0 {
        time.seconds as f64 + time.nanos as f64 / 1e9
    } else {
        0.0
    }
}
#[derive(Clone, Copy, Default, serde::Serialize)]
pub struct Stats {
    pub cpu_ms: f64,
    pub input_ms: f64,
    pub compose_ms: f64,
    pub total_ms: f64,
    pub mask_age_ms: Option<f64>,
    pub mask_worker_ms: Option<f64>,
    pub full_blur_fallback: bool,
}

#[derive(Default)]
pub struct Processor {
    native: native::Buffers,
    worker: Option<Worker>,
    key: Option<((u32, u32), [f64; 8])>,
    generation: u64,
    next_mask: Option<Instant>,
    stats: Stats,
}
impl Processor {
    pub fn stats(&self) -> Stats {
        self.stats
    }
    #[allow(clippy::too_many_arguments)]
    pub fn apply(
        &mut self,
        source: &[u8],
        stride: usize,
        size: (u32, u32),
        canvas: (u32, u32),
        matrix: [f64; 8],
        amount: f32,
        received: Instant,
    ) -> Result<Vec<u8>, String> {
        if !amount.is_finite() || !(0.0..=1.0).contains(&amount) {
            return Err("Desfoque inválido".into());
        }
        let start = Instant::now();
        let cpu_start = thread_cpu_seconds();
        self.native.crop(source, stride, size, canvas, matrix)?;
        if self.key != Some((canvas, matrix)) {
            self.key = Some((canvas, matrix));
            self.generation = self.generation.wrapping_add(1);
            self.next_mask = None;
        }
        if amount == 0.0 {
            self.worker = None;
            self.next_mask = None;
            let rgb = self.native.sharp_rgb(canvas)?;
            self.stats = Stats {
                cpu_ms: (thread_cpu_seconds() - cpu_start) * 1000.0,
                input_ms: start.elapsed().as_secs_f64() * 1000.0,
                total_ms: start.elapsed().as_secs_f64() * 1000.0,
                ..Default::default()
            };
            return Ok(rgb);
        }
        self.native.prepare_small(canvas)?;
        if self.worker.is_none() {
            self.worker = Some(Worker::start());
        }
        if self
            .next_mask
            .is_none_or(|deadline| Instant::now() >= deadline)
        {
            let (size, rgb) = self.native.segmentation_input(canvas)?;
            if let Some(worker) = &self.worker {
                worker.submit(Input {
                    rgb,
                    size,
                    received,
                    generation: self.generation,
                });
            }
            self.next_mask = Some(Instant::now() + Duration::from_millis(66));
        }
        let input_ms = start.elapsed().as_secs_f64() * 1000.0;
        let mask = self
            .worker
            .as_ref()
            .and_then(|worker| worker.current(self.generation));
        let mask_age_ms = mask
            .as_ref()
            .map(|m| m.received.elapsed().as_secs_f64() * 1000.0);
        let compose = Instant::now();
        let rgb = self
            .native
            .blurred(canvas, amount, mask.as_ref().map(|m| &m.image))?;
        self.stats = Stats {
            cpu_ms: (thread_cpu_seconds() - cpu_start) * 1000.0,
            input_ms,
            compose_ms: compose.elapsed().as_secs_f64() * 1000.0,
            total_ms: start.elapsed().as_secs_f64() * 1000.0,
            mask_age_ms,
            mask_worker_ms: mask.as_ref().map(|m| m.work_ms),
            full_blur_fallback: mask.is_none(),
        };
        Ok(rgb)
    }
}

mod native {
    use super::PersonMask;
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
        fn vImageScale_ARGB8888(
            src: *const Buffer,
            dest: *const Buffer,
            temp: *mut c_void,
            flags: u32,
        ) -> isize;
        fn vImageScale_Planar8(
            src: *const Buffer,
            dest: *const Buffer,
            temp: *mut c_void,
            flags: u32,
        ) -> isize;
        fn vImageBoxConvolve_ARGB8888(
            src: *const Buffer,
            dest: *const Buffer,
            temp: *mut c_void,
            x: usize,
            y: usize,
            kh: u32,
            kw: u32,
            bg: *const u8,
            flags: u32,
        ) -> isize;
        fn vImageConvert_BGRA8888toRGB888(
            src: *const Buffer,
            dest: *const Buffer,
            flags: u32,
        ) -> isize;
    }
    fn checked(code: isize) -> Result<(), String> {
        if code < 0 {
            Err(format!("Processamento de vídeo: {code}"))
        } else {
            Ok(())
        }
    }
    fn view(bytes: &mut [u8], w: usize, h: usize, channels: usize) -> Buffer {
        Buffer {
            data: bytes.as_mut_ptr().cast(),
            height: h,
            width: w,
            row_bytes: w * channels,
        }
    }
    unsafe fn scale(
        src: &Buffer,
        dest: &Buffer,
        temp: &mut Vec<u8>,
        planar: bool,
    ) -> Result<(), String> {
        let call = if planar {
            vImageScale_Planar8
        } else {
            vImageScale_ARGB8888
        };
        let size = call(src, dest, ptr::null_mut(), 16 | 128);
        checked(size)?;
        temp.resize(size as usize, 0);
        checked(call(
            src,
            dest,
            if temp.is_empty() {
                ptr::null_mut()
            } else {
                temp.as_mut_ptr().cast()
            },
            16,
        ))
    }
    #[derive(Default)]
    pub(super) struct Buffers {
        sharp: Vec<u8>,
        small: Vec<u8>,
        a: Vec<u8>,
        b: Vec<u8>,
        background: Vec<u8>,
        mid: Vec<u8>,
        mask: Vec<u8>,
        temp: Vec<u8>,
    }
    impl Buffers {
        pub(super) fn crop(
            &mut self,
            bytes: &[u8],
            stride: usize,
            size: (u32, u32),
            canvas: (u32, u32),
            m: [f64; 8],
        ) -> Result<(), String> {
            let (w, h) = (size.0 as usize, size.1 as usize);
            let (ow, oh) = (canvas.0 as usize, canvas.1 as usize);
            if w < 2
                || h < 2
                || w > 16384
                || h > 16384
                || ow < 2
                || oh < 2
                || ow > 1920
                || oh > 1920
                || stride < w * 4
                || stride.checked_mul(h).is_none_or(|n| n > bytes.len())
                || m.iter().any(|v| !v.is_finite())
                || m[1] != 0.0
                || m[3] != 0.0
                || m[6] != 0.0
                || m[7] != 0.0
                || m[0] == 0.0
                || m[4] <= 0.0
            {
                return Err("Buffer/enquadramento de chamada inválido".into());
            }
            let x0 = (m[2] * (w - 1) as f64).round();
            let x1 = ((m[0] + m[2]) * (w - 1) as f64).round();
            let y0 = (m[5] * (h - 1) as f64).round();
            let y1 = ((m[4] + m[5]) * (h - 1) as f64).round();
            if x0.min(x1) < 0.0
                || x0.max(x1) >= (w as f64)
                || y0 < 0.0
                || y1 >= (h as f64)
                || y1 < y0
            {
                return Err("Recorte de chamada fora da câmera".into());
            }
            let x = x0.min(x1) as usize;
            let y = y0 as usize;
            let cw = (x1 - x0).abs() as usize + 1;
            let ch = (y1 - y0) as usize + 1;
            self.sharp.resize(ow * oh * 4, 0);
            let src = Buffer {
                data: unsafe { bytes.as_ptr().add(y * stride + x * 4) } as *mut c_void,
                height: ch,
                width: cw,
                row_bytes: stride,
            };
            let dest = view(&mut self.sharp, ow, oh, 4);
            unsafe {
                scale(&src, &dest, &mut self.temp, false)?;
            }
            if x1 < x0 {
                for row in self.sharp.chunks_exact_mut(ow * 4) {
                    for x in 0..ow / 2 {
                        for c in 0..4 {
                            row.swap(x * 4 + c, (ow - 1 - x) * 4 + c);
                        }
                    }
                }
            }
            Ok(())
        }
        pub(super) fn sharp_rgb(&mut self, canvas: (u32, u32)) -> Result<Vec<u8>, String> {
            let (w, h) = (canvas.0 as usize, canvas.1 as usize);
            let mut rgb = vec![0; w * h * 3];
            unsafe {
                checked(vImageConvert_BGRA8888toRGB888(
                    &view(&mut self.sharp, w, h, 4),
                    &view(&mut rgb, w, h, 3),
                    16,
                ))?;
            }
            Ok(rgb)
        }
        pub(super) fn prepare_small(&mut self, canvas: (u32, u32)) -> Result<(), String> {
            let (w, h) = (canvas.0 as usize, canvas.1 as usize);
            let (sw, sh) = ((w / 4).max(2), (h / 4).max(2));
            self.small.resize(sw * sh * 4, 0);
            unsafe {
                scale(
                    &view(&mut self.sharp, w, h, 4),
                    &view(&mut self.small, sw, sh, 4),
                    &mut self.temp,
                    false,
                )
            }
        }
        pub(super) fn segmentation_input(
            &mut self,
            canvas: (u32, u32),
        ) -> Result<((u32, u32), Vec<u8>), String> {
            let (sw, sh) = (
                ((canvas.0 as usize) / 4).max(2),
                ((canvas.1 as usize) / 4).max(2),
            );
            let mut rgb = vec![0; sw * sh * 3];
            unsafe {
                checked(vImageConvert_BGRA8888toRGB888(
                    &view(&mut self.small, sw, sh, 4),
                    &view(&mut rgb, sw, sh, 3),
                    16,
                ))?;
            }
            Ok(((sw as u32, sh as u32), rgb))
        }
        pub(super) fn blurred(
            &mut self,
            canvas: (u32, u32),
            amount: f32,
            mask: Option<&PersonMask>,
        ) -> Result<Vec<u8>, String> {
            let (w, h) = (canvas.0 as usize, canvas.1 as usize);
            let (sw, sh) = ((w / 4).max(2), (h / 4).max(2));
            self.a.resize(sw * sh * 4, 0);
            self.b.resize(sw * sh * 4, 0);
            self.background.resize(w * h * 4, 0);
            self.mask.resize(w * h, 0);
            unsafe {
                self.a.copy_from_slice(&self.small);
                let k = (((f64::from(amount) * 24.0 * h as f64 / 1080.0 / 4.0 * 2.0).round()
                    as usize)
                    | 1)
                .min((sw.min(sh) - 1) | 1)
                .max(1) as u32;
                for _ in 0..3 {
                    let src = view(&mut self.a, sw, sh, 4);
                    let dest = view(&mut self.b, sw, sh, 4);
                    let size = vImageBoxConvolve_ARGB8888(
                        &src,
                        &dest,
                        ptr::null_mut(),
                        0,
                        0,
                        k,
                        k,
                        ptr::null(),
                        8 | 16 | 128,
                    );
                    checked(size)?;
                    self.temp.resize(size as usize, 0);
                    checked(vImageBoxConvolve_ARGB8888(
                        &src,
                        &dest,
                        if self.temp.is_empty() {
                            ptr::null_mut()
                        } else {
                            self.temp.as_mut_ptr().cast()
                        },
                        0,
                        0,
                        k,
                        k,
                        ptr::null(),
                        8 | 16,
                    ))?;
                    std::mem::swap(&mut self.a, &mut self.b);
                }
                if sw * 4 == w && sh * 4 == h {
                    upsample2x(&self.a, sw, sh, &mut self.mid);
                    upsample2x(&self.mid, sw * 2, sh * 2, &mut self.background);
                } else {
                    scale(
                        &view(&mut self.a, sw, sh, 4),
                        &view(&mut self.background, w, h, 4),
                        &mut self.temp,
                        false,
                    )?;
                }
                if let Some(mask) = mask {
                    if mask.width == 0
                        || mask.height == 0
                        || mask.pixels.len() != mask.width as usize * mask.height as usize
                    {
                        return Err("Máscara de chamada inválida".into());
                    }
                    let src = Buffer {
                        data: mask.pixels.as_ptr() as *mut c_void,
                        width: mask.width as usize,
                        height: mask.height as usize,
                        row_bytes: mask.width as usize,
                    };
                    scale(&src, &view(&mut self.mask, w, h, 1), &mut self.temp, true)?;
                } else {
                    self.mask.fill(0);
                }
            }
            let mut rgb = vec![0; w * h * 3];
            blend(&self.sharp, &self.background, &self.mask, &mut rgb);
            Ok(rgb)
        }
    }
    fn blend(sharp: &[u8], background: &[u8], mask: &[u8], rgb: &mut [u8]) {
        for (((s, b), &m), out) in sharp
            .chunks_exact(4)
            .zip(background.chunks_exact(4))
            .zip(mask)
            .zip(rgb.chunks_exact_mut(3))
        {
            for c in 0..3 {
                out[c] = ((u16::from(s[2 - c]) * u16::from(m)
                    + u16::from(b[2 - c]) * u16::from(255 - m)
                    + 127)
                    / 255) as u8;
            }
        }
    }
    fn upsample2x(src: &[u8], w: usize, h: usize, dst: &mut Vec<u8>) {
        assert_eq!(src.len(), w * h * 4);
        dst.resize(w * h * 16, 0);
        for y in 0..h {
            let r0 = y * w * 4;
            let r1 = (y + 1).min(h - 1) * w * 4;
            let d0 = y * 2 * w * 8;
            let d1 = d0 + w * 8;
            let mut x = 0;
            #[cfg(target_arch = "aarch64")]
            unsafe {
                use std::arch::aarch64::*;
                while x + 4 < w {
                    let a = vld1q_u8(src.as_ptr().add(r0 + x * 4));
                    let b = vld1q_u8(src.as_ptr().add(r0 + (x + 1) * 4));
                    let c = vld1q_u8(src.as_ptr().add(r1 + x * 4));
                    let d = vld1q_u8(src.as_ptr().add(r1 + (x + 1) * 4));
                    let even = vrhaddq_u8(a, b);
                    let odd = vrhaddq_u8(a, c);
                    let odd_next = vrhaddq_u8(b, d);
                    let center = vrhaddq_u8(odd, odd_next);
                    vst1q_u8(
                        dst.as_mut_ptr().add(d0 + x * 8),
                        vreinterpretq_u8_u32(vzip1q_u32(
                            vreinterpretq_u32_u8(a),
                            vreinterpretq_u32_u8(even),
                        )),
                    );
                    vst1q_u8(
                        dst.as_mut_ptr().add(d0 + x * 8 + 16),
                        vreinterpretq_u8_u32(vzip2q_u32(
                            vreinterpretq_u32_u8(a),
                            vreinterpretq_u32_u8(even),
                        )),
                    );
                    vst1q_u8(
                        dst.as_mut_ptr().add(d1 + x * 8),
                        vreinterpretq_u8_u32(vzip1q_u32(
                            vreinterpretq_u32_u8(odd),
                            vreinterpretq_u32_u8(center),
                        )),
                    );
                    vst1q_u8(
                        dst.as_mut_ptr().add(d1 + x * 8 + 16),
                        vreinterpretq_u8_u32(vzip2q_u32(
                            vreinterpretq_u32_u8(odd),
                            vreinterpretq_u32_u8(center),
                        )),
                    );
                    x += 4;
                }
            }
            for x in x..w {
                for ch in 0..4 {
                    let a = u16::from(src[r0 + x * 4 + ch]);
                    let b = u16::from(src[r0 + (x + 1).min(w - 1) * 4 + ch]);
                    let c = u16::from(src[r1 + x * 4 + ch]);
                    let d = u16::from(src[r1 + (x + 1).min(w - 1) * 4 + ch]);
                    dst[d0 + x * 8 + ch] = a as u8;
                    dst[d0 + x * 8 + 4 + ch] = (a + b).div_ceil(2) as u8;
                    dst[d1 + x * 8 + ch] = (a + c).div_ceil(2) as u8;
                    dst[d1 + x * 8 + 4 + ch] = ((a + b + c + d + 2) / 4) as u8;
                }
            }
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn vector_upsampling_matches_scalar_bilinear_at_every_tail_and_row_edge() {
            for w in [1, 2, 5, 8, 13, 480] {
                for h in [1, 4] {
                    let src: Vec<_> = (0..w * h * 4)
                        .map(|i| ((i * 37 + 13) % 256) as u8)
                        .collect();
                    let mut dst = Vec::new();
                    upsample2x(&src, w, h, &mut dst);
                    for y in 0..h * 2 {
                        for x in 0..w * 2 {
                            for c in 0..4 {
                                let sx = x / 2;
                                let sy = y / 2;
                                let fx = (x % 2) as u32;
                                let fy = (y % 2) as u32;
                                let a = u32::from(src[(sy * w + sx) * 4 + c]);
                                let b = u32::from(src[(sy * w + (sx + 1).min(w - 1)) * 4 + c]);
                                let d = u32::from(src[((sy + 1).min(h - 1) * w + sx) * 4 + c]);
                                let e = u32::from(
                                    src[((sy + 1).min(h - 1) * w + (sx + 1).min(w - 1)) * 4 + c],
                                );
                                let expected = ((a * (2 - fx) * (2 - fy)
                                    + b * fx * (2 - fy)
                                    + d * (2 - fx) * fy
                                    + e * fx * fy
                                    + 2)
                                    / 4) as u8;
                                assert!(dst[(y * w * 2 + x) * 4 + c].abs_diff(expected) <= 1);
                            }
                        }
                    }
                    upsample2x(&vec![0; src.len()], w, h, &mut dst);
                    assert!(dst.iter().all(|p| *p == 0));
                }
            }
        }
        #[test]
        fn background_linear_upsampling_preserves_channels_and_edges() {
            let src: Vec<_> = [0, 100, 200, 255]
                .into_iter()
                .flat_map(|r| [10, 20, r, 255])
                .collect();
            let mut out = Vec::new();
            upsample2x(&src, 2, 2, &mut out);
            let reds: Vec<_> = out.chunks_exact(4).map(|p| p[2]).collect();
            assert_eq!(
                reds,
                vec![0, 50, 100, 100, 100, 139, 178, 178, 200, 228, 255, 255, 200, 228, 255, 255]
            );
            assert!(out
                .chunks_exact(4)
                .all(|p| p[0] == 10 && p[1] == 20 && p[3] == 255));
        }
        #[test]
        fn alpha_mask_preserves_rgb_order_and_mixes_edges() {
            let mut rgb = vec![0; 9];
            blend(
                &[10, 20, 30, 255, 10, 20, 30, 255, 10, 20, 30, 255],
                &[90, 100, 110, 255, 90, 100, 110, 255, 90, 100, 110, 255],
                &[255, 0, 128],
                &mut rgb,
            );
            assert_eq!(rgb, vec![30, 20, 10, 110, 100, 90, 70, 60, 50]);
        }
        #[test]
        fn native_crop_uses_padded_camera_rows_and_exact_selected_roi() {
            let mut b = vec![199; 72 * 8];
            for y in 0..8 {
                for x in 0..16 {
                    b[y * 72 + x * 4..y * 72 + x * 4 + 4].copy_from_slice(&[
                        100,
                        (y * 20) as u8,
                        (x * 10) as u8,
                        255,
                    ]);
                }
            }
            let mut p = Buffers::default();
            p.crop(
                &b,
                72,
                (16, 8),
                (8, 4),
                [7.0 / 15.0, 0.0, 8.0 / 15.0, 0.0, 3.0 / 7.0, 0.0, 0.0, 0.0],
            )
            .unwrap();
            let rgb = p.sharp_rgb((8, 4)).unwrap();
            assert_eq!(&rgb[..3], &[80, 0, 100]);
            assert_eq!(&rgb[93..], &[150, 60, 100]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_or_wrong_geometry_masks_cannot_expose_sharp_pixels() {
        let worker = Worker::spawn(|_| Err("unused".into()));
        let mask = Mask {
            image: PersonMask {
                width: 2,
                height: 2,
                pixels: vec![255; 4],
            },
            generation: 1,
            received: Instant::now() - Duration::from_secs(1),
            work_ms: 1.0,
        };
        *worker.shared.mask.lock().unwrap() = Some(Arc::new(mask));
        assert!(worker.current(1).is_none());
        let mask = Mask {
            image: PersonMask {
                width: 2,
                height: 2,
                pixels: vec![255; 4],
            },
            generation: 1,
            received: Instant::now(),
            work_ms: 1.0,
        };
        *worker.shared.mask.lock().unwrap() = Some(Arc::new(mask));
        assert!(worker.current(2).is_none());
        assert!(worker.current(1).is_some());
    }
    #[test]
    fn stalled_model_cannot_block_real_frame_processing() {
        let (entered, waiting) = std::sync::mpsc::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let worker = Worker::spawn(move |_| {
            let _ = entered.send(());
            let _ = blocked.recv();
            Err("delayed model".into())
        });
        worker.submit(Input {
            rgb: vec![0; 12],
            size: (2, 2),
            generation: 1,
            received: Instant::now(),
        });
        waiting.recv_timeout(Duration::from_secs(1)).unwrap();
        let mut processor = Processor {
            worker: Some(worker),
            ..Default::default()
        };
        let bytes: Vec<_> = (0..64 * 32)
            .flat_map(|i| [if i % 64 < 32 { 0 } else { 255 }, 80, 160, 255])
            .collect();
        let start = Instant::now();
        let result = processor.apply(
            &bytes,
            64 * 4,
            (64, 32),
            (64, 32),
            [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            0.8,
            Instant::now(),
        );
        let elapsed = start.elapsed();
        let _ = release.send(());
        let rgb = result.unwrap();
        assert_eq!(rgb.len(), 64 * 32 * 3);
        assert!(processor.stats.full_blur_fallback);
        assert!(
            elapsed < Duration::from_millis(100),
            "frame waited on inference"
        );
        assert!(rgb.chunks_exact(3).any(|p| p[2] > 0 && p[2] < 255));
    }
}
