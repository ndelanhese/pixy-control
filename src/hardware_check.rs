//! Explicit, reversible tests against the connected Pixy. Never called by unit
//! tests or normal application startup; the caller must select --hardware-test.
use crate::{
    audio,
    hid::{self, commands as h, PixyHid},
    uvc::{ControlId, ControlState, UvcCamera},
};
use serde_json::{json, Value};
use std::{
    thread,
    time::{Duration, Instant},
};

pub fn run() -> Value {
    let mut checks = Vec::new();
    let usb = UvcCamera::open();
    let mut vendor = PixyHid::connect();
    let before = json!({
        "uvc": usb.as_ref().map(|camera| camera.controls()).map_err(Clone::clone),
        "audio": audio::state(),
        "mode": vendor.as_mut().map_err(|error| error.clone()).and_then(|camera| camera.get_byte(h::GET_DEVICE_MODE)),
    });
    match &usb {
        Ok(camera) => {
            for id in [
                ControlId::Brightness,
                ControlId::Contrast,
                ControlId::Saturation,
                ControlId::Hue,
                ControlId::Sharpness,
                ControlId::Gamma,
                ControlId::Gain,
                ControlId::Zoom,
                ControlId::PowerLineFrequency,
                ControlId::BacklightCompensation,
            ] {
                checks.push(uvc_scalar(camera, id));
            }
            checks.push(uvc_manual(
                camera,
                ControlId::WhiteBalance,
                ControlId::WhiteBalanceAuto,
                0,
            ));
            checks.push(uvc_manual(
                camera,
                ControlId::Focus,
                ControlId::FocusAuto,
                0,
            ));
            checks.push(uvc_manual(
                camera,
                ControlId::Exposure,
                ControlId::ExposureAuto,
                1,
            ));
        }
        Err(error) => checks.push(failed("uvc_connection", error)),
    }
    match &mut vendor {
        Ok(camera) => {
            for (name, get, set, query) in [
                (
                    "device_mode_same_value",
                    h::GET_DEVICE_MODE,
                    h::SET_DEVICE_MODE,
                    vec![],
                ),
                (
                    "gesture_palm_same_value",
                    h::GET_GESTURE,
                    h::SET_GESTURE,
                    vec![hid::GESTURE_PALM],
                ),
                (
                    "gesture_yes_same_value",
                    h::GET_GESTURE,
                    h::SET_GESTURE,
                    vec![hid::GESTURE_YES],
                ),
                (
                    "horizontal_mirror_same_value",
                    h::GET_REVERSE,
                    h::SET_REVERSE,
                    vec![hid::REVERSE_HORIZONTAL],
                ),
                (
                    "vertical_reverse_same_value",
                    h::GET_REVERSE,
                    h::SET_REVERSE,
                    vec![hid::REVERSE_VERTICAL],
                ),
                (
                    "auto_rotate_same_value",
                    h::GET_REVERSE,
                    h::SET_REVERSE,
                    vec![hid::REVERSE_AUTO_ROTATE],
                ),
                (
                    "audio_mode_same_value",
                    h::GET_MUSIC_MODE,
                    h::SET_MUSIC_MODE,
                    vec![],
                ),
                (
                    "privacy_timer_same_value",
                    h::GET_PRIVACY_TIME,
                    h::SET_PRIVACY_TIME,
                    vec![],
                ),
                (
                    "white_balance_lock_same_value",
                    h::GET_WB_LOCK,
                    h::SET_WB_LOCK,
                    vec![],
                ),
                (
                    "focus_lock_same_value",
                    h::GET_FOCUS_LOCK,
                    h::SET_FOCUS_LOCK,
                    vec![],
                ),
                ("osd_same_value", h::GET_OSD, h::SET_OSD, vec![]),
            ] {
                checks.push(hid_same_value(camera, name, get, set, &query));
            }
            for axis in [hid::MOTOR_YAW, hid::MOTOR_PITCH] {
                checks.push(motor(camera, axis));
            }
        }
        Err(error) => checks.push(failed("hid_connection", error)),
    }
    checks.push(audio_mute());
    checks.push(audio_volume());
    let after = json!({
        "uvc": usb.as_ref().map(|camera| camera.controls()).map_err(Clone::clone),
        "audio": audio::state(),
        "mode": vendor.as_mut().map_err(|error| error.clone()).and_then(|camera| camera.get_byte(h::GET_DEVICE_MODE)),
    });
    let passed = checks
        .iter()
        .filter(|check| check["status"] == "passed")
        .count();
    let failed = checks
        .iter()
        .filter(|check| check["status"] == "failed")
        .count();
    let skipped = checks
        .iter()
        .filter(|check| check["status"] == "skipped")
        .count();
    let restoration_complete = checks.iter().all(|check| check["restored"] != false);
    json!({"test": "explicit_connected_pixy_reversible_checks", "before": before, "after": after,
        "checks": checks, "summary": {"passed": passed, "failed": failed, "skipped": skipped,
            "restoration_complete": restoration_complete},
        "limitations": ["Readback confirms device target/current fields, not optical image quality or calibrated motor angle. Relative motor units are not certified physical degrees.",
            "On the tested Pixy firmware, brightness, exposure-mode and powerline changes require an active video stream; idle attempts correctly fail readback.",
            "Autofocus/exposure values can resume changing after their original automatic modes are restored.",
            "Factory reset, firmware, startup position, and hardware preset writes are excluded."]})
}

