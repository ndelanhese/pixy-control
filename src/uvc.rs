//! Standard USB Video Class controls, independent of the EMEET binaries.
//!
//! Uses endpoint-zero class requests without claiming an interface, detaching a
//! kernel driver, changing configuration, or interrupting the video stream.

use rusb::{Context, DeviceHandle, UsbContext};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const VID: u16 = 0x328f;
const PID: u16 = 0x00c0;
const TIMEOUT: Duration = Duration::from_millis(800);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlId {
    Brightness,
    Contrast,
    Hue,
    Saturation,
    Sharpness,
    Gamma,
    WhiteBalance,
    WhiteBalanceAuto,
    BacklightCompensation,
    Gain,
    PowerLineFrequency,
    ExposureAuto,
    Exposure,
    Focus,
    FocusAuto,
    Zoom,
    Pan,
    Tilt,
    Roll,
}

#[derive(Clone, Debug, Serialize)]
pub struct ControlState {
    pub id: ControlId,
    pub label: String,
    pub current: Option<i64>,
    pub min: Option<i64>,
    pub max: Option<i64>,
    pub step: Option<i64>,
    pub default: Option<i64>,
    pub writable: bool,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug)]
struct Spec {
    id: ControlId,
    label: &'static str,
    terminal: bool,
    selector: u8,
    bit: u8,
    width: usize,
    signed: bool,
    offset: usize,
}

const SPECS: &[Spec] = &[
    Spec {
        id: ControlId::Brightness,
        label: "Brilho",
        terminal: false,
        selector: 2,
        bit: 0,
        width: 2,
        signed: true,
        offset: 0,
    },
    Spec {
        id: ControlId::Contrast,
        label: "Contraste",
        terminal: false,
        selector: 3,
        bit: 1,
        width: 2,
        signed: false,
        offset: 0,
    },
    Spec {
        id: ControlId::Hue,
        label: "Matiz",
        terminal: false,
        selector: 6,
        bit: 2,
        width: 2,
        signed: true,
        offset: 0,
    },
    Spec {
        id: ControlId::Saturation,
        label: "Saturação",
        terminal: false,
        selector: 7,
        bit: 3,
        width: 2,
        signed: false,
        offset: 0,
    },
    Spec {
        id: ControlId::Sharpness,
        label: "Nitidez",
        terminal: false,
        selector: 8,
        bit: 4,
        width: 2,
        signed: false,
        offset: 0,
    },
    Spec {
        id: ControlId::Gamma,
        label: "Gamma",
        terminal: false,
        selector: 9,
        bit: 5,
        width: 2,
        signed: false,
        offset: 0,
    },
    Spec {
        id: ControlId::WhiteBalance,
        label: "Balanço de branco (K)",
        terminal: false,
        selector: 10,
        bit: 6,
        width: 2,
        signed: false,
        offset: 0,
    },
    Spec {
        id: ControlId::BacklightCompensation,
        label: "Compensação de contraluz",
        terminal: false,
        selector: 1,
        bit: 8,
        width: 2,
        signed: false,
        offset: 0,
    },
    Spec {
        id: ControlId::Gain,
        label: "Ganho",
        terminal: false,
        selector: 4,
        bit: 9,
        width: 2,
        signed: false,
        offset: 0,
    },
    Spec {
        id: ControlId::PowerLineFrequency,
        label: "Anticintilação",
        terminal: false,
        selector: 5,
        bit: 10,
        width: 1,
        signed: false,
        offset: 0,
    },
    Spec {
        id: ControlId::WhiteBalanceAuto,
        label: "Balanço de branco automático",
        terminal: false,
        selector: 11,
        bit: 12,
        width: 1,
        signed: false,
        offset: 0,
    },
    Spec {
        id: ControlId::ExposureAuto,
        label: "Exposição automática",
        terminal: true,
        selector: 2,
        bit: 1,
        width: 1,
        signed: false,
        offset: 0,
    },
    Spec {
        id: ControlId::Exposure,
        label: "Exposição (100 µs)",
        terminal: true,
        selector: 4,
        bit: 3,
        width: 4,
        signed: false,
        offset: 0,
    },
    Spec {
        id: ControlId::Focus,
        label: "Foco",
        terminal: true,
        selector: 6,
        bit: 5,
        width: 2,
        signed: false,
        offset: 0,
    },
    Spec {
        id: ControlId::FocusAuto,
        label: "Foco automático",
        terminal: true,
        selector: 8,
        bit: 17,
        width: 1,
        signed: false,
        offset: 0,
    },
    Spec {
        id: ControlId::Zoom,
        label: "Zoom (%)",
        terminal: true,
        selector: 11,
        bit: 9,
        width: 2,
        signed: false,
        offset: 0,
    },
    Spec {
        id: ControlId::Pan,
        label: "Pan (segundos de arco)",
        terminal: true,
        selector: 13,
        bit: 11,
        width: 4,
        signed: true,
        offset: 0,
    },
    Spec {
        id: ControlId::Tilt,
        label: "Tilt (segundos de arco)",
        terminal: true,
        selector: 13,
        bit: 11,
        width: 4,
        signed: true,
        offset: 4,
    },
    Spec {
        id: ControlId::Roll,
        label: "Roll (segundos de arco)",
        terminal: true,
        selector: 15,
        bit: 13,
        width: 2,
        signed: true,
        offset: 0,
    },
];

