//! Synthetic producer-to-consumer proof for the exact existing EMEET extension.
use crate::{
    model,
    preview::{CaptureTiming, Preview},
    virtual_camera::{consumer_state, raw_consumer_count, ConsumerState, VirtualCamera},
};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
const W: u32 = 1920;
const H: u32 = 1080;
const COLORS: [[u8; 3]; 4] = [[220, 30, 40], [20, 210, 60], [30, 50, 220], [220, 190, 20]];
fn expected(phase: u8) -> [[u8; 3]; 4] {
    std::array::from_fn(|i| COLORS[(i + usize::from(phase)) % 4])
}
fn pattern(w: u32, h: u32, phase: u8) -> Vec<u8> {
    let colors = expected(phase);
    let mut out = vec![0; w as usize * h as usize * 3];
    for y in 0..h {
        for x in 0..w {
            let index = usize::from(x >= w / 2) + 2 * usize::from(y >= h / 2);
            let at = (y as usize * w as usize + x as usize) * 3;
            out[at..at + 3].copy_from_slice(&colors[index]);
        }
    }
    out
}
fn means(w: u32, h: u32, rgb: &[u8]) -> Result<[[f64; 3]; 4], String> {
    if w < 8
        || h < 8
        || (w as usize)
            .checked_mul(h as usize)
            .and_then(|n| n.checked_mul(3))
            != Some(rgb.len())
    {
        return Err("Frame RGB inválido".into());
    }
    let radius = (w.min(h) / 100).clamp(1, 8);
    let mut out = [[0.0; 3]; 4];
    for (i, (cx, cy)) in [
        (w / 4, h / 4),
        (3 * w / 4, h / 4),
        (w / 4, 3 * h / 4),
        (3 * w / 4, 3 * h / 4),
    ]
    .into_iter()
    .enumerate()
    {
        let mut count = 0u32;
        for y in cy - radius..=cy + radius {
            for x in cx - radius..=cx + radius {
                let at = (y as usize * w as usize + x as usize) * 3;
                for c in 0..3 {
                    out[i][c] += f64::from(rgb[at + c]);
                }
                count += 1;
            }
        }
        for value in &mut out[i] {
            *value /= f64::from(count);
        }
    }
    Ok(out)
}
fn matches(means: &[[f64; 3]; 4], phase: u8) -> bool {
    means
        .iter()
        .flatten()
        .zip(expected(phase).iter().flatten())
        .all(|(actual, wanted)| actual.is_finite() && (*actual - f64::from(*wanted)).abs() <= 25.0)
}
fn timing(t: CaptureTiming) -> Value {
    json!({"frames":t.frames,"first_pts_ns":t.first_pts_ns,"last_pts_ns":t.last_pts_ns,"width":t.width,"height":t.height})
}