fn failed(name: &str, error: &str) -> Value {
    json!({"name": name, "status": "failed", "error": error, "restored": true, "mutation_attempted": false})
}

fn skipped(name: &str, reason: &str) -> Value {
    json!({"name": name, "status": "skipped", "reason": reason, "restored": true, "mutation_attempted": false})
}

fn control(camera: &UvcCamera, id: ControlId) -> Result<ControlState, String> {
    camera
        .controls()
        .into_iter()
        .find(|control| control.id == id)
        .ok_or_else(|| format!("Controle {id:?} não anunciado"))
}

fn value(camera: &UvcCamera, id: ControlId) -> Result<i64, String> {
    let control = control(camera, id)?;
    control.current.ok_or_else(|| {
        control
            .error
            .unwrap_or_else(|| format!("{id:?}: leitura ausente"))
    })
}

fn next_value(control: &ControlState) -> Result<i64, String> {
    let original = control.current.ok_or("Leitura ausente")?;
    let min = control.min.ok_or("Mínimo ausente")?;
    let max = control.max.ok_or("Máximo ausente")?;
    let step = control.step.unwrap_or(1).max(1);
    if original.checked_add(step).is_some_and(|value| value <= max) {
        Ok(original + step)
    } else if original.checked_sub(step).is_some_and(|value| value >= min) {
        Ok(original - step)
    } else {
        Err("Controle não oferece outro valor reversível".into())
    }
}

fn expect_value(camera: &UvcCamera, id: ControlId, expected: i64) -> Result<i64, String> {
    let actual = value(camera, id)?;
    if actual == expected {
        Ok(actual)
    } else {
        Err(format!(
            "{id:?}: solicitado {expected}, câmera devolveu {actual}"
        ))
    }
}

fn uvc_scalar(camera: &UvcCamera, id: ControlId) -> Value {
    let name = format!("uvc_{id:?}");
    let original = match control(camera, id) {
        Ok(control) => control,
        Err(error) => return skipped(&name, &error),
    };
    let requested = match next_value(&original) {
        Ok(value) => value,
        Err(error) => return skipped(&name, &error),
    };
    let old = original.current.unwrap_or_default();
    let mut readback = None;
    let operation: Result<(), String> = (|| {
        camera.set(id, requested)?;
        readback = Some(expect_value(camera, id, requested)?);
        Ok(())
    })();
    // Unconditional restore: even a failed SET may have reached the hardware.
    let restore = camera
        .set(id, old)
        .and_then(|_| expect_value(camera, id, old));
    json!({"name": name, "before": old, "requested": requested, "readback": readback,
        "restore_readback": restore.as_ref().ok(), "restored": restore.is_ok(),
        "status": if operation.is_ok() && restore.is_ok() {"passed"} else {"failed"},
        "error": operation.err(), "restore_error": restore.err()})
}

