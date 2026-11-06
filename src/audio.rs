//! Local Pixy USB microphone controls through macOS CoreAudio.
//! Does not change the default input or access/record microphone samples.

use serde::Serialize;
use std::{
    ffi::{c_char, c_void, CStr},
    mem::size_of,
};

#[derive(Clone, Debug, Serialize)]
pub struct AudioState {
    pub name: String,
    pub muted: Option<bool>,
    /// Input gain normalized to 0..1, as reported by the CoreAudio device.
    pub volume: Option<f32>,
    pub error: Option<String>,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Address {
    selector: u32,
    scope: u32,
    element: u32,
}

const SYSTEM: u32 = 1;
const GLOBAL: u32 = u32::from_be_bytes(*b"glob");
const INPUT: u32 = u32::from_be_bytes(*b"inpt");
const DEVICES: u32 = u32::from_be_bytes(*b"dev#");
const NAME: u32 = u32::from_be_bytes(*b"lnam");
const STREAMS: u32 = u32::from_be_bytes(*b"stm#");
const MUTE: u32 = u32::from_be_bytes(*b"mute");
const VOLUME: u32 = u32::from_be_bytes(*b"volm");

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyDataSize(
        object: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
    ) -> i32;
    fn AudioObjectGetPropertyData(
        object: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> i32;
    fn AudioObjectSetPropertyData(
        object: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: u32,
        data: *const c_void,
    ) -> i32;
    fn AudioObjectIsPropertySettable(
        object: u32,
        address: *const Address,
        settable: *mut u8,
    ) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringGetCString(
        string: *const c_void,
        buffer: *mut c_char,
        capacity: isize,
        encoding: u32,
    ) -> u8;
    fn CFRelease(value: *const c_void);
}

fn checked(status: i32, operation: &str) -> Result<(), String> {
    if status == 0 {
        Ok(())
    } else {
        Err(format!("CoreAudio {operation}: OSStatus {status}"))
    }
}

fn address(selector: u32, scope: u32, element: u32) -> Address {
    Address {
        selector,
        scope,
        element,
    }
}

fn property_size(object: u32, address: Address) -> Result<u32, String> {
    let mut size = 0;
    // CoreAudio writes a UInt32 size to a live, correctly aligned stack value.
    checked(
        unsafe { AudioObjectGetPropertyDataSize(object, &address, 0, std::ptr::null(), &mut size) },
        "tamanho da propriedade",
    )?;
    Ok(size)
}

fn read<T: Copy + Default>(object: u32, address: Address) -> Result<T, String> {
    let mut value = T::default();
    let mut size = u32::try_from(size_of::<T>()).map_err(|_| "Tipo CoreAudio grande demais")?;
    // Every caller uses a CoreAudio POD type with the exact property ABI.
    checked(
        unsafe {
            AudioObjectGetPropertyData(
                object,
                &address,
                0,
                std::ptr::null(),
                &mut size,
                (&mut value as *mut T).cast(),
            )
        },
        "ler propriedade",
    )?;
    if size as usize != size_of::<T>() {
        return Err("CoreAudio retornou tamanho inesperado".into());
    }
    Ok(value)
}

fn write<T: Copy>(object: u32, address: Address, value: T) -> Result<(), String> {
    let mut settable = 0;
    checked(
        unsafe { AudioObjectIsPropertySettable(object, &address, &mut settable) },
        "consultar escrita",
    )?;
    if settable == 0 {
        return Err("Controle de microfone é somente leitura".into());
    }
    checked(
        unsafe {
            AudioObjectSetPropertyData(
                object,
                &address,
                0,
                std::ptr::null(),
                size_of::<T>() as u32,
                (&value as *const T).cast(),
            )
        },
        "escrever propriedade",
    )
}

fn device_name(device: u32) -> Result<String, String> {
    let string = read::<*const c_void>(device, address(NAME, GLOBAL, 0))?;
    if string.is_null() {
        return Err("CoreAudio retornou nome vazio".into());
    }
    let mut buffer = vec![0 as c_char; 4096];
    let success = unsafe {
        CFStringGetCString(
            string,
            buffer.as_mut_ptr(),
            buffer.len() as isize,
            0x08000100,
        )
    };
    // The kAudioObjectPropertyName contract returns an owned CFStringRef.
    unsafe {
        CFRelease(string);
    }
    if success == 0 {
        return Err("Nome de dispositivo não pôde ser convertido em UTF-8".into());
    }
    Ok(unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned())
}

fn device() -> Result<(u32, String), String> {
    let prop = address(DEVICES, GLOBAL, 0);
    let bytes = property_size(SYSTEM, prop)?;
    if !(bytes as usize).is_multiple_of(size_of::<u32>()) || bytes > 65536 {
        return Err("Lista CoreAudio inválida".into());
    }
    let mut devices = vec![0u32; bytes as usize / size_of::<u32>()];
    let mut actual = bytes;
    checked(
        unsafe {
            AudioObjectGetPropertyData(
                SYSTEM,
                &prop,
                0,
                std::ptr::null(),
                &mut actual,
                devices.as_mut_ptr().cast(),
            )
        },
        "enumerar dispositivos",
    )?;
    if actual > bytes || !(actual as usize).is_multiple_of(size_of::<u32>()) {
        return Err("Lista CoreAudio mudou durante a leitura".into());
    }
    devices.truncate(actual as usize / size_of::<u32>());
    let mut inputs = Vec::new();
    for id in devices {
        if property_size(id, address(STREAMS, INPUT, 0)).unwrap_or(0) == 0 {
            continue;
        }
        if let Ok(name) = device_name(id) {
            inputs.push((id, name));
        }
    }
    choose_pixy(&inputs)
}

fn choose_pixy(devices: &[(u32, String)]) -> Result<(u32, String), String> {
    let mut matching = devices.iter().filter(|(_, name)| {
        name.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .eq_ignore_ascii_case("EMEET PIXY")
    });
    let selected = matching
        .next()
        .ok_or("Microfone EMEET PIXY não encontrado")?;
    if matching.next().is_some() {
        return Err("Mais de um microfone EMEET PIXY; dispositivo ambíguo".into());
    }
    Ok(selected.clone())
}

/// Read master input controls on the Pixy itself, never the system default input.
pub fn state() -> Result<AudioState, String> {
    let (device, name) = device()?;
    let mute = read::<u32>(device, address(MUTE, INPUT, 0));
    let volume = read::<f32>(device, address(VOLUME, INPUT, 0));
    let error = mute
        .as_ref()
        .err()
        .cloned()
        .or_else(|| volume.as_ref().err().cloned());
    let muted = mute.ok().filter(|v| *v <= 1).map(|v| v != 0);
    let volume = volume
        .ok()
        .filter(|v| v.is_finite() && (0.0..=1.0).contains(v));
    Ok(AudioState {
        name,
        muted,
        volume,
        error,
    })
}

pub fn set_mute(muted: bool) -> Result<(), String> {
    let (device, _) = device()?;
    let prop = address(MUTE, INPUT, 0);
    write(device, prop, u32::from(muted))?;
    let actual = read::<u32>(device, prop)?;
    if actual != u32::from(muted) {
        return Err("Microfone não confirmou o estado de mute solicitado".into());
    }
    Ok(())
}

pub fn set_volume_scalar(volume: f32) -> Result<(), String> {
    if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
        return Err("Volume deve estar entre 0 e 1".into());
    }
    let (device, _) = device()?;
    let prop = address(VOLUME, INPUT, 0);
    write(device, prop, volume)?;
    // UAC hardware quantizes gain; a returned scalar is authoritative.
    let actual = read::<f32>(device, prop)?;
    if !actual.is_finite() || !(0.0..=1.0).contains(&actual) {
        return Err("Microfone não retornou volume válido após escrita".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn picks_pixy_without_affecting_system_default_microphone() {
        let devices = vec![(1, "MacBook Microphone".into()), (140, "EMEET PIXY".into())];
        assert_eq!(choose_pixy(&devices).unwrap().0, 140);
    }
    #[test]
    fn refuses_ambiguous_device_and_different_pixy_model() {
        assert!(choose_pixy(&[(1, "EMEET PIXY 2K".into())]).is_err());
        assert!(choose_pixy(&[(1, "EMEET PIXY".into()), (2, "emeet pixy".into())]).is_err());
    }
}
