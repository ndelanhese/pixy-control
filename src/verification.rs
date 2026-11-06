//! Explicit diagnostics; never run during normal app startup.
use crate::{
    hid::{self, PixyHid},
    model,
    preview::Preview,
};
use serde_json::json;
use std::{
    path::Path,
    thread,
    time::{Duration, Instant},
};

pub fn launch(
    run: fn(&Path) -> Result<(), String>,
    path: std::path::PathBuf,
) -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_inner_size([560.0, 180.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Pixy Control · Verificação",
        options,
        Box::new(move |_| {
            let (tx, rx) = std::sync::mpsc::channel();
            thread::spawn(move || {
                let result = run(&path);
                let _ = tx.send(result);
            });
            Ok(Box::new(DiagnosticApp { rx }))
        }),
    )
}

struct DiagnosticApp {
    rx: std::sync::mpsc::Receiver<Result<(), String>>,
}
impl eframe::App for DiagnosticApp {
    fn update(&mut self, ctx: &eframe::egui::Context, _: &mut eframe::Frame) {
        eframe::egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Verificando a Pixy");
            ui.label("Formatos, leituras e restauração são registrados no relatório JSON.");
            ui.spinner();
        });
        if let Ok(result) = self.rx.try_recv() {
            if let Err(e) = result {
                eprintln!("{e}");
            }
            ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Close);
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

fn first_frame(preview: &Preview) -> Result<(u32, u32, Vec<u8>), String> {
    let deadline = Instant::now() + Duration::from_secs(35);
    while Instant::now() < deadline {
        if let Some(frame) = preview.latest() {
            return Ok(frame);
        }
        if preview.is_finished() {
            return Err(preview.status());
        }
        thread::sleep(Duration::from_millis(30));
    }
    Err(preview.status())
}

fn stop(preview: &mut Preview) -> Result<(), String> {
    preview.stop();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !preview.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    if !preview.is_finished() {
        return Err("A sessão não confirmou encerramento".into());
    }
    Ok(())
}

pub fn streams(path: &Path) -> Result<(), String> {
    let mut checks = Vec::new();
    for (w, h, fps) in model::FORMATS {
        let mut preview = Preview::start(w, h, fps);
        let result = (|| -> Result<serde_json::Value, String> {
            let (width, height, rgb) = first_frame(&preview)?;
            if (width, height) != (w, h) || rgb.len() != w as usize * h as usize * 3 {
                return Err("Dimensões de frame divergentes".into());
            }
            thread::sleep(Duration::from_secs(1));
            let begin = Instant::now();
            let before = preview.timing();
            thread::sleep(Duration::from_secs(2));
            let elapsed = begin.elapsed().as_secs_f64();
            let after = preview.timing();
            let delivered = after.frames.saturating_sub(before.frames);
            let pts_seconds = match (before.last_pts_ns, after.last_pts_ns) {
                (Some(a), Some(b)) if b > a => (b - a) as f64 / 1e9,
                _ => return Err("Timestamps nativos ausentes ou não crescentes".into()),
            };
            let measured_fps = delivered as f64 / pts_seconds;
            Ok(
                json!({"width":width,"height":height,"rgb_bytes":rgb.len(),"frames":delivered,"sample_seconds":elapsed,"pts_seconds":pts_seconds,"measured_fps":measured_fps,"wall_fps":delivered as f64/elapsed,"fps_within_tolerance":(measured_fps-fps as f64).abs() <= fps as f64*0.15,"status":preview.status()}),
            )
        })();
        let diagnostics = preview.diagnostics();
        let timing = preview.timing();
        let shutdown = stop(&mut preview);
        let passed = result
            .as_ref()
            .is_ok_and(|v| v["fps_within_tolerance"] == true)
            && shutdown.is_ok();
        checks.push(json!({"requested":[w,h,fps],"passed":passed,"observed":result.as_ref().ok(),"error":result.as_ref().err(),"shutdown_error":shutdown.as_ref().err(),"diagnostics":diagnostics,"last_timing":{"frames":timing.frames,"first_pts_ns":timing.first_pts_ns,"last_pts_ns":timing.last_pts_ns,"width":timing.width,"height":timing.height}}));
        if shutdown.is_err() {
            break;
        }
        if result
            .as_ref()
            .is_err_and(|e| e.to_lowercase().contains("permiss"))
        {
            break;
        }
    }
    let passed = checks.iter().filter(|v| v["passed"] == true).count();
    model::save_json(
        path,
        &json!({"test":"native_avfoundation_formats","passed":passed,"total":model::FORMATS.len(),"checks":checks}),
    )?;
    if passed != model::FORMATS.len() {
        return Err(format!(
            "{passed}/{} formatos passaram; consulte {}",
            model::FORMATS.len(),
            path.display()
        ));
    }
    Ok(())
}

