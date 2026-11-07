//! Local person/background separation using Apple Vision and Core Image.
//! No camera access, network, uploaded images or persisted frames.
use std::time::Instant;
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct Stats {
    pub mask_width: u32,
    pub mask_height: u32,
    /// Mean person confidence, from0(background) to1(person).
    pub foreground_fraction: f32,
    pub input_ms: f64,
    pub segmentation_ms: f64,
    pub compose_ms: f64,
    pub total_ms: f64,
}
#[derive(Debug, serde::Serialize)]
pub struct Benchmark {
    pub width: u32,
    pub height: u32,
    pub amount: f32,
    pub frames: usize,
    pub first_ms: f64,
    pub median_ms: f64,
    pub p90_ms: f64,
    pub last: Stats,
}
pub struct PersonMask {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}
#[cfg(target_os = "macos")]
pub struct Segmenter {
    native: native::Segmenter,
}
// Vision and CV buffers move by exclusive ownership; no concurrent access or UI affinity.
#[cfg(target_os = "macos")]
unsafe impl Send for Segmenter {}
#[cfg(target_os = "macos")]
impl Segmenter {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            native: native::Segmenter::new()?,
        })
    }
    pub fn reset(&mut self) -> Result<(), String> {
        self.native.reset()
    }
    pub fn generate(&mut self, w: u32, h: u32, rgb: &[u8]) -> Result<PersonMask, String> {
        validate(w, h, rgb.len(), 1.0)?;
        self.native.generate(w, h, rgb)
    }
}
pub struct Processor {
    #[cfg(target_os = "macos")]
    native: native::Native,
    stats: Stats,
    last_frame: Option<Instant>,
}
// Owned Objective-C objects have no UI/thread affinity. Vision requests and pixel
// buffers are accessed serially through &mut self; caller's Mutex must serialize
// capture callbacks even when the GCD worker thread changes. No Send+Sync alias.
unsafe impl Send for Processor {}
impl Processor {
    pub fn new() -> Result<Self, String> {
        #[cfg(target_os = "macos")]
        {
            Ok(Self {
                native: native::Native::new()?,
                stats: Stats::default(),
                last_frame: None,
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err("Desfoque de fundo local requer macOS".into())
        }
    }
    pub fn apply(
        &mut self,
        width: u32,
        height: u32,
        rgb: Vec<u8>,
        amount: f32,
    ) -> Result<Vec<u8>, String> {
        self.apply_effects(width, height, rgb, amount, None, false)
    }

    pub fn apply_effects(
        &mut self,
        width: u32,
        height: u32,
        rgb: Vec<u8>,
        amount: f32,
        backdrop: Option<[u8; 3]>,
        light: bool,
    ) -> Result<Vec<u8>, String> {
        validate(width, height, rgb.len(), amount)?;
        if amount == 0.0 && backdrop.is_none() && !light {
            return Ok(rgb);
        }
        #[cfg(target_os = "macos")]
        {
            // Stateful Vision requests are reset after a pause, including privacy
            // and enabling blur again, so no pre-pause person mask is reused.
            if self
                .last_frame
                .is_some_and(|last| last.elapsed().as_secs_f64() > 1.0)
            {
                self.native.reset_sequence()?;
            }
            let (output, stats) = self
                .native
                .apply(width, height, rgb, amount, backdrop, light)?;
            self.stats = stats;
            self.last_frame = Some(Instant::now());
            Ok(output)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err("Desfoque de fundo local requer macOS".into())
        }
    }
    pub fn last_stats(&self) -> Stats {
        self.stats
    }
}
pub fn benchmark(
    width: u32,
    height: u32,
    rgb: &[u8],
    amount: f32,
    frames: usize,
) -> Result<Benchmark, String> {
    validate(width, height, rgb.len(), amount)?;
    if !(1..=120).contains(&frames) || amount == 0.0 {
        return Err("Benchmark requer de 1 a 120 quadros e desfoque ativado".into());
    }
    let mut processor = Processor::new()?;
    let mut times = Vec::with_capacity(frames);
    for _ in 0..frames {
        let start = Instant::now();
        std::hint::black_box(processor.apply(width, height, rgb.to_vec(), amount)?);
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let first_ms = times[0];
    times.sort_by(f64::total_cmp);
    Ok(Benchmark {
        width,
        height,
        amount,
        frames,
        first_ms,
        median_ms: times[frames / 2],
        p90_ms: times[((frames as f64 * 0.9).ceil() as usize).saturating_sub(1)],
        last: processor.last_stats(),
    })
}
fn validate(width: u32, height: u32, length: usize, amount: f32) -> Result<(), String> {
    if width < 2
        || height < 2
        || width > 16384
        || height > 16384
        || (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(3))
            != Some(length)
    {
        return Err("Dimensões ou buffer RGB inválidos para desfoque".into());
    }
    if !amount.is_finite() || !(0.0..=1.0).contains(&amount) {
        return Err("Intensidade de desfoque deve ficar entre 0 e 1".into());
    }
    Ok(())
}
#[cfg(test)]
fn reference_blend(sharp: &[u8], background: &[u8], mask: &[u8]) -> Result<Vec<u8>, String> {
    if sharp.len() != background.len() || mask.len().checked_mul(3) != Some(sharp.len()) {
        return Err("Máscara e imagens incompatíveis".into());
    }
    Ok(sharp
        .iter()
        .zip(background)
        .enumerate()
        .map(|(i, (&s, &b))| {
            let m = u32::from(mask[i / 3]);
            ((u32::from(s) * m + u32::from(b) * (255 - m) + 127) / 255) as u8
        })
        .collect())
}

#[cfg(target_os = "macos")]
pub struct CameraCropper(native::CameraCropper);
#[cfg(target_os = "macos")]
unsafe impl Send for CameraCropper {}
#[cfg(target_os = "macos")]
impl CameraCropper {
    pub fn new() -> Result<Self, String> {
        Ok(Self(native::CameraCropper::new()?))
    }
    /// # Safety
    /// The capture pixel buffer must stay valid until the synchronous render completes.
    pub unsafe fn crop(
        &mut self,
        buffer: *mut std::ffi::c_void,
        size: (u32, u32),
        canvas: (u32, u32),
        matrix: [f64; 8],
        pixels: &mut Vec<u8>,
    ) -> Result<(), String> {
        self.0.crop(buffer, size, canvas, matrix, pixels)
    }
}
#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)] // objc0.2 expands its historical cargo-clippy cfg.
mod native {
    use super::Stats;
    use objc::runtime::{Object, BOOL, YES};
    use objc::{class, msg_send, sel, sel_impl, Encode, Encoding};
    use std::{ffi::c_void, ptr, time::Instant};
    const BGRA: u32 = u32::from_be_bytes(*b"BGRA");
    const MASK: u32 = u32::from_be_bytes(*b"L008");
    #[link(name = "Vision", kind = "framework")]
    unsafe extern "C" {}
    #[link(name = "Metal", kind = "framework")]
    unsafe extern "C" {
        fn MTLCreateSystemDefaultDevice() -> *mut Object;
    }
    #[link(name = "CoreImage", kind = "framework")]
    unsafe extern "C" {
        static kCIContextUseSoftwareRenderer: *mut Object;
        static kCIContextPriorityRequestLow: *mut Object;
        static kCIContextWorkingColorSpace: *mut Object;
        static kCIContextOutputColorSpace: *mut Object;
        static kCIContextWorkingFormat: *mut Object;
        static kCIContextCacheIntermediates: *mut Object;
        static kCIFormatRGBA8: i32;
        static kCIImageColorSpace: *mut Object;
    }
    #[link(name = "CoreVideo", kind = "framework")]
    unsafe extern "C" {
        static kCVPixelBufferIOSurfacePropertiesKey: *mut Object;
        static kCVPixelBufferMetalCompatibilityKey: *mut Object;
        fn CVPixelBufferCreate(
            allocator: *const c_void,
            width: usize,
            height: usize,
            format: u32,
            attrs: *const c_void,
            output: *mut *mut c_void,
        ) -> i32;
        fn CVPixelBufferLockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
        fn CVPixelBufferUnlockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
        fn CVPixelBufferGetBaseAddress(buffer: *mut c_void) -> *mut c_void;
        fn CVPixelBufferGetBytesPerRow(buffer: *mut c_void) -> usize;
        fn CVPixelBufferGetWidth(buffer: *mut c_void) -> usize;
        fn CVPixelBufferGetHeight(buffer: *mut c_void) -> usize;
        fn CVPixelBufferGetPixelFormatType(buffer: *mut c_void) -> u32;
        fn CVPixelBufferRelease(buffer: *mut c_void);
    }
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
        fn vImageConvert_BGRA8888toRGB888(
            src: *const Buffer,
            dest: *const Buffer,
            flags: u32,
        ) -> isize;

    }
    /// Sparse bilinear384px Vision input, not a full-resolution Lanczos pass.
    unsafe fn downsample(src: &Buffer, dest: &Buffer) {
        let mut xs = Vec::with_capacity(dest.width);
        for x in 0..dest.width {
            let sx = ((x as f64 + 0.5) * src.width as f64 / dest.width as f64 - 0.5)
                .clamp(0.0, (src.width - 1) as f64);
            let x0 = sx as usize;
            xs.push((x0, (x0 + 1).min(src.width - 1), sx - x0 as f64));
        }
        for y in 0..dest.height {
            let sy = ((y as f64 + 0.5) * src.height as f64 / dest.height as f64 - 0.5)
                .clamp(0.0, (src.height - 1) as f64);
            let y0 = sy as usize;
            let y1 = (y0 + 1).min(src.height - 1);
            let fy = sy - y0 as f64;
            let row0 = std::slice::from_raw_parts(
                (src.data as *const u8).add(y0 * src.row_bytes),
                src.width * 4,
            );
            let row1 = std::slice::from_raw_parts(
                (src.data as *const u8).add(y1 * src.row_bytes),
                src.width * 4,
            );
            let target = std::slice::from_raw_parts_mut(
                (dest.data as *mut u8).add(y * dest.row_bytes),
                dest.width * 4,
            );
            for (x, &(x0, x1, fx)) in xs.iter().enumerate() {
                for c in 0..3 {
                    let a = f64::from(row0[x0 * 4 + c]);
                    let b = f64::from(row0[x1 * 4 + c]);
                    let d = f64::from(row1[x0 * 4 + c]);
                    let e = f64::from(row1[x1 * 4 + c]);
                    let top = (b - a).mul_add(fx, a);
                    let bottom = (e - d).mul_add(fx, d);
                    target[x * 4 + c] = (bottom - top).mul_add(fy, top).round() as u8;
                }
                target[x * 4 + 3] = 255;
            }
        }
    }
    #[cfg(test)]
    #[test]
    fn bilinear_segmentation_input_preserves_channels_and_row_padding() {
        let mut source = vec![199u8; 80];
        for y in 0..4 {
            for x in 0..4 {
                let v = (x * 10 + y * 40) as u8;
                source[y * 20 + x * 4..y * 20 + x * 4 + 4].copy_from_slice(&[v, v + 1, v + 2, 255]);
            }
        }
        let mut output = vec![231u8; 24];
        let src = Buffer {
            data: source.as_mut_ptr().cast(),
            width: 4,
            height: 4,
            row_bytes: 20,
        };
        let dest = Buffer {
            data: output.as_mut_ptr().cast(),
            width: 2,
            height: 2,
            row_bytes: 12,
        };
        unsafe {
            downsample(&src, &dest);
        }
        assert_eq!(&output[..8], &[25, 26, 27, 255, 45, 46, 47, 255]);
        assert_eq!(&output[12..20], &[105, 106, 107, 255, 125, 126, 127, 255]);
        assert_eq!(&output[8..12], &[231; 4]);
        assert_eq!(&output[20..24], &[231; 4]);
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Affine {
        a: f64,
        b: f64,
        c: f64,
        d: f64,
        tx: f64,
        ty: f64,
    }
    unsafe impl Encode for Affine {
        fn encode() -> Encoding {
            unsafe { Encoding::from_str("{CGAffineTransform=dddddd}") }
        }
    }
    struct Owned(*mut Object);
    impl Owned {
        unsafe fn new(pointer: *mut Object, name: &str) -> Result<Self, String> {
            if pointer.is_null() {
                Err(format!("Não foi possível criar {name}"))
            } else {
                Ok(Self(pointer))
            }
        }
        unsafe fn retain(pointer: *mut Object, name: &str) -> Result<Self, String> {
            let _: *mut Object = msg_send![pointer, retain];
            Self::new(pointer, name)
        }
    }
    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe {
                let _: () = msg_send![self.0, release];
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
    struct Pixel(*mut c_void);
    impl Pixel {
        fn new(w: usize, h: usize, format: u32) -> Result<Self, String> {
            unsafe {
                let mut buffer = ptr::null_mut();
                let status =
                    CVPixelBufferCreate(ptr::null(), w, h, format, ptr::null(), &mut buffer);
                if status != 0 || buffer.is_null() {
                    Err(format!("CVPixelBuffer para desfoque:{status}"))
                } else {
                    Ok(Self(buffer))
                }
            }
        }
    }
    impl Drop for Pixel {
        fn drop(&mut self) {
            unsafe {
                CVPixelBufferRelease(self.0);
            }
        }
    }
    struct Locked {
        buffer: *mut c_void,
    }
    impl Locked {
        unsafe fn new(buffer: *mut c_void) -> Result<Self, String> {
            let status = CVPixelBufferLockBaseAddress(buffer, 0);
            if status != 0 {
                return Err(format!("Falha no buffer de desfoque:{status}"));
            }
            if CVPixelBufferGetBaseAddress(buffer).is_null() {
                CVPixelBufferUnlockBaseAddress(buffer, 0);
                return Err("Buffer de desfoque sem pixels".into());
            }
            Ok(Self { buffer })
        }
        unsafe fn view(&self) -> Buffer {
            Buffer {
                data: CVPixelBufferGetBaseAddress(self.buffer),
                height: CVPixelBufferGetHeight(self.buffer),
                width: CVPixelBufferGetWidth(self.buffer),
                row_bytes: CVPixelBufferGetBytesPerRow(self.buffer),
            }
        }
    }
    impl Drop for Locked {
        fn drop(&mut self) {
            unsafe {
                CVPixelBufferUnlockBaseAddress(self.buffer, 0);
            }
        }
    }
    fn check(status: isize, operation: &str) -> Result<(), String> {
        if status < 0 {
            Err(format!("Desfoque: {operation} ({status})"))
        } else {
            Ok(())
        }
    }
    unsafe fn text(value: *mut Object) -> String {
        if value.is_null() {
            return "erro nativo sem detalhes".into();
        }
        let value: *mut Object = msg_send![value, localizedDescription];
        let c: *const std::ffi::c_char = msg_send![value, UTF8String];
        if c.is_null() {
            "erro nativo sem descrição".into()
        } else {
            std::ffi::CStr::from_ptr(c).to_string_lossy().into_owned()
        }
    }
    unsafe fn key(value: &str) -> *mut Object {
        let bytes = std::ffi::CString::new(value).unwrap();
        msg_send![class!(NSString),stringWithUTF8String:bytes.as_ptr()]
    }
    unsafe fn dictionary(keys: &[*mut Object], values: &[*mut Object]) -> *mut Object {
        msg_send![class!(NSDictionary),dictionaryWithObjects:values.as_ptr() forKeys:keys.as_ptr() count:keys.len()]
    }
    pub(super) struct Segmenter {
        request: Owned,
        sequence: Owned,
        requests: Owned,
        input: Option<(u32, u32, Pixel)>,
    }
    impl Segmenter {
        pub(super) fn new() -> Result<Self, String> {
            unsafe {
                let _pool = Pool::new();
                let (r, s, a) = Native::sequence()?;
                let _: () = msg_send![r.0, setPreferBackgroundProcessing:YES];
                Ok(Self {
                    request: r,
                    sequence: s,
                    requests: a,
                    input: None,
                })
            }
        }
        pub(super) fn reset(&mut self) -> Result<(), String> {
            unsafe {
                let _pool = Pool::new();
                let (r, s, a) = Native::sequence()?;
                let _: () = msg_send![r.0, setPreferBackgroundProcessing:YES];
                self.request = r;
                self.sequence = s;
                self.requests = a;
                Ok(())
            }
        }
        pub(super) fn generate(
            &mut self,
            w: u32,
            h: u32,
            rgb: &[u8],
        ) -> Result<super::PersonMask, String> {
            unsafe {
                let _pool = Pool::new();
                if w > 512 || h > 512 {
                    return Err("Entrada de segmentação grande demais".into());
                }
                if self
                    .input
                    .as_ref()
                    .is_none_or(|(iw, ih, _)| (*iw, *ih) != (w, h))
                {
                    self.input = Some((w, h, Pixel::new(w as usize, h as usize, BGRA)?));
                }
                let input = &self
                    .input
                    .as_ref()
                    .ok_or("Buffer de segmentação indisponível")?
                    .2;
                {
                    let lock = Locked::new(input.0)?;
                    let source = Buffer {
                        data: rgb.as_ptr() as *mut c_void,
                        width: w as usize,
                        height: h as usize,
                        row_bytes: w as usize * 3,
                    };
                    check(
                        vImageConvert_RGB888toBGRA8888(
                            &source,
                            ptr::null(),
                            255,
                            &lock.view(),
                            false,
                            16,
                        ),
                        "entrada da máscara",
                    )?;
                }
                let mut error: *mut Object = ptr::null_mut();
                let ok: BOOL = msg_send![self.sequence.0,performRequests:self.requests.0 onCVPixelBuffer:input.0 orientation:1u32 error:&mut error];
                if ok != YES {
                    return Err(format!("Vision não segmentou a pessoa: {}", text(error)));
                }
                let results: *mut Object = msg_send![self.request.0, results];
                let count: usize = msg_send![results, count];
                if count != 1 {
                    return Err("Vision não gerou uma máscara".into());
                }
                let result: *mut Object = msg_send![results,objectAtIndex:0usize];
                let mask: *mut c_void = msg_send![result, pixelBuffer];
                if mask.is_null() || CVPixelBufferGetPixelFormatType(mask) != MASK {
                    return Err("Máscara inválida".into());
                }
                let (mw, mh) = (CVPixelBufferGetWidth(mask), CVPixelBufferGetHeight(mask));
                if mw == 0 || mh == 0 || mw > 4096 || mh > 4096 {
                    return Err("Dimensões de máscara inválidas".into());
                }
                let lock = Locked::new(mask)?;
                let b = lock.view();
                if b.row_bytes < mw {
                    return Err("Stride da máscara inválido".into());
                }
                let mut pixels = vec![0; mw * mh];
                for y in 0..mh {
                    pixels[y * mw..(y + 1) * mw].copy_from_slice(std::slice::from_raw_parts(
                        (b.data as *const u8).add(y * b.row_bytes),
                        mw,
                    ));
                }
                Ok(super::PersonMask {
                    width: mw as u32,
                    height: mh as u32,
                    pixels,
                })
            }
        }
    }
    struct Frames {
        input: Pixel,
        small: Pixel,
        output: Pixel,
        width: u32,
        height: u32,
    }
    pub(super) struct Native {
        request: Owned,
        sequence: Owned,
        requests: Owned,
        context: Owned,
        options: Owned,
        frames: Option<Frames>,
    }

    pub(super) struct CameraCropper {
        context: Owned,
        output: Option<((u32, u32), Pixel)>,
    }
    impl CameraCropper {
        pub(super) fn new() -> Result<Self, String> {
            unsafe {
                let _pool = Pool::new();
                let device = Owned::new(MTLCreateSystemDefaultDevice(), "Metal indisponível")?;
                let null: *mut Object = msg_send![class!(NSNull), null];
                let no: *mut Object = msg_send![class!(NSNumber), numberWithBool:objc::runtime::NO];
                let format: *mut Object = msg_send![class!(NSNumber), numberWithInt:kCIFormatRGBA8];
                let options = dictionary(
                    &[
                        kCIContextWorkingColorSpace,
                        kCIContextOutputColorSpace,
                        kCIContextWorkingFormat,
                        kCIContextCacheIntermediates,
                        kCIContextUseSoftwareRenderer,
                        kCIContextPriorityRequestLow,
                    ],
                    &[null, null, format, no, no, no],
                );
                let context = Owned::retain(
                    msg_send![class!(CIContext), contextWithMTLDevice:device.0 options:options],
                    "GPU de recorte",
                )?;
                Ok(Self {
                    context,
                    output: None,
                })
            }
        }
        pub(super) unsafe fn crop(
            &mut self,
            buffer: *mut c_void,
            size: (u32, u32),
            canvas: (u32, u32),
            matrix: [f64; 8],
            pixels: &mut Vec<u8>,
        ) -> Result<(), String> {
            let _pool = Pool::new();
            let roi = crate::transform::camera_roi(size, canvas, matrix)?;
            if buffer.is_null()
                || CVPixelBufferGetWidth(buffer) != size.0 as usize
                || CVPixelBufferGetHeight(buffer) != size.1 as usize
            {
                return Err("Buffer da câmera inválido para recorte".into());
            }
            if self.output.as_ref().is_none_or(|(s, _)| *s != canvas) {
                let empty: *mut Object = msg_send![class!(NSDictionary), dictionary];
                let yes: *mut Object = msg_send![class!(NSNumber), numberWithBool:YES];
                let attrs = dictionary(
                    &[
                        kCVPixelBufferIOSurfacePropertiesKey,
                        kCVPixelBufferMetalCompatibilityKey,
                    ],
                    &[empty, yes],
                );
                let mut pixel = ptr::null_mut();
                let status = CVPixelBufferCreate(
                    ptr::null(),
                    canvas.0 as usize,
                    canvas.1 as usize,
                    BGRA,
                    attrs.cast(),
                    &mut pixel,
                );
                check(status as isize, "superfície de recorte")?;
                if pixel.is_null() {
                    return Err("Superfície de recorte ausente".into());
                }
                self.output = Some((canvas, Pixel(pixel)));
            }
            let image: *mut Object = msg_send![class!(CIImage), imageWithCVPixelBuffer:buffer];
            let rect = CropRect {
                x: roi.x as f64,
                y: (size.1 as usize - roi.y - roi.height) as f64,
                width: roi.width as f64,
                height: roi.height as f64,
            };
            let image: *mut Object = msg_send![image, imageByCroppingToRect:rect];
            let move_to_origin = Affine {
                a: 1.0,
                b: 0.0,
                c: 0.0,
                d: 1.0,
                tx: -rect.x,
                ty: -rect.y,
            };
            let image: *mut Object = msg_send![image, imageByApplyingTransform:move_to_origin];
            let filter: *mut Object =
                msg_send![class!(CIFilter), filterWithName:key("CILanczosScaleTransform")];
            let sy = f64::from(canvas.1) / rect.height;
            let sx = f64::from(canvas.0) / rect.width;
            let scale: *mut Object = msg_send![class!(NSNumber), numberWithDouble:sy];
            let aspect: *mut Object = msg_send![class!(NSNumber), numberWithDouble:sx / sy];
            let _: () = msg_send![filter, setValue:image forKey:key("inputImage")];
            let _: () = msg_send![filter, setValue:scale forKey:key("inputScale")];
            let _: () = msg_send![filter, setValue:aspect forKey:key("inputAspectRatio")];
            let mut image: *mut Object = msg_send![filter, outputImage];
            if roi.mirror {
                let flip = Affine {
                    a: -1.0,
                    b: 0.0,
                    c: 0.0,
                    d: 1.0,
                    tx: f64::from(canvas.0),
                    ty: 0.0,
                };
                image = msg_send![image, imageByApplyingTransform:flip];
            }
            if image.is_null() {
                return Err("GPU não produziu o recorte".into());
            }
            let output = &self.output.as_ref().ok_or("Recorte indisponível")?.1;
            let _: () = msg_send![self.context.0, render:image toCVPixelBuffer:output.0];
            let lock = Locked::new(output.0)?;
            let view = lock.view();
            let row = canvas.0 as usize * 4;
            pixels.resize(row * canvas.1 as usize, 0);
            for (y, destination) in pixels.chunks_exact_mut(row).enumerate() {
                destination.copy_from_slice(std::slice::from_raw_parts(
                    (view.data as *const u8).add(y * view.row_bytes),
                    row,
                ));
            }
            Ok(())
        }
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CropRect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }
    unsafe impl Encode for CropRect {
        fn encode() -> Encoding {
            unsafe { Encoding::from_str("{CGRect={CGPoint=dd}{CGSize=dd}}") }
        }
    }
    #[test]
    #[ignore = "requires a Metal device"]
    fn gpu_crop_preserves_selected_camera_roi_and_channel_order() {
        unsafe {
            let _pool = Pool::new();
            let input = Pixel::new(16, 8, BGRA).unwrap();
            {
                let lock = Locked::new(input.0).unwrap();
                let view = lock.view();
                for y in 0..8 {
                    let row = std::slice::from_raw_parts_mut(
                        (view.data as *mut u8).add(y * view.row_bytes),
                        64,
                    );
                    for x in 0..16 {
                        row[x * 4..x * 4 + 4].copy_from_slice(&[
                            100,
                            (y * 20) as u8,
                            (x * 10) as u8,
                            255,
                        ]);
                    }
                }
            }
            let mut cropper = CameraCropper::new().unwrap();
            let mut rgb = Vec::new();
            cropper
                .crop(
                    input.0,
                    (16, 8),
                    (8, 4),
                    [7.0 / 15.0, 0.0, 8.0 / 15.0, 0.0, 3.0 / 7.0, 0.0, 0.0, 0.0],
                    &mut rgb,
                )
                .unwrap();
            assert_eq!(&rgb[..4], &[100, 0, 80, 255]);
            assert_eq!(&rgb[rgb.len() - 4..], &[100, 60, 150, 255]);
            cropper
                .crop(
                    input.0,
                    (16, 8),
                    (16, 8),
                    [-1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0],
                    &mut rgb,
                )
                .unwrap();
            assert_eq!(&rgb[..4], &[100, 0, 150, 255]);
            assert_eq!(&rgb[rgb.len() - 4..], &[100, 140, 0, 255]);
        }
    }
    #[cfg(test)]
    pub(super) fn compose_fixture(
        sharp: &[u8],
        background: &[u8],
        mask: &[u8],
    ) -> Result<Vec<u8>, String> {
        unsafe {
            let _pool = Pool::new();
            let processor = Native::new()?;
            let foreground = Pixel::new(2, 2, BGRA)?;
            let blurred = Pixel::new(2, 2, BGRA)?;
            let matte = Pixel::new(2, 2, MASK)?;
            let output = Pixel::new(2, 2, BGRA)?;
            for (pixel, rgb) in [(&foreground, sharp), (&blurred, background)] {
                let lock = Locked::new(pixel.0)?;
                let view = lock.view();
                for y in 0..2 {
                    let row = std::slice::from_raw_parts_mut(
                        (view.data as *mut u8).add(y * view.row_bytes),
                        8,
                    );
                    for x in 0..2 {
                        let i = (y * 2 + x) * 3;
                        row[x * 4..x * 4 + 4].copy_from_slice(&[
                            rgb[i + 2],
                            rgb[i + 1],
                            rgb[i],
                            255,
                        ]);
                    }
                }
            }
            {
                let lock = Locked::new(matte.0)?;
                let view = lock.view();
                for y in 0..2 {
                    std::slice::from_raw_parts_mut(
                        (view.data as *mut u8).add(y * view.row_bytes),
                        2,
                    )
                    .copy_from_slice(&mask[y * 2..y * 2 + 2]);
                }
            }
            processor.composite(
                processor.image(foreground.0)?,
                processor.image(blurred.0)?,
                processor.image(matte.0)?,
                output.0,
            )?;
            let mut rgb = vec![0; 12];
            let lock = Locked::new(output.0)?;
            let dest = Buffer {
                data: rgb.as_mut_ptr().cast(),
                width: 2,
                height: 2,
                row_bytes: 6,
            };
            check(
                vImageConvert_BGRA8888toRGB888(&lock.view(), &dest, 0),
                "fixture RGB",
            )?;
            Ok(rgb)
        }
    }
    impl Native {
        pub(super) fn new() -> Result<Self, String> {
            unsafe {
                let _pool = Pool::new();
                let null: *mut Object = msg_send![class!(NSNull), null];
                // Source/output are SDR8-bit. Avoid half-float intermediates and
                // frame-content caches when the same CVPixelBuffers are reused.
                let format: *mut Object = msg_send![class!(NSNumber),numberWithInt:kCIFormatRGBA8];
                let no_cache: *mut Object =
                    msg_send![class!(NSNumber),numberWithBool:objc::runtime::NO];
                let options = dictionary(
                    &[
                        kCIContextWorkingColorSpace,
                        kCIContextOutputColorSpace,
                        kCIContextWorkingFormat,
                        kCIContextCacheIntermediates,
                    ],
                    &[null, null, format, no_cache],
                );
                let context: *mut Object = msg_send![class!(CIContext),contextWithOptions:options];
                let context = Owned::retain(context, "contexto Core Image")?;
                let image_options = Owned::retain(
                    dictionary(&[kCIImageColorSpace], &[null]),
                    "opções de imagem",
                )?;
                let (request, sequence, requests) = Self::sequence()?;
                Ok(Self {
                    request,
                    sequence,
                    requests,
                    context,
                    options: image_options,
                    frames: None,
                })
            }
        }
        unsafe fn sequence() -> Result<(Owned, Owned, Owned), String> {
            let request = Owned::new(
                msg_send![class!(VNGeneratePersonSegmentationRequest), new],
                "segmentação de pessoa",
            )?;
            let _: () = msg_send![request.0,setQualityLevel:2usize];
            let _: () = msg_send![request.0,setOutputPixelFormat:MASK];
            let sequence = Owned::new(
                msg_send![class!(VNSequenceRequestHandler), new],
                "sequência Vision",
            )?;
            let requests = Owned::retain(
                msg_send![class!(NSArray),arrayWithObject:request.0],
                "fila Vision",
            )?;
            Ok((request, sequence, requests))
        }
        pub(super) fn reset_sequence(&mut self) -> Result<(), String> {
            unsafe {
                let _pool = Pool::new();
                let (r, s, a) = Self::sequence()?;
                self.request = r;
                self.sequence = s;
                self.requests = a;
                Ok(())
            }
        }
        fn frames(&mut self, w: u32, h: u32) -> Result<(), String> {
            if self
                .frames
                .as_ref()
                .is_some_and(|f| f.width == w && f.height == h)
            {
                return Ok(());
            }
            let scale = (384.0 / f64::from(w.max(h))).min(1.0);
            let sw = (f64::from(w) * scale).round().max(2.0) as usize;
            let sh = (f64::from(h) * scale).round().max(2.0) as usize;
            self.frames = Some(Frames {
                input: Pixel::new(w as usize, h as usize, BGRA)?,
                small: Pixel::new(sw, sh, BGRA)?,
                output: Pixel::new(w as usize, h as usize, BGRA)?,
                width: w,
                height: h,
            });
            Ok(())
        }
        unsafe fn image(&self, buffer: *mut c_void) -> Result<*mut Object, String> {
            let image: *mut Object =
                msg_send![class!(CIImage),imageWithCVPixelBuffer:buffer options:self.options.0];
            if image.is_null() {
                Err("Core Image recusou o buffer de desfoque".into())
            } else {
                Ok(image)
            }
        }
        unsafe fn exposure(image: *mut Object, ev: f64) -> *mut Object {
            let filter: *mut Object =
                msg_send![class!(CIFilter), filterWithName:key("CIExposureAdjust")];
            let amount: *mut Object = msg_send![class!(NSNumber), numberWithDouble:ev];
            let _: () = msg_send![filter, setValue:image forKey:key("inputImage")];
            let _: () = msg_send![filter, setValue:amount forKey:key("inputEV")];
            msg_send![filter, outputImage]
        }
        unsafe fn composite(
            &self,
            input: *mut Object,
            background: *mut Object,
            mask: *mut Object,
            output: *mut c_void,
        ) -> Result<(), String> {
            let filter: *mut Object =
                msg_send![class!(CIFilter),filterWithName:key("CIBlendWithMask")];
            if filter.is_null() {
                return Err("Composição Core Image indisponível".into());
            }
            let _: () = msg_send![filter,setValue:input forKey:key("inputImage")];
            let _: () = msg_send![filter,setValue:background forKey:key("inputBackgroundImage")];
            let _: () = msg_send![filter,setValue:mask forKey:key("inputMaskImage")];
            let image: *mut Object = msg_send![filter, outputImage];
            if image.is_null() {
                return Err("Core Image não gerou a composição de desfoque".into());
            }
            let _: () = msg_send![self.context.0,render:image toCVPixelBuffer:output];
            Ok(())
        }
        pub(super) fn apply(
            &mut self,
            w: u32,
            h: u32,
            mut rgb: Vec<u8>,
            amount: f32,
            backdrop: Option<[u8; 3]>,
            light: bool,
        ) -> Result<(Vec<u8>, Stats), String> {
            unsafe {
                let _pool = Pool::new();
                let start = Instant::now();
                self.frames(w, h)?;
                let f = self
                    .frames
                    .as_ref()
                    .ok_or("Buffers de desfoque indisponíveis")?;
                {
                    let input = Locked::new(f.input.0)?;
                    let small = Locked::new(f.small.0)?;
                    let source = Buffer {
                        data: rgb.as_mut_ptr().cast(),
                        width: w as usize,
                        height: h as usize,
                        row_bytes: w as usize * 3,
                    };
                    check(
                        vImageConvert_RGB888toBGRA8888(
                            &source,
                            ptr::null(),
                            255,
                            &input.view(),
                            false,
                            0,
                        ),
                        "RGB→BGRA",
                    )?;
                    downsample(&input.view(), &small.view());
                }
                let input_ms = start.elapsed().as_secs_f64() * 1000.0;
                let segment_start = Instant::now();
                let mut error: *mut Object = ptr::null_mut();
                let ok: BOOL = msg_send![self.sequence.0,performRequests:self.requests.0 onCVPixelBuffer:f.small.0 orientation:1u32 error:&mut error];
                if ok != YES {
                    return Err(format!("Vision não segmentou a pessoa: {}", text(error)));
                }
                let results: *mut Object = msg_send![self.request.0, results];
                let count: usize = msg_send![results, count];
                if count != 1 {
                    return Err(format!("Vision retornou {count} máscaras; esperado 1"));
                }
                let result: *mut Object = msg_send![results,objectAtIndex:0usize];
                let mask: *mut c_void = msg_send![result, pixelBuffer];
                if mask.is_null() || CVPixelBufferGetPixelFormatType(mask) != MASK {
                    return Err("Vision retornou máscara inválida".into());
                }
                let (mw, mh) = (CVPixelBufferGetWidth(mask), CVPixelBufferGetHeight(mask));
                if mw == 0 || mh == 0 || mw > 4096 || mh > 4096 {
                    return Err("Vision retornou dimensões de máscara inválidas".into());
                }
                let fraction = {
                    let lock = Locked::new(mask)?;
                    let v = lock.view();
                    if v.row_bytes < mw {
                        return Err("Vision retornou stride de máscara inválido".into());
                    }
                    let mut sum = 0u64;
                    for y in 0..mh {
                        let row = std::slice::from_raw_parts(
                            (v.data as *const u8).add(y * v.row_bytes),
                            mw,
                        );
                        sum += row.iter().map(|&v| u64::from(v)).sum::<u64>();
                    }
                    sum as f32 / (mw * mh) as f32 / 255.0
                };
                let segmentation_ms = segment_start.elapsed().as_secs_f64() * 1000.0;
                let compose = Instant::now();
                let input = self.image(f.input.0)?;
                let mut background: *mut Object = if let Some([r, g, b]) = backdrop {
                    let color: *mut Object = msg_send![class!(CIColor),
                        colorWithRed:f64::from(r) / 255.0 green:f64::from(g) / 255.0 blue:f64::from(b) / 255.0 alpha:1.0f64];
                    msg_send![class!(CIImage), imageWithColor:color]
                } else if amount > 0.0 {
                    let clamped: *mut Object = msg_send![input, imageByClampingToExtent];
                    let radius = f64::from(amount) * 24.0 * (f64::from(h) / 1080.0).max(0.5);
                    msg_send![clamped, imageByApplyingGaussianBlurWithSigma:radius]
                } else {
                    input
                };
                let foreground = if light {
                    if backdrop.is_none() {
                        background = Self::exposure(background, 0.75f64.log2());
                    }
                    Self::exposure(input, 1.25f64.log2())
                } else {
                    input
                };
                let mask_image = self.image(mask)?;
                let affine = Affine {
                    a: f64::from(w) / mw as f64,
                    b: 0.0,
                    c: 0.0,
                    d: f64::from(h) / mh as f64,
                    tx: 0.0,
                    ty: 0.0,
                };
                let mask_image: *mut Object = msg_send![mask_image,imageByApplyingTransform:affine];
                if background.is_null() || mask_image.is_null() {
                    return Err("Core Image não gerou as imagens de desfoque".into());
                }
                self.composite(foreground, background, mask_image, f.output.0)?;
                {
                    let out = Locked::new(f.output.0)?;
                    let dest = Buffer {
                        data: rgb.as_mut_ptr().cast(),
                        width: w as usize,
                        height: h as usize,
                        row_bytes: w as usize * 3,
                    };
                    check(
                        vImageConvert_BGRA8888toRGB888(&out.view(), &dest, 0),
                        "BGRA→RGB",
                    )?;
                }
                Ok((
                    rgb,
                    Stats {
                        mask_width: mw as u32,
                        mask_height: mh as u32,
                        foreground_fraction: fraction,
                        input_ms,
                        segmentation_ms,
                        compose_ms: compose.elapsed().as_secs_f64() * 1000.0,
                        total_ms: start.elapsed().as_secs_f64() * 1000.0,
                    },
                ))
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_dimensions_buffers_and_amount() {
        for (w, h, n, a) in [
            (0, 2, 0, 0.5),
            (2, 2, 11, 0.5),
            (2, 2, 13, 0.5),
            (u32::MAX, u32::MAX, 0, 0.5),
            (2, 2, 12, f32::NAN),
            (2, 2, 12, -0.1),
            (2, 2, 12, 1.1),
        ] {
            assert!(validate(w, h, n, a).is_err());
        }
        assert!(validate(2, 2, 12, 0.0).is_ok());
    }
    #[test]
    fn white_person_mask_keeps_subject_and_black_mask_uses_background() {
        let sharp = [200, 40, 10, 200, 40, 10, 200, 40, 10];
        let back = [20, 80, 250, 20, 80, 250, 20, 80, 250];
        assert_eq!(
            reference_blend(&sharp, &back, &[255, 0, 128]).unwrap(),
            [200, 40, 10, 20, 80, 250, 110, 60, 130]
        );
        assert!(reference_blend(&sharp, &back, &[0]).is_err());
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn actual_core_image_composition_matches_mask_and_top_down_rgb() {
        let sharp = [200, 40, 10, 13, 37, 211, 80, 120, 180, 247, 30, 60];
        let background = [20, 80, 250, 170, 60, 33, 200, 20, 70, 99, 211, 42];
        let mask = [255, 0, 128, 255];
        let expected = reference_blend(&sharp, &background, &mask).unwrap();
        let actual = native::compose_fixture(&sharp, &background, &mask).unwrap();
        for (i, (a, e)) in actual.iter().zip(expected).enumerate() {
            assert!(a.abs_diff(e) <= 1, "channel{i}: actual{a},expected{e}");
        }
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn disabled_blur_reuses_input_allocation() {
        let mut processor = Processor::new().unwrap();
        let rgb = vec![77; 12];
        let address = rgb.as_ptr();
        let out = processor.apply(2, 2, rgb, 0.0).unwrap();
        assert_eq!(out.as_ptr(), address);
        assert_eq!(out, vec![77; 12]);
    }
}
