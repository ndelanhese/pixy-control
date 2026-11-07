#![allow(unexpected_cfgs)] // objc 0.2 selector macros
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reaction {
    Heart,
    Like,
    Dislike,
    Balloons,
    Confetti,
    Fireworks,
    Rain,
    Lasers,
}

impl Reaction {
    pub const ALL: [(Self, &str); 8] = [
        (Self::Heart, "Corações"),
        (Self::Like, "Joinha"),
        (Self::Dislike, "Polegar para baixo"),
        (Self::Balloons, "Balões"),
        (Self::Confetti, "Confete"),
        (Self::Fireworks, "Fogos"),
        (Self::Rain, "Chuva"),
        (Self::Lasers, "Lasers"),
    ];
}

#[derive(Default)]
struct Gate {
    candidate: Option<(Reaction, Instant, Instant)>,
    held: Option<Reaction>,
    last_fired: Option<Instant>,
    last_seen: Option<Instant>,
}

impl Gate {
    fn observe(&mut self, pose: Option<Reaction>, now: Instant) -> Option<Reaction> {
        if self
            .last_seen
            .is_some_and(|seen| now.duration_since(seen) > Duration::from_millis(550))
        {
            self.candidate = None;
            self.held = None;
        }
        self.last_seen = Some(now);
        let Some(pose) = pose else {
            self.candidate = None;
            self.held = None;
            return None;
        };
        if self.held == Some(pose) {
            return None;
        }
        self.held = None;
        let first = self
            .candidate
            .filter(|(previous, _, seen)| {
                *previous == pose && now.duration_since(*seen) <= Duration::from_millis(550)
            })
            .map_or(now, |(_, first, _)| first);
        self.candidate = Some((pose, first, now));
        if now.duration_since(first) < Duration::from_millis(450)
            || self
                .last_fired
                .is_some_and(|t| now.duration_since(t) < Duration::from_secs(3))
        {
            return None;
        }
        self.last_fired = Some(now);
        self.held = Some(pose);
        Some(pose)
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::{Gate, Reaction};
    use objc::runtime::{Object, BOOL, YES};
    use objc::{class, msg_send, sel, sel_impl, Encode, Encoding};
    use std::{
        ffi::c_void,
        ptr,
        sync::mpsc,
        time::{Duration, Instant},
    };

    // Small copied BGRA input. Each channel carries at most one item.
    struct Input {
        size: (usize, usize),
        bytes: Vec<u8>,
        at: Instant,
    }

    struct Worker {
        input: mpsc::SyncSender<Input>,
        output: mpsc::Receiver<(Reaction, Instant)>,
    }

    impl Worker {
        fn start() -> Self {
            let (input, pending) = mpsc::sync_channel::<Input>(1);
            let (completed, output) = mpsc::sync_channel(1);
            std::thread::spawn(move || unsafe {
                let _ = crate::video_activity::prioritize_metadata_thread();
                let _pool = Pool::new();
                let request: *mut Object = msg_send![class!(VNDetectHumanHandPoseRequest), new];
                let _: () = msg_send![request, setMaximumHandCount:2usize];
                let _: () = msg_send![request, setPreferBackgroundProcessing:YES];
                let sequence: *mut Object = msg_send![class!(VNSequenceRequestHandler), new];
                let requests: *mut Object = msg_send![class!(NSArray), arrayWithObject:request];
                let mut gate = Gate::default();
                while let Ok(input) = pending.recv() {
                    if input.at.elapsed() > Duration::from_millis(600) {
                        continue;
                    }
                    let _frame_pool = Pool::new();
                    let at = input.at;
                    let pose = detect(input, sequence, request, requests);
                    if at.elapsed() > Duration::from_millis(850) {
                        continue;
                    }
                    if let Some(reaction) = gate.observe(pose, at) {
                        let _ = completed.try_send((reaction, at));
                    }
                }
                let _: () = msg_send![request, release];
                let _: () = msg_send![sequence, release];
            });
            Self { input, output }
        }
    }

    #[derive(Default)]
    pub struct Live {
        worker: Option<Worker>,
        next: Option<Instant>,
        active: Option<(Reaction, Instant)>,
    }

    impl Live {
        pub fn update(
            &mut self,
            enabled: bool,
            manual: Option<Reaction>,
            source: &[u8],
            stride: usize,
            size: (usize, usize),
        ) -> Option<(Reaction, Duration)> {
            let now = Instant::now();
            if !enabled {
                if self.worker.take().is_some() {
                    self.active = None;
                }
                self.next = None;
            } else {
                let worker = self.worker.get_or_insert_with(Worker::start);
                if self.next.is_none_or(|at| now >= at) {
                    let (w, h) = size;
                    let max = w.max(h);
                    let (sw, sh) = ((w * 480 / max).max(1), (h * 480 / max).max(1));
                    let mut bytes = Vec::with_capacity(sw * sh * 4);
                    for y in 0..sh {
                        for x in 0..sw {
                            let i = (y * h / sh) * stride + (x * w / sw) * 4;
                            bytes.extend_from_slice(&source[i..i + 4]);
                        }
                    }
                    let _ = worker.input.try_send(Input {
                        size: (sw, sh),
                        bytes,
                        at: now,
                    });
                    self.next = Some(now + Duration::from_millis(205));
                }
                if let Ok((reaction, at)) = worker.output.try_recv() {
                    if at.elapsed() <= Duration::from_millis(850) {
                        self.active = Some((reaction, now));
                    }
                }
            }
            if let Some(reaction) = manual {
                self.active = Some((reaction, now));
            }
            self.active
                .map(|(reaction, at)| (reaction, now.duration_since(at)))
                .filter(|(_, elapsed)| elapsed.as_secs_f32() < 2.6)
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

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Point {
        x: f64,
        y: f64,
    }
    unsafe impl Encode for Point {
        fn encode() -> Encoding {
            unsafe { Encoding::from_str("{CGPoint=dd}") }
        }
    }

    #[link(name = "CoreVideo", kind = "framework")]
    unsafe extern "C" {
        fn CVPixelBufferCreateWithBytes(
            allocator: *const c_void,
            width: usize,
            height: usize,
            format: u32,
            bytes: *mut c_void,
            stride: usize,
            release: *const c_void,
            context: *mut c_void,
            attributes: *const c_void,
            output: *mut *mut c_void,
        ) -> i32;
        fn CVPixelBufferRelease(buffer: *mut c_void);
    }
    #[link(name = "Vision", kind = "framework")]
    unsafe extern "C" {
        static VNHumanHandPoseObservationJointsGroupNameAll: *mut Object;
        static VNHumanHandPoseObservationJointNameWrist: *mut Object;
        static VNHumanHandPoseObservationJointNameThumbIP: *mut Object;
        static VNHumanHandPoseObservationJointNameThumbTip: *mut Object;
        static VNHumanHandPoseObservationJointNameIndexMCP: *mut Object;
        static VNHumanHandPoseObservationJointNameIndexPIP: *mut Object;
        static VNHumanHandPoseObservationJointNameIndexTip: *mut Object;
        static VNHumanHandPoseObservationJointNameMiddleMCP: *mut Object;
        static VNHumanHandPoseObservationJointNameMiddlePIP: *mut Object;
        static VNHumanHandPoseObservationJointNameMiddleTip: *mut Object;
        static VNHumanHandPoseObservationJointNameRingPIP: *mut Object;
        static VNHumanHandPoseObservationJointNameRingTip: *mut Object;
        static VNHumanHandPoseObservationJointNameLittlePIP: *mut Object;
        static VNHumanHandPoseObservationJointNameLittleTip: *mut Object;
    }

    unsafe extern "C" fn release_pixels(context: *mut c_void, _: *const c_void) {
        drop(Box::from_raw(context.cast::<Vec<u8>>()));
    }

    unsafe fn detect(
        input: Input,
        sequence: *mut Object,
        request: *mut Object,
        requests: *mut Object,
    ) -> Option<Reaction> {
        let mut buffer = ptr::null_mut();
        // CVPixelBuffer and Vision may retain the pixels after performRequests.
        // Transfer allocation ownership to the Core Video release callback.
        let bytes = Box::into_raw(Box::new(input.bytes));
        if CVPixelBufferCreateWithBytes(
            ptr::null(),
            input.size.0,
            input.size.1,
            u32::from_be_bytes(*b"BGRA"),
            (*bytes).as_mut_ptr().cast(),
            input.size.0 * 4,
            release_pixels as *const c_void,
            bytes.cast(),
            ptr::null(),
            &mut buffer,
        ) != 0
            || buffer.is_null()
        {
            drop(Box::from_raw(bytes));
            return None;
        }
        let mut error: *mut Object = ptr::null_mut();
        let ok: BOOL = msg_send![sequence, performRequests:requests onCVPixelBuffer:buffer orientation:1u32 error:&mut error];
        CVPixelBufferRelease(buffer);
        if ok != YES {
            return None;
        }
        let results: *mut Object = msg_send![request, results];
        let count: usize = msg_send![results, count];
        let mut hands = [None; 2];
        for (i, hand) in hands.iter_mut().enumerate().take(count) {
            let observation: *mut Object = msg_send![results, objectAtIndex:i];
            let points: *mut Object = msg_send![observation, recognizedPointsForJointsGroupName:VNHumanHandPoseObservationJointsGroupNameAll error:&mut error];
            if points.is_null() {
                continue;
            }
            *hand = Hand::read(points, input.size.0 as f64 / input.size.1 as f64);
        }
        match hands {
            [Some(a), Some(b)] if a.heart_with(b) => Some(Reaction::Heart),
            [Some(a), Some(b)] => match (a.pose(), b.pose()) {
                (Some(Reaction::Like), Some(Reaction::Like)) => Some(Reaction::Fireworks),
                (Some(Reaction::Dislike), Some(Reaction::Dislike)) => Some(Reaction::Rain),
                (Some(Reaction::Balloons), Some(Reaction::Balloons)) => Some(Reaction::Confetti),
                (Some(Reaction::Lasers), Some(Reaction::Lasers)) => Some(Reaction::Lasers),
                (Some(Reaction::Lasers), _) | (_, Some(Reaction::Lasers)) => None,
                (Some(pose), None) | (None, Some(pose)) => Some(pose),
                _ => None,
            },
            [Some(a), None] | [None, Some(a)] => a.pose().filter(|r| *r != Reaction::Lasers),
            _ => None,
        }
    }

    #[cfg(test)]
    #[test]
    #[ignore = "requires local MediaPipe photo fixtures in PIXY_GESTURE_FIXTURES"]
    fn vision_classifies_real_hand_photos() {
        let dir = std::path::PathBuf::from(std::env::var_os("PIXY_GESTURE_FIXTURES").unwrap());
        let fixture = |name: &str| image::open(dir.join(name)).unwrap().into_rgb8();
        let up = fixture("thumb_up.jpg");
        let down = image::imageops::rotate180(&up);
        let victory = fixture("victory.jpg");
        let chart = fixture("hagrid.png");
        let rock = image::imageops::crop_imm(&chart, 1785, 494, 300, 300).to_image();
        let two = |img: &image::RgbImage| {
            let mut pair = image::RgbImage::new(img.width() * 2, img.height());
            image::imageops::replace(&mut pair, img, 0, 0);
            image::imageops::replace(&mut pair, img, i64::from(img.width()), 0);
            pair
        };
        for (image, expected) in [
            (up.clone(), Some(Reaction::Like)),
            (down.clone(), Some(Reaction::Dislike)),
            (victory.clone(), Some(Reaction::Balloons)),
            (two(&up), Some(Reaction::Fireworks)),
            (two(&down), Some(Reaction::Rain)),
            (two(&victory), Some(Reaction::Confetti)),
            (fixture("heart.jpg"), Some(Reaction::Heart)),
            (two(&rock), Some(Reaction::Lasers)),
            (rock, None),
            (fixture("fist.jpg"), None),
            (fixture("pointing_up.jpg"), None),
        ] {
            let longest = image.width().max(image.height()).max(480);
            let image = image::imageops::resize(
                &image,
                image.width() * 480 / longest,
                image.height() * 480 / longest,
                image::imageops::FilterType::Nearest,
            );
            let worker = Worker::start();
            let input = Input {
                size: (image.width() as usize, image.height() as usize),
                bytes: image
                    .pixels()
                    .flat_map(|p| [p[2], p[1], p[0], 255])
                    .collect(),
                at: Instant::now(),
            };
            let deadline =
                Instant::now() + Duration::from_secs(if expected.is_some() { 8 } else { 2 });
            let mut detected = None;
            while Instant::now() < deadline {
                let _ = worker.input.try_send(Input {
                    at: Instant::now(),
                    bytes: input.bytes.clone(),
                    ..input
                });
                std::thread::sleep(Duration::from_millis(210));
                if let Ok((reaction, _)) = worker.output.try_recv() {
                    detected = Some(reaction);
                    break;
                }
            }
            assert_eq!(detected, expected);
        }
    }

    impl Point {
        fn distance(self, other: Self) -> f64 {
            (self.x - other.x).hypot(self.y - other.y)
        }
    }

    #[derive(Clone, Copy)]
    struct Hand {
        wrist: Point,
        palm: f64,
        thumb: Point,
        index: Point,
        index_base: Point,
        thumb_ip: Point,
        extended: [bool; 4],
    }
    impl Hand {
        unsafe fn read(points: *mut Object, ratio: f64) -> Option<Self> {
            let point = |key| {
                let value: *mut Object = msg_send![points, objectForKey:key];
                if value.is_null() {
                    return None;
                }
                let confidence: f32 = msg_send![value, confidence];
                if confidence < 0.32 {
                    return None;
                }
                let mut p: Point = msg_send![value, location];
                p.x *= ratio;
                Some(p)
            };
            let wrist = point(VNHumanHandPoseObservationJointNameWrist)?;
            let palm = wrist.distance(point(VNHumanHandPoseObservationJointNameMiddleMCP)?);
            if palm < 0.022 {
                return None;
            }
            let index = point(VNHumanHandPoseObservationJointNameIndexTip)?;
            let index_base = point(VNHumanHandPoseObservationJointNameIndexMCP)?;
            let pairs = [
                (
                    VNHumanHandPoseObservationJointNameIndexPIP,
                    VNHumanHandPoseObservationJointNameIndexTip,
                ),
                (
                    VNHumanHandPoseObservationJointNameMiddlePIP,
                    VNHumanHandPoseObservationJointNameMiddleTip,
                ),
                (
                    VNHumanHandPoseObservationJointNameRingPIP,
                    VNHumanHandPoseObservationJointNameRingTip,
                ),
                (
                    VNHumanHandPoseObservationJointNameLittlePIP,
                    VNHumanHandPoseObservationJointNameLittleTip,
                ),
            ];
            let mut extended = [false; 4];
            for (out, (pip, tip)) in extended.iter_mut().zip(pairs) {
                *out = wrist.distance(point(tip)?) - wrist.distance(point(pip)?) > palm * 0.19;
            }
            Some(Self {
                wrist,
                palm,
                index,
                index_base,
                extended,
                thumb: point(VNHumanHandPoseObservationJointNameThumbTip)?,
                thumb_ip: point(VNHumanHandPoseObservationJointNameThumbIP)?,
            })
        }
        fn pose(self) -> Option<Reaction> {
            match self.extended {
                [true, true, false, false] if self.index.y > self.wrist.y => {
                    Some(Reaction::Balloons)
                }
                [true, false, false, true] if self.index.y > self.wrist.y => Some(Reaction::Lasers),
                [false, false, false, false] => {
                    let dy = self.thumb.y - self.thumb_ip.y;
                    let dx = (self.thumb.x - self.thumb_ip.x).abs();
                    if dy.abs() < self.palm * 0.14 || dx > dy.abs() * 0.9 {
                        return None;
                    }
                    if self.thumb.distance(self.index_base) < self.palm * 0.38 {
                        return None;
                    }
                    Some(if dy > 0.0 {
                        Reaction::Like
                    } else {
                        Reaction::Dislike
                    })
                }
                _ => None,
            }
        }
        fn heart_with(self, other: Self) -> bool {
            let scale = (self.palm + other.palm) * 0.5;
            self.wrist.distance(other.wrist) > scale * 0.9
                && self.thumb.distance(other.thumb) < scale * 0.55
                && self.index.distance(other.index) < scale * 0.55
                && self.index.y - self.thumb.y > scale * 0.32
                && other.index.y - other.thumb.y > scale * 0.32
        }
    }
}

#[cfg(target_os = "macos")]
pub use mac::Live;
pub fn draw(rgb: &mut [u8], w: u32, h: u32, reaction: Reaction, elapsed: Duration) {
    let t = elapsed.as_secs_f32();
    let fade = ((2.6 - t) * 2.5).clamp(0.0, 1.0) * (t * 8.0).clamp(0.0, 1.0);
    let scale = h.min(w) as f32;
    let colors: [[u8; 3]; 4] = [
        [255, 95, 141],
        [89, 213, 255],
        [255, 214, 106],
        [143, 104, 255],
    ];
    let count = match reaction {
        Reaction::Like | Reaction::Dislike => 1,
        Reaction::Heart | Reaction::Balloons => 9,
        Reaction::Lasers => 20,
        _ => 64,
    };
    for i in 0..count {
        let spread = ((i * 37 + 5) % 101) as f32 / 101.0;
        let phase = ((i * 73 + 21) % 97) as f32 / 97.0;
        let (x, y, r, color) = match reaction {
            Reaction::Like | Reaction::Dislike => (
                0.79,
                0.34 - 0.025 * t,
                0.085,
                if reaction == Reaction::Like {
                    colors[1]
                } else {
                    colors[0]
                },
            ),
            Reaction::Heart | Reaction::Balloons => (
                0.06 + spread * 0.88,
                1.2 - t * (0.23 + phase * 0.28) + phase * 0.12,
                0.022 + phase * 0.035,
                if reaction == Reaction::Heart {
                    colors[0]
                } else {
                    colors[i % 4]
                },
            ),
            Reaction::Confetti | Reaction::Rain => (
                spread + (t * 6.0 + phase * 11.0).sin() * 0.017,
                (phase + t * 0.63).fract(),
                0.006 + phase * 0.004,
                if reaction == Reaction::Rain {
                    colors[1]
                } else {
                    colors[i % 4]
                },
            ),
            Reaction::Fireworks => {
                let angle = (i % 16) as f32 * std::f32::consts::TAU / 16.0;
                let p = (t * 0.66 + (i / 16) as f32 * 0.24).fract();
                (
                    0.16 + (i / 16) as f32 * 0.23 + angle.cos() * p * 0.13,
                    0.22 + angle.sin() * p * 0.22 + p * p * 0.07,
                    0.007,
                    colors[(i / 16) % 4],
                )
            }
            Reaction::Lasers => (
                (t * 0.27 + spread).fract(),
                0.12 + phase * 0.76,
                0.08,
                colors[i % 4],
            ),
        };
        let cx = x * w as f32;
        let cy = y * h as f32;
        let radius = (r * scale).max(1.0);
        let xmax = (cx + radius).ceil().min(w as f32) as u32;
        let ymax = (cy + radius).ceil().min(h as f32) as u32;
        for yy in (cy - radius).floor().max(0.0) as u32..ymax {
            for xx in (cx - radius).floor().max(0.0) as u32..xmax {
                let dx = (xx as f32 - cx) / radius;
                let dy = (yy as f32 - cy) / radius;
                let inside = match reaction {
                    Reaction::Heart => {
                        let y = -dy + 0.2;
                        let a = dx * dx + y * y - 0.65;
                        a * a * a <= dx * dx * y * y * y
                    }
                    Reaction::Balloons => {
                        dx * dx * 1.7 + dy * dy < 0.85 || (dy > 0.87 && dx.abs() < 0.06)
                    }
                    Reaction::Like | Reaction::Dislike => {
                        let y = if reaction == Reaction::Like { dy } else { -dy };
                        ((-0.52..0.62).contains(&dx) && (-0.18..0.49).contains(&y))
                            || ((-0.32..0.03).contains(&dx) && (-0.82..-0.13).contains(&y))
                            || ((-0.81..-0.57).contains(&dx) && (-0.16..0.49).contains(&y))
                    }
                    Reaction::Rain => (dx + 0.18 * dy).abs() < 0.17,
                    Reaction::Lasers => (dy - 0.75 * dx).abs() < 0.045,
                    Reaction::Confetti => (dx + 0.4 * dy).abs() < 0.45 && dy.abs() < 0.75,
                    Reaction::Fireworks => dx * dx + dy * dy < 1.0,
                };
                if inside {
                    let offset = (yy as usize * w as usize + xx as usize) * 3;
                    for (value, color) in rgb[offset..offset + 3].iter_mut().zip(color) {
                        *value = (f32::from(*value) * (1.0 - fade) + f32::from(color) * fade) as u8;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sustained_gesture_fires_once_until_released() {
        let mut gate = Gate::default();
        let start = std::time::Instant::now();
        let at = |ms| start + std::time::Duration::from_millis(ms);
        assert_eq!(gate.observe(Some(Reaction::Like), at(0)), None);
        assert_eq!(gate.observe(Some(Reaction::Like), at(250)), None);
        assert_eq!(
            gate.observe(Some(Reaction::Like), at(500)),
            Some(Reaction::Like)
        );
        assert_eq!(gate.observe(Some(Reaction::Like), at(3500)), None);
        assert_eq!(gate.observe(None, at(3750)), None);
        assert_eq!(gate.observe(Some(Reaction::Like), at(4000)), None);
        assert_eq!(
            gate.observe(Some(Reaction::Like), at(4500)),
            Some(Reaction::Like)
        );
    }

    #[test]
    fn stale_observations_cannot_trigger_reactions() {
        let mut gate = Gate::default();
        let start = std::time::Instant::now();
        assert_eq!(gate.observe(Some(Reaction::Heart), start), None);
        assert_eq!(
            gate.observe(
                Some(Reaction::Heart),
                start + std::time::Duration::from_secs(3)
            ),
            None
        );
    }

    #[test]
    fn each_reaction_draws_pixels_at_odd_and_even_aspects() {
        for (reaction, _) in Reaction::ALL {
            for (w, h) in [(320, 180), (181, 321)] {
                let mut rgb = vec![0; (w * h * 3) as usize];
                draw(&mut rgb, w, h, reaction, Duration::from_secs(1));
                assert!(rgb.iter().any(|&v| v != 0), "{reaction:?}");
            }
        }
    }
}
