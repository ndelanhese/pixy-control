//! Live level meter. Audio samples are measured in the callback and discarded.
//! The CoreAudio stream belongs to a worker so initialization and TCC never
//! block the interface; dropping Meter requests prompt stream teardown.

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    FromSample, Sample, SampleFormat, SizedSample,
};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

pub struct Meter {
    level: Arc<AtomicU32>,
    status: Arc<Mutex<String>>,
    stop: Arc<AtomicBool>,
}

impl Meter {
    pub fn start() -> Self {
        let meter = Self {
            level: Arc::new(AtomicU32::new(0f32.to_bits())),
            status: Arc::new(Mutex::new(
                "Iniciando microfone; aguardando permissão do macOS…".into(),
            )),
            stop: Arc::new(AtomicBool::new(false)),
        };
        let level = meter.level.clone();
        let status = meter.status.clone();
        let stop = meter.stop.clone();
        let failure_status = status.clone();
        if let Err(error) = thread::Builder::new()
            .name("pixy-level-meter".into())
            .spawn(move || {
                if let Err(error) = run(level.clone(), status.clone(), stop) {
                    level.store(0f32.to_bits(), Ordering::Relaxed);
                    update_status(&status, &error);
                }
            })
        {
            update_status(
                &failure_status,
                &format!("Não foi possível iniciar medidor: {error}"),
            );
        }
        meter
    }

    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    pub fn status(&self) -> String {
        self.status
            .lock()
            .map(|value| value.clone())
            .unwrap_or_else(|_| "Estado do medidor indisponível".into())
    }
}

impl Drop for Meter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.level.store(0f32.to_bits(), Ordering::Relaxed);
    }
}

fn update_status(status: &Mutex<String>, text: &str) {
    if let Ok(mut state) = status.lock() {
        *state = text.to_owned();
    }
}

fn is_pixy(name: &str) -> bool {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .eq_ignore_ascii_case("EMEET PIXY")
}

fn run(
    level: Arc<AtomicU32>,
    status: Arc<Mutex<String>>,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    let host = cpal::default_host();
    let mut selected = None;
    for device in host
        .input_devices()
        .map_err(|error| format!("Enumerar entradas de áudio: {error}"))?
    {
        if device.name().ok().is_some_and(|name| is_pixy(&name)) {
            if selected.is_some() {
                return Err("Mais de um microfone EMEET PIXY; dispositivo ambíguo".into());
            }
            selected = Some(device);
        }
    }
    let device = selected.ok_or("Microfone EMEET PIXY não encontrado")?;
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }
    let supported = device
        .default_input_config()
        .map_err(|error| format!("Formato de áudio da Pixy: {error}"))?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let failed = Arc::new(AtomicBool::new(false));
    macro_rules! build {
        ($type:ty) => {
            build::<$type>(
                &device,
                &config,
                level.clone(),
                status.clone(),
                failed.clone(),
            )?
        };
    }
    let stream = match format {
        SampleFormat::I8 => build!(i8),
        SampleFormat::I16 => build!(i16),
        SampleFormat::I32 => build!(i32),
        SampleFormat::I64 => build!(i64),
        SampleFormat::U8 => build!(u8),
        SampleFormat::U16 => build!(u16),
        SampleFormat::U32 => build!(u32),
        SampleFormat::U64 => build!(u64),
        SampleFormat::F32 => build!(f32),
        SampleFormat::F64 => build!(f64),
        other => return Err(format!("Formato do microfone não suportado: {other}")),
    };
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }
    update_status(
        &status,
        &format!(
            "EMEET PIXY · {} Hz · {} canal(is)",
            config.sample_rate.0, config.channels
        ),
    );
    stream.play().map_err(|error| {
        format!("Iniciar medidor; confira permissão de microfone no macOS: {error}")
    })?;
    while !stop.load(Ordering::Acquire) && !failed.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(50));
    }
    drop(stream);
    level.store(0f32.to_bits(), Ordering::Relaxed);
    Ok(())
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    level: Arc<AtomicU32>,
    status: Arc<Mutex<String>>,
    failed: Arc<AtomicBool>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + Sample,
    f32: FromSample<T>,
{
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                let measured = rms(data.iter().copied().map(|sample| sample.to_sample::<f32>()));
                // Fast attack with a short release, and no allocation/lock/audio copy.
                let previous = f32::from_bits(level.load(Ordering::Relaxed));
                level.store(measured.max(previous * 0.88).to_bits(), Ordering::Relaxed);
            },
            move |error| {
                update_status(&status, &format!("Microfone: {error}"));
                failed.store(true, Ordering::Release);
            },
            None,
        )
        .map_err(|error| format!("Abrir microfone; confira permissão do macOS: {error}"))
}

fn rms(samples: impl Iterator<Item = f32>) -> f32 {
    let mut sum = 0f64;
    let mut count = 0u64;
    for sample in samples.filter(|sample| sample.is_finite()) {
        let sample = f64::from(sample.clamp(-1.0, 1.0));
        sum += sample * sample;
        count += 1;
    }
    if count == 0 {
        0.0
    } else {
        (sum / count as f64).sqrt() as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rms_matches_signal_energy_and_ignores_invalid_samples() {
        assert!((rms([0.5, -0.5, 0.5, -0.5].into_iter()) - 0.5).abs() < 0.00001);
        assert_eq!(rms([f32::NAN, f32::INFINITY].into_iter()), 0.0);
        assert_eq!(rms([].into_iter()), 0.0);
        assert_eq!(rms([2.0, -2.0].into_iter()), 1.0);
    }

    #[test]
    fn exact_pixy_selection_excludes_other_models_and_computer_microphone() {
        assert!(is_pixy("EMEET PIXY"));
        assert!(is_pixy("emeet  pixy"));
        assert!(!is_pixy("EMEET PIXY 2K"));
        assert!(!is_pixy("MacBook Microphone"));
    }
}
