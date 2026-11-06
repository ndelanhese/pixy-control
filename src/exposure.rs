//! Studio 2.0.5 EvHelper lookup, recovered from arm64 0x1004350e8.
//! UVC ExposureTimeAbsolute units are 100 microseconds. These EV labels denote
//! shutter-time steps, as in the original app, rather than exposure compensation.
pub const EV_RAW: &[(i64, i64)] = &[
    (-13, 1),
    (-12, 2),
    (-11, 5),
    (-10, 10),
    (-9, 20),
    (-8, 39),
    (-7, 78),
    (-6, 156),
    (-5, 312),
    (-4, 625),
    (-3, 1250),
    (-2, 2500),
    (-1, 5000),
];
/// Native deviceToUi chooses nearest absolute raw distance; equal distances
/// preserve the earlier (lower EV) entry. This deliberately is not log rounding.
pub fn exposure_raw_to_ev(raw: i64) -> Result<i64, String> {
    if raw <= 0 {
        return Err("Tempo de exposição inválido".into());
    }
    EV_RAW
        .iter()
        .min_by_key(|(_, value)| value.abs_diff(raw))
        .map(|(ev, _)| *ev)
        .ok_or_else(|| "Tabela de exposição vazia".into())
}
/// Return only exact Studio steps which the connected camera advertises.
/// Never clamp or quantize to a different shutter time behind the user's choice.
pub fn ev_to_raw(ev: i64, min: i64, max: i64, step: i64) -> Result<i64, String> {
    supported_ev(min, max, step)
        .into_iter()
        .find(|(value, _)| *value == ev)
        .map(|(_, raw)| raw)
        .ok_or_else(|| "EV fora dos passos suportados pela câmera".into())
}
pub fn supported_ev(min: i64, max: i64, step: i64) -> Vec<(i64, i64)> {
    if min <= 0 || min > max || step <= 0 {
        return Vec::new();
    }
    EV_RAW
        .iter()
        .copied()
        .filter(|(_, raw)| {
            *raw >= min
                && *raw <= max
                && (i128::from(*raw) - i128::from(min)) % i128::from(step) == 0
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_native_entry_roundtrips_exactly() {
        let expected = [
            (-13, 1),
            (-12, 2),
            (-11, 5),
            (-10, 10),
            (-9, 20),
            (-8, 39),
            (-7, 78),
            (-6, 156),
            (-5, 312),
            (-4, 625),
            (-3, 1250),
            (-2, 2500),
            (-1, 5000),
        ];
        assert_eq!(EV_RAW, expected);
        for (ev, raw) in expected {
            assert_eq!(ev_to_raw(ev, 1, 5000, 1).unwrap(), raw);
            assert_eq!(exposure_raw_to_ev(raw).unwrap(), ev);
        }
    }
    #[test]
    fn raw_values_use_nearest_raw_distance_and_lower_ev_on_tie() {
        assert_eq!(exposure_raw_to_ev(300).unwrap(), -5);
        assert_eq!(exposure_raw_to_ev(234).unwrap(), -6);
        assert_eq!(exposure_raw_to_ev(235).unwrap(), -5);
        assert_eq!(exposure_raw_to_ev(i64::MAX).unwrap(), -1);
        assert!(exposure_raw_to_ev(0).is_err());
        assert!(exposure_raw_to_ev(-1).is_err());
    }
    #[test]
    fn writes_require_exact_native_value_inside_live_range_and_step() {
        assert_eq!(ev_to_raw(-5, 1, 2000, 1).unwrap(), 312);
        assert!(ev_to_raw(-2, 1, 2000, 1).is_err());
        assert!(ev_to_raw(-5, 1, 5000, 2).is_err());
        assert!(ev_to_raw(0, 1, 5000, 1).is_err());
        assert!(ev_to_raw(-5, 5000, 1, 1).is_err());
        assert!(ev_to_raw(-5, 1, 5000, 0).is_err());
        assert_eq!(supported_ev(100, 700, 1), [(-6, 156), (-5, 312), (-4, 625)]);
    }
}
