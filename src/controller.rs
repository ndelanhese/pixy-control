use crate::{
    hid::{self, PixyHid},
    model::{self, Preset},
    uvc::{ControlId, ControlState, UvcCamera},
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
};

#[derive(Clone, Default, serde::Serialize)]
pub struct Snapshot {
    pub controls: Vec<ControlState>,
    pub hid_connected: bool,
    pub vendor: BTreeMap<String, Vec<u8>>,
    pub message: String,
    pub audio: Option<crate::audio::AudioState>,
    pub motor: Option<[f32; 2]>,
    pub serial: Option<String>,
    pub firmware: Vec<String>,
}

pub enum Command {
    Refresh,
    Disconnect,
    Set(ControlId, i64),
    Filter(usize),
    RestoreImage,
    AutoMode(u8),
    Vendor {
        get: [u8; 4],
        set: [u8; 4],
        query: Vec<u8>,
        payload: Vec<u8>,
    },
    Move {
        axis: ControlId,
        delta: i64,
    },
    Jog(f32, f32),
    Center,
    Startup(bool),
    Mute(bool),
    Volume(f32),
    FactoryReset,
    CapturePreset(String, Option<u64>),
    Recall(Preset),
}

pub struct Controller {
    background: Arc<AtomicBool>,
    tx: mpsc::SyncSender<Command>,
    rx: mpsc::Receiver<Snapshot>,
    captured: mpsc::Receiver<(Option<u64>, Result<Preset, String>)>,
    auto_modes: mpsc::Receiver<(u8, Result<(), String>)>,
}