#[derive(Clone, Copy, Debug)]
struct Entities {
    interface: u8,
    terminal: u8,
    processing: u8,
    terminal_controls: u64,
    processing_controls: u64,
}

pub struct UvcCamera {
    handle: DeviceHandle<Context>,
    entities: Entities,
}

impl UvcCamera {
    pub fn open() -> Result<Self, String> {
        let context = Context::new().map_err(|e| format!("Inicializar USB: {e}"))?;
        let devices = context
            .devices()
            .map_err(|e| format!("Enumerar USB: {e}"))?;
        let count = devices
            .iter()
            .filter(|d| {
                d.device_descriptor()
                    .is_ok_and(|desc| desc.vendor_id() == VID && desc.product_id() == PID)
            })
            .count();
        if count > 1 {
            return Err("Mais de uma EMEET PIXY conectada; seleção USB ambígua.".into());
        }
        for device in devices.iter() {
            let desc = match device.device_descriptor() {
                Ok(d) => d,
                Err(_) => continue,
            };
            if desc.vendor_id() != VID || desc.product_id() != PID {
                continue;
            }
            let config = device
                .active_config_descriptor()
                .map_err(|e| format!("Descritores da Pixy: {e}"))?;
            let mut entities = None;
            for interface in config.interfaces() {
                for descriptor in interface.descriptors() {
                    if descriptor.class_code() == 14
                        && descriptor.sub_class_code() == 1
                        && descriptor.setting_number() == 0
                    {
                        entities =
                            parse_entities(descriptor.interface_number(), descriptor.extra());
                    }
                }
            }
            let entities = entities.ok_or("A Pixy não publicou terminais UVC compatíveis")?;
            let handle = device.open().map_err(|e| format!("Abrir Pixy USB: {e}"))?;
            return Ok(Self { handle, entities });
        }
        Err("EMEET PIXY não encontrada (USB 328f:00c0)".into())
    }

    fn supported(&self, spec: &Spec) -> bool {
        let bits = if spec.terminal {
            self.entities.terminal_controls
        } else {
            self.entities.processing_controls
        };
        bits & (1u64 << spec.bit) != 0
    }

    fn index(&self, spec: &Spec) -> u16 {
        let entity = if spec.terminal {
            self.entities.terminal
        } else {
            self.entities.processing
        };
        (u16::from(entity) << 8) | u16::from(self.entities.interface)
    }

    fn raw(&self, spec: &Spec, request: u8) -> Result<Vec<u8>, String> {
        let size = if request == 0x86 {
            1
        } else if spec.selector == 13 && spec.terminal {
            8
        } else {
            spec.width
        };
        let mut bytes = vec![0u8; size];
        let n = self
            .handle
            .read_control(
                0xa1,
                request,
                u16::from(spec.selector) << 8,
                self.index(spec),
                &mut bytes,
                TIMEOUT,
            )
            .map_err(|e| format!("{}: consulta UVC 0x{request:02x}: {e}", spec.label))?;
        if n != size {
            return Err(format!(
                "{}: resposta UVC incompleta ({n}/{size})",
                spec.label
            ));
        }
        Ok(bytes)
    }

    fn value(&self, spec: &Spec, request: u8) -> Result<i64, String> {
        decode_value(
            &self.raw(spec, request)?,
            spec.signed,
            spec.offset,
            spec.width,
        )
    }

