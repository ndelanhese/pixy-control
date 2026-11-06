//! Local EMEET V2 transport recovered from Studio 2.0.5 arm64 code.
//! Sources: hidCmdSend 0x10063c5e0, UsbHidCtrl::write 0x10072aa30,
//! EMHidCmdV2RecvFsm::onDataRecv 0x10063e9e8. No network/vendor SDK.
use hidapi::{HidApi, HidDevice};
use std::time::{Duration, Instant};

pub const VID: u16 = 0x328f;
pub const PID: u16 = 0x00c0;
pub mod commands {
    pub const GET_SERIAL: [u8; 4] = [9, 1, 0, 3];
    pub const GET_VERSION: [u8; 4] = [9, 1, 0, 4];
    pub const SET_FACTORY_RESET: [u8; 4] = [9, 1, 0, 5];
    pub const GET_DEVICE_MODE: [u8; 4] = [9, 1, 1, 1];
    pub const SET_DEVICE_MODE: [u8; 4] = [9, 1, 1, 0];
    pub const GET_PRIVACY_TIME: [u8; 4] = [9, 2, 1, 1];
    pub const SET_PRIVACY_TIME: [u8; 4] = [9, 2, 1, 0];
    pub const GET_STARTUP_MODE: [u8; 4] = [9, 3, 1, 0x14];
    pub const SET_STARTUP_MODE: [u8; 4] = [9, 3, 1, 0x13];
    pub const SET_MOTOR_RELATIVE: [u8; 4] = [9, 0x63, 1, 0x19];
    pub const GET_GESTURE: [u8; 4] = [9, 4, 2, 1];
    pub const SET_GESTURE: [u8; 4] = [9, 4, 2, 0];
    pub const GET_MUSIC_MODE: [u8; 4] = [9, 5, 0, 4];
    pub const SET_MUSIC_MODE: [u8; 4] = [9, 5, 0, 3];
    #[cfg(test)]
    pub const GET_HDR: [u8; 4] = [9, 4, 0, 0x13];
    #[cfg(test)]
    pub const GET_LOW_LIGHT: [u8; 4] = [9, 4, 0, 0x36];
    pub const GET_OSD: [u8; 4] = [9, 4, 0, 6];
    pub const SET_OSD: [u8; 4] = [9, 4, 0, 5];
    pub const GET_WB_LOCK: [u8; 4] = [9, 4, 0, 10];
    pub const SET_WB_LOCK: [u8; 4] = [9, 4, 0, 9];
    pub const GET_FOCUS_LOCK: [u8; 4] = [9, 4, 0, 14];
    pub const SET_FOCUS_LOCK: [u8; 4] = [9, 4, 0, 13];
    pub const GET_REVERSE: [u8; 4] = [9, 4, 0, 7];
    pub const SET_REVERSE: [u8; 4] = [9, 4, 0, 8];
    pub const GET_MOTOR_POSITION: [u8; 4] = [9, 0x63, 1, 1];
    pub const SET_MOTOR_POSITION: [u8; 4] = [9, 0x63, 1, 0];
}

// Enum values observed beside QVariant(int) and translation calls in
// PtzDeviceOptVM::init*Options, not inferred from visual order.
pub const DEVICE_MODES: &[(u8, &str)] = &[(0, "Manual"), (1, "Rastreamento"), (2, "Privacidade")];
pub const AUDIO_MODES: &[(u8, &str)] = &[(2, "Ao vivo"), (1, "Redução de ruído"), (3, "Original")];
pub const PRIVACY_TIMES: &[(u32, &str)] = &[
    (10, "10 segundos"),
    (60, "1 minuto"),
    (900, "15 minutos"),
    (0, "Nunca"),
];
pub const GESTURE_PALM: u8 = 2;
pub const GESTURE_YES: u8 = 3;
pub const REVERSE_AUTO_ROTATE: u8 = 4;
pub const REVERSE_HORIZONTAL: u8 = 1;
pub const REVERSE_VERTICAL: u8 = 2;
pub const MOTOR_YAW: u8 = 1;
pub const MOTOR_PITCH: u8 = 2;
pub const MOTOR_ROLL: u8 = 3;