impl Controller {
    pub fn start() -> Self {
        let background = Arc::new(AtomicBool::new(
            std::env::args().any(|arg| arg == "--background"),
        ));
        let worker_background = background.clone();
        let (tx, commands) = mpsc::sync_channel(32);
        let (states, rx) = mpsc::channel();
        let (captures, captured) = mpsc::channel();
        let (auto_results, auto_modes) = mpsc::channel();
        thread::spawn(move || {
            let mut usb = UvcCamera::open();
            let mut hid = PixyHid::connect();
            let mut snapshot = read_snapshot(&usb, &mut hid);
            let mut removed = false;
            let _ = states.send(snapshot.clone());
            loop {
                let command = match commands.recv_timeout(std::time::Duration::from_secs(2)) {
                    Ok(c) => c,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if removed {
                            continue;
                        }
                        let mut current = if worker_background.load(Ordering::Acquire) {
                            background_snapshot(&snapshot, || {
                                hid.as_mut()
                                    .map_err(|error| error.clone())?
                                    .get_byte(hid::commands::GET_DEVICE_MODE)
                            })
                        } else {
                            read_snapshot(&usb, &mut hid)
                        };
                        current.message.clear();
                        snapshot = current;
                        if states.send(snapshot.clone()).is_err() {
                            break;
                        }
                        continue;
                    }
                };
                if matches!(command, Command::Disconnect) {
                    removed = true;
                    usb = Err("Removida da sessão".into());
                    hid = Err("Removida da sessão".into());
                    snapshot = Snapshot {
                        message: "Câmera removida desta sessão. Use Reconectar para adicioná-la."
                            .into(),
                        ..Default::default()
                    };
                    if states.send(snapshot.clone()).is_err() {
                        break;
                    }
                    continue;
                }
                let auto_mode = match &command {
                    Command::AutoMode(mode) => Some(*mode),
                    _ => None,
                };
                let result: Result<(), String> = (|| {
                    match command {
                        Command::Refresh => {
                            removed = false;
                            usb = UvcCamera::open();
                            hid = PixyHid::connect();
                        }
                        Command::Disconnect => unreachable!(),
                        Command::AutoMode(mode) => {
                            if mode > 2 {
                                return Err("Modo automático inválido".into());
                            }
                            let camera = hid.as_mut().map_err(|e| e.clone())?;
                            hid::require_ack(
                                &camera.set_byte(hid::commands::SET_DEVICE_MODE, mode)?,
                            )?;
                            let actual = hid::normalize_device_mode(
                                camera.get_byte(hid::commands::GET_DEVICE_MODE)?,
                            );
                            if actual != mode {
                                return Err(format!("Modo não confirmado: {actual}"));
                            }
                        }
                        Command::Set(id, value) => {
                            set_verified(usb.as_ref().map_err(Clone::clone)?, id, value)?
                        }
                        Command::Move { axis, delta } => {
                            let camera = hid.as_mut().map_err(|e| e.clone())?;
                            ensure_manual(camera)?;
                            let motor = match axis {
                                ControlId::Pan => hid::MOTOR_YAW,
                                ControlId::Tilt => hid::MOTOR_PITCH,
                                _ => return Err("Eixo inválido".into()),
                            };
                            camera.move_relative(motor, delta as f32 / 3600.0)?;
                        }
                        Command::Jog(horizontal, vertical) => {
                            let camera = hid.as_mut().map_err(|e| e.clone())?;
                            ensure_manual(camera)?;
                            if horizontal.abs() > 0.02 {
                                camera.move_relative(hid::MOTOR_YAW, horizontal)?;
                            }
                            if vertical.abs() > 0.02 {
                                camera.move_relative(hid::MOTOR_PITCH, vertical)?;
                            }
                        }
                        Command::Center => {
                            let camera = hid.as_mut().map_err(|e| e.clone())?;
                            ensure_manual(camera)?;
                            for axis in [hid::MOTOR_YAW, hid::MOTOR_PITCH] {
                                let ack = camera.set_motor_position(axis, 0.0)?;
                                if ack != [axis, 0x20] {
                                    return Err(format!("Motor não confirmou centro: {ack:02x?}"));
                                }
                            }
                        }
                        Command::Startup(save) => {
                            let camera = hid.as_mut().map_err(|e| e.clone())?;
                            if save {
                                camera.save_startup_position()?;
                            } else {
                                camera.clear_startup_position()?;
                            }
                            let position = camera.startup_position()?;
                            if position.mode != save as u8 {
                                return Err("Posição inicial não confirmada pela câmera".into());
                            }
                            if save && position.coordinates.is_none() {
                                return Err("Posição inicial sem coordenadas confirmadas".into());
                            }
                        }
                        Command::Mute(muted) => crate::audio::set_mute(muted)?,
                        Command::Volume(value) => crate::audio::set_volume_scalar(value)?,
                        Command::FactoryReset => {
                            hid.as_mut().map_err(|e| e.clone())?.factory_reset()?
                        }
                        Command::CapturePreset(name, target_id) => {
                            let result = (|| -> Result<Preset, String> {
                                let camera = hid.as_mut().map_err(|e| e.clone())?;
                                // Native Studio saves curPos (second GET float), not toPos.
                                let yaw = camera.motor_position(hid::MOTOR_YAW)?;
                                let pitch = camera.motor_position(hid::MOTOR_PITCH)?;
                                if !yaw.is_finite() || !pitch.is_finite() {
                                    return Err("Posição inválida".into());
                                }
                                let zoom = usb
                                    .as_ref()
                                    .map_err(Clone::clone)?
                                    .controls()
                                    .into_iter()
                                    .find(|c| c.id == ControlId::Zoom)
                                    .and_then(|c| c.current)
                                    .ok_or("Zoom não lido")?;
                                Ok(Preset {
                                    id: match target_id {
                                        Some(id) => id,
                                        None => model::new_preset_id()?,
                                    },
                                    name,
                                    position: [yaw, pitch, 0.0],
                                    controls: BTreeMap::from([("Zoom".into(), zoom)]),
                                    thumbnail: None,
                                })
                            })();
                            let failure = result.as_ref().err().cloned();
                            let _ = captures.send((target_id, result));
                            if let Some(e) = failure {
                                return Err(e);
                            }
                        }
                        Command::Vendor {
                            get,
                            set,
                            query,
                            payload,
                        } => {
                            let camera = hid.as_mut().map_err(|e| e.clone())?;
                            let ack = camera.exchange(set, &payload)?;
                            let expected = if set == hid::commands::SET_REVERSE
                                || set == hid::commands::SET_GESTURE
                            {
                                vec![*query.first().ok_or("Tipo de controle ausente")?, 0x20]
                            } else {
                                vec![0x20]
                            };
                            if ack != expected {
                                return Err(format!("Câmera recusou comando: {ack:02x?}"));
                            }
                            let current = camera.exchange(get, &query)?;
                            let matches = if get == hid::commands::GET_DEVICE_MODE {
                                current.first().copied().map(hid::normalize_device_mode)
                                    == payload.first().copied().map(hid::normalize_device_mode)
                            } else {
                                current == payload
                            };
                            if !matches {
                                return Err(format!(
                                    "Valor enviado; leitura devolveu {current:02x?}"
                                ));
                            }
                        }
                        Command::Filter(index) => {
                            let filters = model::filters();
                            let filter = filters.get(index).ok_or("Filtro inválido")?;
                            let limits = snapshot
                                .controls
                                .iter()
                                .filter_map(|c| Some((format!("{:?}", c.id), (c.min?, c.max?))))
                                .collect();
                            let plan = model::filter_writes(filter, &limits)?;
                            let camera = usb.as_ref().map_err(Clone::clone)?;
                            set_verified(camera, ControlId::ExposureAuto, 8)?;
                            camera.set(
                                ControlId::WhiteBalanceAuto,
                                filter.white_balance_auto as i64,
                            )?;
                            for (name, value) in plan {
                                if name == "WhiteBalance" && filter.white_balance_auto {
                                    continue;
                                }
                                if let Some(c) = snapshot
                                    .controls
                                    .iter()
                                    .find(|c| format!("{:?}", c.id) == name)
                                {
                                    camera.set(c.id, value)?;
                                }
                            }
                        }
                        Command::RestoreImage => {
                            let camera = usb.as_ref().map_err(Clone::clone)?;
                            let mut errors = Vec::new();
                            // Automatic loops can ignore manual value writes.
                            for (id, manual) in [
                                (ControlId::ExposureAuto, 1),
                                (ControlId::WhiteBalanceAuto, 0),
                                (ControlId::FocusAuto, 0),
                            ] {
                                if snapshot.controls.iter().any(|c| c.id == id && c.writable) {
                                    if let Err(error) = camera.set(id, manual) {
                                        errors.push(error);
                                    }
                                }
                            }
                            for c in camera.controls() {
                                if c.writable
                                    && !matches!(
                                        c.id,
                                        ControlId::Pan
                                            | ControlId::Tilt
                                            | ControlId::Roll
                                            | ControlId::Zoom
                                            | ControlId::ExposureAuto
                                            | ControlId::WhiteBalanceAuto
                                            | ControlId::FocusAuto
                                    )
                                {
                                    if let Some(value) = c.default {
                                        if let Err(error) = camera.set(c.id, value) {
                                            errors.push(error);
                                        }
                                    }
                                }
                            }
                            // Restore auto defaults even if an individual value failed.
                            for (id, fallback) in [
                                (ControlId::ExposureAuto, 8),
                                (ControlId::WhiteBalanceAuto, 1),
                                (ControlId::FocusAuto, 1),
                            ] {
                                if let Some(control) =
                                    snapshot.controls.iter().find(|c| c.id == id && c.writable)
                                {
                                    if let Err(error) =
                                        camera.set(id, control.default.unwrap_or(fallback))
                                    {
                                        errors.push(error);
                                    }
                                }
                            }
                            if !errors.is_empty() {
                                return Err(errors.join("; "));
                            }
                        }
                        Command::Recall(preset) => {
                            if preset
                                .position
                                .iter()
                                .any(|v| !v.is_finite() || v.abs() > 180.0)
                            {
                                return Err("Posição inválida".into());
                            }
                            if snapshot.motor.is_none() {
                                return Err("Posição do gimbal indisponível".into());
                            }
                            let camera = usb.as_ref().map_err(Clone::clone)?;
                            // Physical Pixy presets restore gimbal and hardware zoom.
                            // Image/exposure keys from earlier local files are ignored.
                            if let Some(&zoom) = preset.controls.get("Zoom") {
                                camera.set(ControlId::Zoom, zoom)?;
                            }
                            let camera = hid.as_mut().map_err(|e| e.clone())?;
                            ensure_manual(camera)?;
                            for (axis, value) in [
                                (hid::MOTOR_YAW, preset.position[0]),
                                (hid::MOTOR_PITCH, preset.position[1]),
                            ] {
                                let ack = camera.set_motor_position(axis, value)?;
                                if ack != [axis, 0x20] {
                                    return Err("Posição do preset não confirmada".into());
                                }
                            }
                        }
                    }
                    Ok(())
                })();
                if let Some(mode) = auto_mode {
                    let _ = auto_results.send((mode, result.clone()));
                }
                snapshot = read_snapshot(&usb, &mut hid);
                snapshot.message = match result {
                    Ok(()) => "Leitura atualizada da câmera".into(),
                    Err(e) => format!("Falha: {e}"),
                };
                if states.send(snapshot.clone()).is_err() {
                    break;
                }
            }
        });
        Self {
            background,
            tx,
            rx,
            captured,
            auto_modes,
        }
    }
    pub fn set_background(&self, background: bool) {
        self.background.store(background, Ordering::Release);
    }
    pub fn send(&self, command: Command) -> Result<(), String> {
        self.tx.try_send(command).map_err(|e| e.to_string())
    }
    pub fn latest(&self) -> Option<Snapshot> {
        let mut latest = None;
        let mut completion = None;
        for mut snapshot in self.rx.try_iter() {
            if !snapshot.message.is_empty() {
                completion = Some(std::mem::take(&mut snapshot.message));
            }
            latest = Some(snapshot);
        }
        if let (Some(snapshot), Some(message)) = (latest.as_mut(), completion) {
            snapshot.message = message;
        }
        latest
    }
    pub fn captured_presets(&self) -> Vec<(Option<u64>, Result<Preset, String>)> {
        self.captured.try_iter().collect()
    }
    pub fn auto_mode_results(&self) -> Vec<(u8, Result<(), String>)> {
        self.auto_modes.try_iter().collect()
    }
}