fn uvc_manual(camera: &UvcCamera, id: ControlId, auto: ControlId, manual: i64) -> Value {
    let name = format!("uvc_{id:?}_manual_auto");
    let original = match control(camera, id) {
        Ok(control) => control,
        Err(error) => return skipped(&name, &error),
    };
    let original_mode = match value(camera, auto) {
        Ok(value) => value,
        Err(error) => return skipped(&name, &error),
    };
    let requested = match next_value(&original) {
        Ok(value) => value,
        Err(error) => return skipped(&name, &error),
    };
    let original_value = original.current.unwrap_or_default();
    let mut readback = None;
    let operation: Result<(), String> = (|| {
        camera.set(auto, manual)?;
        expect_value(camera, auto, manual)?;
        camera.set(id, requested)?;
        readback = Some(expect_value(camera, id, requested)?);
        Ok(())
    })();
    // Each restore step is attempted even when preceding restore steps fail.
    let restore_manual_mode = camera.set(auto, manual);
    let restore_value = camera
        .set(id, original_value)
        .and_then(|_| expect_value(camera, id, original_value));
    let restore_mode = camera
        .set(auto, original_mode)
        .and_then(|_| expect_value(camera, auto, original_mode));
    // The restoration claim concerns the original value and original mode,
    // both read back above. A failed intermediate attempt to enter Manual is
    // retained below but does not negate verified restoration of those originals.
    let restored = restore_value.is_ok() && restore_mode.is_ok();
    json!({"name": name, "before": {"mode": original_mode, "value": original_value},
        "requested": {"mode": manual, "value": requested}, "readback": readback,
        "restore_readback": {"value_before_auto_resumes": restore_value.as_ref().ok(), "mode": restore_mode.as_ref().ok()},
        "restored": restored, "status": if operation.is_ok() && restored {"passed"} else {"failed"},
        "error": operation.err(), "restore_errors": [restore_manual_mode.err(), restore_value.err(), restore_mode.err()]})
}

fn hid_same_value(
    camera: &mut PixyHid,
    name: &str,
    get: [u8; 4],
    set: [u8; 4],
    query: &[u8],
) -> Value {
    let before = match camera.exchange(get, query) {
        Ok(value) => value,
        Err(error) => return skipped(name, &error),
    };
    if before.is_empty() {
        return skipped(name, "Resposta de estado vazia");
    }
    let mut payload = before.clone();
    let mode = get == h::GET_DEVICE_MODE;
    if mode {
        payload[0] = hid::normalize_device_mode(payload[0]);
    }
    let expected_ack = if set == h::SET_GESTURE || set == h::SET_REVERSE {
        vec![query[0], 0x20]
    } else {
        vec![0x20]
    };
    let mut ack = None;
    let mut after = None;
    let operation: Result<(), String> = (|| {
        let actual = camera.exchange(set, &payload)?;
        ack = Some(actual.clone());
        if actual != expected_ack {
            return Err(format!("ACK inesperado: {actual:02x?}"));
        }
        let actual = camera.exchange(get, query)?;
        after = Some(actual.clone());
        if equivalent(&actual, &before, mode) {
            Ok(())
        } else {
            Err("GET após SET não manteve o estado original".into())
        }
    })();
    // A same-value command already requests the original state. Retry restore
    // only on failure and verify through GET, including firmware mode3->Manual.
    let restore: Result<Vec<u8>, String> = if operation.is_ok() {
        Ok(after.clone().unwrap_or_default())
    } else {
        let _ = camera.exchange(set, &payload);
        camera.exchange(get, query).and_then(|actual| {
            if equivalent(&actual, &before, mode) {
                Ok(actual)
            } else {
                Err("Estado HID original não confirmado".into())
            }
        })
    };
    json!({"name": name, "before": before, "requested": payload, "ack": ack, "readback": after,
        "restore_readback": restore.as_ref().ok(), "restored": restore.is_ok(),
        "status": if operation.is_ok() && restore.is_ok() {"passed"} else {"failed"},
        "error": operation.err(), "restore_error": restore.err()})
}

fn equivalent(actual: &[u8], before: &[u8], mode: bool) -> bool {
    if mode && actual.len() == 1 && before.len() == 1 {
        hid::normalize_device_mode(actual[0]) == hid::normalize_device_mode(before[0])
    } else {
        actual == before
    }
}

const MOTOR_SETTLE_TIMEOUT: Duration = Duration::from_secs(2);
const MOTOR_POLL_INTERVAL: Duration = Duration::from_millis(50);
const MOTOR_POSITION_TOLERANCE: f32 = 0.1;