pub fn run(path: &Path) -> Result<(), String> {
    let mut producer = match VirtualCamera::start() {
        Ok(v) => v,
        Err(error) => {
            model::save_json(
                path,
                &json!({"test":"emeet_virtual_camera_roundtrip","passed":false,"error":error}),
            )?;
            return Err(error);
        }
    };
    let sink = producer.sink();
    let mut consumer = Preview::start_virtual(W, H, 30);
    let stop = Arc::new(AtomicBool::new(false));
    let phase = Arc::new(AtomicU8::new(0));
    let worker_stop = stop.clone();
    let worker_phase = phase.clone();
    let feeder = thread::spawn(move || -> Result<u64, String> {
        let images = [pattern(W, H, 0), pattern(W, H, 1)];
        let mut count = 0;
        let mut deadline = Instant::now();
        while !worker_stop.load(Ordering::Acquire) {
            sink.submit_rgb(
                W,
                H,
                images[usize::from(worker_phase.load(Ordering::Acquire))].clone(),
            )?;
            count += 1;
            deadline += Duration::from_nanos(1_000_000_000 / 30);
            let now = Instant::now();
            if deadline > now {
                thread::sleep(deadline - now);
            } else {
                deadline = now;
            }
        }
        Ok(count)
    });
    let mut matched = [0u64; 2];
    let mut observed = Vec::new();
    let mut begin = None;
    let mut end = None;
    let check = (|| -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(35);
        while Instant::now() < deadline {
            if consumer.is_finished() {
                return Err(consumer.status());
            }
            if let Some((w, h, rgb)) = consumer.latest() {
                if (w, h) != (W, H) {
                    return Err(format!("Extensão entregou {w}×{h}"));
                }
                let sampled = means(w, h, &rgb)?;
                for p in 0..2 {
                    if matches(&sampled, p) {
                        matched[usize::from(p)] += 1;
                        if observed.len() < 2 && (!observed.iter().any(|v: &Value| v["phase"] == p))
                        {
                            observed.push(json!({"phase":p,"patch_rgb_means":sampled}));
                        }
                        if begin.is_none() {
                            begin = Some((Instant::now(), consumer.timing()));
                        }
                    }
                }
            }
            if let Some((started, _)) = begin {
                if started.elapsed() >= Duration::from_millis(1500) {
                    phase.store(1, Ordering::Release);
                }
                if started.elapsed() >= Duration::from_secs(3) && matched.iter().all(|n| *n >= 3) {
                    end = Some(consumer.timing());
                    return Ok(());
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        Err(format!(
            "Sem prova dos dois padrões sintéticos em35segundos; correspondências {matched:?}; {}",
            consumer.status()
        ))
    })();
    stop.store(true, Ordering::Release);
    let produced = feeder
        .join()
        .unwrap_or_else(|_| Err("Falha na thread do produtor".into()));
    let producer_status = producer.status();
    let diagnostics = consumer.diagnostics();
    consumer.stop();
    let shutdown_deadline = Instant::now() + Duration::from_secs(8);
    while !consumer.is_finished() && Instant::now() < shutdown_deadline {
        thread::sleep(Duration::from_millis(20));
    }
    let consumer_stopped = consumer.is_finished();
    let producer_stopped = producer.stop();
    let baseline = begin.map(|(_, t)| t);
    let native_frames = baseline
        .zip(end)
        .map(|(a, b)| b.frames.saturating_sub(a.frames));
    let pts_seconds = baseline
        .zip(end)
        .and_then(|(a, b)| a.last_pts_ns.zip(b.last_pts_ns))
        .and_then(|(a, b)| (b > a).then_some((b - a) as f64 / 1e9));
    let fps = native_frames.zip(pts_seconds).map(|(n, t)| n as f64 / t);
    let cadence = fps.is_some_and(|n| n.is_finite() && (n - 30.0).abs() <= 4.5);
    let passed = check.is_ok()
        && produced.is_ok()
        && consumer_stopped
        && producer_stopped.is_ok()
        && cadence;
    model::save_json(
        path,
        &json!({"test":"emeet_virtual_camera_roundtrip","passed":passed,"target_name":"EMEET STUDIO Virtual Camera","target_uid":"F3B9DD6A-3AA0-45D9-83D5-90A6FC254320","width":W,"height":H,"matched_frames_per_phase":matched,"observed":observed,"produced_frames":produced.as_ref().ok(),"producer_error":produced.as_ref().err(),"capture_error":check.err(),"producer_status":producer_status,"consumer_status":consumer.status(),"consumer_stopped":consumer_stopped,"producer_shutdown_error":producer_stopped.err(),"native_frames":native_frames,"native_pts_seconds":pts_seconds,"native_fps":fps,"cadence_within_15_percent":cadence,"capture_before":baseline.map(timing),"capture_after":end.map(timing),"diagnostics":diagnostics,"physical_camera_opened":false}),
    )?;
    if passed {
        Ok(())
    } else {
        Err(format!(
            "V-Cam não passou na prova de conteúdo/cadência; consulte {}",
            path.display()
        ))
    }
}
fn persistent_colors(tag: u8) -> [[u8; 3]; 4] {
    std::array::from_fn(|i| {
        std::array::from_fn(|channel| {
            ((u32::from(tag) * 37 + i as u32 * 71 + channel as u32 * 53) % 220 + 16) as u8
        })
    })
}
fn persistent_pattern(tag: u8) -> Vec<u8> {
    let colors = persistent_colors(tag);
    let mut rgb = vec![0; W as usize * H as usize * 3];
    for y in 0..H {
        for x in 0..W {
            let quadrant = usize::from(x >= W / 2) + 2 * usize::from(y >= H / 2);
            let offset = (y as usize * W as usize + x as usize) * 3;
            rgb[offset..offset + 3].copy_from_slice(&colors[quadrant]);
        }
    }
    rgb
}
fn exact_colors(actual: &[[f64; 3]; 4], wanted: &[[u8; 3]; 4]) -> bool {
    actual
        .iter()
        .flatten()
        .zip(wanted.iter().flatten())
        .all(|(a, b)| a.is_finite() && (*a - f64::from(*b)).abs() <= 1.0)
}
#[derive(Default)]
struct MonotonicCapture {
    frames: u64,
    pts: Option<i64>,
    observations: u64,
}
impl MonotonicCapture {
    fn observe(&mut self, capture: CaptureTiming) -> Result<bool, String> {
        if capture.frames < self.frames {
            return Err("Contador da mesma sessão reiniciou".into());
        }
        if capture.frames == self.frames {
            return Ok(false);
        }
        let pts = capture.last_pts_ns.ok_or("Frame sem PTS nativo")?;
        if self.pts.is_some_and(|previous| pts <= previous) {
            return Err("PTS da sessão persistente não avançou".into());
        }
        self.frames = capture.frames;
        self.pts = Some(pts);
        self.observations += 1;
        Ok(true)
    }
}
fn raw_count_state() -> ConsumerState {
    match raw_consumer_count() {
        Ok(count) => ConsumerState::Known { count },
        Err(reason) => ConsumerState::Unknown { reason },
    }
}
fn wait_consumer_count(wanted: u32) -> ConsumerState {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let observed = raw_count_state();
        if observed == (ConsumerState::Known { count: wanted }) || Instant::now() >= deadline {
            return observed;
        }
        thread::sleep(Duration::from_millis(50));
    }
}
fn wait_fresh_consumer_count(wanted: u32) -> ConsumerState {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let observed = consumer_state();
        if observed == (ConsumerState::Known { count: wanted }) || Instant::now() >= deadline {
            return observed;
        }
        thread::sleep(Duration::from_millis(50));
    }
}
fn persistent_stage(
    consumer: &Preview,
    tracker: &mut MonotonicCapture,
    wanted: [[u8; 3]; 4],
    cycle: usize,
    label: &str,
) -> Result<Value, String> {
    let started = Instant::now();
    let before = consumer.timing();
    let mut matched = 0;
    let mut first_match_ms = None;
    while started.elapsed() < Duration::from_secs(8) {
        if consumer.is_finished() {
            return Err(consumer.status());
        }
        let capture = consumer.timing();
        let advanced = tracker.observe(capture)?;
        if let Some((w, h, rgb)) = consumer.latest() {
            if (w, h) != (W, H) {
                return Err(format!("Formato inesperado {w}×{h}"));
            }
            let sampled = means(w, h, &rgb)?;
            if advanced && exact_colors(&sampled, &wanted) {
                matched += 1;
                first_match_ms.get_or_insert(started.elapsed().as_millis());
                if matched >= 3 {
                    return Ok(
                        json!({"cycle":cycle,"stage":label,"matched_distinct_observations":matched,
                        "first_match_ms":first_match_ms,"elapsed_ms":started.elapsed().as_millis(),
                        "expected":wanted,"observed":sampled,"before":timing(before),"after":timing(capture)}),
                    );
                }
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err(format!(
        "Ciclo{cycle} {label}: não recebeu três padrões atuais em8s; correspondências{matched}; {}",
        consumer.status()
    ))
}
/// Ten pause/resume cycles through one producer and ONE unchanged native consumer.
/// Explicit diagnostic only; it never opens the physical Pixy or changes controls.
pub fn persistent_session(path: &Path) -> Result<(), String> {
    let baseline = raw_count_state();
    let external_baseline = consumer_state();
    let baseline_count = match baseline {
        ConsumerState::Known { count } => Some(count),
        _ => None,
    };
    let mut producer = match VirtualCamera::start() {
        Ok(producer) => producer,
        Err(error) => {
            model::save_json(
                path,
                &json!({"test":"persistent_virtual_content","passed":false,"baseline":baseline,"error":error}),
            )?;
            return Err(error);
        }
    };
    producer.set_placeholder(true);
    let producer_alone = baseline_count.map_or_else(raw_count_state, wait_consumer_count);
    let external_producer_alone = consumer_state();
    let fresh_baseline_count = match external_baseline {
        ConsumerState::Known { count } => Some(count),
        _ => None,
    };
    // This is the only consumer constructor in this diagnostic. It is never
    // replaced, reconnected or stopped between stages.
    let mut consumer = Preview::start_virtual(W, H, 30);
    let stop = Arc::new(AtomicBool::new(false));
    let phase = Arc::new(AtomicU8::new(1));
    let worker_stop = stop.clone();
    let worker_phase = phase.clone();
    let sink = producer.sink();
    let feeder = thread::spawn(move || -> Result<u64, String> {
        let mut current_tag = 0;
        let mut image = Vec::new();
        let mut count = 0;
        let mut deadline = Instant::now();
        while !worker_stop.load(Ordering::Acquire) {
            let tag = worker_phase.load(Ordering::Acquire);
            if current_tag != tag {
                image = persistent_pattern(tag);
                current_tag = tag;
            }
            sink.submit_rgb(W, H, image.clone())?;
            count += 1;
            deadline += Duration::from_nanos(1_000_000_000 / 30);
            let now = Instant::now();
            if deadline > now {
                thread::sleep(deadline - now);
            } else {
                deadline = now;
            }
        }
        Ok(count)
    });
    let after_open = baseline_count.map_or_else(raw_count_state, |count| {
        wait_consumer_count(count.saturating_add(1))
    });
    let fresh_after_open = fresh_baseline_count.map_or_else(consumer_state, |count| {
        wait_fresh_consumer_count(count.saturating_add(1))
    });
    let mut tracker = MonotonicCapture::default();
    let mut stages = Vec::new();
    let checked = (|| -> Result<(), String> {
        stages.push(persistent_stage(
            &consumer,
            &mut tracker,
            [[28; 3]; 4],
            0,
            "initial_placeholder",
        )?);
        for cycle in 0..10usize {
            let first = (cycle * 2 + 1) as u8;
            phase.store(first, Ordering::Release);
            producer.set_placeholder(false);
            stages.push(persistent_stage(
                &consumer,
                &mut tracker,
                persistent_colors(first),
                cycle + 1,
                "live_before_pause",
            )?);
            producer.set_placeholder(true);
            stages.push(persistent_stage(
                &consumer,
                &mut tracker,
                [[28; 3]; 4],
                cycle + 1,
                "neutral_while_paused",
            )?);
            let second = first + 1;
            phase.store(second, Ordering::Release);
            producer.set_placeholder(false);
            stages.push(persistent_stage(
                &consumer,
                &mut tracker,
                persistent_colors(second),
                cycle + 1,
                "fresh_after_resume",
            )?);
        }
        Ok(())
    })();
    let capture_after = consumer.timing();
    let diagnostics = consumer.diagnostics();
    let before_stop_count = raw_count_state();
    let external_same_process_consumer = consumer_state();
    consumer.stop();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !consumer.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    let consumer_stopped = consumer.is_finished();
    let after_stop = baseline_count.map_or_else(raw_count_state, wait_consumer_count);
    let external_after_stop =
        fresh_baseline_count.map_or_else(consumer_state, wait_fresh_consumer_count);
    stop.store(true, Ordering::Release);
    let submitted = feeder
        .join()
        .unwrap_or_else(|_| Err("Falha no alimentador sintético".into()));
    let producer_status = producer.status();
    let producer_stopped = producer.stop();
    let source_starts = diagnostics
        .iter()
        .filter(|line| line.contains("after_start: active="))
        .count();
    // The long-lived CMIO client cached scnt across transitions. Retain that
    // failure alongside the fresh-process replacement instead of hiding it.
    let raw_counter_verified = baseline_count.is_some_and(|count| {
        producer_alone == baseline
            && after_open
                == (ConsumerState::Known {
                    count: count.saturating_add(1),
                })
            && after_stop == baseline
    });
    let content_passed = checked.is_ok() && stages.len() == 31 && source_starts == 1;
    let fresh_counter_verified = fresh_baseline_count.is_some_and(|count| {
        external_producer_alone == external_baseline
            && fresh_after_open
                == (ConsumerState::Known {
                    count: count.saturating_add(1),
                })
            && external_after_stop == external_baseline
    });
    let passed = content_passed
        && fresh_counter_verified
        && consumer_stopped
        && submitted.is_ok()
        && producer_stopped.is_ok();
    model::save_json(
        path,
        &json!({"test":"persistent_virtual_content","passed":passed,"cycles":10,
        "completed_stages":stages.len(),"stages":stages,"consumer_baseline":baseline,
        "content_passed":content_passed,"fresh_counter_verified":fresh_counter_verified,
        "fresh_consumer_after_open":fresh_after_open,"legacy_scnt_verified":raw_counter_verified,
        "legacy_scnt_error":if raw_counter_verified {None} else {Some("Installed scnt property failed the independently observed consumer transition; never use zero as idle")},
        "external_baseline":external_baseline,"external_producer_alone":external_producer_alone,
        "external_same_process_consumer":external_same_process_consumer,"external_after_stop":external_after_stop,
        "external_other_process_transition_tested":false,
        "producer_alone":producer_alone,"consumer_after_open":after_open,"consumer_before_stop":before_stop_count,
        "consumer_after_stop":after_stop,"capture_error":checked.as_ref().err(),"consumer_stopped":consumer_stopped,
        "submitted_frames":submitted.as_ref().ok(),"feeder_error":submitted.as_ref().err(),
        "producer_shutdown_error":producer_stopped.as_ref().err(),"producer_status":producer_status,
        "native_pts_monotonic":checked.is_ok(),"native_timing_observations":tracker.observations,
        "capture_after":timing(capture_after),"source_start_trace_count":source_starts,
        "consumer_constructor_calls":1,"consumer_reconnects":0,"producer_constructor_calls":1,
        "diagnostics":diagnostics,"physical_camera_opened":false,"pixel_channel_tolerance":1,
        "limits":"Native CMIO/AVFoundation consumer only; does not establish Teams reconnection or physical privacy/mode integration. Timing snapshots are not atomic with the sampled RGB slot."}),
    )?;
    if passed {
        Ok(())
    } else {
        Err(format!(
            "Sessão virtual persistente não passou; consulte {}",
            path.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_restart_pattern_is_distinct_from_all_old_patterns_and_placeholder() {
        for tag in 1..=20 {
            let actual = persistent_colors(tag).map(|pixel| pixel.map(f64::from));
            assert!(exact_colors(&actual, &persistent_colors(tag)));
            assert!(!exact_colors(&actual, &[[28; 3]; 4]));
            for previous in 1..tag {
                assert!(!exact_colors(&actual, &persistent_colors(previous)));
            }
        }
        assert!(!exact_colors(&[[f64::NAN; 3]; 4], &persistent_colors(1)));
    }
    #[test]
    fn persistent_timing_rejects_reconnect_reset_and_nonadvancing_pts() {
        let mut tracker = MonotonicCapture::default();
        assert!(!tracker.observe(CaptureTiming::default()).unwrap());
        let first = CaptureTiming {
            frames: 10,
            last_pts_ns: Some(100),
            ..Default::default()
        };
        assert!(tracker.observe(first).unwrap());
        assert!(!tracker.observe(first).unwrap());
        assert!(tracker
            .observe(CaptureTiming {
                frames: 11,
                last_pts_ns: Some(100),
                ..first
            })
            .is_err());
        assert!(tracker
            .observe(CaptureTiming {
                frames: 1,
                last_pts_ns: Some(110),
                ..first
            })
            .is_err());
        assert!(tracker
            .observe(CaptureTiming {
                frames: 11,
                last_pts_ns: None,
                ..first
            })
            .is_err());
        assert!(tracker
            .observe(CaptureTiming {
                frames: 12,
                last_pts_ns: Some(200),
                ..first
            })
            .unwrap());
        assert_eq!(tracker.observations, 2);
    }
    #[test]
    fn known_quadrants_are_distinct_and_phase_order_is_verified() {
        let a = means(8, 8, &pattern(8, 8, 0)).unwrap();
        assert!(matches(&a, 0));
        assert!(!matches(&a, 1));
        let b = means(8, 8, &pattern(8, 8, 1)).unwrap();
        assert!(matches(&b, 1));
        assert!(!matches(&b, 0));
        assert!(!matches(&[[255.0; 3]; 4], 0));
    }
    #[test]
    fn incomplete_or_nonfinite_observations_are_never_success() {
        assert!(means(8, 8, &[0; 191]).is_err());
        assert!(means(u32::MAX, u32::MAX, &[]).is_err());
        assert!(!matches(&[[f64::NAN; 3]; 4], 0));
    }
}
