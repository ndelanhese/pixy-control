//! Local preview, photo and recording framing; never changes camera settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Aspect {
    #[default]
    Original,
    Landscape,
    Portrait,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Backdrop {
    #[default]
    Original,
    Slate,
    Midnight,
    Sage,
}
impl Backdrop {
    pub const ALL: [(Self, &str); 4] = [
        (Self::Original, "Original"),
        (Self::Slate, "Grafite"),
        (Self::Midnight, "Azul"),
        (Self::Sage, "Verde"),
    ];

    pub fn color(self) -> Option<[u8; 3]> {
        match self {
            Self::Original => None,
            Self::Slate => Some([50, 55, 65]),
            Self::Midnight => Some([34, 55, 94]),
            Self::Sage => Some([63, 91, 77]),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FrameTransform {
    #[serde(default)]
    pub quad: Option<crate::quad::Quad>,
    #[serde(default)]
    pub background_blur: f32,
    #[serde(default)]
    pub reactions: bool,
    #[serde(default)]
    pub studio_light: bool,
    #[serde(default)]
    pub backdrop: Backdrop,
    pub rotation: u8,
    pub aspect: Aspect,
    pub zoom: f32,
    pub x: f32,
    pub y: f32,
    pub mirror: bool,
    pub flip: bool,
}
impl Default for FrameTransform {
    fn default() -> Self {
        Self {
            quad: None,
            background_blur: 0.0,
            reactions: false,
            studio_light: false,
            backdrop: Backdrop::Original,
            rotation: 0,
            aspect: Aspect::Original,
            zoom: 1.0,
            x: 0.5,
            y: 0.5,
            mirror: false,
            flip: false,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct CameraRoi {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub mirror: bool,
}

pub(crate) fn camera_roi(
    size: (u32, u32),
    canvas: (u32, u32),
    m: [f64; 8],
) -> Result<CameraRoi, String> {
    let (w, h) = (size.0 as usize, size.1 as usize);
    if w < 2
        || h < 2
        || w > 16384
        || h > 16384
        || canvas.0 < 2
        || canvas.1 < 2
        || canvas.0 > 1920
        || canvas.1 > 1920
        || m.iter().any(|v| !v.is_finite())
        || m[1] != 0.0
        || m[3] != 0.0
        || m[6] != 0.0
        || m[7] != 0.0
        || m[0] == 0.0
        || m[4] <= 0.0
    {
        return Err("Enquadramento de chamada inválido".into());
    }
    let x0 = (m[2] * (w - 1) as f64).round();
    let x1 = ((m[0] + m[2]) * (w - 1) as f64).round();
    let y0 = (m[5] * (h - 1) as f64).round();
    let y1 = ((m[4] + m[5]) * (h - 1) as f64).round();
    if x0.min(x1) < 0.0 || x0.max(x1) >= w as f64 || y0 < 0.0 || y1 >= h as f64 || y1 < y0 {
        return Err("Recorte de chamada fora da câmera".into());
    }
    Ok(CameraRoi {
        x: x0.min(x1) as usize,
        y: y0 as usize,
        width: (x1 - x0).abs() as usize + 1,
        height: (y1 - y0) as usize + 1,
        mirror: x1 < x0,
    })
}
/// Render the same normalized framing within a bounded live-output canvas.
pub fn apply_bounded(
    width: u32,
    height: u32,
    rgb: Vec<u8>,
    view: &FrameTransform,
    limit: u32,
) -> Result<(u32, u32, Vec<u8>), String> {
    if limit < 2 {
        return Err("Limite de saída inválido".into());
    }
    apply_inner(width, height, rgb, view, Some(limit))
}
/// Native source flips precede rotation, so their display axes swap at90/270°.
pub fn effective_flips(view: &FrameTransform) -> (bool, bool) {
    if view.rotation % 2 == 1 {
        (view.flip, view.mirror)
    } else {
        (view.mirror, view.flip)
    }
}
/// Fit the rotated source into the selected fixed canvas, then apply digital
/// PTZ and four-corner correction. Portrait swaps the capture canvas dimensions;
/// rotation alone keeps that canvas. Uncovered pixels are black, as in Studio.
/// All geometry shares one bilinear sampling pass.
pub fn apply(
    width: u32,
    height: u32,
    rgb: Vec<u8>,
    transform: &FrameTransform,
) -> Result<(u32, u32, Vec<u8>), String> {
    apply_inner(width, height, rgb, transform, None)
}
fn apply_inner(
    width: u32,
    height: u32,
    rgb: Vec<u8>,
    transform: &FrameTransform,
    limit: Option<u32>,
) -> Result<(u32, u32, Vec<u8>), String> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(3));
    if width < 2 || height < 2 || expected != Some(rgb.len()) {
        return Err("Dimensões ou buffer RGB inválidos".into());
    }
    if !transform.background_blur.is_finite()
        || !(0.0..=1.0).contains(&transform.background_blur)
        || transform.rotation > 3
        || !transform.zoom.is_finite()
        || !(1.0..=4.0).contains(&transform.zoom)
        || !transform.x.is_finite()
        || !(0.0..=1.0).contains(&transform.x)
        || !transform.y.is_finite()
        || !(0.0..=1.0).contains(&transform.y)
    {
        return Err("Transformação de imagem inválida".into());
    }
    // Preserve allocation on the common unmodified preview path.
    if limit.is_none_or(|limit| width.max(height) <= limit)
        && transform.rotation == 0
        && transform.aspect == Aspect::Original
        && transform.zoom == 1.0
        && !transform.mirror
        && !transform.flip
        && width.is_multiple_of(2)
        && height.is_multiple_of(2)
    {
        return apply_quad(width, height, rgb, transform, width, height);
    }
    let (ow, oh, matrix) = render_plan(width, height, transform, limit)?;
    crate::quad::warp_rgb_mapped(width, height, rgb, ow, oh, matrix)
}
/// Geometry is evaluated in the original capture canvas, then sampled directly
/// into the live canvas. Zoom never discards 4K detail before the crop.
pub(crate) fn render_plan(
    width: u32,
    height: u32,
    transform: &FrameTransform,
    limit: Option<u32>,
) -> Result<(u32, u32, [f64; 8]), String> {
    if width < 2
        || height < 2
        || limit.is_some_and(|n| n < 2)
        || transform.rotation > 3
        || !transform.zoom.is_finite()
        || !(1.0..=4.0).contains(&transform.zoom)
        || !transform.x.is_finite()
        || !(0.0..=1.0).contains(&transform.x)
        || !transform.y.is_finite()
        || !(0.0..=1.0).contains(&transform.y)
    {
        return Err("Transformação inválida".into());
    }
    let (canvas_width, canvas_height) = match transform.aspect {
        Aspect::Portrait => (height & !1, width & !1),
        Aspect::Original | Aspect::Landscape => (width & !1, height & !1),
    };
    let (rotated_width, rotated_height) = if transform.rotation % 2 == 1 {
        (height, width)
    } else {
        (width, height)
    };
    let fit = (f64::from(canvas_width) / f64::from(rotated_width))
        .min(f64::from(canvas_height) / f64::from(rotated_height));
    let fitted_width = ((f64::from(rotated_width) * fit).floor() as u32 & !1).max(2);
    let fitted_height = ((f64::from(rotated_height) * fit).floor() as u32 & !1).max(2);
    let (out_mirror, out_flip) = effective_flips(transform);
    let (ax, bx) = fitted_axis(
        rotated_width,
        canvas_width,
        fitted_width,
        transform.zoom,
        transform.x,
        out_mirror,
    );
    let (ay, by) = fitted_axis(
        rotated_height,
        canvas_height,
        fitted_height,
        transform.zoom,
        transform.y,
        out_flip,
    );
    let affine = match transform.rotation {
        0 => [[ax, 0.0, bx], [0.0, ay, by]],
        1 => [[0.0, ay, by], [-ax, 0.0, 1.0 - bx]],
        2 => [[-ax, 0.0, 1.0 - bx], [0.0, -ay, 1.0 - by]],
        3 => [[0.0, -ay, 1.0 - by], [ax, 0.0, bx]],
        _ => unreachable!(),
    };
    let quad = transform.quad.as_ref().unwrap_or(&crate::quad::FULL_QUAD);
    crate::quad::validate_quad(quad)?;
    let h = crate::quad::homography(quad)?;
    let mut matrix = [0.0; 8];
    for (row, coefficients) in affine.iter().enumerate() {
        let [a, b, c] = *coefficients;
        matrix[row * 3] = a * h[0] + b * h[3] + c * h[6];
        matrix[row * 3 + 1] = a * h[1] + b * h[4] + c * h[7];
        matrix[row * 3 + 2] = a * h[2] + b * h[5] + c;
    }
    matrix[6] = h[6];
    matrix[7] = h[7];
    let (ow, oh) =
        if let Some(limit) = limit.filter(|limit| canvas_width.max(canvas_height) > *limit) {
            let scale = f64::from(limit) / f64::from(canvas_width.max(canvas_height));
            (
                ((f64::from(canvas_width) * scale).round() as u32 & !1).max(2),
                ((f64::from(canvas_height) * scale).round() as u32 & !1).max(2),
            )
        } else {
            (canvas_width, canvas_height)
        };
    Ok((ow, oh, matrix))
}
/// Map canvas pixel endpoints into the selected source pixel endpoints. A small
/// fitted source is centered with bars; an enlarged source pans within its crop.
fn fitted_axis(
    source: u32,
    canvas: u32,
    fitted: u32,
    zoom: f32,
    position: f32,
    reverse: bool,
) -> (f64, f64) {
    let scaled = (f64::from(fitted) * f64::from(zoom)).floor() as u32;
    let visible = scaled.min(canvas).max(2);
    let (crop, start, offset) = if scaled >= canvas {
        let crop = ((f64::from(source) * f64::from(canvas) / f64::from(scaled)).floor() as u32
            & !1)
            .max(2)
            .min(source);
        let start = (f64::from(source - crop) * f64::from(position)).round();
        (crop, start, 0.0)
    } else {
        (source, 0.0, f64::from(canvas - visible) / 2.0)
    };
    let span = f64::from(crop - 1) / f64::from(source - 1);
    let scale = span * f64::from(canvas - 1) / f64::from(visible - 1);
    if reverse {
        (
            -scale,
            (start + f64::from(crop - 1)) / f64::from(source - 1)
                + scale * offset / f64::from(canvas - 1),
        )
    } else {
        (
            scale,
            start / f64::from(source - 1) - scale * offset / f64::from(canvas - 1),
        )
    }
}
/// Keep the selected source center fixed when changing digital zoom.
pub fn set_zoom(view: &mut FrameTransform, zoom: f32) {
    let zoom = (zoom * 10.0).round().clamp(10.0, 40.0) / 10.0;
    let old_size = 1.0 / view.zoom;
    let new_size = 1.0 / zoom;
    for position in [&mut view.x, &mut view.y] {
        let center = *position * (1.0 - old_size) + old_size / 2.0;
        *position = if zoom == 1.0 {
            0.5
        } else {
            ((center - new_size / 2.0) / (1.0 - new_size)).clamp(0.0, 1.0)
        };
    }
    view.zoom = zoom;
}

/// Rectangles use normalized [left, top, width, height] in the zoom-one
/// overview canvas. Travel is the per-axis range available for dragging.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverviewGeometry {
    pub content: [f32; 4],
    pub crop: [f32; 4],
    pub travel: [f32; 2],
}
pub fn overview_geometry(
    width: u32,
    height: u32,
    view: &FrameTransform,
) -> Result<OverviewGeometry, String> {
    if width < 2
        || height < 2
        || view.rotation > 3
        || !view.zoom.is_finite()
        || !(1.0..=4.0).contains(&view.zoom)
        || !view.x.is_finite()
        || !(0.0..=1.0).contains(&view.x)
        || !view.y.is_finite()
        || !(0.0..=1.0).contains(&view.y)
    {
        return Err("Geometria de enquadramento inválida".into());
    }
    let (cw, ch) = if view.aspect == Aspect::Portrait {
        (height & !1, width & !1)
    } else {
        (width & !1, height & !1)
    };
    let (rw, rh) = if view.rotation % 2 == 1 {
        (height, width)
    } else {
        (width, height)
    };
    let fit = (f64::from(cw) / f64::from(rw)).min(f64::from(ch) / f64::from(rh));
    let fw = ((f64::from(rw) * fit).floor() as u32 & !1).max(2) as f32;
    let fh = ((f64::from(rh) * fit).floor() as u32 & !1).max(2) as f32;
    let (cw, ch) = (cw as f32, ch as f32);
    let content = [(cw - fw) / 2.0 / cw, (ch - fh) / 2.0 / ch, fw / cw, fh / ch];
    let sw = (cw / (fw * view.zoom)).min(1.0);
    let sh = (ch / (fh * view.zoom)).min(1.0);
    let travel = [content[2] * (1.0 - sw), content[3] * (1.0 - sh)];
    let (out_mirror, out_flip) = effective_flips(view);
    let px = if out_mirror { 1.0 - view.x } else { view.x };
    let py = if out_flip { 1.0 - view.y } else { view.y };
    let crop = [
        content[0] + travel[0] * px,
        content[1] + travel[1] * py,
        content[2] * sw,
        content[3] * sh,
    ];
    Ok(OverviewGeometry {
        content,
        crop,
        travel,
    })
}

/// Small full-field image for the draggable digital PTZ overview.
pub fn overview(
    width: u32,
    height: u32,
    rgb: &[u8],
    view: &FrameTransform,
) -> Result<(u32, u32, Vec<u8>), String> {
    if width < 2
        || height < 2
        || (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(3))
            != Some(rgb.len())
    {
        return Err("Buffer inválido".into());
    }
    let w = width.min(320) & !1;
    let h = ((height as u64 * w as u64 / width as u64) as u32 & !1).max(2);
    let mut small = vec![0; w as usize * h as usize * 3];
    for y in 0..h {
        for x in 0..w {
            let source = ((y as u64 * height as u64 / h as u64) * width as u64
                + x as u64 * width as u64 / w as u64) as usize
                * 3;
            let target = (y * w + x) as usize * 3;
            small[target..target + 3].copy_from_slice(&rgb[source..source + 3]);
        }
    }
    apply(
        w,
        h,
        small,
        &FrameTransform {
            zoom: 1.0,
            quad: None,
            ..*view
        },
    )
}

fn apply_quad(
    w: u32,
    h: u32,
    rgb: Vec<u8>,
    t: &FrameTransform,
    canvas_width: u32,
    canvas_height: u32,
) -> Result<(u32, u32, Vec<u8>), String> {
    let quad = t.quad.as_ref().unwrap_or(&crate::quad::FULL_QUAD);
    if (w, h) == (canvas_width, canvas_height) {
        crate::quad::warp_rgb(w, h, rgb, quad)
    } else {
        crate::quad::warp_rgb_to(w, h, rgb, quad, canvas_width, canvas_height)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn pixels(values: &[u8]) -> Vec<u8> {
        values.iter().flat_map(|v| [*v, 0, 0]).collect()
    }
    fn reds(rgb: &[u8]) -> Vec<u8> {
        rgb.chunks_exact(3).map(|p| p[0]).collect()
    }

    #[test]
    fn previous_settings_keep_framing_and_disable_new_effects() {
        let previous = serde_json::json!({
            "background_blur": 0.65, "rotation": 0, "aspect": "Original",
            "zoom": 1.3, "x": 0.5, "y": 0.5, "mirror": false, "flip": false
        });
        let view: FrameTransform = serde_json::from_value(previous).unwrap();
        assert_eq!(view.background_blur, 0.65);
        assert_eq!(view.zoom, 1.3);
        assert!(!view.reactions && !view.studio_light);
        assert_eq!(view.backdrop, Backdrop::Original);
        let roundtrip: FrameTransform =
            serde_json::from_value(serde_json::to_value(view).unwrap()).unwrap();
        assert_eq!(roundtrip, view);
    }
    #[test]
    fn rotates_clockwise_and_mirrors_asymmetric_pixels() {
        let fixture = pixels(&[1, 2, 3, 4]);
        let transform = FrameTransform {
            rotation: 1,
            ..Default::default()
        };
        assert_eq!(
            reds(&apply(2, 2, fixture.clone(), &transform).unwrap().2),
            [3, 1, 4, 2]
        );
        let transform = FrameTransform {
            mirror: true,
            ..Default::default()
        };
        assert_eq!(
            reds(&apply(2, 2, fixture.clone(), &transform).unwrap().2),
            [2, 1, 4, 3]
        );
        let transform = FrameTransform {
            flip: true,
            ..Default::default()
        };
        assert_eq!(
            reds(&apply(2, 2, fixture, &transform).unwrap().2),
            [3, 4, 1, 2]
        );
    }
    #[test]
    fn source_flip_precedes_clockwise_rotation() {
        let source = pixels(&[1, 2, 3, 4]);
        let (_, _, out) = apply(
            2,
            2,
            source.clone(),
            &FrameTransform {
                rotation: 1,
                mirror: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(reds(&out), [4, 2, 3, 1]);
        let (_, _, out) = apply(
            2,
            2,
            source,
            &FrameTransform {
                rotation: 1,
                flip: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(reds(&out), [1, 3, 2, 4]);
    }
    #[test]
    fn crop_positions_fill_the_original_canvas_with_selected_source() {
        let fixture = pixels(&(0..32).collect::<Vec<_>>());
        let transform = FrameTransform {
            zoom: 2.0,
            x: 1.0,
            y: 1.0,
            ..Default::default()
        };
        let (w, h, rgb) = apply(8, 4, fixture, &transform).unwrap();
        assert_eq!((w, h), (8, 4));
        let values = reds(&rgb);
        assert_eq!(
            [values[0], values[7], values[31], values[24]],
            [20, 23, 31, 28]
        );
    }
    #[test]
    fn portrait_canvas_fits_full_landscape_source_with_black_bars() {
        let mut source = vec![0; 32 * 18 * 3];
        for pixel in source.chunks_exact_mut(3) {
            pixel.copy_from_slice(&[41, 71, 101]);
        }
        let (w, h, out) = apply(
            32,
            18,
            source,
            &FrameTransform {
                aspect: Aspect::Portrait,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!((w, h), (18, 32));
        assert_eq!(&out[..3], &[0, 0, 0]);
        assert_eq!(&out[(11 * 18) * 3..(11 * 18) * 3 + 3], &[41, 71, 101]);
        assert_eq!(
            &out[(20 * 18 + 17) * 3..(20 * 18 + 17) * 3 + 3],
            &[41, 71, 101]
        );
        assert_eq!(&out[(21 * 18) * 3..(21 * 18) * 3 + 3], &[0, 0, 0]);
    }
    #[test]
    fn rotation_keeps_canvas_and_fits_all_asymmetric_corners() {
        let mut source = vec![0; 32 * 18 * 3];
        for (x, y, color) in [(0, 0, 11), (31, 0, 22), (31, 17, 33), (0, 17, 44)] {
            source[(y * 32 + x) * 3] = color;
        }
        let (w, h, out) = apply(
            32,
            18,
            source,
            &FrameTransform {
                rotation: 1,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!((w, h), (32, 18));
        let red = |x: usize, y: usize| out[(y * 32 + x) * 3];
        assert_eq!(
            [red(11, 0), red(20, 0), red(20, 17), red(11, 17)],
            [44, 11, 22, 33]
        );
        assert_eq!([red(0, 0), red(31, 17)], [0, 0]);
    }
    #[test]
    fn portrait_plus_rotation_maps_source_edges_without_clipping() {
        let source = pixels(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let (w, h, out) = apply(
            4,
            2,
            source,
            &FrameTransform {
                aspect: Aspect::Portrait,
                rotation: 1,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!((w, h), (2, 4));
        assert_eq!(reds(&out), [5, 1, 6, 2, 7, 3, 8, 4]);
    }
    #[test]
    fn framing_still_validates_the_user_quad_before_internal_mapping() {
        let invalid = [[0.0, 0.0], [1.0, 1.0], [1.0, 0.0], [0.0, 1.0]];
        assert!(apply(
            4,
            2,
            vec![0; 24],
            &FrameTransform {
                rotation: 1,
                quad: Some(invalid),
                ..Default::default()
            }
        )
        .is_err());
        let quad = [
            [1.0 / 3.0, 1.0 / 3.0],
            [2.0 / 3.0, 1.0 / 3.0],
            [2.0 / 3.0, 2.0 / 3.0],
            [1.0 / 3.0, 2.0 / 3.0],
        ];
        let source = pixels(&[
            0, 30, 60, 90, 40, 70, 100, 130, 80, 110, 140, 170, 120, 150, 180, 210,
        ]);
        let (_, _, out) = apply(
            4,
            4,
            source,
            &FrameTransform {
                rotation: 1,
                quad: Some(quad),
                ..Default::default()
            },
        )
        .unwrap();
        let values = reds(&out);
        assert_eq!(
            [values[0], values[3], values[15], values[12]],
            [110, 70, 100, 140]
        );
    }
    #[test]
    fn validates_parameters_and_exact_checked_buffer_length() {
        for transform in [
            FrameTransform {
                rotation: 4,
                ..Default::default()
            },
            FrameTransform {
                zoom: f32::NAN,
                ..Default::default()
            },
            FrameTransform {
                zoom: 0.9,
                ..Default::default()
            },
            FrameTransform {
                zoom: 4.1,
                ..Default::default()
            },
            FrameTransform {
                x: -0.1,
                ..Default::default()
            },
            FrameTransform {
                y: f32::INFINITY,
                ..Default::default()
            },
        ] {
            assert!(apply(2, 2, vec![0; 12], &transform).is_err());
        }
        for (w, h, len) in [
            (0, 2, 0),
            (1, 2, 6),
            (2, 2, 11),
            (2, 2, 13),
            (u32::MAX, u32::MAX, 0),
        ] {
            assert!(apply(w, h, vec![0; len], &FrameTransform::default()).is_err());
        }
    }
    #[test]
    fn default_even_frame_reuses_allocation() {
        let rgb = vec![0; 24];
        let original = rgb.as_ptr();
        let (_, _, output) = apply(4, 2, rgb, &FrameTransform::default()).unwrap();
        assert_eq!(original, output.as_ptr());
        let (w, h, _) = apply(3, 3, vec![0; 27], &FrameTransform::default()).unwrap();
        assert_eq!((w, h), (2, 2));
    }
    #[test]
    fn native_four_times_zoom_preserves_the_selected_source_region() {
        let fixture = pixels(&(0..64).collect::<Vec<_>>());
        let framing = FrameTransform {
            zoom: 4.0,
            x: 1.0,
            y: 1.0,
            ..Default::default()
        };
        let (width, height, rgb) = apply(8, 8, fixture, &framing).unwrap();
        assert_eq!((width, height), (8, 8));
        let values = reds(&rgb);
        assert_eq!(
            [values[0], values[7], values[63], values[56]],
            [54, 55, 63, 62]
        );
        assert_eq!(values[3 * 8 + 3], 58);
        assert!(apply(
            8,
            8,
            vec![0; 192],
            &FrameTransform {
                zoom: 4.1,
                ..framing
            }
        )
        .is_err());
    }
}

#[cfg(test)]
mod overview_tests {
    use super::*;
    #[test]
    fn zoom_preserves_source_center_and_clamps_edges() {
        let mut v = FrameTransform {
            zoom: 2.0,
            x: 0.3,
            y: 0.7,
            ..Default::default()
        };
        let old = [v.x * 0.5 + 0.25, v.y * 0.5 + 0.25];
        set_zoom(&mut v, 4.0);
        assert!((v.x * 0.75 + 0.125 - old[0]).abs() < 1e-6);
        assert!((v.y * 0.75 + 0.125 - old[1]).abs() < 1e-6);
        set_zoom(&mut v, 1.0);
        assert_eq!((v.x, v.y, v.zoom), (0.5, 0.5, 1.0));
    }
    #[test]
    fn overview_geometry_accounts_for_portrait_bars_and_axis_specific_pan() {
        let view = FrameTransform {
            aspect: Aspect::Portrait,
            zoom: 2.0,
            x: 1.0,
            y: 0.0,
            ..Default::default()
        };
        let g = overview_geometry(32, 18, &view).unwrap();
        assert_eq!(g.content, [0.0, 11.0 / 32.0, 1.0, 10.0 / 32.0]);
        assert_eq!(g.crop, [0.5, 11.0 / 32.0, 0.5, 10.0 / 32.0]);
        assert_eq!(g.travel, [0.5, 0.0]);
        let reflected = overview_geometry(
            32,
            18,
            &FrameTransform {
                mirror: true,
                ..view
            },
        )
        .unwrap();
        assert_eq!(reflected.crop[0], 0.0);
        let g = overview_geometry(32, 18, &FrameTransform { zoom: 4.0, ..view }).unwrap();
        assert_eq!(g.crop[2], 0.25);
        assert!((g.crop[3] - 0.25).abs() < 1e-6);
        assert!((g.travel[1] - 0.0625).abs() < 1e-6);
    }
    #[test]
    fn overview_displays_full_field_despite_zoom_and_quad() {
        let view = FrameTransform {
            zoom: 4.0,
            quad: Some(crate::quad::FULL_QUAD),
            ..Default::default()
        };
        let (_, _, rgb) = overview(4, 2, &(0..24).collect::<Vec<u8>>(), &view).unwrap();
        assert_eq!(rgb, (0..24).collect::<Vec<u8>>());
    }
}

#[cfg(test)]
mod blur_settings_tests {
    #[test]
    fn invalid_blur_preferences_are_rejected_instead_of_silently_ignored() {
        let mut value = serde_json::to_value(super::FrameTransform::default()).unwrap();
        value["background_blur"] = serde_json::json!(2.0);
        let view: super::FrameTransform = serde_json::from_value(value).unwrap();
        assert!(super::apply(2, 2, vec![0; 12], &view).is_err());
    }
}

#[cfg(test)]
mod bounded_tests {
    use super::*;
    #[test]
    fn live_canvas_is_bounded_without_losing_pan_zoom_or_aspect() {
        let mut rgb = vec![0; 16 * 8 * 3];
        for y in 0..8 {
            for x in 0..16 {
                let i = (y * 16 + x) * 3;
                rgb[i..i + 3].copy_from_slice(&[(x * 10) as u8, (y * 20) as u8, 100]);
            }
        }
        let view = FrameTransform {
            zoom: 2.0,
            x: 1.0,
            y: 0.0,
            ..Default::default()
        };
        let (w, h, out) = apply_bounded(16, 8, rgb, &view, 8).unwrap();
        assert_eq!((w, h), (8, 4));
        assert_eq!(&out[..3], &[80, 0, 100]);
        assert_eq!(&out[out.len() - 3..], &[150, 60, 100]);
    }
    #[test]
    fn bounded_portrait_and_small_inputs_keep_shape_and_never_upscale() {
        let view = FrameTransform {
            aspect: Aspect::Portrait,
            rotation: 1,
            ..Default::default()
        };
        let (w, h, _) = apply_bounded(16, 8, vec![100; 16 * 8 * 3], &view, 8).unwrap();
        assert_eq!((w, h), (4, 8));
        let (w, h, _) = apply_bounded(4, 2, vec![100; 24], &FrameTransform::default(), 8).unwrap();
        assert_eq!((w, h), (4, 2));
    }
}