struct MotorSettling {
    actual: Option<f32>,
    reported_target: Option<f32>,
    elapsed_ms: u64,
    trace: Vec<Value>,
    error: Option<String>,
}
impl MotorSettling {
    fn outcome(&self) -> Result<(), String> {
        self.error.clone().map_or(Ok(()), Err)
    }
}

fn settle_motor(camera: &mut PixyHid, axis: u8, target: f32) -> MotorSettling {
    let started = Instant::now();
    poll_motor_position(
        target,
        || camera.motor_positions(axis),
        || started.elapsed(),
        thread::sleep,
    )
}

/// Read the actual encoder position until it reaches the original strict
/// tolerance. A fixed delay cannot distinguish a slow motor from a failed move.
fn poll_motor_position(
    target: f32,
    mut read: impl FnMut() -> Result<hid::MotorPosition, String>,
    mut elapsed: impl FnMut() -> Duration,
    mut wait: impl FnMut(Duration),
) -> MotorSettling {
    let mut result = MotorSettling {
        actual: None,
        reported_target: None,
        elapsed_ms: 0,
        trace: Vec::new(),
        error: None,
    };
    loop {
        let observation = read();
        let now = elapsed();
        result.elapsed_ms = now.as_millis().min(u128::from(u64::MAX)) as u64;
        match observation {
            Ok(position) if position.target.is_finite() && position.current.is_finite() => {
                result.actual = Some(position.current);
                result.reported_target = Some(position.target);
                result.trace.push(json!({"elapsed_ms": result.elapsed_ms,
                    "target": position.target, "current": position.current, "actual": position.current}));
                if (position.current - target).abs() <= MOTOR_POSITION_TOLERANCE
                    && now <= MOTOR_SETTLE_TIMEOUT
                {
                    return result;
                }
            }
            Ok(_) => {
                let error = "Leitura do motor não finita".to_string();
                result
                    .trace
                    .push(json!({"elapsed_ms": result.elapsed_ms, "error": error}));
                result.error = Some(error);
                return result;
            }
            Err(error) => {
                result
                    .trace
                    .push(json!({"elapsed_ms": result.elapsed_ms, "error": error}));
                result.error = Some(error);
                return result;
            }
        }
        if now >= MOTOR_SETTLE_TIMEOUT {
            result.error = Some(format!(
                "Alvo esperado {target}, câmera devolveu {:?} após {} ms; tolerância mantida em ±{}",
                result.actual, result.elapsed_ms, MOTOR_POSITION_TOLERANCE
            ));
            return result;
        }
        wait(MOTOR_POLL_INTERVAL.min(MOTOR_SETTLE_TIMEOUT.saturating_sub(now)));
    }
}