// Full UVC ranges, firmware, audio endpoints and motor positions are useful in
// the visible controller, but need not repeatedly interrupt USB video while a
// call runs in the background. Privacy still gets a fresh mode observation.
fn background_snapshot(
    previous: &Snapshot,
    read_mode: impl FnOnce() -> Result<u8, String>,
) -> Snapshot {
    let mut current = previous.clone();
    current.message.clear();
    current.vendor.remove("mode");
    if let Ok(mode) = read_mode() {
        current
            .vendor
            .insert("mode".into(), vec![hid::normalize_device_mode(mode)]);
    }
    current
}

fn set_verified(camera: &UvcCamera, id: ControlId, value: i64) -> Result<(), String> {
    camera.set(id, value)
}

fn ensure_manual(camera: &mut PixyHid) -> Result<(), String> {
    if hid::normalize_device_mode(camera.get_byte(hid::commands::GET_DEVICE_MODE)?) != 0 {
        hid::require_ack(&camera.set_byte(hid::commands::SET_DEVICE_MODE, 0)?)?;
    }
    Ok(())
}

pub fn read_snapshot(
    usb: &Result<UvcCamera, String>,
    hid: &mut Result<PixyHid, String>,
) -> Snapshot {
    let mut s = Snapshot {
        audio: crate::audio::state().ok(),
        ..Default::default()
    };
    match usb {
        Ok(camera) => s.controls = camera.controls(),
        Err(e) => s.message = e.clone(),
    }
    if let Ok(camera) = hid {
        s.hid_connected = true;
        s.serial = camera.serial_number().ok();
        s.firmware = [0, 2, 3]
            .into_iter()
            .filter_map(|device| camera.firmware_version(device).ok())
            .collect();
        if let (Ok(yaw), Ok(pitch)) = (
            camera.motor_position(hid::MOTOR_YAW),
            camera.motor_position(hid::MOTOR_PITCH),
        ) {
            if yaw.is_finite() && pitch.is_finite() {
                s.motor = Some([yaw, pitch]);
            }
        }
        // Proven getters on this Pixy. No guessed setter or firmware commands.
        for (name, header) in [
            ("mode", hid::commands::GET_DEVICE_MODE),
            ("osd", hid::commands::GET_OSD),
            ("wb_lock", hid::commands::GET_WB_LOCK),
            ("focus_lock", hid::commands::GET_FOCUS_LOCK),
            ("audio_mode", hid::commands::GET_MUSIC_MODE),
            ("privacy", hid::commands::GET_PRIVACY_TIME),
            ("startup", hid::commands::GET_STARTUP_MODE),
        ] {
            if let Ok(value) = camera.exchange(header, &[]) {
                s.vendor.insert(name.into(), value);
            }
        }
        for (name, header, query) in [
            ("palm", hid::commands::GET_GESTURE, vec![hid::GESTURE_PALM]),
            (
                "auto_rotate",
                hid::commands::GET_REVERSE,
                vec![hid::REVERSE_AUTO_ROTATE],
            ),
            ("mirror", hid::commands::GET_REVERSE, vec![1]),
            ("flip", hid::commands::GET_REVERSE, vec![2]),
        ] {
            if let Ok(value) = camera.exchange(header, &query) {
                s.vendor.insert(name.into(), value);
            }
        }
        if let Some(mode) = s.vendor.get_mut("mode").and_then(|v| v.first_mut()) {
            *mode = hid::normalize_device_mode(*mode);
        }
    }
    if s.message.is_empty() {
        s.message = "Pixy conectada por USB · controles lidos do dispositivo".into();
    }
    s
}

