//! Explicit synthetic motion diagnostic. Opens only the EMEET virtual camera.
use crate::{preview::Preview, virtual_camera::VirtualCamera};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
const OUTPUT: (u32, u32) = (1920, 1080);
const SECONDS: u64 = 12;
#[cfg(test)]
const PERIOD_NS: u64 = 1_000_000_000 / 30;
const MAGIC: u16 = 0x5049;
fn checksum(id: u32) -> u16 {
    let mut crc = 0xffffu16;
    for byte in id.to_be_bytes() {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}
fn marker_word(id: u32) -> u64 {
    (u64::from(MAGIC) << 48) | (u64::from(id) << 16) | u64::from(checksum(id))
}
fn stamp(rgb: &mut [u8], w: u32, h: u32, id: u32) -> Result<(), String> {
    if w < 120 || h < 120 || rgb.len() != w as usize * h as usize * 3 {
        return Err("Buffer de marcador inválido".into());
    }
    let cell = w / 120;
    let origin = cell * 2;
    let word = marker_word(id);
    if origin + cell * 8 > h {
        return Err("Altura insuficiente para marcador".into());
    }
    for bit in 0..64u32 {
        let value = if (word >> (63 - bit)) & 1 == 1 {
            240
        } else {
            16
        };
        let left = origin + (bit % 8) * cell;
        let top = origin + (bit / 8) * cell;
        for y in top..top + cell {
            for x in left..left + cell {
                let at = (y as usize * w as usize + x as usize) * 3;
                rgb[at..at + 3].fill(value);
            }
        }
    }
    Ok(())
}
fn decode(rgb: &[u8], w: u32, h: u32) -> Option<u32> {
    if w < 120 || h < 120 || rgb.len() != w as usize * h as usize * 3 {
        return None;
    }
    let cell = w / 120;
    let origin = cell * 2;
    if origin + cell * 8 > h {
        return None;
    }
    let mut word = 0u64;
    for bit in 0..64u32 {
        let x = origin + (bit % 8) * cell + cell / 2;
        let y = origin + (bit / 8) * cell + cell / 2;
        let at = (y as usize * w as usize + x as usize) * 3;
        let luma = (u32::from(rgb[at]) + u32::from(rgb[at + 1]) + u32::from(rgb[at + 2])) / 3;
        let value = if luma >= 180 {
            1
        } else if luma <= 70 {
            0
        } else {
            return None;
        };
        word = (word << 1) | value;
    }
    if (word >> 48) as u16 != MAGIC {
        return None;
    }
    let id = (word >> 16) as u32;
    (word as u16 == checksum(id)).then_some(id)
}
#[derive(Clone, Debug, Serialize)]
struct Observation {
    arrival_ns: u64,
    callback_ns: u64,
    frame_id: Option<u32>,
    native_count: u64,
    native_last_pts_ns: Option<i64>,
}
#[derive(Clone, Debug, Serialize)]
struct SourceFrame {
    frame_id: u32,
    created_ns: u64,
    submitted_ns: u64,
}
fn percentile(values: &[f64], percent: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    Some(
        sorted[((percent * sorted.len() as f64).ceil() as usize)
            .saturating_sub(1)
            .min(sorted.len() - 1)],
    )
}
fn summary(values: &[f64]) -> Value {
    json!({"count":values.len(),"p50":percentile(values,0.50),"p95":percentile(values,0.95),"p99":percentile(values,0.99),"max":percentile(values,1.0)})
}
fn metrics(
    observed: &[Observation],
    sources: &[SourceFrame],
    seconds: f64,
    target_fps: u32,
) -> Value {
    let period_ns = 1_000_000_000 / u64::from(target_fps);
    let source_map: HashMap<_, _> = sources.iter().map(|s| (s.frame_id, s)).collect();
    let mut seen = HashSet::new();
    let mut duplicates = 0;
    let mut invalid = 0;
    let mut reordered = 0;
    let mut highest = 0;
    let mut ages = Vec::new();
    let mut first_ages = Vec::new();
    let mut unique_gaps = Vec::new();
    let mut arrival_gaps = Vec::new();
    let mut last_unique = None;
    let mut last_arrival = None;
    for frame in observed {
        if let Some(previous) = last_arrival {
            arrival_gaps.push(frame.arrival_ns.saturating_sub(previous) as f64 / 1e6);
        }
        last_arrival = Some(frame.arrival_ns);
        let Some((id, source)) = frame
            .frame_id
            .and_then(|id| source_map.get(&id).map(|s| (id, *s)))
        else {
            invalid += 1;
            continue;
        };
        if frame.arrival_ns < source.created_ns {
            invalid += 1;
            continue;
        }
        let age = (frame.arrival_ns - source.created_ns) as f64 / 1e6;
        ages.push(age);
        if id < highest {
            reordered += 1
        }
        highest = highest.max(id);
        if !seen.insert(id) {
            duplicates += 1;
            continue;
        }
        first_ages.push(age);
        if let Some(previous) = last_unique {
            unique_gaps.push(frame.arrival_ns.saturating_sub(previous) as f64 / 1e6);
        }
        last_unique = Some(frame.arrival_ns);
    }
    let missing: Vec<_> = sources
        .iter()
        .filter(|s| !seen.contains(&s.frame_id))
        .map(|s| s.frame_id)
        .collect();
    let source_gaps: Vec<_> = sources
        .windows(2)
        .map(|p| p[1].created_ns.saturating_sub(p[0].created_ns) as f64 / 1e6)
        .collect();
    let submission_work: Vec<_> = sources
        .iter()
        .map(|s| s.submitted_ns.saturating_sub(s.created_ns) as f64 / 1e6)
        .collect();
    let source_lateness: Vec<_> = sources
        .iter()
        .map(|s| {
            s.created_ns
                .saturating_sub(u64::from(s.frame_id.saturating_sub(1)) * period_ns)
                as f64
                / 1e6
        })
        .collect();
    let unique_fps = seen.len() as f64 / seconds;
    let source_fps = sources.len() as f64 / seconds;
    let loss = missing.len() as f64 / sources.len().max(1) as f64;
    let invalid_ratio = invalid as f64 / observed.len().max(1) as f64;
    let gap_p95 = percentile(&unique_gaps, 0.95);
    let gap_max = percentile(&unique_gaps, 1.0);
    let age_p95 = percentile(&ages, 0.95);
    let age_max = percentile(&ages, 1.0);
    let native_gaps: Vec<_> = observed
        .windows(2)
        .filter_map(
            |pair| match (pair[0].native_last_pts_ns, pair[1].native_last_pts_ns) {
                (Some(a), Some(b)) if b > a => Some((b - a) as f64 / 1e6),
                _ => None,
            },
        )
        .collect();
    let receiver_skipped: u64 = observed
        .windows(2)
        .map(|pair| {
            pair[1]
                .native_count
                .saturating_sub(pair[0].native_count)
                .saturating_sub(1)
        })
        .sum();
    let callback_to_read: Vec<_> = observed
        .iter()
        .map(|frame| frame.arrival_ns.saturating_sub(frame.callback_ns) as f64 / 1e6)
        .collect();
    let unique_gap_jitter: Vec<_> = unique_gaps
        .iter()
        .map(|gap| (gap - 1000.0 / f64::from(target_fps)).abs())
        .collect();
    let minimum_fps = f64::from(target_fps) * 0.95;
    let maximum_p95_gap_ms = (1500.0 / f64::from(target_fps)).max(50.0);
    let passed = seconds > 0.0
        && unique_fps >= minimum_fps
        && source_fps >= minimum_fps
        && loss <= 0.05
        && invalid_ratio <= 0.01
        && reordered == 0
        && gap_p95.is_some_and(|v| v <= maximum_p95_gap_ms)
        && gap_max.is_some_and(|v| v <= 100.0)
        && age_p95.is_some_and(|v| v <= 150.0)
        && age_max.is_some_and(|v| v <= 250.0);
    json!({"passed":passed,"received_rgb_frames":observed.len(),"source_frames":sources.len(),"unique_ids":seen.len(),"unique_update_fps":unique_fps,"source_update_fps":source_fps,"duplicates":duplicates,"invalid_or_unattributed_ids":invalid,"reordered":reordered,"source_ids_not_observed":missing,"source_loss_fraction":loss,"schedule_slots_skipped":sources.last().map(|s|s.frame_id as usize).unwrap_or(0).saturating_sub(sources.len()),"unique_gap_ms":summary(&unique_gaps),"unique_gap_absolute_jitter_ms":summary(&unique_gap_jitter),"native_pts_gap_ms":summary(&native_gaps),"receiver_native_sequences_skipped":receiver_skipped,"callback_to_receiver_read_ms":summary(&callback_to_read),"arrival_gap_ms":summary(&arrival_gaps),"visible_frame_age_ms":summary(&ages),"first_delivery_age_ms":summary(&first_ages),"source_gap_ms":summary(&source_gaps),"source_deadline_lateness_ms":summary(&source_lateness),"source_clone_stamp_submit_ms":summary(&submission_work),"unique_gaps_ms":unique_gaps,"thresholds":{"minimum_unique_and_source_fps":minimum_fps,"maximum_source_loss_fraction":0.05,"maximum_invalid_fraction":0.01,"maximum_p95_unique_gap_ms":maximum_p95_gap_ms,"maximum_worst_unique_gap_ms":100,"maximum_p95_visible_age_ms":150,"maximum_worst_visible_age_ms":250,"reordering_allowed":false},"end_window_note":"Source IDs near the end may still be in transit when the fixed12s receiver window closes; these remain included in reported loss, never silently removed."})
}
#[derive(Default)]
struct FeedState {
    stop: AtomicBool,
    start: Mutex<Option<Instant>>,
    sources: Mutex<Vec<SourceFrame>>,
}
fn base_pattern(w: u32, h: u32) -> Vec<u8> {
    let mut image = vec![0; w as usize * h as usize * 3];
    for y in 0..h {
        for x in 0..w {
            let at = (y as usize * w as usize + x as usize) * 3;
            image[at..at + 3].copy_from_slice(&[
                ((x / 16) % 220 + 20) as u8,
                ((y / 16) % 220 + 20) as u8,
                ((x / 32 + y / 32) % 180 + 40) as u8,
            ]);
        }
    }
    image
}
pub fn run(path: &Path) -> Result<(), String> {
    let mut cases = Vec::new();
    for (w, h, fps) in [(1920, 1080, 30), (3840, 2160, 30), (3840, 2160, 21)] {
        cases.push(case(w, h, fps));
    }
    let passed = cases.iter().all(|case| case["passed"] == true);
    let report = json!({"test":"virtual_motion_fluency","physical_camera_opened":false,"target_fps":30,"seconds_per_case":SECONDS,"cases":cases,"passed":passed,"timing_scope":"RGB, sequence, PTS and callback time are atomically associated; receiver arrival measures completed RGB delivery. RGB IDs, not transport PTS alone, identify processed-frame novelty."});
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if passed {
        Ok(())
    } else {
        Err("Fluência temporal reprovada; consulte métricas e observações no JSON".into())
    }
}
fn case(w: u32, h: u32, fps: u32) -> Value {
    let period_ns = 1_000_000_000 / u64::from(fps);
    let mut producer = match VirtualCamera::start() {
        Ok(p) => p,
        Err(e) => return json!({"input":[w,h],"passed":false,"error":e}),
    };
    let sink = producer.sink();
    let mut consumer = Preview::start_virtual(OUTPUT.0, OUTPUT.1, 30);
    let state = Arc::new(FeedState::default());
    let worker_state = state.clone();
    let feeder = thread::spawn(move || -> Result<(), String> {
        let base = base_pattern(w, h);
        let mut last_id = None;
        while !worker_state.stop.load(Ordering::Acquire) {
            let epoch = *worker_state
                .start
                .lock()
                .map_err(|_| "Mutex do gerador indisponível")?;
            let (id, created_ns) = if let Some(epoch) = epoch {
                let elapsed = epoch.elapsed();
                if elapsed >= Duration::from_secs(SECONDS) {
                    break;
                }
                (
                    (elapsed.as_nanos() / u128::from(period_ns) + 1) as u32,
                    elapsed.as_nanos() as u64,
                )
            } else {
                (0, 0)
            };
            if last_id == Some(id) && id != 0 {
                thread::sleep(Duration::from_millis(1));
                continue;
            }
            let mut rgb = base.clone();
            // A moving bar is visible independently of the machine-readable ID.
            let left = (u64::from(id) * 17 % u64::from(w - 16)) as u32;
            for y in h / 2..h / 2 + 16 {
                for x in left..left + 16 {
                    let at = (y as usize * w as usize + x as usize) * 3;
                    rgb[at..at + 3].copy_from_slice(&[255, 240, 10]);
                }
            }
            stamp(&mut rgb, w, h, id)?;
            if id != 0 {
                worker_state
                    .sources
                    .lock()
                    .map_err(|_| "Mutex de quadros indisponível")?
                    .push(SourceFrame {
                        frame_id: id,
                        created_ns,
                        submitted_ns: 0,
                    });
            }
            sink.submit_rgb(w, h, rgb)?;
            if let Some(epoch) = epoch {
                if let Some(source) = worker_state
                    .sources
                    .lock()
                    .map_err(|_| "Mutex de quadros indisponível")?
                    .last_mut()
                {
                    source.submitted_ns = epoch.elapsed().as_nanos() as u64;
                }
                let deadline = epoch + Duration::from_nanos(u64::from(id) * period_ns);
                let now = Instant::now();
                if deadline > now {
                    thread::sleep(deadline - now);
                }
            } else {
                thread::sleep(Duration::from_millis(33));
            }
            last_id = Some(id);
        }
        Ok(())
    });
    let mut observations = Vec::new();
    let mut native_before = consumer.timing();
    let mut native_after = native_before;
    let mut native_fps = None;
    let operation = (|| -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(35);
        loop {
            if let Some((cw, ch, rgb)) = consumer.latest() {
                if (cw, ch) == OUTPUT && decode(&rgb, cw, ch) == Some(0) {
                    break;
                }
            }
            if consumer.is_finished() {
                return Err(consumer.status());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "Marcador inicial não chegou: {}",
                    consumer.status()
                ));
            }
            thread::sleep(Duration::from_millis(5));
        }
        let start = Instant::now();
        native_before = consumer.timing();
        *state
            .start
            .lock()
            .map_err(|_| "Mutex do gerador indisponível")? = Some(start);
        while start.elapsed() < Duration::from_secs(SECONDS) {
            if consumer.is_finished() {
                return Err(consumer.status());
            }
            if let Some(frame) = consumer.latest_timed() {
                let (cw, ch, rgb) = frame.image;
                if (cw, ch) != OUTPUT {
                    return Err(format!("Resolução inesperada {cw}×{ch}"));
                }
                observations.push(Observation {
                    arrival_ns: start.elapsed().as_nanos() as u64,
                    callback_ns: frame
                        .received_at
                        .saturating_duration_since(start)
                        .as_nanos() as u64,
                    frame_id: decode(&rgb, cw, ch).filter(|id| *id != 0),
                    native_count: frame.sequence,
                    native_last_pts_ns: frame.pts_ns,
                });
            }
            thread::sleep(Duration::from_millis(1));
        }
        native_after = consumer.timing();
        if let (Some(a), Some(b)) = (native_before.last_pts_ns, native_after.last_pts_ns) {
            if b > a {
                native_fps = Some(
                    native_after.frames.saturating_sub(native_before.frames) as f64 * 1e9
                        / (b - a) as f64,
                );
            }
        }
        Ok(())
    })();
    state.stop.store(true, Ordering::Release);
    let feeder_error = match feeder.join() {
        Ok(result) => result.err(),
        Err(_) => Some("Thread do gerador falhou".into()),
    };
    let sources = state
        .sources
        .lock()
        .map(|sources| sources.clone())
        .unwrap_or_default();
    consumer.stop();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !consumer.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    let consumer_stopped = consumer.is_finished();
    let publisher_metrics = producer.metrics();
    let producer_error = producer.stop().err();
    let summary = metrics(&observations, &sources, SECONDS as f64, fps);
    let passed = operation.is_ok()
        && feeder_error.is_none()
        && producer_error.is_none()
        && consumer_stopped
        && native_fps.is_some_and(|fps| (fps - 30.0).abs() <= 1.5)
        && summary["passed"] == true;
    json!({"input":[w,h],"source_target_fps":fps,"output":[OUTPUT.0,OUTPUT.1],"passed":passed,"error":operation.err(),"feeder_error":feeder_error,"producer_shutdown_error":producer_error,"consumer_stopped":consumer_stopped,"publisher_metrics":publisher_metrics,"native_transport_fps":native_fps,"native_before":{"frames":native_before.frames,"pts_ns":native_before.last_pts_ns},"native_after":{"frames":native_after.frames,"pts_ns":native_after.last_pts_ns},"metrics":summary,"sources":sources,"observations":observations})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn marker_roundtrips_extreme_ids_and_detects_corruption() {
        for id in [0, 1, 900, u32::MAX] {
            let mut rgb = vec![80; 1920 * 1080 * 3];
            stamp(&mut rgb, 1920, 1080, id).unwrap();
            assert_eq!(decode(&rgb, 1920, 1080), Some(id));
            for y in 32..48 {
                for x in 32..48 {
                    let at = (y * 1920 + x) * 3;
                    rgb[at..at + 3].fill(240);
                }
            }
            assert_eq!(decode(&rgb, 1920, 1080), None);
        }
    }
    #[test]
    fn duplicated_frames_do_not_masquerade_as_30fps_motion() {
        let observed: (Vec<_>, Vec<_>) = (
            (0..30)
                .map(|i| Observation {
                    arrival_ns: i * PERIOD_NS,
                    callback_ns: i * PERIOD_NS,
                    frame_id: Some((i / 3 + 1) as u32),
                    native_count: i + 1,
                    native_last_pts_ns: Some((i * PERIOD_NS) as i64),
                })
                .collect(),
            (1..=10)
                .map(|id| SourceFrame {
                    frame_id: id,
                    created_ns: u64::from(id - 1) * 3 * PERIOD_NS,
                    submitted_ns: 0,
                })
                .collect(),
        );
        let m = metrics(&observed.0, &observed.1, 1.0, 30);
        assert_eq!(m["unique_ids"], 10);
        assert_eq!(m["duplicates"], 20);
        assert_eq!(m["unique_update_fps"], 10.0);
        assert_eq!(m["passed"], false);
    }
    #[test]
    fn worst_gap_is_preserved_not_averaged_away() {
        let mut obs = Vec::new();
        let mut src = Vec::new();
        for id in 1..=30 {
            let arrival = if id < 16 {
                u64::from(id - 1) * PERIOD_NS
            } else {
                u64::from(id - 1) * PERIOD_NS + 200_000_000
            };
            obs.push(Observation {
                arrival_ns: arrival,
                callback_ns: arrival,
                frame_id: Some(id),
                native_count: u64::from(id),
                native_last_pts_ns: None,
            });
            src.push(SourceFrame {
                frame_id: id,
                created_ns: u64::from(id - 1) * PERIOD_NS,
                submitted_ns: 0,
            });
        }
        let m = metrics(&obs, &src, 1.2, 30);
        assert!(m["unique_gap_ms"]["max"].as_f64().unwrap() > 230.0);
        assert_eq!(m["passed"], false);
    }
    #[test]
    fn honest_21fps_source_can_pass_over_30fps_transport_with_expected_duplicates() {
        let sources: Vec<_> = (1..=21)
            .map(|id| SourceFrame {
                frame_id: id,
                created_ns: u64::from(id - 1) * (1_000_000_000 / 21),
                submitted_ns: u64::from(id - 1) * (1_000_000_000 / 21),
            })
            .collect();
        let observations: Vec<_> = (0..30u64)
            .map(|i| Observation {
                arrival_ns: i * PERIOD_NS,
                callback_ns: i * PERIOD_NS,
                frame_id: Some((i * 21 / 30 + 1) as u32),
                native_count: i + 1,
                native_last_pts_ns: Some((i * PERIOD_NS) as i64),
            })
            .collect();
        let m = metrics(&observations, &sources, 1.0, 21);
        assert_eq!(m["unique_update_fps"], 21.0);
        assert_eq!(m["duplicates"], 9);
        assert_eq!(m["passed"], true);
    }
    #[test]
    fn marker_survives_4k_to_1080p_resampling_and_crc_rejects_changed_id() {
        let mut rgb = vec![80; 3840 * 2160 * 3];
        stamp(&mut rgb, 3840, 2160, 729).unwrap();
        let image = image::RgbImage::from_raw(3840, 2160, rgb).unwrap();
        let resized =
            image::imageops::resize(&image, 1920, 1080, image::imageops::FilterType::Triangle);
        assert_eq!(decode(resized.as_raw(), 1920, 1080), Some(729));
        let mut rgb = resized.into_raw();
        let bit = 20;
        let left = 32 + (bit % 8) * 16;
        let top = 32 + (bit / 8) * 16;
        for y in top..top + 16 {
            for x in left..left + 16 {
                let at = (y * 1920 + x) * 3;
                let value = 255 - rgb[at];
                rgb[at..at + 3].fill(value);
            }
        }
        assert_eq!(decode(&rgb, 1920, 1080), None);
    }
}
