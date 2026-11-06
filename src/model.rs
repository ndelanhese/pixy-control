#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_or_unknown_controls_never_generate_writes() {
        let filters = filters();
        let mut supported = std::collections::BTreeMap::new();
        supported.insert("Brightness".to_string(), (0, 255));
        assert_eq!(
            filter_writes(&filters[1], &supported).unwrap(),
            vec![("Brightness".to_string(), 138)]
        );
        supported.insert("Contrast".to_string(), (0, 100));
        assert!(filter_writes(&filters[1], &supported).is_err());
    }

    #[test]
    fn preset_corruption_and_nonfinite_positions_are_rejected() {
        assert!(Preset::parse(r#"{"name":"desk","position":[0,999,0],"controls":{}}"#).is_err());
        assert!(Preset::parse("{}").is_err());
        let p = Preset {
            id: 1,
            name: "Desk".into(),
            position: [0.0, 0.0, 0.0],
            controls: Default::default(),
            thumbnail: None,
        };
        assert!(Preset::parse(&serde_json::to_string(&p).unwrap()).is_ok());
    }
}
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_PRESET_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Serialize, Deserialize)]
pub struct Preset {
    #[serde(default)]
    pub id: u64,
    pub name: String,
    pub position: [f32; 3],
    pub controls: BTreeMap<String, i64>,
    #[serde(default)]
    pub thumbnail: Option<String>,
}

impl Preset {
    pub fn parse(json: &str) -> Result<Self, String> {
        let p: Self = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if p.name.trim().is_empty()
            || p.name.chars().count() > 120
            || p.position.iter().any(|p| !p.is_finite() || p.abs() > 180.0)
        {
            return Err("Preset inválido".into());
        }
        if p.thumbnail.as_ref().is_some_and(|name| {
            name.len() > 80
                || !name.starts_with("preset-")
                || !name.ends_with(".jpg")
                || name.contains('/')
                || name.contains('\\')
        }) {
            return Err("Miniatura inválida".into());
        }
        Ok(p)
    }
}

pub fn load_presets(path: &std::path::Path) -> Result<Vec<Preset>, String> {
    let content = match std::fs::read_to_string(path) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    parse_presets(&content)
}

pub fn parse_presets(content: &str) -> Result<Vec<Preset>, String> {
    let values: Vec<serde_json::Value> =
        serde_json::from_str(content).map_err(|e| e.to_string())?;
    let mut presets: Vec<Preset> = values
        .into_iter()
        .map(|v| Preset::parse(&v.to_string()))
        .collect::<Result<_, _>>()?;
    if presets.len() > 8 {
        return Err("Arquivo contém mais de oito presets".into());
    }
    let used = checked_preset_ids(&presets, true)?;
    // Deterministic legacy migration: an unchanged legacy file gets the same
    // IDs on every load, even before the user next saves it. Existing IDs stay.
    let mut next = used
        .last()
        .copied()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or("Identificadores de preset esgotados")?;
    for preset in &mut presets {
        if preset.id == 0 {
            preset.id = next;
            next = next
                .checked_add(1)
                .ok_or("Identificadores de preset esgotados")?;
        }
    }
    NEXT_PRESET_ID.fetch_max(next, Ordering::AcqRel);
    Ok(presets)
}

pub fn new_preset_id() -> Result<u64, String> {
    NEXT_PRESET_ID
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |next| {
            next.checked_add(1)
        })
        .map_err(|_| "Identificadores de preset esgotados".into())
}

fn checked_preset_ids(presets: &[Preset], allow_missing: bool) -> Result<BTreeSet<u64>, String> {
    let mut ids = BTreeSet::new();
    for preset in presets {
        if preset.id == 0 {
            if allow_missing {
                continue;
            } else {
                return Err("Preset sem identificador estável".into());
            }
        }
        if !ids.insert(preset.id) {
            return Err(format!("Identificador de preset duplicado: {}", preset.id));
        }
    }
    Ok(ids)
}