/// Studio normalizes firmware state 3 to Manual in setDeviceModeConfig.
pub fn normalize_device_mode(raw: u8) -> u8 {
    if raw == 3 {
        0
    } else {
        raw
    }
}

/// VersionUtil::convertVersion converts u16 to hex, then separates the first
/// two digits and the final two-digit patch (omitting its leading zero).
pub fn firmware_version_string(raw: u16) -> String {
    let hex = format!("{raw:03x}");
    let patch = hex[2..].trim_start_matches('0');
    format!(
        "{}.{}.{}",
        &hex[..1],
        &hex[1..2],
        if patch.is_empty() { "0" } else { patch }
    )
}

/// Native GET_MOTOR_POS returns axis, target f32LE, current f32LE.
/// Studio logs the first as toPos and passes the SECOND (curPos) to presets:
/// parser0x10067e394, caller0x100362adc..0x100362b9c.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotorPosition {
    pub target: f32,
    pub current: f32,
}
impl MotorPosition {
    fn decode(axis: u8, payload: &[u8]) -> Result<Self, String> {
        if !matches!(axis, MOTOR_YAW | MOTOR_PITCH | MOTOR_ROLL)
            || payload.len() < 9
            || payload[0] != axis
        {
            return Err("Resposta de posição inválida".into());
        }
        let position = Self {
            target: f32::from_le_bytes(payload[1..5].try_into().unwrap()),
            current: f32::from_le_bytes(payload[5..9].try_into().unwrap()),
        };
        if !position.target.is_finite() || !position.current.is_finite() {
            return Err("Alvo ou posição atual do motor inválidos".into());
        }
        Ok(position)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SavedPosition {
    pub mode: u8,
    pub coordinates: Option<[f32; 3]>,
}

impl SavedPosition {
    /// Wire parser mirrored from hidCmdParseGetMotorPowerOnDefaultPosMode.
    fn decode(payload: &[u8]) -> Result<Self, String> {
        let mode = *payload.first().ok_or("Configuração de posição vazia")?;
        let coordinates = if mode == 1 {
            if payload.len() < 13 {
                return Err("Configuração de posição incompleta".into());
            }
            let values = std::array::from_fn(|i| {
                f32::from_le_bytes(payload[1 + i * 4..5 + i * 4].try_into().unwrap())
            });
            if values.iter().any(|v| !v.is_finite()) {
                return Err("Coordenadas de posição inválidas".into());
            }
            Some(values)
        } else {
            None
        };
        Ok(Self { mode, coordinates })
    }
}

pub struct PixyHid {
    device: HidDevice,
}

impl PixyHid {
    pub fn connect() -> Result<Self, String> {
        let api = HidApi::new().map_err(|e| e.to_string())?;
        // Observed Pixy descriptor: usage page 0x0083, usage 0x0083, interface 4.
        let mut errors = Vec::new();
        let matching: Vec<_> = api
            .device_list()
            .filter(|d| {
                d.vendor_id() == VID
                    && d.product_id() == PID
                    && d.usage_page() == 0x83
                    && d.usage() == 0x83
            })
            .collect();
        if matching.len() > 1 {
            return Err("Mais de uma EMEET PIXY conectada; seleção HID ambígua.".into());
        }
        for info in matching {
            match info.open_device(&api) {
                Ok(device) => return Ok(Self { device }),
                Err(e) => errors.push(e.to_string()),
            }
        }
        Err(if errors.is_empty() {
            "Pixy HID não encontrada (328f:00c0).".into()
        } else {
            errors.join("; ")
        })
    }
    /// Transaction is serialized by &mut self. A reply is required; a successful
    /// OS write alone is never reported as a camera acknowledgment.
    pub fn exchange(&mut self, header: [u8; 4], payload: &[u8]) -> Result<Vec<u8>, String> {
        let reports = encode_reports(header, payload)?;
        let mut stale = [0u8; 256];
        // Bound draining so unsolicited reports cannot keep the UI blocked.
        for _ in 0..32 {
            if self
                .device
                .read_timeout(&mut stale, 0)
                .map_err(|e| e.to_string())?
                == 0
            {
                break;
            }
        }
        for report in reports {
            let written = self.device.write(&report).map_err(|e| e.to_string())?;
            if written != report.len() {
                return Err(format!("HID escrita incompleta: {written}/32"));
            }
        }
        let deadline = Instant::now() + Duration::from_millis(1200);
        let mut assembly = ReplyAssembly::default();
        while Instant::now() < deadline {
            let mut buf = [0u8; 256];
            let n = self
                .device
                .read_timeout(&mut buf, 100)
                .map_err(|e| e.to_string())?;
            if n == 0 {
                continue;
            }
            if let Some(reply) = assembly.accept(header, &buf[..n])? {
                return Ok(reply);
            }
        }
        Err(format!("Pixy não respondeu ao comando {:02x?}.", header))
    }
    pub fn get_byte(&mut self, header: [u8; 4]) -> Result<u8, String> {
        self.exchange(header, &[])?
            .first()
            .copied()
            .ok_or_else(|| "Resposta vazia da câmera".into())
    }
    pub fn serial_number(&mut self) -> Result<String, String> {
        let bytes = self.exchange(commands::GET_SERIAL, &[])?;
        let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
        if end == 0 || bytes[..end].iter().any(|b| !b.is_ascii_graphic()) {
            return Err("Número de série inválido".into());
        }
        String::from_utf8(bytes[..end].to_vec()).map_err(|e| e.to_string())
    }
    /// The attached Pixy answered subsystem types 0, 2 and 3. Native V2 GET_VER
    /// selects a subsystem by bits 5..7 of header byte 1.
    pub fn firmware_version(&mut self, device_type: u8) -> Result<String, String> {
        if device_type > 7 {
            return Err("Subsistema inválido".into());
        }
        let mut header = commands::GET_VERSION;
        header[1] |= device_type << 5;
        let bytes = self.exchange(header, &[])?;
        let raw: [u8; 2] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| "Resposta de firmware inválida")?;
        Ok(firmware_version_string(u16::from_le_bytes(raw)))
    }
    pub fn set_byte(&mut self, header: [u8; 4], value: u8) -> Result<Vec<u8>, String> {
        self.exchange(header, &[value])
    }
    pub fn motor_positions(&mut self, axis: u8) -> Result<MotorPosition, String> {
        if !matches!(axis, MOTOR_YAW | MOTOR_PITCH | MOTOR_ROLL) {
            return Err("Eixo de motor inválido".into());
        }
        MotorPosition::decode(axis, &self.exchange(commands::GET_MOTOR_POSITION, &[axis])?)
    }
    /// Preset coordinates use curPos, matching Studio, not its distinct toPos.
    pub fn motor_position(&mut self, axis: u8) -> Result<f32, String> {
        Ok(self.motor_positions(axis)?.current)
    }
    pub fn set_motor_position(&mut self, axis: u8, value: f32) -> Result<Vec<u8>, String> {
        if !value.is_finite() {
            return Err("Posição inválida".into());
        }
        let mut payload = vec![axis];
        payload.extend(value.to_le_bytes());
        self.exchange(commands::SET_MOTOR_POSITION, &payload)
    }
    /// Plain PIXY uses reset mode 0. Studio's mode 1 reboot path is for other
    /// product types. Destructive: callers must make the action explicit.
    pub fn factory_reset(&mut self) -> Result<(), String> {
        require_ack(&self.exchange(commands::SET_FACTORY_RESET, &[0])?)
    }
    pub fn startup_position(&mut self) -> Result<SavedPosition, String> {
        SavedPosition::decode(&self.exchange(commands::GET_STARTUP_MODE, &[])?)
    }
    /// Device stores its current position itself; no guessed coordinates.
    pub fn save_startup_position(&mut self) -> Result<(), String> {
        require_ack(&self.set_byte(commands::SET_STARTUP_MODE, 1)?)
    }
    pub fn clear_startup_position(&mut self) -> Result<(), String> {
        require_ack(&self.set_byte(commands::SET_STARTUP_MODE, 0)?)
    }
    /// Relative motor units pass unchanged through Studio's float path.
    /// The firmware-relative target is based on CURRENT position, not the prior
    /// target. Physical degrees are not calibrated here. The 15-unit per-action
    /// bound is this app's chosen limit.
    pub fn move_relative(&mut self, axis: u8, units: f32) -> Result<(), String> {
        if !matches!(axis, MOTOR_YAW | MOTOR_PITCH | MOTOR_ROLL)
            || !units.is_finite()
            || units.abs() > 15.0
        {
            return Err("Movimento relativo inválido".into());
        }
        let mut payload = vec![axis];
        payload.extend(units.to_le_bytes());
        let reply = self.exchange(commands::SET_MOTOR_RELATIVE, &payload)?;
        if reply.len() != 2 || reply[0] != axis {
            return Err("Confirmação do motor inválida".into());
        }
        require_ack(&reply[1..])
    }
}

/// Standard nonmotor setters acknowledge with 0x20 (Studio success branch).
/// Commands that echo an axis/type/slot need their command-specific parser.
pub fn require_ack(payload: &[u8]) -> Result<(), String> {
    match payload.first() {
        Some(0x20) => Ok(()),
        Some(code) => Err(format!("Câmera rejeitou o comando: código 0x{code:02x}")),
        None => Err("Câmera não enviou confirmação".into()),
    }
}

pub fn encode_reports(header: [u8; 4], payload: &[u8]) -> Result<Vec<[u8; 32]>, String> {
    if header[0] != 9 {
        return Err("Report ID V2 inválido".into());
    }
    // u8 sequence index constrains a message to 256 fragments.
    if payload.len() > 24 * 256 {
        return Err("Payload HID excede 6144 bytes".into());
    }
    let total = (payload.len() as u16).to_le_bytes();
    let count = payload.len().div_ceil(24).max(1);
    Ok((0..count)
        .map(|index| {
            let start = index * 24;
            let end = (start + 24).min(payload.len());
            let mut report = [0u8; 32];
            report[..4].copy_from_slice(&header);
            report[4] = index as u8;
            report[5..7].copy_from_slice(&total);
            report[7] = (end - start) as u8;
            report[8..8 + end - start].copy_from_slice(&payload[start..end]);
            report
        })
        .collect())
}

#[derive(Default)]
struct ReplyAssembly {
    next: usize,
    total: Option<usize>,
    payload: Vec<u8>,
}
impl ReplyAssembly {
    fn accept(&mut self, expected: [u8; 4], report: &[u8]) -> Result<Option<Vec<u8>>, String> {
        if report.len() < 8
            || report[0] != expected[0]
            || (report[1] & 31) != (expected[1] & 31)
            || report[2..4] != expected[2..4]
        {
            return Ok(None);
        }
        let total = u16::from_le_bytes([report[5], report[6]]) as usize;
        let size = report[7] as usize;
        if size > 24 || report.len() < 8 + size || total > 6144 {
            return Err("Fragmento HID inválido".into());
        }
        if report[4] as usize != self.next || self.total.is_some_and(|n| n != total) {
            return Err("Sequência HID inválida".into());
        }
        self.total = Some(total);
        self.next += 1;
        if self.payload.len() + size > total {
            return Err("Resposta HID excedeu tamanho declarado".into());
        }
        self.payload.extend_from_slice(&report[8..8 + size]);
        if self.payload.len() == total {
            Ok(Some(std::mem::take(&mut self.payload)))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn motor_target_and_current_are_distinct_native_fields() {
        let mut payload = vec![super::MOTOR_PITCH];
        payload.extend(0.0f32.to_le_bytes());
        payload.extend(0.112012125f32.to_le_bytes());
        let position = super::MotorPosition::decode(super::MOTOR_PITCH, &payload).unwrap();
        assert_eq!(position.target, 0.0);
        assert_eq!(position.current, 0.112012125);
        assert!(super::MotorPosition::decode(super::MOTOR_YAW, &payload).is_err());
        assert!(super::MotorPosition::decode(super::MOTOR_PITCH, &payload[..8]).is_err());
        payload[5..9].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(super::MotorPosition::decode(super::MOTOR_PITCH, &payload).is_err());
        payload[5..9].copy_from_slice(&0.0f32.to_le_bytes());
        payload[1..5].copy_from_slice(&f32::INFINITY.to_le_bytes());
        assert!(super::MotorPosition::decode(super::MOTOR_PITCH, &payload).is_err());
    }

    use super::*;
    #[test]
    fn hdr_query_matches_recovered_zero_payload_layout() {
        let report = encode_reports(commands::GET_HDR, &[]).unwrap()[0];
        assert_eq!(&report[..8], &[9, 4, 0, 0x13, 0, 0, 0, 0]);
        assert_eq!(report.len(), 32);
    }
    #[test]
    fn plain_pixy_factory_reset_matches_native_payload() {
        let report = encode_reports(commands::SET_FACTORY_RESET, &[0]).unwrap()[0];
        assert_eq!(&report[..9], &[9, 1, 0, 5, 0, 1, 0, 1, 0]);
    }
    #[test]
    fn fragments_roundtrip_with_device_type_bits() {
        let payload: Vec<_> = (0..70).collect();
        let mut reports = encode_reports(commands::GET_HDR, &payload).unwrap();
        let mut assembly = ReplyAssembly::default();
        for report in &mut reports {
            report[1] |= 0x60;
        }
        assert_eq!(
            assembly.accept(commands::GET_HDR, &reports[0]).unwrap(),
            None
        );
        assert_eq!(
            assembly.accept(commands::GET_HDR, &reports[1]).unwrap(),
            None
        );
        assert_eq!(
            assembly.accept(commands::GET_HDR, &reports[2]).unwrap(),
            Some(payload)
        );
    }
    #[test]
    fn rejects_missing_first_fragment_and_short_payload() {
        let reports = encode_reports(commands::GET_HDR, &[1; 25]).unwrap();
        assert!(ReplyAssembly::default()
            .accept(commands::GET_HDR, &reports[1])
            .is_err());
        assert!(ReplyAssembly::default()
            .accept(commands::GET_HDR, &reports[0][..9])
            .is_err());
    }
    #[test]
    fn unrelated_reply_is_ignored() {
        let report = encode_reports(commands::GET_LOW_LIGHT, &[1]).unwrap()[0];
        assert_eq!(
            ReplyAssembly::default()
                .accept(commands::GET_HDR, &report)
                .unwrap(),
            None
        );
    }
    #[test]
    fn saved_position_requires_all_coordinates_only_when_enabled() {
        assert!(SavedPosition::decode(&[0]).unwrap().coordinates.is_none());
        assert!(SavedPosition::decode(&[1, 0]).is_err());
        let mut payload = vec![1];
        for value in [1.0_f32, -2.5, 3.0] {
            payload.extend(value.to_le_bytes());
        }
        assert_eq!(
            SavedPosition::decode(&payload).unwrap().coordinates,
            Some([1.0, -2.5, 3.0])
        );
    }
    #[test]
    fn rejected_setter_cannot_be_reported_as_success() {
        assert!(require_ack(&[0x20]).is_ok());
        assert!(require_ack(&[0]).is_err());
        assert!(require_ack(&[]).is_err());
    }
    #[test]
    fn native_identity_values_match_observed_studio_versions() {
        assert_eq!(firmware_version_string(0x2004), "2.0.4");
        assert_eq!(firmware_version_string(0x2005), "2.0.5");
        assert_eq!(firmware_version_string(0x2007), "2.0.7");
        assert_eq!(firmware_version_string(0x2014), "2.0.14");
        assert_eq!(firmware_version_string(0x0014), "0.1.4");
        assert_eq!(normalize_device_mode(3), 0);
        assert_eq!(normalize_device_mode(1), 1);
    }
}