pub fn hardware(path: &Path) -> Result<(), String> {
    let mut preview = Preview::start(1920, 1080, 30);
    first_frame(&preview)?;
    let mut camera = PixyHid::connect()?;
    let original = camera.get_byte(hid::commands::GET_DEVICE_MODE)?;
    hid::require_ack(&camera.set_byte(hid::commands::SET_DEVICE_MODE, 0)?)?;
    let mut result = crate::hardware_check::run();
    let restored = (|| -> Result<u8, String> {
        hid::require_ack(&camera.set_byte(
            hid::commands::SET_DEVICE_MODE,
            hid::normalize_device_mode(original),
        )?)?;
        let current = camera.get_byte(hid::commands::GET_DEVICE_MODE)?;
        if hid::normalize_device_mode(current) != hid::normalize_device_mode(original) {
            return Err("Modo original não restaurado".into());
        }
        Ok(current)
    })();
    result["mode_restore"] = json!({"original":original,"actual":restored.as_ref().ok(),"error":restored.as_ref().err()});
    let stopped = stop(&mut preview);
    result["preview_shutdown_error"] = json!(stopped.as_ref().err());
    model::save_json(path, &result)?;
    if result["summary"]["failed"] != 0
        || result["summary"]["restoration_complete"] != true
        || restored.is_err()
        || stopped.is_err()
    {
        return Err(format!(
            "Verificação incompleta; consulte {}",
            path.display()
        ));
    }
    Ok(())
}

/// Reversible long privacy interval; observe whether this firmware stalls video.
pub fn privacy(path: &Path) -> Result<(), String> {
    let mut camera = PixyHid::connect()?;
    let original = camera.get_byte(hid::commands::GET_DEVICE_MODE)?;
    let mut preview = Preview::start(1920, 1080, 30);
    let result = (|| -> Result<serde_json::Value, String> {
        first_frame(&preview)?;
        hid::require_ack(&camera.set_byte(hid::commands::SET_DEVICE_MODE, 2)?)?;
        let private = camera.get_byte(hid::commands::GET_DEVICE_MODE)?;
        if private != 2 {
            return Err("Modo privado não confirmado".into());
        }
        thread::sleep(Duration::from_secs(11));
        let finished_in_privacy = preview.is_finished();
        let privacy_status = preview.status();
        hid::require_ack(&camera.set_byte(hid::commands::SET_DEVICE_MODE, 0)?)?;
        if finished_in_privacy {
            stop(&mut preview)?;
            preview = Preview::start(1920, 1080, 30);
        }
        while preview.latest().is_some() {}
        let (w, h, _) = first_frame(&preview)?;
        Ok(
            json!({"privacy_mode":private,"held_seconds":11,"preview_finished_in_privacy":finished_in_privacy,"privacy_status":privacy_status,"resumed":[w,h],"resumed_status":preview.status()}),
        )
    })();
    let restored = (|| -> Result<(), String> {
        hid::require_ack(&camera.set_byte(
            hid::commands::SET_DEVICE_MODE,
            hid::normalize_device_mode(original),
        )?)?;
        let actual = camera.get_byte(hid::commands::GET_DEVICE_MODE)?;
        if hid::normalize_device_mode(actual) != hid::normalize_device_mode(original) {
            return Err("Modo original não restaurado".into());
        }
        Ok(())
    })();
    let shutdown = stop(&mut preview);
    let passed = result.is_ok() && restored.is_ok() && shutdown.is_ok();
    model::save_json(
        path,
        &json!({"test":"privacy_resume","passed":passed,"original_mode":original,"observed":result.as_ref().ok(),"error":result.as_ref().err(),"restore_error":restored.as_ref().err(),"shutdown_error":shutdown.as_ref().err()}),
    )?;
    if passed {
        Ok(())
    } else {
        Err("Falha no ciclo de privacidade; consulte JSON".into())
    }
}

/// Save/clear only when no user startup preset exists; never overwrite one.
pub fn startup(path: &Path) -> Result<(), String> {
    let mut camera = PixyHid::connect()?;
    let before = camera.startup_position()?;
    if before.mode != 0 {
        return model::save_json(
            path,
            &json!({"test":"startup_position_roundtrip","status":"skipped","original_mode":before.mode,"reason":"Posição inicial existente preservada"}),
        );
    }
    let saved = (|| -> Result<[f32; 3], String> {
        camera.save_startup_position()?;
        let value = camera.startup_position()?;
        if value.mode != 1 {
            return Err("Posição inicial não foi habilitada".into());
        }
        value
            .coordinates
            .ok_or("Posição inicial não devolveu coordenadas".into())
    })();
    // Always clear after the save attempt, including a failed readback.
    let restored = (|| -> Result<(), String> {
        camera.clear_startup_position()?;
        if camera.startup_position()?.mode != 0 {
            return Err("Posição inicial não voltou ao estado original vazio".into());
        }
        Ok(())
    })();
    let passed = saved.is_ok() && restored.is_ok();
    model::save_json(
        path,
        &json!({"test":"startup_position_roundtrip","passed":passed,"original_mode":before.mode,"saved_coordinates":saved.as_ref().ok(),"error":saved.as_ref().err(),"restored":restored.is_ok(),"restore_error":restored.as_ref().err()}),
    )?;
    if passed {
        Ok(())
    } else {
        Err("Falha na posição inicial; consulte JSON".into())
    }
}