fn position_json(position: hid::MotorPosition) -> Value {
    json!({"target": position.target, "current": position.current})
}
fn target_matches(position: hid::MotorPosition, expected: f32) -> bool {
    (position.target - expected).abs() <= MOTOR_POSITION_TOLERANCE
}
fn accuracy_observation(settling: &MotorSettling, target: f32) -> Value {
    json!({"reported_target": settling.reported_target, "reported_current": settling.actual,
        "target": target, "current_minus_target": settling.actual.map(|actual| actual-target),
        "within_tolerance": settling.outcome().is_ok(), "tolerance": MOTOR_POSITION_TOLERANCE,
        "elapsed_ms": settling.elapsed_ms, "trace": settling.trace, "observation_error": settling.error,
        "interpretation": "Firmware current/target floats only; physical degree accuracy is not calibrated or certified."})
}
fn motor(camera: &mut PixyHid, axis: u8) -> Value {
    let name = format!("motor_axis_{axis}_protocol_move_and_target_return");
    match camera.get_byte(h::GET_DEVICE_MODE) {
        Ok(mode) if hid::normalize_device_mode(mode) == 0 => (),
        Ok(_) => {
            return skipped(
                &name,
                "Movimento pequeno só é executado quando a câmera já está em modo Manual",
            )
        }
        Err(error) => return skipped(&name, &error),
    }
    let before = match camera.motor_positions(axis) {
        Ok(position) if position.target.abs() <= 149.0 && position.current.abs() <= 149.0 => {
            position
        }
        Ok(_) => return skipped(&name, "Posição fora da faixa conservadora de teste"),
        Err(error) => return skipped(&name, &error),
    };
    let delta = if before.current >= 0.0 { -1.0 } else { 1.0 };
    // Relative requests are based on CURRENT, not the previous target. The two
    // fields can differ at rest, as the native wire readback explicitly shows.
    let expected_target = before.current + delta;
    let mut readback = None;
    let operation: Result<(), String> = (|| {
        camera.move_relative(axis, delta)?;
        let observed = camera.motor_positions(axis)?;
        readback = Some(observed);
        if !target_matches(observed, expected_target) {
            return Err(format!(
                "Alvo relativo esperado {expected_target}, alvo reportado {} (atual {})",
                observed.target, observed.current
            ));
        }
        Ok(())
    })();
    // Mechanical settling is measured and retained independently. An encoder
    // accuracy observation must not be confused with wire-command correctness.
    let movement = settle_motor(camera, axis, readback.map_or(expected_target, |p| p.target));
    // Restore ORIGINAL TARGET, not current. Replacing the original target with
    // the measured current would silently change the user's commanded position.
    let restore_set = camera
        .set_motor_position(axis, before.target)
        .and_then(|ack| {
            if ack == [axis, 0x20] {
                Ok(())
            } else {
                Err(format!("ACK de retorno inválido: {ack:02x?}"))
            }
        });
    let restore_readback = camera.motor_positions(axis);
    let restored_target = restore_readback
        .as_ref()
        .is_ok_and(|position| target_matches(*position, before.target));
    let restore = settle_motor(camera, axis, before.target);
    let restored = restore_set.is_ok() && restored_target;
    json!({"name": name, "criterion": "relative target ACK/readback and original target restoration; encoder accuracy reported separately",
        "units": "native motor protocol units; physical degrees unproven",
        "before": position_json(before), "requested_delta": delta, "expected_target": expected_target,
        "readback": readback.map(position_json),
        "target_error": readback.map(|position| position.target-expected_target),
        "target_tolerance": MOTOR_POSITION_TOLERANCE,
        "movement_direction_observed": movement.actual.map(|current| (current-before.current)*delta>0.0),
        "accuracy_observation": accuracy_observation(&movement, readback.map_or(expected_target, |p| p.target)),
        "restore_requested_target": before.target,
        "restore_readback": restore_readback.as_ref().ok().copied().map(position_json),
        "restore_target_confirmed": restored_target,
        "restore_accuracy_observation": accuracy_observation(&restore, before.target),
        "restore_current_minus_original_current": restore.actual.map(|actual|actual-before.current),
        "restored": restored, "restoration_scope": "original commanded target, not a claim of calibrated mechanical angle",
        "status": if operation.is_ok() && restored {"passed"} else {"failed"},
        "error": operation.err(), "restore_errors": [restore_set.err(), restore_readback.err()]})
}

fn audio_mute() -> Value {
    let name = "coreaudio_mute_toggle_return";
    let before = match audio::state() {
        Ok(state) => state,
        Err(error) => return skipped(name, &error),
    };
    let original = match before.muted {
        Some(value) => value,
        None => return skipped(name, "Mute não disponível"),
    };
    let mut readback = None;
    let operation: Result<(), String> = (|| {
        audio::set_mute(!original)?;
        readback = audio::state()?.muted;
        if readback == Some(!original) {
            Ok(())
        } else {
            Err("Microfone não confirmou mute invertido".into())
        }
    })();
    let restore_set = audio::set_mute(original);
    let restore = audio::state();
    let restored = restore_set.is_ok()
        && restore
            .as_ref()
            .is_ok_and(|state| state.muted == Some(original));
    json!({"name": name, "before": before, "requested": !original, "readback": readback,
        "restore_readback": restore.as_ref().ok(), "restored": restored,
        "status": if operation.is_ok() && restored {"passed"} else {"failed"},
        "error": operation.err(), "restore_errors": [restore_set.err(), restore.err()]})
}

