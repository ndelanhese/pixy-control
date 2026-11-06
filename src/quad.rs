//! Local four-corner perspective correction; no camera state or AI processing.
#[cfg(target_os = "macos")]
#[path = "quad_native.rs"]
mod native;
/// Release the shared full-frame scratch after capture has fully stopped.
pub(crate) fn release_scratch() {
    #[cfg(target_os = "macos")]
    native::release_scratch();
}
pub type Quad = [[f32; 2]; 4];
pub const FULL_QUAD: Quad = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
/// Points must follow TL, TR, BR, BL around a strictly convex region. Image
/// coordinates increase downward, so that winding has positive cross products.
pub fn validate_quad(quad: &Quad) -> Result<(), String> {
    if quad
        .iter()
        .flatten()
        .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
    {
        return Err("Os quatro pontos devem ficar dentro da imagem".into());
    }
    for i in 0..4 {
        let a = quad[i];
        let b = quad[(i + 1) % 4];
        let c = quad[(i + 2) % 4];
        let cross = (f64::from(b[0]) - f64::from(a[0])) * (f64::from(c[1]) - f64::from(b[1]))
            - (f64::from(b[1]) - f64::from(a[1])) * (f64::from(c[0]) - f64::from(b[0]));
        if cross <= 1e-8 {
            return Err("Selecione quatro cantos convexos, em ordem, sem cruzar as linhas".into());
        }
    }
    Ok(())
}
/// Rectify the chosen quadrilateral into the full output canvas. Pixels outside
/// that quadrilateral are discarded. Output stays at the input's even resolution.
pub fn warp_rgb(w: u32, h: u32, rgb: Vec<u8>, quad: &Quad) -> Result<(u32, u32, Vec<u8>), String> {
    warp_rgb_to(w, h, rgb, quad, w & !1, h & !1)
}
/// Rectify and resize directly into one explicit even canvas. Source cropping,
/// perspective correction and digital-zoom enlargement share one sampling pass.
pub fn warp_rgb_to(
    w: u32,
    h: u32,
    rgb: Vec<u8>,
    quad: &Quad,
    out_width: u32,
    out_height: u32,
) -> Result<(u32, u32, Vec<u8>), String> {
    let length = (w as usize)
        .checked_mul(h as usize)
        .and_then(|n| n.checked_mul(3));
    if w < 2 || h < 2 || length != Some(rgb.len()) {
        return Err("Dimensões ou buffer RGB inválidos".into());
    }
    validate_quad(quad)?;
    if out_width < 2
        || out_height < 2
        || !out_width.is_multiple_of(2)
        || !out_height.is_multiple_of(2)
    {
        return Err("Canvas de saída deve ter dimensões pares de pelo menos2pixels".into());
    }
    if *quad == FULL_QUAD && (w, h) == (out_width, out_height) {
        return Ok((w, h, rgb));
    }
    let matrix = homography(quad)?;
    warp_rgb_mapped(w, h, rgb, out_width, out_height, matrix)
}
/// Internal normalized output-to-source projective map. Unlike user quads,
/// mapped coordinates may leave the source; those pixels become black bars.
pub(crate) fn warp_rgb_mapped(
    w: u32,
    h: u32,
    rgb: Vec<u8>,
    out_width: u32,
    out_height: u32,
    matrix: [f64; 8],
) -> Result<(u32, u32, Vec<u8>), String> {
    if matrix.iter().any(|v| !v.is_finite()) {
        return Err("Perspectiva não finita".into());
    }
    if matrix == [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0] && (w, h) == (out_width, out_height) {
        return Ok((w, h, rgb));
    }
    let (ow, oh) = (out_width, out_height);
    let _length = (ow as usize)
        .checked_mul(oh as usize)
        .and_then(|n| n.checked_mul(3))
        .ok_or("Imagem grande demais")?;
    #[cfg(target_os = "macos")]
    {
        let output = native::warp(w, h, rgb, ow, oh, &matrix)?;
        Ok((ow, oh, output))
    }
    #[cfg(not(target_os = "macos"))]
    warp_rgb_cpu(w, h, rgb, ow, oh, matrix, _length)
}
/// Portable scalar reference retained for independent native geometry tests.
#[cfg(any(test, not(target_os = "macos")))]
fn warp_rgb_cpu(
    w: u32,
    h: u32,
    rgb: Vec<u8>,
    ow: u32,
    oh: u32,
    matrix: [f64; 8],
    length: usize,
) -> Result<(u32, u32, Vec<u8>), String> {
    let mut output = vec![0; length];
    let available = std::thread::available_parallelism().map_or(1, |n| n.get());
    let workers = if length >= 3840 * 2160 * 3 {
        available.min(8)
    } else if length >= 640 * 480 * 3 {
        available.min(4)
    } else {
        1
    };
    let rows_per_worker = (oh as usize).div_ceil(workers);
    let bytes_per_band = rows_per_worker * ow as usize * 3;
    std::thread::scope(|scope| -> Result<(), String> {
        let mut jobs = Vec::new();
        for (index, band) in output.chunks_mut(bytes_per_band).enumerate() {
            let source = &rgb;
            jobs.push(scope.spawn(move || {
                render_rows(w, h, ow, oh, source, &matrix, index * rows_per_worker, band)
            }));
        }
        for job in jobs {
            job.join()
                .map_err(|_| "Falha no processamento de perspectiva")??;
        }
        Ok(())
    })?;
    Ok((ow, oh, output))
}
/// Each worker borrows immutable source/matrix and a disjoint output band. There
/// is no shared mutable renderer, queue, persistent thread context or global lock.
#[cfg(any(test, not(target_os = "macos")))]
#[allow(clippy::too_many_arguments)]
fn render_rows(
    w: u32,
    h: u32,
    ow: u32,
    oh: u32,
    rgb: &[u8],
    matrix: &[f64; 8],
    start_row: usize,
    output: &mut [u8],
) -> Result<(), String> {
    let dx = 1.0 / f64::from(ow - 1);
    let dy = 1.0 / f64::from(oh - 1);
    let (nx_step, ny_step, d_step) = (matrix[0] * dx, matrix[3] * dx, matrix[6] * dx);
    for (local_y, row) in output.chunks_exact_mut(ow as usize * 3).enumerate() {
        let v = (start_row + local_y) as f64 * dy;
        let (mut nx, mut ny, mut denominator) = (
            matrix[1] * v + matrix[2],
            matrix[4] * v + matrix[5],
            matrix[7] * v + 1.0,
        );
        for pixel in row.chunks_exact_mut(3) {
            if !denominator.is_finite() || denominator.abs() < 1e-12 {
                return Err("Perspectiva inválida".into());
            }
            let inverse = 1.0 / denominator;
            let (u, v) = (nx * inverse, ny * inverse);
            if !(-1e-7..=1.0 + 1e-7).contains(&u) || !(-1e-7..=1.0 + 1e-7).contains(&v) {
                nx += nx_step;
                ny += ny_step;
                denominator += d_step;
                continue;
            }
            let sx = u.clamp(0.0, 1.0) * f64::from(w - 1);
            let sy = v.clamp(0.0, 1.0) * f64::from(h - 1);
            let (x0, y0) = (sx as usize, sy as usize);
            let (x1, y1) = ((x0 + 1).min(w as usize - 1), (y0 + 1).min(h as usize - 1));
            let (fx, fy) = (sx - x0 as f64, sy - y0 as f64);
            let weights = [
                (1.0 - fx) * (1.0 - fy),
                fx * (1.0 - fy),
                (1.0 - fx) * fy,
                fx * fy,
            ];
            let offsets = [
                (y0 * w as usize + x0) * 3,
                (y0 * w as usize + x1) * 3,
                (y1 * w as usize + x0) * 3,
                (y1 * w as usize + x1) * 3,
            ];
            for channel in 0..3 {
                let value = f64::from(rgb[offsets[0] + channel]) * weights[0]
                    + f64::from(rgb[offsets[1] + channel]) * weights[1]
                    + f64::from(rgb[offsets[2] + channel]) * weights[2]
                    + f64::from(rgb[offsets[3] + channel]) * weights[3];
                pixel[channel] = value.round() as u8;
            }
            nx += nx_step;
            ny += ny_step;
            denominator += d_step;
        }
    }
    Ok(())
}
/// Solve destination-unit-rectangle → source-quad projective transform, h[8]=1.
/// Partial pivoting keeps non-axis-aligned camera selections numerically stable.
pub(crate) fn homography(quad: &Quad) -> Result<[f64; 8], String> {
    let mut rows = [[0.0; 9]; 8];
    for (i, (u, v)) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
        .into_iter()
        .enumerate()
    {
        let (x, y) = (f64::from(quad[i][0]), f64::from(quad[i][1]));
        rows[2 * i] = [u, v, 1.0, 0.0, 0.0, 0.0, -x * u, -x * v, x];
        rows[2 * i + 1] = [0.0, 0.0, 0.0, u, v, 1.0, -y * u, -y * v, y];
    }
    for column in 0..8 {
        let pivot = (column..8)
            .max_by(|&a, &b| rows[a][column].abs().total_cmp(&rows[b][column].abs()))
            .unwrap();
        if rows[pivot][column].abs() < 1e-12 {
            return Err("Os quatro pontos não definem uma perspectiva estável".into());
        }
        rows.swap(column, pivot);
        let divisor = rows[column][column];
        for entry in &mut rows[column][column..] {
            *entry /= divisor;
        }
        let normalized = rows[column];
        for (row_index, row) in rows.iter_mut().enumerate() {
            if row_index == column {
                continue;
            }
            let factor = row[column];
            for j in column..9 {
                row[j] -= factor * normalized[j];
            }
        }
    }
    let mut result = [0.0; 8];
    for i in 0..8 {
        result[i] = rows[i][8];
    }
    if result.iter().any(|v| !v.is_finite()) {
        return Err("Perspectiva não finita".into());
    }
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn pixels(values: &[u8]) -> Vec<u8> {
        values.iter().flat_map(|v| [*v, 255 - *v, 17]).collect()
    }
    fn red(rgb: &[u8], w: usize, x: usize, y: usize) -> u8 {
        rgb[(y * w + x) * 3]
    }
    #[test]
    fn identity_keeps_asymmetric_pixels_and_allocation() {
        let rgb = pixels(&[1, 17, 91, 255]);
        let address = rgb.as_ptr();
        let (w, h, out) = warp_rgb(2, 2, rgb, &FULL_QUAD).unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(out, pixels(&[1, 17, 91, 255]));
        assert_eq!(out.as_ptr(), address);
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn native_matches_portable_reference_for_asymmetric_projective_color_ramp() {
        let (w, h) = (128u32, 96u32);
        let mut source = Vec::new();
        for y in 0..h {
            for x in 0..w {
                source.extend_from_slice(&[(x * 2) as u8, (y * 2) as u8, ((x + y) / 2) as u8]);
            }
        }
        for q in [
            FULL_QUAD,
            [[0.12, 0.08], [0.88, 0.04], [0.95, 0.92], [0.06, 0.95]],
            [[0.2, 0.2], [0.8, 0.2], [0.8, 0.8], [0.2, 0.8]],
        ] {
            let matrix = homography(&q).unwrap();
            let accelerated = native::warp(w, h, source.clone(), w, h, &matrix).unwrap();
            let reference = warp_rgb_cpu(
                w,
                h,
                source.clone(),
                w,
                h,
                matrix,
                w as usize * h as usize * 3,
            )
            .unwrap()
            .2;
            for (index, (actual, expected)) in accelerated.iter().zip(&reference).enumerate() {
                assert!(
                    actual.abs_diff(*expected) <= 1,
                    "channel{index}: native{actual}, reference{expected}"
                );
            }
        }
    }
    #[test]
    fn target_canvas_resizes_selected_source_with_bilinear_corners() {
        let (w, h, out) = warp_rgb_to(2, 2, pixels(&[0, 30, 90, 120]), &FULL_QUAD, 4, 4).unwrap();
        assert_eq!((w, h), (4, 4));
        assert_eq!(
            [
                red(&out, 4, 0, 0),
                red(&out, 4, 3, 0),
                red(&out, 4, 3, 3),
                red(&out, 4, 0, 3)
            ],
            [0, 30, 120, 90]
        );
        assert_eq!(red(&out, 4, 1, 1), 40);
    }
    #[test]
    fn target_canvas_must_be_even_nonempty_and_not_overflow() {
        for (w, h) in [(0, 2), (1, 2), (3, 2), (2, 3), (u32::MAX - 1, u32::MAX - 1)] {
            assert!(warp_rgb_to(2, 2, vec![0; 12], &FULL_QUAD, w, h).is_err());
        }
    }
    #[test]
    fn selected_quad_maps_corners_and_bilinear_interior() {
        // Planar color ramp makes expected interpolation independent of homography code.
        let source = pixels(&[
            0, 30, 60, 90, 40, 70, 100, 130, 80, 110, 140, 170, 120, 150, 180, 210,
        ]);
        let quad = [
            [1.0 / 3.0, 1.0 / 3.0],
            [2.0 / 3.0, 1.0 / 3.0],
            [2.0 / 3.0, 2.0 / 3.0],
            [1.0 / 3.0, 2.0 / 3.0],
        ];
        let (w, h, out) = warp_rgb(4, 4, source, &quad).unwrap();
        assert_eq!((w, h), (4, 4));
        assert_eq!(
            [
                red(&out, 4, 0, 0),
                red(&out, 4, 3, 0),
                red(&out, 4, 3, 3),
                red(&out, 4, 0, 3)
            ],
            [70, 100, 140, 110]
        );
        assert_eq!(red(&out, 4, 1, 1), 93);
        assert_eq!(&out[(4 + 1) * 3..(4 + 1) * 3 + 3], &[93, 162, 17]);
    }
    #[test]
    fn trapezoid_uses_perspective_not_bounding_rectangle() {
        let source = pixels(&[
            0, 30, 60, 90, 40, 70, 100, 130, 80, 110, 140, 170, 120, 150, 180, 210,
        ]);
        let quad = [[1.0 / 3.0, 0.0], [2.0 / 3.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let (_, _, out) = warp_rgb(4, 4, source, &quad).unwrap();
        assert_eq!(
            [
                red(&out, 4, 0, 0),
                red(&out, 4, 3, 0),
                red(&out, 4, 3, 3),
                red(&out, 4, 0, 3)
            ],
            [30, 60, 210, 120]
        );
        // Halfway along height in destination does not equal halfway in source.
        // At destination(1/3,1/3), inverse perspective maps source(3/7,1/7).
        assert_eq!(red(&out, 4, 1, 1), 56);
    }
    #[test]
    fn parallel_bands_preserve_source_row_coordinates() {
        let (w, h) = (1024usize, 512usize);
        let mut source = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for _ in 0..w {
                source.extend_from_slice(&[17, (y % 256) as u8, 229]);
            }
        }
        let quad = [[0.1, 0.0], [0.9, 0.0], [0.9, 1.0], [0.1, 1.0]];
        let (_, _, out) = warp_rgb(w as u32, h as u32, source, &quad).unwrap();
        for y in [0, 63, 64, 127, 128, 255, 256, 383, 384, 510, 511] {
            for x in [0, 511, 1023] {
                let at = (y * w + x) * 3;
                assert_eq!(&out[at..at + 3], &[17, (y % 256) as u8, 229]);
            }
        }
    }
    #[test]
    fn rejects_crossed_concave_degenerate_and_nonfinite_quads() {
        assert!(validate_quad(&FULL_QUAD).is_ok());
        for quad in [
            [[0.0, 0.0], [1.0, 1.0], [1.0, 0.0], [0.0, 1.0]],
            [[0.0, 0.0], [1.0, 0.0], [0.2, 0.2], [0.0, 1.0]],
            [[0.0, 0.0], [0.5, 0.0], [1.0, 0.0], [0.0, 1.0]],
            [[0.0, 0.0], [1.1, 0.0], [1.0, 1.0], [0.0, 1.0]],
            [[f32::NAN, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]],
        ] {
            assert!(validate_quad(&quad).is_err());
        }
    }
    #[test]
    fn validates_exact_buffer_and_returns_even_dimensions() {
        for (w, h, len) in [
            (0, 2, 0),
            (1, 2, 6),
            (2, 2, 11),
            (2, 2, 13),
            (u32::MAX, u32::MAX, 0),
        ] {
            assert!(warp_rgb(w, h, vec![0; len], &FULL_QUAD).is_err());
        }
        let (w, h, out) = warp_rgb(3, 3, vec![31; 27], &FULL_QUAD).unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(out, vec![31; 12]);
    }
}