    /// Returns only controls advertised by the device descriptors. Failed reads
    /// remain visible as errors instead of masquerading as zero-valued controls.
    pub fn controls(&self) -> Vec<ControlState> {
        SPECS
            .iter()
            .filter(|s| self.supported(s))
            .map(|s| {
                let info = self.raw(s, 0x86);
                let current = self.value(s, 0x81);
                let error = current
                    .as_ref()
                    .err()
                    .cloned()
                    .or_else(|| info.as_ref().err().cloned());
                ControlState {
                    id: s.id,
                    label: s.label.into(),
                    current: current.ok(),
                    min: self.value(s, 0x82).ok(),
                    max: self.value(s, 0x83).ok(),
                    // For AE mode GET_RES is a supported-mode bitmap, not a step.
                    step: if s.id == ControlId::ExposureAuto {
                        Some(1)
                    } else {
                        self.value(s, 0x84).ok()
                    },
                    default: self.value(s, 0x87).ok(),
                    writable: info.as_ref().is_ok_and(|b| b[0] & 2 != 0 && b[0] & 4 == 0),
                    error,
                }
            })
            .collect()
    }

    /// Validate against live device limits and write one standard control.
    /// Pan/tilt share a selector; the other axis is read immediately and retained.
    pub fn set(&self, id: ControlId, value: i64) -> Result<(), String> {
        let spec = SPECS
            .iter()
            .find(|s| s.id == id && self.supported(s))
            .ok_or("Controle não anunciado pela câmera")?;
        let info = self.raw(spec, 0x86)?[0];
        if info & 2 == 0 || info & 4 != 0 {
            return Err(format!(
                "{}: controle indisponível para escrita; confira o modo automático",
                spec.label
            ));
        }
        if id == ControlId::ExposureAuto {
            let mask = self.value(spec, 0x84)?;
            if value <= 0 || value > 8 || (value & (value - 1)) != 0 || mask & value == 0 {
                return Err(format!(
                    "Modo de exposição {value} não suportado (máscara {mask})"
                ));
            }
        } else {
            let min = self.value(spec, 0x82)?;
            let max = self.value(spec, 0x83)?;
            let step = self.value(spec, 0x84).unwrap_or(1).max(1);
            validate_range(value, min, max, step)?;
        }
        let payload = if spec.terminal && spec.selector == 13 {
            replace_axis(&self.raw(spec, 0x81)?, spec.offset, value)?
        } else {
            encode_value(value, spec.signed, spec.width)?
        };
        let n = self
            .handle
            .write_control(
                0x21,
                0x01,
                u16::from(spec.selector) << 8,
                self.index(spec),
                &payload,
                TIMEOUT,
            )
            .map_err(|e| format!("{}: escrita UVC: {e}", spec.label))?;
        if n != payload.len() {
            return Err(format!(
                "{}: escrita incompleta ({n}/{})",
                spec.label,
                payload.len()
            ));
        }
        // The attached Pixy acknowledges some class requests while no video
        // stream runs, but keeps its old brightness/AE/powerline state. An OS
        // transfer success is therefore insufficient evidence of application.
        let actual = self.value(spec, 0x81)?;
        if actual != value {
            return Err(format!("{}: câmera manteve {actual} após solicitar {value}. Inicie a prévia e confira o modo automático antes de tentar novamente.", spec.label));
        }
        Ok(())
    }
}

fn parse_entities(interface: u8, extra: &[u8]) -> Option<Entities> {
    let mut terminal = None;
    let mut processing = None;
    let mut pos = 0;
    while pos + 3 <= extra.len() {
        let len = usize::from(extra[pos]);
        if len < 3 || pos + len > extra.len() {
            return None;
        }
        let d = &extra[pos..pos + len];
        if d[1] == 0x24 {
            match d[2] {
                2 if len >= 15 && d[4..6] == [0x01, 0x02] => {
                    let size = usize::from(d[14]);
                    if size > 8 || 15 + size > len {
                        return None;
                    }
                    terminal = Some((d[3], bitmap(&d[15..15 + size])));
                }
                5 if len >= 8 => {
                    let size = usize::from(d[7]);
                    if size > 8 || 8 + size > len {
                        return None;
                    }
                    processing = Some((d[3], bitmap(&d[8..8 + size])));
                }
                _ => (),
            }
        }
        pos += len;
    }
    let (terminal, terminal_controls) = terminal?;
    let (processing, processing_controls) = processing?;
    Some(Entities {
        interface,
        terminal,
        processing,
        terminal_controls,
        processing_controls,
    })
}

