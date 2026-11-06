//! Explicit second-process consumers for real shared-camera diagnostics.
//! No producer, HID control, hardware settings, or implicit startup capture.
use crate::preview::Preview;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

pub fn virtual_consumer(path: &Path) -> Result<(), String> {
    consume(path, "virtual", Preview::start_virtual(1920, 1080, 30))
}
pub fn physical_consumer(path: &Path) -> Result<(), String> {
    consume(path, "physical", Preview::start(1920, 1080, 30))
}
fn patch_means(width: u32, height: u32, rgb: &[u8]) -> Result<Vec<[f64; 3]>, String> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|size| size.checked_mul(3));
    if width < 8 || height < 8 || expected != Some(rgb.len()) {
        return Err("Buffer RGB inválido para grade 8×8".into());
    }
    let mut means = Vec::with_capacity(64);
    for row in 0..8u32 {
        for column in 0..8u32 {
            let left = (u64::from(column) * u64::from(width) / 8) as usize;
            let right = (u64::from(column + 1) * u64::from(width) / 8) as usize;
            let top = (u64::from(row) * u64::from(height) / 8) as usize;
            let bottom = (u64::from(row + 1) * u64::from(height) / 8) as usize;
            let mut sums = [0u64; 3];
            for y in top..bottom {
                for x in left..right {
                    let start = (y * width as usize + x) * 3;
                    for channel in 0..3 {
                        sums[channel] += u64::from(rgb[start + channel]);
                    }
                }
            }
            let pixels = ((right - left) * (bottom - top)) as f64;
            means.push(sums.map(|sum| sum as f64 / pixels));
        }
    }
    Ok(means)
}
fn png_path(report: &Path, second: u64) -> PathBuf {
    let stem = report.file_stem().unwrap_or_default().to_string_lossy();
    report.with_file_name(format!("{stem}-frame-{second:02}s.png"))
}
fn consume(path: &Path, kind: &str, mut preview: Preview) -> Result<(), String> {
    let started = Instant::now();
    let mut captures = Vec::<Value>::new();
    let mut observations = Vec::<Value>::new();
    let mut error = None;
    let mut last_pts = None;
    let mut last_count = 0;
    let mut accepted = 0u64;
    let mut ambiguous_frames_skipped = 0u64;
    let mut monotonic = true;
    let mut dimensions_valid = true;
    let mut measured_fps = None;
    let mut capture_started_ms = None;
    let operation = (|| -> Result<(), String> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let deadline = Instant::now() + Duration::from_secs(35);
        let initial = loop {
            if let Some(frame) = preview.latest() {
                break frame;
            }
            if preview.is_finished() {
                return Err(preview.status());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "Nenhum quadro inicial em 35 s: {}",
                    preview.status()
                ));
            }
            thread::sleep(Duration::from_millis(20));
        };
        if initial.0 != 1920 || initial.1 != 1080 {
            dimensions_valid = false;
            return Err(format!(
                "Formato inicial inesperado: {}×{}",
                initial.0, initial.1
            ));
        }
        let active = Instant::now();
        capture_started_ms = Some(started.elapsed().as_millis());
        let baseline = preview.timing();
        let mut next_capture = 0;
        let targets = [3, 10, 18];
        let mut next_observation = Duration::ZERO;
        while active.elapsed() < Duration::from_secs(20) {
            if preview.is_finished() {
                return Err(preview.status());
            }
            if let Some((width, height, rgb)) = preview.latest() {
                let timing = preview.timing();
                let elapsed = active.elapsed();
                if width != 1920
                    || height != 1080
                    || rgb.len() != width as usize * height as usize * 3
                {
                    dimensions_valid = false;
                    return Err(format!(
                        "Quadro inválido: {width}×{height}, {} bytes",
                        rgb.len()
                    ));
                }
                let pts = timing.last_pts_ns.ok_or("Quadro sem timestamp nativo")?;
                // latest RGB and timing have separate locks: a callback can
                // advance timing between taking RGB and reading this counter.
                // Do not accept a later RGB with that same ambiguous snapshot.
                if timing.frames == last_count {
                    ambiguous_frames_skipped += 1;
                    continue;
                }
                if timing.frames < last_count || last_pts.is_some_and(|previous| pts <= previous) {
                    monotonic = false;
                    return Err(
                        "Contador/PTS não avançou: quadro antigo ou timestamp regressivo".into(),
                    );
                }
                last_pts = Some(pts);
                last_count = timing.frames;
                accepted += 1;
                if elapsed >= next_observation {
                    observations.push(json!({"elapsed_ms":elapsed.as_millis(),"native_frame_count":timing.frames,"native_last_pts_ns":pts,"width":width,"height":height}));
                    next_observation = elapsed + Duration::from_millis(500);
                }
                if next_capture < targets.len()
                    && elapsed >= Duration::from_secs(targets[next_capture])
                {
                    let target = targets[next_capture];
                    let destination = png_path(path, target);
                    let means = patch_means(width, height, &rgb)?;
                    image::save_buffer(&destination, &rgb, width, height, image::ColorType::Rgb8)
                        .map_err(|error| error.to_string())?;
                    captures.push(json!({"requested_second":target,"elapsed_ms":elapsed.as_millis(),"path":destination,"width":width,"height":height,"rgb_bytes":rgb.len(),"native_frame_count":timing.frames,"native_last_pts_ns":pts,"patch_grid":[8,8],"patch_rgb_means":means}));
                    next_capture += 1;
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
        let end = preview.timing();
        let frames = end.frames.saturating_sub(baseline.frames);
        if let (Some(before), Some(after)) = (baseline.last_pts_ns, end.last_pts_ns) {
            if after > before {
                measured_fps = Some(frames as f64 * 1e9 / (after - before) as f64);
            }
        }
        if captures.len() != 3 {
            return Err(format!("Capturas incompletas: {}/3", captures.len()));
        }
        if !measured_fps.is_some_and(|fps| (fps - 30.0).abs() <= 4.5) {
            return Err(format!(
                "Cadência nativa fora de 30 fps ±15%: {measured_fps:?}"
            ));
        }
        Ok(())
    })();
    if let Err(reason) = operation {
        error = Some(reason);
    }
    let status_before_stop = preview.status();
    let timing = preview.timing();
    preview.stop();
    let stop_deadline = Instant::now() + Duration::from_secs(8);
    while !preview.is_finished() && Instant::now() < stop_deadline {
        thread::sleep(Duration::from_millis(20));
    }
    let stopped = preview.is_finished();
    let shutdown_error = if stopped {
        None
    } else {
        Some("Consumidor não confirmou encerramento em 8 s")
    };
    let passed = error.is_none() && stopped && monotonic && dimensions_valid;
    let report = json!({"test":"second_process_camera_consumer","kind":kind,"pid":std::process::id(),"passed":passed,"error":error,"shutdown_error":shutdown_error,"stopped":stopped,"capture_started_ms":capture_started_ms,"requested_hold_seconds":20,"elapsed_seconds":started.elapsed().as_secs_f64(),"accepted_latest_frames":accepted,"ambiguous_frames_skipped":ambiguous_frames_skipped,"native_frames":timing.frames,"native_first_pts_ns":timing.first_pts_ns,"native_last_pts_ns":timing.last_pts_ns,"native_fps":measured_fps,"monotonic_observed_pts":monotonic,"dimensions_valid":dimensions_valid,"captures":captures,"observations":observations,"status_before_stop":status_before_stop,"diagnostics":preview.diagnostics(),"timestamp_scope":"Native count/PTS sampled alongside latest RGB; no atomic per-frame PTS association is asserted."});
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if passed {
        Ok(())
    } else {
        Err(error.unwrap_or_else(|| shutdown_error.unwrap_or("Consumidor falhou").into()))
    }
}
/// Real camera pixels, in a second process, measured without image encoding or
/// UI screenshots inside the timed interval. Repeated transport samples do not
/// count as new images. No raw frame is written to disk.
pub fn measure_virtual(path: &Path) -> Result<(), String> {
    use std::hash::{Hash, Hasher};
    crate::video_activity::prioritize_current_thread()?;
    let mut preview = Preview::start_virtual(1920, 1080, 30);
    let result = (|| -> Result<Value, String> {
        let deadline = Instant::now() + Duration::from_secs(35);
        while preview.latest_timed().is_none() {
            if preview.is_finished() || Instant::now() > deadline {
                return Err(preview.status());
            }
            thread::sleep(Duration::from_millis(2));
        }
        thread::sleep(Duration::from_secs(6));
        preview.latest_timed();
        let start = Instant::now();
        let mut frames = Vec::new();
        let mut previous = None;
        while start.elapsed() < Duration::from_secs(60) {
            if preview.is_finished() {
                return Err(preview.status());
            }
            if let Some(frame) = preview.latest_timed() {
                let (w, h, rgb) = frame.image;
                if w < 2
                    || h < 2
                    || w > 3840
                    || h > 2160
                    || rgb.len() != w as usize * h as usize * 3
                {
                    return Err(format!("Unexpected dimensions {w}x{h}"));
                }
                let mut hash = std::collections::hash_map::DefaultHasher::new();
                rgb.hash(&mut hash);
                let digest = hash.finish();
                let source_id = crate::fluency_check::decode(&rgb, w, h);
                let changed = previous != Some(digest);
                previous = Some(digest);
                frames.push(json!({"received_ms":frame.received_at.saturating_duration_since(start).as_secs_f64()*1000.0,"sequence":frame.sequence,"pts_ns":frame.pts_ns,"pixels_changed":changed,"fingerprint":digest,"width":w,"height":h,"source_id":source_id}));
            }
            thread::sleep(Duration::from_millis(1));
        }
        let elapsed = start.elapsed().as_secs_f64();
        let unique: Vec<_> = frames
            .iter()
            .filter(|f| f["pixels_changed"] == true)
            .collect();
        let mut gaps: Vec<_> = unique
            .windows(2)
            .map(|f| f[1]["received_ms"].as_f64().unwrap() - f[0]["received_ms"].as_f64().unwrap())
            .collect();
        gaps.sort_by(f64::total_cmp);
        let p95 = gaps
            .get((gaps.len() as f64 * 0.95) as usize)
            .copied()
            .unwrap_or(f64::INFINITY);
        let max = gaps.last().copied().unwrap_or(f64::INFINITY);
        let fps = unique.len() as f64 / elapsed;
        let marker_frames = frames
            .iter()
            .filter(|f| f["source_id"].as_u64().is_some())
            .count();
        let marker_ids: std::collections::HashSet<_> = frames
            .iter()
            .filter_map(|f| f["source_id"].as_u64())
            .collect();
        let passed = fps >= 28.5 && p95 <= 50.0 && max <= 100.0;
        Ok(
            json!({"test":"real_virtual_pixel_fluency","marker_frames":marker_frames,"unique_marker_frames":marker_ids.len(),"marker_fps":marker_ids.len() as f64 / elapsed,"passed":passed,"seconds":elapsed,"unique_fps":fps,"unique_frames":unique.len(),"samples":frames.len(),"gap_p95_ms":p95,"gap_max_ms":max,"thresholds":{"minimum_unique_fps":28.5,"p95_gap_ms":50,"worst_gap_ms":100},"frames":frames,"scope":"Atomic sample timing and full RGB fingerprints; optional CRC-verified source marker identifies frames when the producer uses --trace-camera-frames. Identical pixels alone cannot distinguish a static image from a repeated capture; no source latency is inferred."}),
        )
    })();
    preview.stop();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !preview.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    let mut report = result.unwrap_or_else(|e| json!({"passed":false,"error":e}));
    report["consumer_stopped"] = json!(preview.is_finished());
    if !preview.is_finished() {
        report["passed"] = json!(false);
    }
    crate::model::save_json(path, &report)?;
    if report["passed"] == true {
        Ok(())
    } else {
        Err("Real virtual-camera motion failed its cadence limits".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grid_means_preserve_rgb_channels_and_all_64_patch_locations() {
        let mut rgb = Vec::new();
        for y in 0..8 {
            for x in 0..8 {
                rgb.extend_from_slice(&[x * 20, y * 20, 123]);
            }
        }
        let means = patch_means(8, 8, &rgb).unwrap();
        assert_eq!(means.len(), 64);
        assert_eq!(means[0], [0.0, 0.0, 123.0]);
        assert_eq!(means[7], [140.0, 0.0, 123.0]);
        assert_eq!(means[56], [0.0, 140.0, 123.0]);
        assert_eq!(means[63], [140.0, 140.0, 123.0]);
    }
    #[test]
    fn patch_means_reject_invalid_buffers_and_too_small_frames() {
        assert!(patch_means(8, 8, &[0; 191]).is_err());
        assert!(patch_means(7, 8, &[0; 168]).is_err());
    }
    #[test]
    fn captures_use_report_location_and_unique_target_names() {
        assert_eq!(
            png_path(Path::new("/tmp/run.json"), 3),
            Path::new("/tmp/run-frame-03s.png")
        );
        assert_ne!(
            png_path(Path::new("/tmp/run.json"), 3),
            png_path(Path::new("/tmp/run.json"), 18)
        );
    }
}