fn audio_volume() -> Value {
    let name = "coreaudio_volume_change_return";
    let before = match audio::state() {
        Ok(state) => state,
        Err(error) => return skipped(name, &error),
    };
    let original = match before.volume {
        Some(value) => value,
        None => return skipped(name, "Volume não disponível"),
    };
    // Live Pixy UAC reports 0..10 dB in 1 dB steps; CoreAudio scalar0.85
    // becomes0.9. Exercise one observable0.1 step rather than expecting an
    // unsupported half-step to survive hardware quantization.
    let requested = if original <= 0.89 {
        original + 0.1
    } else {
        original - 0.1
    };
    let mut readback = None;
    let operation: Result<(), String> = (|| {
        audio::set_volume_scalar(requested)?;
        readback = audio::state()?.volume;
        if readback.is_some_and(|value| {
            (value - requested).abs() <= 0.03 && (value - original).abs() >= 0.01
        }) {
            Ok(())
        } else {
            Err("Microfone não confirmou volume alterado dentro da quantização esperada".into())
        }
    })();
    let restore_set = audio::set_volume_scalar(original);
    let restore = audio::state();
    let restored = restore_set.is_ok()
        && restore.as_ref().is_ok_and(|state| {
            state
                .volume
                .is_some_and(|value| (value - original).abs() <= 0.001)
        });
    json!({"name": name, "before": before, "requested": requested, "readback": readback,
        "restore_readback": restore.as_ref().ok(), "restored": restored,
        "status": if operation.is_ok() && restored {"passed"} else {"failed"},
        "error": operation.err(), "restore_errors": [restore_set.err(), restore.err()]})
}

#[cfg(test)]
mod motor_poll_tests {
    use super::*;
    use std::{cell::Cell, collections::VecDeque, rc::Rc};
    fn simulated(values: impl IntoIterator<Item = Result<f32, String>>) -> MotorSettling {
        let clock = Rc::new(Cell::new(Duration::ZERO));
        let now = clock.clone();
        let mut values: VecDeque<_> = values.into_iter().collect();
        let last = values.back().unwrap().clone();
        poll_motor_position(
            -1.0,
            || {
                values
                    .pop_front()
                    .unwrap_or_else(|| last.clone())
                    .map(|current| hid::MotorPosition {
                        target: -1.0,
                        current,
                    })
            },
            move || now.get(),
            move |delay| clock.set(clock.get() + delay),
        )
    }
    #[test]
    fn protocol_target_is_checked_independently_of_mechanical_current() {
        let before = hid::MotorPosition {
            target: 0.0,
            current: 0.112012125,
        };
        let expected = before.current - 1.0;
        let observed = hid::MotorPosition {
            target: expected,
            current: expected + 0.13,
        };
        assert!(target_matches(observed, expected));
        assert!((observed.current - expected).abs() > MOTOR_POSITION_TOLERANCE);
        assert!(!target_matches(
            hid::MotorPosition {
                target: -1.0,
                current: -1.0
            },
            expected
        ));
        assert_eq!(before.target, 0.0); // restoration must preserve this, not +0.112.
    }
    #[test]
    fn waits_for_actual_position_after_the_old_fixed_delay() {
        let result = simulated([0.0, -0.1, -0.4, -0.65, -0.883, -0.95].map(Ok));
        assert!(result.outcome().is_ok());
        assert_eq!(result.elapsed_ms, 250);
        assert_eq!(result.actual, Some(-0.95));
        assert_eq!(result.trace[4]["elapsed_ms"], 200);
        assert_eq!(result.trace.len(), 6);
    }
    #[test]
    fn persistent_outside_tolerance_fails_with_a_bounded_trace() {
        let result = simulated([Ok(-0.883)]);
        assert!(result.outcome().is_err());
        assert_eq!(result.elapsed_ms, 2000);
        assert_eq!(result.trace.len(), 41);
        assert_eq!(result.actual, Some(-0.883));
    }
    #[test]
    fn a_read_that_finishes_after_the_deadline_cannot_pass() {
        let clock = Rc::new(Cell::new(Duration::ZERO));
        let now = clock.clone();
        let result = poll_motor_position(
            -1.0,
            move || {
                clock.set(Duration::from_millis(2100));
                Ok(hid::MotorPosition {
                    target: -1.0,
                    current: -1.0,
                })
            },
            move || now.get(),
            |_| panic!("No further polling after deadline"),
        );
        assert!(result.outcome().is_err());
        assert_eq!(result.elapsed_ms, 2100);
        assert_eq!(result.trace.len(), 1);
    }
    #[test]
    fn transport_failure_is_preserved_without_hiding_it_as_settling() {
        let result = simulated([Ok(-0.8), Err("USB desconectado".into())]);
        assert_eq!(result.outcome(), Err("USB desconectado".into()));
        assert_eq!(result.elapsed_ms, 50);
        assert_eq!(result.trace[1]["error"], "USB desconectado");
    }
}