fn bitmap(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .enumerate()
        .fold(0, |n, (i, b)| n | (u64::from(*b) << (i * 8)))
}

fn validate_range(value: i64, min: i64, max: i64, step: i64) -> Result<(), String> {
    if value < min || value > max {
        return Err(format!("Valor {value} fora do intervalo {min}…{max}"));
    }
    if (value - min) % step.max(1) != 0 {
        return Err(format!(
            "Valor {value} deve respeitar o incremento {step} a partir de {min}"
        ));
    }
    Ok(())
}

fn decode_value(bytes: &[u8], signed: bool, offset: usize, width: usize) -> Result<i64, String> {
    if !matches!(width, 1 | 2 | 4) || offset + width > bytes.len() {
        return Err("Comprimento UVC inválido".into());
    }
    let value = bitmap(&bytes[offset..offset + width]) as i64;
    Ok(if signed {
        (value << (64 - width * 8)) >> (64 - width * 8)
    } else {
        value
    })
}

fn encode_value(value: i64, signed: bool, width: usize) -> Result<Vec<u8>, String> {
    if !matches!(width, 1 | 2 | 4) {
        return Err("Comprimento UVC inválido".into());
    }
    let bits = width * 8;
    let (min, max) = if signed {
        (-(1i64 << (bits - 1)), (1i64 << (bits - 1)) - 1)
    } else {
        (0, (1i64 << bits) - 1)
    };
    if value < min || value > max {
        return Err("Valor não cabe no controle UVC".into());
    }
    Ok(value.to_le_bytes()[..width].to_vec())
}

fn replace_axis(current: &[u8], offset: usize, value: i64) -> Result<Vec<u8>, String> {
    if current.len() != 8 || !matches!(offset, 0 | 4) {
        return Err("Pan/tilt exige uma resposta de 8 bytes".into());
    }
    let mut bytes = current.to_vec();
    bytes[offset..offset + 4].copy_from_slice(&encode_value(value, true, 4)?);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brightness_is_signed_little_endian() {
        assert_eq!(decode_value(&[0xf6, 0xff], true, 0, 2).unwrap(), -10);
        assert_eq!(encode_value(-10, true, 2).unwrap(), [0xf6, 0xff]);
    }

    #[test]
    fn unsigned_exposure_uses_all_four_bytes() {
        assert_eq!(
            decode_value(&[0x70, 0x11, 0x01, 0], false, 0, 4).unwrap(),
            70000
        );
        assert_eq!(
            encode_value(70000, false, 4).unwrap(),
            [0x70, 0x11, 0x01, 0]
        );
        assert!(encode_value(-1, false, 4).is_err());
    }

    #[test]
    fn pan_write_preserves_tilt_and_rejects_truncated_payload() {
        let current = [0, 0, 0, 0, 0x20, 0x4e, 0, 0];
        assert_eq!(
            replace_axis(&current, 0, -3600).unwrap(),
            [0xf0, 0xf1, 0xff, 0xff, 0x20, 0x4e, 0, 0]
        );
        assert!(replace_axis(&current[..4], 4, 0).is_err());
    }

    #[test]
    fn descriptors_use_advertised_entity_ids_and_control_bitmaps() {
        let extra = [
            18, 0x24, 2, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0x6a, 0x0e, 2, 11, 0x24, 5, 3, 2, 0,
            64, 2, 0x7f, 0x17, 0,
        ];
        let parsed = parse_entities(0, &extra).unwrap();
        assert_eq!((parsed.terminal, parsed.processing), (1, 3));
        assert_eq!(parsed.terminal_controls, 0x020e6a);
        assert_eq!(parsed.processing_controls, 0x177f);
        assert!(parse_entities(0, &extra[..extra.len() - 1]).is_none());
    }

    #[test]
    fn rejects_out_of_range_and_off_step_motor_values() {
        assert!(validate_range(540001, -540000, 540000, 3600).is_err());
        assert!(validate_range(1, -540000, 540000, 3600).is_err());
        assert!(validate_range(-3600, -540000, 540000, 3600).is_ok());
    }
}