pub fn apply_captured_preset(
    presets: &mut Vec<Preset>,
    target: Option<u64>,
    captured: Preset,
) -> Result<(), String> {
    checked_preset_ids(presets, false)?;
    if captured.id == 0 {
        return Err("Captura de preset sem identificador estável".into());
    }
    if let Some(id) = target {
        if id != captured.id {
            return Err("Resposta de captura não corresponde ao preset solicitado".into());
        }
        let existing = presets
            .iter_mut()
            .find(|preset| preset.id == id)
            .ok_or("O preset foi excluído antes de a captura terminar")?;
        *existing = captured;
    } else {
        if presets.len() >= 8 {
            return Err("Limite de oito presets atingido".into());
        }
        if presets.iter().any(|preset| preset.id == captured.id) {
            return Err("Captura retornou identificador de preset duplicado".into());
        }
        presets.push(captured);
    }
    Ok(())
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    fn preset(id: u64, name: &str) -> Preset {
        Preset {
            id,
            name: name.into(),
            position: [0.0; 3],
            controls: Default::default(),
            thumbnail: None,
        }
    }

    #[test]
    fn asynchronous_capture_updates_stable_target_after_an_earlier_item_is_deleted() {
        let mut presets = vec![preset(101, "A"), preset(202, "B"), preset(303, "C")];
        let mut captured = presets[1].clone();
        captured.position = [12.0, 8.0, 0.0];
        presets.remove(0);
        apply_captured_preset(&mut presets, Some(202), captured).unwrap();
        assert_eq!(presets[0].id, 202);
        assert_eq!(presets[0].position, [12.0, 8.0, 0.0]);
        assert_eq!(presets[1].id, 303);
        assert_eq!(presets[1].position, [0.0; 3]);
    }

    #[test]
    fn deleted_capture_target_never_overwrites_a_replacement_or_resurrects_itself() {
        let mut presets = vec![preset(404, "New entry")];
        assert!(
            apply_captured_preset(&mut presets, Some(202), preset(202, "Deleted entry")).is_err()
        );
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].id, 404);
        assert!(apply_captured_preset(&mut presets, None, preset(404, "Duplicate")).is_err());
    }

    #[test]
    fn legacy_ids_migrate_deterministically_and_explicit_duplicates_fail() {
        let legacy = r#"[{"name":"Legacy","position":[0,0,0],"controls":{}},{"id":20,"name":"Existing","position":[0,0,0],"controls":{}}]"#;
        let first = parse_presets(legacy).unwrap();
        let second = parse_presets(legacy).unwrap();
        assert_ne!(first[0].id, 0);
        assert_ne!(first[0].id, 20);
        assert_eq!(first[0].id, second[0].id);
        assert_eq!(first[1].id, 20);
        let duplicate =
            serde_json::to_string(&vec![preset(42, "First"), preset(42, "Second")]).unwrap();
        assert!(parse_presets(&duplicate).is_err());
    }

    #[test]
    fn generated_capture_ids_are_reserved_above_loaded_ids_and_never_reused() {
        let existing = new_preset_id().unwrap();
        let mut saved = preset(existing + 100, "Saved");
        let parsed = parse_presets(&serde_json::to_string(&vec![saved.clone()]).unwrap()).unwrap();
        assert_eq!(parsed[0].id, saved.id);
        let first = new_preset_id().unwrap();
        let second = new_preset_id().unwrap();
        assert!(first > saved.id);
        assert!(second > first);
        saved.id = second;
        let mut active = vec![saved];
        assert!(apply_captured_preset(&mut active, Some(first), preset(first, "Deleted")).is_err());
        assert_eq!(active[0].id, second);
    }
}

pub fn save_presets(path: &std::path::Path, presets: &[Preset]) -> Result<(), String> {
    checked_preset_ids(presets, false)?;
    if presets.len() > 8 {
        return Err("Limite de oito presets atingido".into());
    }
    save_json(path, presets)
}

