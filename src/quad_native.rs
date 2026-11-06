//! Accelerate vImage projective resampling (macOS13+), preserving RGB byte values.
//! SDK Geometry.h documents bottom-left pixel coordinates, source-to-destination
//! matrices, linear interpolation and black background fill. No colour conversion.
use std::{ffi::c_void, ptr, sync::Mutex};
#[repr(C)]
struct Buffer {
    data: *mut c_void,
    height: usize,
    width: usize,
    row_bytes: usize,
}
#[repr(C)]
#[derive(Default)]
struct Perspective {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    tx: f32,
    ty: f32,
    vx: f32,
    vy: f32,
    v: f32,
}
#[link(name = "Accelerate", kind = "framework")]
unsafe extern "C" {
    fn vImageGetPerspectiveWarp(
        src: *const [[f32; 2]; 4],
        dest: *const [[f32; 2]; 4],
        transform: *mut Perspective,
        flags: u32,
    ) -> isize;
    fn vImagePerspectiveWarp_ARGB8888(
        src: *const Buffer,
        dest: *const Buffer,
        temp: *mut c_void,
        transform: *const Perspective,
        interpolation: i32,
        background: *const u8,
        flags: u32,
    ) -> isize;
    fn vImageConvert_RGB888toARGB8888(
        src: *const Buffer,
        alpha_src: *const Buffer,
        alpha: u8,
        dest: *const Buffer,
        premultiply: bool,
        flags: u32,
    ) -> isize;
    fn vImageConvert_ARGB8888toRGB888(src: *const Buffer, dest: *const Buffer, flags: u32)
        -> isize;
}
#[derive(Default)]
struct Scratch {
    source: Vec<u8>,
    destination: Vec<u8>,
    temp: Vec<u8>,
}
// GCD serial capture queues may migrate between worker threads. One bounded
// scratch context avoids retaining a pair of full-frame buffers per GCD thread.
static SCRATCH: Mutex<Scratch> = Mutex::new(Scratch {
    source: Vec::new(),
    destination: Vec::new(),
    temp: Vec::new(),
});
fn checked(status: isize, operation: &str) -> Result<(), String> {
    if status < 0 {
        Err(format!("Accelerate {operation}: {status}"))
    } else {
        Ok(())
    }
}
pub(crate) fn warp(
    w: u32,
    h: u32,
    mut rgb: Vec<u8>,
    ow: u32,
    oh: u32,
    matrix: &[f64; 8],
) -> Result<Vec<u8>, String> {
    if w < 2
        || h < 2
        || ow < 2
        || oh < 2
        || (w as usize)
            .checked_mul(h as usize)
            .and_then(|n| n.checked_mul(3))
            != Some(rgb.len())
    {
        return Err("Buffer RGB nativo inválido".into());
    }
    let source_bytes = (w as usize)
        .checked_mul(h as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or("Imagem nativa grande demais")?;
    let destination_bytes = (ow as usize)
        .checked_mul(oh as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or("Imagem nativa grande demais")?;
    if matrix.iter().any(|v| !v.is_finite()) {
        return Err("Perspectiva nativa inválida".into());
    }
    let mut source_points = [[0.0f32; 2]; 4];
    let mut destination_points = [[0.0f32; 2]; 4];
    for (i, (u, v)) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
        .into_iter()
        .enumerate()
    {
        let denominator = matrix[6] * u + matrix[7] * v + 1.0;
        if !denominator.is_finite() || denominator.abs() < 1e-12 {
            return Err("Perspectiva nativa inválida".into());
        }
        let x = (matrix[0] * u + matrix[1] * v + matrix[2]) / denominator;
        let y = (matrix[3] * u + matrix[4] * v + matrix[5]) / denominator;
        source_points[i] = [
            (x * f64::from(w - 1)) as f32,
            ((1.0 - y) * f64::from(h - 1)) as f32,
        ];
        destination_points[i] = [
            (u * f64::from(ow - 1)) as f32,
            ((1.0 - v) * f64::from(oh - 1)) as f32,
        ];
    }
    let mut transform = Perspective::default();
    unsafe {
        checked(
            vImageGetPerspectiveWarp(&source_points, &destination_points, &mut transform, 0),
            "homografia",
        )?;
    }
    {
        let mut scratch = SCRATCH
            .lock()
            .map_err(|_| "Contexto de imagem nativa indisponível")?;
        scratch.source.resize(source_bytes, 0);
        scratch.destination.resize(destination_bytes, 0);
        let src_rgb = Buffer {
            data: rgb.as_mut_ptr().cast(),
            width: w as usize,
            height: h as usize,
            row_bytes: w as usize * 3,
        };
        let src = Buffer {
            data: scratch.source.as_mut_ptr().cast(),
            width: w as usize,
            height: h as usize,
            row_bytes: w as usize * 4,
        };
        let dest = Buffer {
            data: scratch.destination.as_mut_ptr().cast(),
            width: ow as usize,
            height: oh as usize,
            row_bytes: ow as usize * 4,
        };
        let background = [255u8, 0, 0, 0];
        unsafe {
            checked(
                vImageConvert_RGB888toARGB8888(&src_rgb, ptr::null(), 255, &src, false, 0),
                "RGB para ARGB",
            )?;
            let size = vImagePerspectiveWarp_ARGB8888(
                &src,
                &dest,
                ptr::null_mut(),
                &transform,
                1,
                background.as_ptr(),
                4 | 128,
            );
            checked(size, "buffer temporário")?;
            scratch.temp.resize(size as usize, 0);
            let temp = if scratch.temp.is_empty() {
                ptr::null_mut()
            } else {
                scratch.temp.as_mut_ptr().cast()
            };
            checked(
                vImagePerspectiveWarp_ARGB8888(
                    &src,
                    &dest,
                    temp,
                    &transform,
                    1,
                    background.as_ptr(),
                    4,
                ),
                "perspectiva",
            )?;
            rgb.resize(ow as usize * oh as usize * 3, 0);
            let output = Buffer {
                data: rgb.as_mut_ptr().cast(),
                width: ow as usize,
                height: oh as usize,
                row_bytes: ow as usize * 3,
            };
            checked(
                vImageConvert_ARGB8888toRGB888(&dest, &output, 0),
                "ARGB para RGB",
            )?;
        }
        Ok(rgb)
    }
}
