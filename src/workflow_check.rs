//! Explicit local capture/recording diagnostics. Never runs on ordinary startup.
use crate::{
    model,
    preview::{CaptureTiming, Preview},
    recording::Recorder,
    transform::{self, Aspect, FrameTransform},
};
use image::ImageEncoder;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;
const FPS: u32 = 30;
const TOTAL: usize = 7;
#[derive(Clone, Copy)]
struct CaptureFormat {
    width: u32,
    height: u32,
    fps: u32,
}
const NORMAL: CaptureFormat = CaptureFormat {
    width: WIDTH,
    height: HEIGHT,
    fps: FPS,
};

/// Records only on explicit diagnostic invocation. Camera modes/UVC/HID values
/// are never written. Report and genuine camera artifacts are saved together.
pub fn run(path: &Path) -> Result<(), String> {
    let directory = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let ffprobe = if Path::new("/opt/homebrew/bin/ffprobe").is_file() {
        PathBuf::from("/opt/homebrew/bin/ffprobe")
    } else {
        PathBuf::from("ffprobe")
    };
    if let Err(error) = command_output(
        Command::new(&ffprobe).arg("-version"),
        Duration::from_secs(5),
    ) {
        model::save_json(
            path,
            &json!({"test":"native_capture_record_workflows","passed":0,"total":TOTAL,"error":error}),
        )?;
        return Err(error);
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let prefix = format!("{stem}-{timestamp}");
    let mut preview = Preview::start(WIDTH, HEIGHT, FPS);
    let mut checks = Vec::new();
    let mut fatal = None;
    match first_frame(&preview) {
        Ok((w, h, rgb)) => {
            let photo = directory.join(format!("{prefix}-camera.png"));
            let saved = (|| -> Result<(), String> {
                if (w, h) != (WIDTH, HEIGHT) || rgb.len() != WIDTH as usize * HEIGHT as usize * 3 {
                    return Err("Frame nativo com dimensões inesperadas".into());
                }
                let file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&photo)
                    .map_err(|e| e.to_string())?;
                image::codecs::png::PngEncoder::new(file)
                    .write_image(&rgb, w, h, image::ExtendedColorType::Rgb8)
                    .map_err(|e| e.to_string())
            })();
            checks.push(json!({"workflow":"camera_png","passed":saved.is_ok(),"path":photo,"width":w,"height":h,"rgb_bytes":rgb.len(),"error":saved.err()}));
            let perspective = FrameTransform {
                quad: Some([[0.12, 0.08], [0.88, 0.04], [0.95, 0.92], [0.06, 0.95]]),
                ..Default::default()
            };
            for (name, audio, view) in [
                ("silent", false, FrameTransform::default()),
                ("pixy_microphone", true, FrameTransform::default()),
                ("perspective", false, perspective),
                (
                    "digital_zoom_4x",
                    false,
                    FrameTransform {
                        zoom: 4.0,
                        ..Default::default()
                    },
                ),
                (
                    "portrait",
                    false,
                    FrameTransform {
                        aspect: Aspect::Portrait,
                        ..Default::default()
                    },
                ),
                (
                    "rotation_90",
                    false,
                    FrameTransform {
                        rotation: 1,
                        ..Default::default()
                    },
                ),
            ] {
                if preview.is_finished() {
                    fatal = Some(preview.status());
                    break;
                }
                let output = directory.join(format!("{prefix}-{name}.mp4"));
                checks.push(record_case(
                    &preview, &output, &ffprobe, name, audio, view, NORMAL,
                ));
            }
        }
        Err(error) => fatal = Some(error),
    }
    let diagnostics = preview.diagnostics();
    let timing = preview.timing();
    let shutdown = stop_preview(&mut preview);
    let passed = checks.iter().filter(|v| v["passed"] == true).count();
    let complete = passed == TOTAL && checks.len() == TOTAL && fatal.is_none() && shutdown.is_ok();
    model::save_json(
        path,
        &json!({"test":"native_capture_record_workflows","passed":passed,"total":TOTAL,"complete":complete,"checks":checks,"error":fatal,"preview_shutdown_error":shutdown.as_ref().err(),"capture":timing_json(timing),"diagnostics":diagnostics,"uvc_hid_settings_written":false}),
    )?;
    if complete {
        Ok(())
    } else {
        Err(format!(
            "{passed}/{TOTAL} fluxos passaram; consulte {}",
            path.display()
        ))
    }
}
/// Explicit end-to-end stress capture, including native callbacks, perspective
/// processing, bounded recorder input, finalization and actual encoded packets.
pub fn stress(path: &Path) -> Result<(), String> {
    let directory = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let ffprobe = if Path::new("/opt/homebrew/bin/ffprobe").is_file() {
        PathBuf::from("/opt/homebrew/bin/ffprobe")
    } else {
        PathBuf::from("ffprobe")
    };
    if let Err(error) = command_output(
        Command::new(&ffprobe).arg("-version"),
        Duration::from_secs(5),
    ) {
        model::save_json(
            path,
            &json!({"test":"native_pipeline_stress","passed":0,"total":2,"complete":false,"error":error}),
        )?;
        return Err(error);
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let mut checks = Vec::new();
    let mut fatal = None;
    for (name, format, view) in [
        (
            "perspective_4k30",
            CaptureFormat {
                width: 3840,
                height: 2160,
                fps: 30,
            },
            FrameTransform {
                quad: Some([[0.12, 0.08], [0.88, 0.04], [0.95, 0.92], [0.06, 0.95]]),
                ..Default::default()
            },
        ),
        (
            "native_1080p60",
            CaptureFormat {
                width: 1920,
                height: 1080,
                fps: 60,
            },
            FrameTransform::default(),
        ),
    ] {
        let mut preview = Preview::start(format.width, format.height, format.fps);
        let output = directory.join(format!("{stem}-{timestamp}-{name}.mp4"));
        let mut result = match first_frame(&preview) {
            Ok((w, h, rgb))
                if (w, h) == (format.width, format.height)
                    && rgb.len() == w as usize * h as usize * 3 =>
            {
                record_case(&preview, &output, &ffprobe, name, false, view, format)
            }
            Ok((w, h, _)) => {
                json!({"workflow":name,"passed":false,"error":format!("Frame recebido{w}x{h}; esperado{}x{}",format.width,format.height)})
            }
            Err(error) => json!({"workflow":name,"passed":false,"error":error}),
        };
        result["requested_capture"] =
            json!({"width":format.width,"height":format.height,"fps":format.fps});
        result["capture_final"] = timing_json(preview.timing());
        result["diagnostics"] = json!(preview.diagnostics());
        let shutdown = stop_preview(&mut preview);
        result["preview_shutdown_error"] = json!(shutdown.as_ref().err());
        if let Err(error) = shutdown {
            result["passed"] = json!(false);
            fatal = Some(error);
        }
        checks.push(result);
        if fatal.is_some() {
            break;
        }
    }
    let passed = checks.iter().filter(|v| v["passed"] == true).count();
    let complete = passed == 2 && checks.len() == 2 && fatal.is_none();
    model::save_json(
        path,
        &json!({"test":"native_pipeline_stress","passed":passed,"total":2,"complete":complete,"checks":checks,"error":fatal,"uvc_hid_settings_written":false}),
    )?;
    if complete {
        Ok(())
    } else {
        Err(format!(
            "{passed}/2 fluxos de desempenho passaram; consulte {}",
            path.display()
        ))
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
        thread::sleep(Duration::from_millis(25));
    }
    Err(format!(
        "Sem frame nativo em35segundos: {}",
        preview.status()
    ))
}
fn stop_preview(preview: &mut Preview) -> Result<(), String> {
    preview.clear_recording_sink();
    preview.stop();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !preview.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    if preview.is_finished() {
        Ok(())
    } else {
        Err("A sessão de câmera não confirmou encerramento em8segundos".into())
    }
}
fn timing_json(t: CaptureTiming) -> Value {
    json!({"frames":t.frames,"first_pts_ns":t.first_pts_ns,"last_pts_ns":t.last_pts_ns,"width":t.width,"height":t.height})
}
fn record_case(
    preview: &Preview,
    path: &Path,
    ffprobe: &Path,
    name: &str,
    audio: bool,
    view: FrameTransform,
    format: CaptureFormat,
) -> Value {
    let (output_width, output_height) = match expected_dimensions(&view, format) {
        Ok(dimensions) => dimensions,
        Err(error) => {
            return json!({"workflow":name,"passed":false,"path":path,"transform":view,"error":error})
        }
    };
    let expected_format = json!({"width":output_width,"height":output_height,"fps":format.fps,"video_codec":"h264","audio_requested":audio});
    let mut recorder = match Recorder::start_with_options(
        output_width,
        output_height,
        format.fps,
        path.to_path_buf(),
        audio,
        0,
    ) {
        Ok(recorder) => recorder,
        Err(error) => {
            return json!({"workflow":name,"passed":false,"path":path,"transform":view,"expected_format":expected_format,"error":error})
        }
    };
    preview.set_recording_sink(recorder.sink(), view);
    let mut before = None;
    let mut after = None;
    let mut sample_seconds = None;
    let capture = (|| -> Result<(), String> {
        // Recorder publishes this state only after FFmpeg encoded-progress reports
        // an actual frame; a stdin write before microphone startup is insufficient.
        let startup = Instant::now() + Duration::from_secs(35);
        while !recorder.status().starts_with("Gravando vídeo local") {
            if recorder.is_finished() {
                return Err(recorder.status());
            }
            if preview.is_finished() {
                return Err(preview.status());
            }
            if Instant::now() >= startup {
                return Err(format!(
                    "Sem confirmação de frame codificado: {}",
                    recorder.status()
                ));
            }
            thread::sleep(Duration::from_millis(25));
        }
        before = Some(preview.timing());
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(3) {
            if recorder.is_finished() {
                return Err(recorder.status());
            }
            if !preview.frame_fresh(Duration::from_secs(2)) {
                return Err(format!(
                    "Câmera deixou de entregar frames durante gravação: {}",
                    preview.status()
                ));
            }
            thread::sleep(Duration::from_millis(20));
        }
        sample_seconds = Some(started.elapsed().as_secs_f64());
        after = Some(preview.timing());
        let first = before.unwrap();
        let last = after.unwrap();
        let frames = last.frames.saturating_sub(first.frames);
        let seconds = match (first.last_pts_ns, last.last_pts_ns) {
            (Some(a), Some(b)) if b > a => (b - a) as f64 / 1e9,
            _ => return Err("Timestamps nativos ausentes ou não crescentes".into()),
        };
        let fps = frames as f64 / seconds;
        if (frames as f64) < f64::from(format.fps) * 3.0 * 0.85
            || (fps - f64::from(format.fps)).abs() > f64::from(format.fps) * 0.15
        {
            return Err(format!(
                "Cadência nativa insuficiente: {frames}frames, {fps:.2}fps"
            ));
        }
        if preview.status().contains(" · Gravação:") {
            return Err(preview.status());
        }
        Ok(())
    })();
    let recording_status = recorder.status();
    preview.clear_recording_sink();
    recorder.request_stop();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !recorder.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(25));
    }
    let finalized = if recorder.is_finished() {
        recorder.stop()
    } else {
        Err("Gravador não confirmou encerramento em20segundos".into())
    };
    let dropped = recorder.dropped();
    let probe = if finalized.is_ok() {
        probe_file(ffprobe, path)
    } else {
        Err("Arquivo não finalizado; FFprobe não executado".into())
    };
    let validation = probe
        .as_ref()
        .map_err(Clone::clone)
        .and_then(|value| validate_probe(value, audio, output_width, output_height, format.fps));
    let passed = capture.is_ok() && finalized.is_ok() && validation.is_ok();
    let native_frames = match (before, after) {
        (Some(a), Some(b)) => Some(b.frames.saturating_sub(a.frames)),
        _ => None,
    };
    let native_pts_seconds = match (
        before.and_then(|t| t.last_pts_ns),
        after.and_then(|t| t.last_pts_ns),
    ) {
        (Some(a), Some(b)) if b > a => Some((b - a) as f64 / 1e9),
        _ => None,
    };
    json!({"workflow":name,"passed":passed,"path":path,"audio_requested":audio,"transform":view,"expected_format":expected_format,"sample_seconds":sample_seconds,"native_frames":native_frames,"native_pts_seconds":native_pts_seconds,"native_fps":native_frames.zip(native_pts_seconds).map(|(n,t)|n as f64/t),"capture_before":before.map(timing_json),"capture_after":after.map(timing_json),"capture_error":capture.err(),"finalize_error":finalized.err(),"probe_error":probe.as_ref().err(),"validation_error":validation.as_ref().err(),"validated":validation.as_ref().ok(),"ffprobe":probe.as_ref().ok(),"dropped_input_frames":dropped,"recording_status":recording_status,"final_status":recorder.status()})
}
fn expected_dimensions(view: &FrameTransform, format: CaptureFormat) -> Result<(u32, u32), String> {
    let (width, height, _) = transform::apply(
        format.width,
        format.height,
        vec![0; format.width as usize * format.height as usize * 3],
        view,
    )?;
    Ok((width, height))
}
fn command_output(command: &mut Command, timeout: Duration) -> Result<Vec<u8>, String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let output = child.wait_with_output().map_err(|e| e.to_string())?;
                if !output.status.success() {
                    return Err(format!(
                        "FFprobe: {}",
                        String::from_utf8_lossy(&output.stderr)
                    ));
                }
                return Ok(output.stdout);
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("FFprobe excedeu o tempo limite".into());
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.to_string());
            }
        }
    }
}
fn probe_file(ffprobe: &Path, path: &Path) -> Result<Value, String> {
    let output=command_output(Command::new(ffprobe).args(["-v","error","-count_packets","-show_entries","stream=codec_type,codec_name,width,height,avg_frame_rate,r_frame_rate,nb_read_packets,duration,channels,sample_rate:format=duration,size","-of","json"]).arg(path),Duration::from_secs(15))?;
    serde_json::from_slice(&output).map_err(|e| format!("FFprobe retornou JSON inválido: {e}"))
}
fn parse_number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|v| v.is_finite())
}
fn packet_count(stream: &Value) -> Option<u64> {
    stream["nb_read_packets"]
        .as_u64()
        .or_else(|| stream["nb_read_packets"].as_str()?.parse().ok())
}
fn validate_probe(
    probe: &Value,
    audio: bool,
    expected_width: u32,
    expected_height: u32,
    expected_fps: u32,
) -> Result<Value, String> {
    let streams = probe["streams"]
        .as_array()
        .ok_or("FFprobe não informou streams")?;
    let videos: Vec<_> = streams
        .iter()
        .filter(|s| s["codec_type"] == "video")
        .collect();
    if videos.len() != 1 {
        return Err("MP4 deve conter exatamente um stream de vídeo".into());
    }
    let video = videos[0];
    if video["codec_name"] != "h264"
        || video["width"] != expected_width
        || video["height"] != expected_height
    {
        return Err("Codec ou dimensões de vídeo divergentes".into());
    }
    let rate = video["avg_frame_rate"]
        .as_str()
        .and_then(|s| s.split_once('/'))
        .and_then(|(n, d)| Some((n.parse::<f64>().ok()?, d.parse::<f64>().ok()?)))
        .filter(|(n, d)| n.is_finite() && d.is_finite() && *n > 0.0 && *d > 0.0)
        .map(|(n, d)| n / d)
        .ok_or("Taxa de quadros inválida no MP4")?;
    if (rate - f64::from(expected_fps)).abs() > f64::from(expected_fps) * 0.15 {
        return Err(format!("MP4 reportou {rate:.3}fps"));
    }
    let duration = if video["duration"].is_null() {
        parse_number(&probe["format"]["duration"])
    } else {
        parse_number(&video["duration"])
    }
    .ok_or("Duração ausente ou inválida no MP4")?;
    if !(2.5..=4.5).contains(&duration) {
        return Err(format!(
            "Duração {duration:.3}s fora do intervalo de3s esperado"
        ));
    }
    let packets = packet_count(video)
        .filter(|n| (*n as f64) >= f64::from(expected_fps) * 3.0 * 0.85)
        .ok_or("MP4 sem pacotes de vídeo suficientes para3segundos")?;
    let audios: Vec<_> = streams
        .iter()
        .filter(|s| s["codec_type"] == "audio")
        .collect();
    if audios.len() != usize::from(audio) {
        return Err("Presença do microfone no MP4 diverge da solicitação".into());
    }
    if audio && (audios[0]["codec_name"] != "aac" || packet_count(audios[0]).is_none_or(|n| n == 0))
    {
        return Err("Áudio AAC sem pacotes codificados".into());
    }
    Ok(
        json!({"width":expected_width,"height":expected_height,"fps":rate,"video_packets":packets,"duration_seconds":duration,"audio_streams":audios.len(),"audio_packets":audios.first().and_then(|s|packet_count(s))}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    fn probe(audio: bool) -> Value {
        let mut streams = vec![
            json!({"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"avg_frame_rate":"30/1","duration":"3.033333","nb_read_packets":"91"}),
        ];
        if audio {
            streams.push(json!({"codec_type":"audio","codec_name":"aac","nb_read_packets":"140"}));
        }
        json!({"streams":streams,"format":{"duration":"3.033333","size":"100000"}})
    }
    #[test]
    fn validates_realistic_video_and_optional_audio_metadata() {
        assert!(validate_probe(&probe(false), false, WIDTH, HEIGHT, FPS).is_ok());
        assert!(validate_probe(&probe(true), true, WIDTH, HEIGHT, FPS).is_ok());
        assert!(validate_probe(&probe(false), true, WIDTH, HEIGHT, FPS).is_err());
        assert!(validate_probe(&probe(true), false, WIDTH, HEIGHT, FPS).is_err());
    }
    #[test]
    fn expected_canvas_comes_from_the_actual_transform_pipeline() {
        for (view, expected) in [
            (FrameTransform::default(), (1920, 1080)),
            (
                FrameTransform {
                    zoom: 4.0,
                    ..Default::default()
                },
                (1920, 1080),
            ),
            (
                FrameTransform {
                    aspect: Aspect::Portrait,
                    ..Default::default()
                },
                (1080, 1920),
            ),
            (
                FrameTransform {
                    rotation: 1,
                    ..Default::default()
                },
                (1920, 1080),
            ),
        ] {
            assert_eq!(expected_dimensions(&view, NORMAL).unwrap(), expected);
        }
    }
    #[test]
    fn validates_portrait_metadata_against_its_own_expected_canvas() {
        let mut p = probe(false);
        p["streams"][0]["width"] = json!(1080);
        p["streams"][0]["height"] = json!(1920);
        assert!(validate_probe(&p, false, 1080, 1920, FPS).is_ok());
        assert!(validate_probe(&p, false, WIDTH, HEIGHT, FPS).is_err());
    }
    #[test]
    fn sixty_fps_requires_enough_encoded_packets_for_real_throughput() {
        let mut p = probe(false);
        p["streams"][0]["avg_frame_rate"] = json!("60/1");
        assert!(validate_probe(&p, false, WIDTH, HEIGHT, 60).is_err());
        p["streams"][0]["nb_read_packets"] = json!("182");
        assert!(validate_probe(&p, false, WIDTH, HEIGHT, 60).is_ok());
        p["streams"][0]["width"] = json!(3840);
        p["streams"][0]["height"] = json!(2160);
        p["streams"][0]["avg_frame_rate"] = json!("30/1");
        p["streams"][0]["nb_read_packets"] = json!("91");
        assert!(validate_probe(&p, false, 3840, 2160, 30).is_ok());
        assert!(validate_probe(&p, false, WIDTH, HEIGHT, 30).is_err());
    }
    #[test]
    fn rejects_wrong_size_rate_duration_and_empty_encoded_output() {
        for (key, value) in [
            ("width", json!(1280)),
            ("height", json!(720)),
            ("avg_frame_rate", json!("15/1")),
            ("avg_frame_rate", json!("30/0")),
            ("duration", json!("0.033333")),
            ("duration", json!("NaN")),
            ("nb_read_packets", json!("0")),
            ("codec_name", json!("rawvideo")),
        ] {
            let mut p = probe(false);
            p["streams"][0][key] = value;
            assert!(
                validate_probe(&p, false, WIDTH, HEIGHT, FPS).is_err(),
                "{key}"
            );
        }
        let mut p = probe(true);
        p["streams"][1]["nb_read_packets"] = json!("0");
        assert!(validate_probe(&p, true, WIDTH, HEIGHT, FPS).is_err());
    }
}