pub fn save_json<T: Serialize + ?Sized>(path: &std::path::Path, value: &T) -> Result<(), String> {
    let parent = path.parent().ok_or("Caminho inválido")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    use std::io::Write;
    let mut file = std::fs::File::create(&temporary).map_err(|e| e.to_string())?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    std::fs::rename(&temporary, path).map_err(|e| e.to_string())
}

pub struct Filter {
    pub name: &'static str,
    pub values: [i64; 6],
    pub white_balance_auto: bool,
}

pub const FILTER_KEYS: [&str; 6] = [
    "Brightness",
    "Contrast",
    "Sharpness",
    "Saturation",
    "Hue",
    "WhiteBalance",
];

pub fn filters() -> Vec<Filter> {
    vec![
        Filter {
            name: "Padrão",
            values: [128, 128, 128, 128, 128, 5000],
            white_balance_auto: true,
        },
        Filter {
            name: "Claro",
            values: [138, 136, 128, 128, 128, 5000],
            white_balance_auto: true,
        },
        Filter {
            name: "Nostalgia",
            values: [128, 128, 100, 128, 128, 7500],
            white_balance_auto: false,
        },
        Filter {
            name: "Blues",
            values: [128, 128, 128, 128, 128, 5000],
            white_balance_auto: false,
        },
        Filter {
            name: "Frio",
            values: [128, 118, 132, 138, 128, 5000],
            white_balance_auto: true,
        },
        Filter {
            name: "Vívido",
            values: [130, 132, 128, 140, 128, 5000],
            white_balance_auto: true,
        },
    ]
}

pub fn filter_writes(
    filter: &Filter,
    supported: &BTreeMap<String, (i64, i64)>,
) -> Result<Vec<(String, i64)>, String> {
    let mut plan = Vec::new();
    for (key, value) in FILTER_KEYS.iter().zip(filter.values) {
        if let Some(&(min, max)) = supported.get(*key) {
            if !(min..=max).contains(&value) {
                return Err(format!("{key}: receita fora da faixa do dispositivo"));
            }
            plan.push((key.to_string(), value));
        }
    }
    Ok(plan)
}

pub const FORMATS: [(u32, u32, u32); 10] = [
    (3840, 2160, 30),
    (2560, 1440, 30),
    (1920, 1080, 60),
    (1920, 1080, 30),
    (1280, 720, 30),
    (1280, 720, 60),
    (1024, 576, 30),
    (1024, 576, 60),
    (640, 360, 30),
    (640, 360, 60),
];

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PtzPreferences {
    pub reverse: bool,
    pub orientation: u8,
}

pub fn ptz_direction(
    mut direction: u8,
    reverse: bool,
    orientation: u8,
) -> Result<(u8, i64), String> {
    if direction > 3 || orientation > 3 {
        return Err("Direção inválida".into());
    }
    if reverse {
        direction = match direction {
            2 => 3,
            3 => 2,
            v => v,
        };
    }
    direction = match orientation {
        0 => direction,
        1 => [3, 2, 1, 0][direction as usize],
        2 => [1, 0, 3, 2][direction as usize],
        3 => [2, 3, 0, 1][direction as usize],
        _ => unreachable!(),
    };
    Ok(match direction {
        0 => (2, 1),
        1 => (2, -1),
        2 => (1, 1),
        _ => (1, -1),
    })
}

#[cfg(test)]
mod ptz_tests {
    use super::*;
    #[test]
    fn placement_and_reverse_match_native_input_mapping() {
        assert_eq!(ptz_direction(2, false, 0).unwrap(), (1, 1));
        assert_eq!(ptz_direction(2, true, 0).unwrap(), (1, -1));
        assert_eq!(ptz_direction(0, false, 1).unwrap(), (1, -1));
        assert_eq!(ptz_direction(0, false, 2).unwrap(), (2, -1));
        assert_eq!(ptz_direction(3, false, 3).unwrap(), (2, -1));
        assert!(ptz_direction(4, false, 0).is_err());
    }
}