#[cfg(test)]
mod completion_tests {
    use super::*;

    #[test]
    fn background_mode_refresh_keeps_cached_controls_without_reusing_stale_mode() {
        let mut previous = Snapshot {
            serial: Some("cached".into()),
            motor: Some([2.0, 3.0]),
            ..Default::default()
        };
        previous.vendor.insert("mode".into(), vec![1]);
        previous.vendor.insert("osd".into(), vec![7]);
        let fresh = background_snapshot(&previous, || Ok(2));
        assert_eq!(fresh.vendor.get("mode"), Some(&vec![2]));
        assert_eq!(fresh.serial, previous.serial);
        assert_eq!(fresh.motor, previous.motor);
        assert_eq!(fresh.vendor.get("osd"), Some(&vec![7]));
        let unknown = background_snapshot(&previous, || Err("disconnected".into()));
        assert!(!unknown.vendor.contains_key("mode"));
        assert_eq!(previous.vendor.get("mode"), Some(&vec![1]));
    }

    #[test]
    fn later_periodic_snapshot_preserves_command_completion_and_fresh_state() {
        let (tx, _commands) = mpsc::sync_channel(1);
        let (states, rx) = mpsc::channel();
        let (_captures, captured) = mpsc::channel();
        let (_, auto_modes) = mpsc::channel();
        let controller = Controller {
            background: Arc::new(AtomicBool::new(false)),
            tx,
            rx,
            captured,
            auto_modes,
        };
        states
            .send(Snapshot {
                message: "Comando concluído".into(),
                motor: Some([1.0, 2.0]),
                ..Default::default()
            })
            .unwrap();
        states
            .send(Snapshot {
                message: String::new(),
                motor: Some([9.0, 8.0]),
                ..Default::default()
            })
            .unwrap();
        let latest = controller.latest().unwrap();
        assert_eq!(latest.motor, Some([9.0, 8.0]));
        assert_eq!(latest.message, "Comando concluído");
        assert!(controller.latest().is_none());
    }

    #[test]
    fn most_recent_completion_wins_without_replaying_it_on_next_poll() {
        let (tx, _commands) = mpsc::sync_channel(1);
        let (states, rx) = mpsc::channel();
        let (_captures, captured) = mpsc::channel();
        let (_, auto_modes) = mpsc::channel();
        let controller = Controller {
            background: Arc::new(AtomicBool::new(false)),
            tx,
            rx,
            captured,
            auto_modes,
        };
        states
            .send(Snapshot {
                message: "Falha anterior".into(),
                ..Default::default()
            })
            .unwrap();
        states
            .send(Snapshot {
                message: "Último comando concluído".into(),
                ..Default::default()
            })
            .unwrap();
        states.send(Snapshot::default()).unwrap();
        assert_eq!(
            controller.latest().unwrap().message,
            "Último comando concluído"
        );
        states.send(Snapshot::default()).unwrap();
        assert!(controller.latest().unwrap().message.is_empty());
    }
}
