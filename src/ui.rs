#[path = "ui_theme.rs"]
mod theme;
use crate::{
    controller::{Command, Controller, Snapshot},
    hid,
    model::{self, Preset},
    preview::Preview,
    recording::Recorder,
    transform::{self, Aspect, FrameTransform},
    uvc::ControlId,
};
use eframe::egui::{self, Color32, RichText};
use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Settings {
    view: FrameTransform,
    format: usize,
    audio: bool,
    folder: String,
    segment: u32,
    virtual_enabled: bool,
    auto_privacy: bool,
    auto_privacy_restore: Option<u8>,
    auto_privacy_hint: Option<u8>,
    manual_privacy_lock: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            view: FrameTransform::default(),
            format: 3,
            audio: true,
            folder: capture_dir().to_string_lossy().into_owned(),
            segment: 0,
            virtual_enabled: true,
            auto_privacy: true,
            auto_privacy_restore: None,
            auto_privacy_hint: None,
            manual_privacy_lock: false,
        }
    }
}

pub struct PixyApp {
    controller: Controller,
    snapshot: Snapshot,
    page: usize,
    output_page: usize,
    startup_tab: bool,
    preset_dialog: bool,
    zoom_open: bool,
    ptz_open: bool,
    ptz_draft: model::PtzPreferences,
    step_draft: i64,
    preview: Option<Preview>,
    texture: Option<egui::TextureHandle>,
    overview_texture: Option<egui::TextureHandle>,
    overview_visible: bool,
    overview_source: Option<(u32, u32)>,
    frame: Option<(u32, u32, Vec<u8>)>,
    frame_view: FrameTransform,
    frame_format: usize,
    stopping_preview: bool,
    quad_edit: Option<crate::quad::Quad>,
    format: usize,
    presets: Vec<Preset>,
    preset_textures: std::collections::HashMap<String, egui::TextureHandle>,
    preset_name: String,
    notice: String,
    step: i64,
    view: FrameTransform,
    preview_visible: bool,
    lifecycle: crate::lifecycle::Lifecycle,
    output_folder: String,
    segment_seconds: u32,
    recorder: Option<Recorder>,
    fullscreen: bool,
    record_audio: bool,
    restart_preview: bool,
    meter: Option<crate::meter::Meter>,
    presets_error: Option<String>,
    preview_error: Option<String>,
    ptz: model::PtzPreferences,
    confirm_reset: bool,
    settings_open: bool,
    hardware_busy: bool,
    last_jog: std::time::Instant,
    settings_error: Option<String>,
    virtual_camera: Option<crate::virtual_camera::VirtualCamera>,
    virtual_start:
        Option<std::sync::mpsc::Receiver<Result<crate::virtual_camera::VirtualCamera, String>>>,
    virtual_view: Option<FrameTransform>,
    virtual_error: Option<String>,
    virtual_enabled: bool,
    privacy_blocked: bool,
    manual_privacy_lock: bool,
    camera_seen: bool,
    virtual_retry_at: std::time::Instant,
    transport_closed_at: std::time::Instant,
    wake_mode: Option<(u8, std::time::Instant)>,
    resume_mode: Option<u8>,
    last_active_mode: Option<u8>,
    capture_idle: crate::capture_idle::IdleProbe,
    auto_privacy_enabled: bool,
    auto_privacy: crate::auto_privacy::AutoPrivacy,
    usage_monitor: crate::sharing_usage::UsageMonitor,
    usage_sample: Option<crate::sharing_usage::UsageSample>,
    usage_status: String,
    started: std::time::Instant,
    retry_at: Option<std::time::Instant>,
    capture_retries: u8,
    last_settings: Option<Vec<u8>>,
    telemetry_path: Option<PathBuf>,
    telemetry_at: std::time::Instant,
}

impl PixyApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        let (presets, presets_error) = match model::load_presets(&data_dir().join("presets.json")) {
            Ok(v) => (v, None),
            Err(e) => (Vec::new(), Some(e)),
        };
        let ptz = std::fs::read(data_dir().join("ptz.json"))
            .ok()
            .and_then(|v| serde_json::from_slice(&v).ok())
            .unwrap_or_default();
        let loaded = (|| -> Result<Settings, String> {
            let bytes = match std::fs::read(data_dir().join("settings.json")) {
                Ok(v) => v,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(Settings::default())
                }
                Err(e) => return Err(e.to_string()),
            };
            let s: Settings = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            if s.auto_privacy_restore.is_some_and(|v| v > 1)
                || s.auto_privacy_hint.is_some_and(|v| v > 1)
                || s.format >= model::FORMATS.len()
                || s.segment > 86400
                || transform::apply(2, 2, vec![0; 12], &s.view).is_err()
            {
                return Err("Preferências inválidas; arquivo preservado.".into());
            }
            Ok(s)
        })();
        let (settings, settings_error) = match loaded {
            Ok(v) => (v, None),
            Err(e) => (Settings::default(), Some(e)),
        };
        let mut app = Self {
            controller: Controller::start(),
            snapshot: Snapshot::default(),
            page: 0,
            output_page: 0,
            startup_tab: false,
            preset_dialog: false,
            zoom_open: false,
            ptz_open: false,
            ptz_draft: model::PtzPreferences::default(),
            step_draft: 5,
            preview: None,
            texture: None,
            overview_texture: None,
            overview_visible: true,
            overview_source: None,
            frame: None,
            frame_view: settings.view,
            frame_format: settings.format,
            stopping_preview: false,
            quad_edit: None,
            format: settings.format,
            presets,
            preset_textures: Default::default(),
            preset_name: String::new(),
            notice: String::new(),
            step: 5,
            view: settings.view,
            preview_visible: true,
            lifecycle: crate::lifecycle::Lifecycle {
                hidden: std::env::args().any(|a| a == "--background"),
                ..Default::default()
            },
            output_folder: settings.folder,
            segment_seconds: settings.segment,
            recorder: None,
            fullscreen: false,
            record_audio: settings.audio,
            restart_preview: false,
            meter: None,
            presets_error,
            preview_error: None,
            ptz,
            confirm_reset: false,
            settings_open: false,
            hardware_busy: false,
            last_jog: std::time::Instant::now(),
            settings_error,
            virtual_camera: None,
            virtual_start: None,
            virtual_view: None,
            virtual_error: None,
            virtual_enabled: settings.virtual_enabled,
            privacy_blocked: settings.manual_privacy_lock,
            manual_privacy_lock: settings.manual_privacy_lock,
            camera_seen: false,
            virtual_retry_at: std::time::Instant::now(),
            transport_closed_at: std::time::Instant::now(),
            wake_mode: None,
            resume_mode: None,
            last_active_mode: None,
            capture_idle: crate::capture_idle::IdleProbe::default(),
            auto_privacy_enabled: settings.auto_privacy,
            auto_privacy: crate::auto_privacy::AutoPrivacy::resume_with_hint(
                settings.auto_privacy_restore,
                settings.auto_privacy_hint,
            ),
            usage_monitor: crate::sharing_usage::UsageMonitor::start(),
            usage_sample: None,
            usage_status: "Verificando uso da câmera…".into(),
            started: std::time::Instant::now(),
            retry_at: None,
            capture_retries: 0,
            last_settings: None,
            telemetry_path: {
                let args: Vec<String> = std::env::args().collect();
                args.iter()
                    .position(|a| a == "--sharing-report")
                    .and_then(|i| args.get(i + 1))
                    .map(PathBuf::from)
            },
            telemetry_at: std::time::Instant::now(),
        };
        if app.virtual_enabled {
            app.ensure_virtual();
        }
        app
    }
    fn send(&mut self, c: Command) {
        if matches!(&c, Command::Vendor { set, payload, .. } if *set == hid::commands::SET_DEVICE_MODE && payload.first() == Some(&2))
        {
            self.wake_mode = None;
            self.manual_privacy_lock = true;
            self.pause_capture_for_privacy();
        }
        if matches!(&c, Command::Vendor { set, .. } if *set == hid::commands::SET_DEVICE_MODE)
            || matches!(
                &c,
                Command::Jog(..)
                    | Command::Move { .. }
                    | Command::Center
                    | Command::Recall(_)
                    | Command::FactoryReset
            )
        {
            self.auto_privacy.user_override();
        }
        if let Command::Vendor { set, payload, .. } = &c {
            if *set == hid::commands::SET_DEVICE_MODE {
                if let Some(mode @ 0..=1) = payload.first().copied() {
                    self.manual_privacy_lock = false;
                    self.resume_mode = None;
                    self.capture_retries = 0;
                    if self
                        .preview
                        .as_ref()
                        .is_none_or(|p| p.is_finished() || p.timing().frames == 0)
                    {
                        self.privacy_blocked = true;
                        self.pause_virtual();
                        if self.preview.as_ref().is_none_or(Preview::is_finished) {
                            let (w, h, fps) = model::FORMATS[self.format];
                            self.preview = Some(Preview::start(w, h, fps));
                            self.stopping_preview = false;
                        }
                        self.wake_mode = Some((mode, std::time::Instant::now()));
                        self.hardware_busy = true;
                        self.notice = "Reconectando câmera antes de aplicar o modo…".into();
                        return;
                    }
                }
            }
        }
        match self.controller.send(c) {
            Ok(()) => {
                self.notice = "Aplicando na câmera…".into();
                self.hardware_busy = true;
            }
            Err(e) => self.notice = e,
        }
    }
    fn toggle_preview(&mut self) {
        if self.preview.is_some() {
            self.stopping_preview = true;
            self.frame = None;
            self.texture = None;
            self.pause_virtual();
            if let Some(r) = &self.recorder {
                r.request_stop();
            }
            if let Some(p) = &self.preview {
                p.clear_recording_sink();
            }
            if let Some(p) = &mut self.preview {
                p.stop();
            }
            self.restart_preview = false;
        } else {
            if self.camera_mode() == Some(2) {
                self.notice =
                    "Câmera em privacidade. Selecione Manual ou Rastreamento para abrir.".into();
                return;
            }
            self.stopping_preview = false;
            self.preview_error = None;
            self.notice = "Iniciando prévia…".into();
            let (w, h, fps) = model::FORMATS[self.format];
            self.preview = Some(Preview::start(w, h, fps));
            if let Some(mode @ 0..=1) = self
                .resume_mode
                .take()
                .or(self.auto_privacy.restore_mode())
                .or(self.camera_mode())
            {
                self.auto_privacy.retain_idle_mode(mode);
                self.wake_mode = Some((mode, std::time::Instant::now()));
                self.hardware_busy = true;
                self.privacy_blocked = true;
            }
        }
    }
    fn vendor_choice(
        &mut self,
        ui: &mut egui::Ui,
        key: &str,
        label: &str,
        get: [u8; 4],
        set: [u8; 4],
        options: &[(u8, &str)],
    ) {
        if let Some(data) = self.snapshot.vendor.get(key).cloned() {
            if let Some(&current) = data.first() {
                ui.label(label);
                let (row, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 32.0),
                    egui::Sense::hover(),
                );
                ui.painter().rect_filled(row, 6.0, theme::CONTROL);
                let count = options.len().max(1) as f32;
                for (i, &(value, text)) in options.iter().enumerate() {
                    let rect = egui::Rect::from_min_max(
                        egui::pos2(row.left() + row.width() * i as f32 / count, row.top()),
                        egui::pos2(
                            row.left() + row.width() * (i + 1) as f32 / count,
                            row.bottom(),
                        ),
                    )
                    .shrink(2.0);
                    let selected = current == value;
                    let response = ui.interact(
                        rect,
                        ui.id().with(("camera_choice", key, value)),
                        egui::Sense::click(),
                    );
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::WidgetType::SelectableLabel,
                            ui.is_enabled(),
                            selected,
                            text,
                        )
                    });
                    if selected || response.hovered() {
                        ui.painter().rect_filled(rect, 4.0, theme::PANEL);
                    }
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        text,
                        egui::FontId::proportional(13.0),
                        if selected { theme::TEXT } else { theme::MUTED },
                    );
                    if response.clicked() {
                        let mut payload = data.clone();
                        payload[0] = value;
                        self.send(Command::Vendor {
                            get,
                            set,
                            query: vec![],
                            payload,
                        });
                    }
                }
                if !options.iter().any(|(value, _)| *value == current) {
                    ui.small(format!("Estado reportado: {current}"));
                }
            }
        }
    }
    fn vendor_toggle(
        &mut self,
        ui: &mut egui::Ui,
        key: &str,
        label: &str,
        get: [u8; 4],
        set: [u8; 4],
    ) {
        if let Some(mut data) = self
            .snapshot
            .vendor
            .get(key)
            .cloned()
            .filter(|d| d.len() == 2)
        {
            let mut checked = data[1] != 0;
            if studio_switch(ui, &mut checked, label).changed() {
                data[1] = checked as u8;
                self.send(Command::Vendor {
                    get,
                    set,
                    query: vec![data[0]],
                    payload: data,
                });
            }
        }
    }
    fn control(&mut self, ui: &mut egui::Ui, id: ControlId) {
        let Some(index) = self.snapshot.controls.iter().position(|c| c.id == id) else {
            return;
        };
        ui.spacing_mut().slider_width = (ui.available_width() - 68.0).max(100.0);
        let c = self.snapshot.controls[index].clone();
        let Some(mut value) = c.current else {
            ui.weak(format!(
                "{} · {}",
                c.label,
                c.error.as_deref().unwrap_or("sem leitura")
            ));
            return;
        };
        let mut send = false;
        let stream_required = matches!(
            id,
            ControlId::Brightness
                | ControlId::ExposureAuto
                | ControlId::Exposure
                | ControlId::PowerLineFrequency
        );
        ui.add_enabled_ui(
            c.writable && (!stream_required || self.frame_ready()),
            |ui| {
                if !matches!(
                    id,
                    ControlId::ExposureAuto
                        | ControlId::FocusAuto
                        | ControlId::WhiteBalanceAuto
                        | ControlId::Focus
                        | ControlId::WhiteBalance
                        | ControlId::Exposure
                ) {
                    ui.label(if id == ControlId::Exposure {
                        "Exposição (EV)"
                    } else {
                        &c.label
                    });
                }
                match id {
                    ControlId::ExposureAuto
                    | ControlId::WhiteBalanceAuto
                    | ControlId::FocusAuto => {
                        let (manual, automatic, label) = match id {
                            ControlId::ExposureAuto => (1, 8, "Exposição"),
                            ControlId::FocusAuto => (0, 1, "Foco"),
                            _ => (0, 1, "Balanço de branco"),
                        };
                        ui.horizontal(|ui| {
                            ui.label(label);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.spacing_mut().item_spacing.x = 0.0;
                                    for (v, label) in [(automatic, "Auto"), (manual, "M")] {
                                        let selected = value == v;
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    RichText::new(label).size(12.0).color(
                                                        if selected {
                                                            theme::BG
                                                        } else {
                                                            theme::MUTED
                                                        },
                                                    ),
                                                )
                                                .fill(if selected {
                                                    theme::BLUE
                                                } else {
                                                    theme::CONTROL
                                                })
                                                .corner_radius(12.0)
                                                .min_size(egui::vec2(32.0, 22.0)),
                                            )
                                            .clicked()
                                        {
                                            value = v;
                                            send = true;
                                        }
                                    }
                                },
                            );
                        });
                    }
                    ControlId::PowerLineFrequency => {
                        egui::ComboBox::from_id_salt("powerline")
                            .width(ui.available_width())
                            .selected_text(if value == 1 { "50 Hz" } else { "60 Hz" })
                            .show_ui(ui, |ui| {
                                for (v, label) in [(1, "50 Hz"), (2, "60 Hz")] {
                                    if ui.selectable_label(value == v, label).clicked() {
                                        value = v;
                                        send = true;
                                    }
                                }
                            });
                    }
                    ControlId::Exposure => {
                        let manual = self
                            .snapshot
                            .controls
                            .iter()
                            .find(|c| c.id == ControlId::ExposureAuto)
                            .and_then(|c| c.current)
                            == Some(1);
                        if let (Some(min), Some(max)) = (c.min, c.max) {
                            let choices =
                                crate::exposure::supported_ev(min, max, c.step.unwrap_or(1));
                            if let (Some((ev_min, _)), Some((ev_max, _))) =
                                (choices.first(), choices.last())
                            {
                                let mut ev =
                                    crate::exposure::exposure_raw_to_ev(value).unwrap_or(-5);
                                ui.add_enabled_ui(manual, |ui| {
                                    let r = studio_slider(ui, &mut ev, *ev_min..=*ev_max, 1.0);
                                    if r.changed() {
                                        if let Ok(raw) = crate::exposure::ev_to_raw(
                                            ev,
                                            min,
                                            max,
                                            c.step.unwrap_or(1),
                                        ) {
                                            value = raw;
                                            self.snapshot.controls[index].current = Some(raw);
                                        }
                                    }
                                    send = r.drag_stopped() || (r.changed() && !r.dragged());
                                });
                            }
                        }
                    }
                    _ => {
                        if let (Some(min), Some(max)) = (c.min, c.max) {
                            let auto = match id {
                                ControlId::Exposure => Some(ControlId::ExposureAuto),
                                ControlId::WhiteBalance => Some(ControlId::WhiteBalanceAuto),
                                ControlId::Focus => Some(ControlId::FocusAuto),
                                _ => None,
                            };
                            let manual = auto
                                .and_then(|auto| {
                                    self.snapshot.controls.iter().find(|c| c.id == auto)
                                })
                                .and_then(|c| c.current)
                                .is_none_or(|v| {
                                    if id == ControlId::Exposure {
                                        v == 1
                                    } else {
                                        v == 0
                                    }
                                });
                            ui.add_enabled_ui(manual, |ui| {
                                let r = studio_slider(
                                    ui,
                                    &mut value,
                                    min..=max,
                                    c.step.unwrap_or(1).max(1) as f64,
                                );
                                send = r.drag_stopped() || (r.changed() && !r.dragged());
                                if r.changed() {
                                    self.snapshot.controls[index].current = Some(value);
                                }
                            });
                        } else {
                            ui.weak(format!("{value} · faixa não informada"));
                        }
                    }
                }
            },
        );
        if send {
            self.send(Command::Set(id, value));
        }
    }
    fn move_direction(&mut self, direction: u8) {
        if let Ok((axis, sign)) =
            model::ptz_direction(direction, self.ptz.reverse, self.ptz.orientation)
        {
            self.send(Command::Move {
                axis: if axis == 1 {
                    ControlId::Pan
                } else {
                    ControlId::Tilt
                },
                delta: sign * self.step * 3600,
            });
        }
    }
    fn save_presets(&mut self) {
        let result = model::save_presets(&data_dir().join("presets.json"), &self.presets);
        self.notice = match result {
            Ok(()) => "Presets salvos neste Mac".into(),
            Err(e) => e.to_string(),
        };
    }
    fn capture(&mut self) {
        if !self.frame_ready() {
            self.notice = "Aguardando um quadro no enquadramento escolhido…".into();
            return;
        }
        if let Some((w, h, rgb)) = &self.frame {
            let folder = PathBuf::from(&self.output_folder);
            if !folder.is_absolute() {
                self.notice = "Use uma pasta de saída com caminho absoluto.".into();
                return;
            }
            let path = folder.join(format!("Pixy-{}.png", timestamp()));
            self.notice = match std::fs::create_dir_all(folder).and_then(|_| {
                image::save_buffer(&path, rgb, *w, *h, image::ColorType::Rgb8)
                    .map_err(std::io::Error::other)
            }) {
                Ok(()) => format!("Foto salva: {}", path.display()),
                Err(e) => e.to_string(),
            };
        }
    }
    fn record(&mut self) {
        if let Some(recorder) = &self.recorder {
            if let Some(p) = &self.preview {
                p.clear_recording_sink();
            }
            recorder.request_stop();
            return;
        }
        if !self.frame_ready() {
            self.notice = "Aguardando um quadro no enquadramento escolhido…".into();
            return;
        }
        let Some((w, h, _)) = &self.frame else { return };
        let folder = PathBuf::from(&self.output_folder);
        if !folder.is_absolute() {
            self.notice = "Use uma pasta de saída com caminho absoluto.".into();
            return;
        }
        let path = folder.join(format!("Pixy-{}.mp4", timestamp()));
        if let Err(e) = std::fs::create_dir_all(folder) {
            self.notice = e.to_string();
            return;
        }
        match Recorder::start_with_options(
            *w,
            *h,
            model::FORMATS[self.format].2,
            path,
            self.record_audio,
            self.segment_seconds,
        ) {
            Ok(r) => {
                if let Some(p) = &self.preview {
                    p.set_recording_sink(r.sink(), self.view);
                }
                self.recorder = Some(r);
            }
            Err(e) => self.notice = e,
        }
    }
    fn frame_ready(&self) -> bool {
        !self.privacy_blocked
            && self.camera_mode().is_some_and(|m| m != 2)
            && self.frame.is_some()
            && self.preview.is_some()
            && !self.stopping_preview
            && self.quad_edit.is_none()
            && self
                .preview
                .as_ref()
                .is_some_and(|p| p.frame_fresh(Duration::from_secs(1)))
            && !self.restart_preview
            && self.frame_view == self.view
            && self.frame_format == self.format
    }
    fn stop_virtual(&mut self) {
        self.virtual_enabled = false;
        self.pause_virtual();
    }
    fn pause_virtual(&mut self) {
        if let Some(preview) = &self.preview {
            preview.clear_virtual_sink();
        }
        self.virtual_view = None;
        if let Some(camera) = &self.virtual_camera {
            camera.set_placeholder(true);
        }
    }
    fn toggle_virtual(&mut self) {
        self.virtual_error = None;
        if self.virtual_enabled {
            self.stop_virtual();
            return;
        }
        self.virtual_enabled = true;
        self.virtual_retry_at = std::time::Instant::now();
        self.ensure_virtual();
    }
    fn ensure_virtual(&mut self) {
        if self.virtual_camera.is_some() || self.virtual_start.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.virtual_start = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(crate::virtual_camera::VirtualCamera::start());
        });
    }
}

impl eframe::App for PixyApp {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        match crate::lifecycle::take_action() {
            1 => {
                self.lifecycle.reopen();
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                crate::lifecycle::activate();
            }
            2 => {
                self.lifecycle.quitting = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            _ => {}
        }
        if ctx.input(|i| i.viewport().close_requested()) && self.lifecycle.close_requested() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        // eframe reveals its native window after the initial paint even if the
        // viewport builder requested hidden. Keep the service's state authoritative.
        if self.lifecycle.hidden && !self.lifecycle.quitting {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        if let Some(result) = self
            .virtual_start
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.virtual_start = None;
            match result {
                Ok(camera) => {
                    camera.set_placeholder(true);
                    self.virtual_camera = Some(camera);
                    self.virtual_view = None;
                }
                Err(e) => {
                    self.virtual_error = Some(e);
                    self.virtual_retry_at = std::time::Instant::now() + Duration::from_secs(3);
                }
            }
        }
        if let Some(s) = self.controller.latest() {
            if !s.message.is_empty() {
                self.notice = s.message.clone();
                self.hardware_busy = false;
            }
            if !self.camera_seen && s.hid_connected {
                if s.vendor.get("mode").and_then(|v| v.first()) == Some(&2)
                    && self.auto_privacy.owned_restore_mode().is_none()
                {
                    self.manual_privacy_lock = true;
                }
                self.camera_seen = true;
            }
            self.snapshot = s;
        }
        self.update_sharing(ctx);
        for (index, result) in self.controller.captured_presets() {
            match result {
                Err(error) => self.notice = error,
                Ok(mut preset) => {
                    if let Some((w, h, rgb)) = &self.frame {
                        let folder = data_dir().join("thumbnails");
                        let name = format!("preset-{}.jpg", timestamp());
                        if std::fs::create_dir_all(&folder).is_ok() {
                            if let Some(image) = image::RgbImage::from_raw(*w, *h, rgb.clone()) {
                                if image::DynamicImage::ImageRgb8(image)
                                    .thumbnail(160, 90)
                                    .save(folder.join(&name))
                                    .is_ok()
                                {
                                    preset.thumbnail = Some(name);
                                }
                            }
                        }
                    }
                    match model::apply_captured_preset(&mut self.presets, index, preset) {
                        Ok(()) => self.save_presets(),
                        Err(error) => self.notice = error,
                    }
                }
            }
        }
        if !self.lifecycle.hidden
            && (self.output_page == 1 || self.page == 2)
            && self.meter.is_none()
        {
            self.meter = Some(crate::meter::Meter::start());
        }
        if self.lifecycle.hidden || (self.output_page != 1 && self.page != 2) {
            self.meter = None;
        }
        if self.preview.as_ref().is_some_and(Preview::is_finished) {
            self.virtual_view = None;
            self.pause_virtual();
            if let Some(p) = &self.preview {
                p.clear_recording_sink();
            }
            if let Some(recorder) = &self.recorder {
                recorder.request_stop();
            }
            let restart = self.restart_preview;
            self.notice = self
                .preview
                .as_ref()
                .map(Preview::status)
                .unwrap_or_default();
            if !self.notice.contains("encerrada") {
                self.preview_error = Some(self.notice.clone());
                if self.virtual_enabled
                    && self.capture_retries < 3
                    && self.camera_mode().is_some_and(|m| m != 2)
                {
                    self.capture_retries += 1;
                    self.retry_at = Some(
                        std::time::Instant::now()
                            + Duration::from_secs(2 * u64::from(self.capture_retries)),
                    );
                }
            }
            self.preview = None;
            self.transport_closed_at = std::time::Instant::now();
            self.texture = None;
            self.frame = None;
            self.restart_preview = false;
            if restart {
                self.toggle_preview();
            }
        }
        if let Some(error) = self.preview.as_ref().and_then(Preview::processing_error) {
            self.frame = None;
            self.texture = None;
            self.notice = format!("Desfoque / enquadramento: {error}");
            self.pause_virtual();
        }
        if self.virtual_view.is_some_and(|view| view != self.view) {
            self.pause_virtual();
        }
        if let Some(preview) = self.preview.as_ref().filter(|_| {
            !self.stopping_preview && !self.privacy_blocked && self.camera_mode() != Some(2)
        }) {
            let display_view = FrameTransform {
                quad: if self.quad_edit.is_some() {
                    None
                } else {
                    self.view.quad
                },
                ..self.view
            };
            let display_active = !self.lifecycle.hidden
                && self.preview_visible
                && ctx.input(|i| i.viewport().focused.unwrap_or(false))
                && !ctx.input(|i| i.viewport().minimized.unwrap_or(false));
            let prepared = preview.latest_prepared(display_view);
            if !display_active {
                self.texture = None;
                self.overview_texture = None;
            }
            if display_active
                && self.view.zoom > 1.0
                && self.view.quad.is_none()
                && self.quad_edit.is_none()
            {
                if let Some((w, h, rgb)) = preview.latest() {
                    self.overview_source = Some((w, h));
                    if let Ok((pw, ph, pixels)) = transform::overview(w, h, &rgb, &self.view) {
                        let image = egui::ColorImage::from_rgb([pw as usize, ph as usize], &pixels);
                        if let Some(texture) = &mut self.overview_texture {
                            texture.set(image, egui::TextureOptions::LINEAR);
                        } else {
                            self.overview_texture = Some(ctx.load_texture(
                                "Campo completo",
                                image,
                                egui::TextureOptions::LINEAR,
                            ));
                        }
                    }
                }
            }
            if let Some((w, h, rgb)) = prepared {
                if display_active {
                    let image = egui::ColorImage::from_rgb([w as usize, h as usize], &rgb);
                    if let Some(texture) = &mut self.texture {
                        texture.set(image, egui::TextureOptions::LINEAR);
                    } else {
                        self.texture = Some(ctx.load_texture(
                            "Pixy preview",
                            image,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                }
                self.frame = Some((w, h, rgb));
                self.frame_view = self.view;
                self.frame_format = self.format;
                if self.notice == "Iniciando prévia…" {
                    self.notice = "Prévia ativa".into();
                }
            }
            ctx.request_repaint_after(Duration::from_millis(if !display_active {
                500
            } else if model::FORMATS[self.format].2 >= 60 {
                8
            } else {
                25
            }));
        } else {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        if self.virtual_enabled
            && self.camera_mode().is_some_and(|m| m != 2)
            && self.frame_ready()
            && self.virtual_view != Some(self.view)
        {
            if let (Some(preview), Some(camera)) = (&self.preview, &self.virtual_camera) {
                camera.set_placeholder(false);
                preview.set_virtual_sink(camera.sink(), self.view);
                self.virtual_view = Some(self.view);
            }
        }
        if self.recorder.as_ref().is_some_and(Recorder::is_finished) {
            if let Some(mut r) = self.recorder.take() {
                self.notice = r.stop().map(|_| r.status()).unwrap_or_else(|e| e);
            }
        }
        if self.lifecycle.hidden {
            self.texture = None;
            self.overview_texture = None;
            self.persist_settings();
            self.write_sharing_report();
            return;
        }
        self.studio_layout(ctx);
        self.persist_settings();
        self.write_sharing_report();
        if self.settings_open {
            let mut open = true;
            egui::Window::new("Ajustes de gravação e enquadramento")
                .default_width(440.0)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .open(&mut open)
                .show(ctx, |ui| {
                    if let Some(error) = &self.settings_error {
                        ui.colored_label(Color32::LIGHT_RED, error);
                    }
                    ui.add_enabled_ui(self.recorder.is_none() && self.quad_edit.is_none(), |ui| {
                        ui.add(
                            egui::Slider::new(&mut self.view.x, 0.0..=1.0)
                                .text("Recorte horizontal"),
                        );
                        ui.add(
                            egui::Slider::new(&mut self.view.y, 0.0..=1.0).text("Recorte vertical"),
                        );
                        if self.view.quad.is_some() && ui.button("Remover perspectiva").clicked() {
                            self.view.quad = None;
                        }
                        if ui.button("Repor enquadramento").clicked() {
                            self.view = FrameTransform::default();
                        }
                        ui.label("Pasta de fotos e vídeos");
                        ui.text_edit_singleline(&mut self.output_folder);
                        ui.label("Duração de cada arquivo de vídeo");
                        egui::ComboBox::from_id_salt("record_segment")
                            .selected_text(if self.segment_seconds == 0 {
                                "Arquivo único".into()
                            } else {
                                format!("{} min", self.segment_seconds / 60)
                            })
                            .show_ui(ui, |ui| {
                                for (seconds, label) in [
                                    (0, "Arquivo único"),
                                    (900, "15 minutos"),
                                    (1800, "30 minutos"),
                                    (3600, "1 hora"),
                                    (7200, "2 horas"),
                                ] {
                                    ui.selectable_value(&mut self.segment_seconds, seconds, label);
                                }
                            });
                    });
                });
            self.settings_open = open;
        }
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if !self.manual_privacy_lock {
            if let Some(mode @ 0..=1) = self
                .wake_mode
                .map(|(mode, _)| mode)
                .or(self.last_active_mode)
                .or(self.camera_mode())
            {
                // Preserve framing across transport shutdown without granting
                // permission to leave an independently chosen Privacy mode.
                self.auto_privacy.retain_idle_mode(mode);
            }
        }
        if let Some(preview) = &self.preview {
            preview.clear_virtual_sink();
        }
        if let Some(mut camera) = self.virtual_camera.take() {
            let _ = camera.stop();
        }
        self.virtual_start = None;
        if let Some(p) = &self.preview {
            p.clear_recording_sink();
        }
        if let Some(mut r) = self.recorder.take() {
            let _ = r.stop();
        }
        if let Some(p) = &mut self.preview {
            p.stop();
        }
        self.meter = None;
        if self.settings_error.is_none() {
            let settings = self.current_settings();
            let _ = model::save_json(&data_dir().join("settings.json"), &settings);
        }
    }
}

fn format_label(i: usize) -> String {
    let (w, h, fps) = model::FORMATS[i];
    format!("{w} × {h} / {fps} fps")
}
fn timestamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join("Library/Application Support/Pixy Control")
}
fn capture_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Movies/Pixy Control")
}

fn quad_handles(ui: &mut egui::Ui, rect: egui::Rect, quad: &mut crate::quad::Quad) {
    for (i, corner) in quad.iter_mut().enumerate() {
        let point = egui::pos2(
            rect.left() + corner[0] * rect.width(),
            rect.top() + corner[1] * rect.height(),
        );
        let handle = ui.interact(
            egui::Rect::from_center_size(point, egui::vec2(30.0, 30.0)),
            ui.id().with(("quad", i)),
            egui::Sense::drag(),
        );
        if handle.dragged() {
            if let Some(pos) = handle.interact_pointer_pos() {
                *corner = [
                    ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0),
                    ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0),
                ];
            }
        }
    }
    let points: Vec<_> = quad
        .iter()
        .map(|p| {
            egui::pos2(
                rect.left() + p[0] * rect.width(),
                rect.top() + p[1] * rect.height(),
            )
        })
        .collect();
    let color = if crate::quad::validate_quad(quad).is_ok() {
        Color32::LIGHT_GREEN
    } else {
        Color32::LIGHT_RED
    };
    for i in 0..4 {
        ui.painter().line_segment(
            [points[i], points[(i + 1) % 4]],
            egui::Stroke::new(2.0_f32, color),
        );
        ui.painter().circle_filled(points[i], 7.0, color);
        ui.painter().text(
            points[i] + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            (i + 1).to_string(),
            egui::FontId::proportional(16.0),
            Color32::WHITE,
        );
    }
}

fn digital_ptz(
    ui: &mut egui::Ui,
    image: egui::Rect,
    texture: &egui::TextureHandle,
    view: &mut FrameTransform,
    visible: &mut bool,
    source: (u32, u32),
    enabled: bool,
) {
    let aspect = texture.size_vec2().x / texture.size_vec2().y;
    let w = 160.0_f32.min(160.0 * aspect).min(image.width() * 0.4);
    let h = w / aspect;
    let top_right = image.right_top() + egui::vec2(-8.0, 8.0);
    let toolbar = egui::Rect::from_min_size(top_right - egui::vec2(w, 0.0), egui::vec2(w, 24.0));
    ui.painter()
        .rect_filled(toolbar, 3.0, Color32::from_gray(35));
    ui.painter().text(
        toolbar.center(),
        egui::Align2::CENTER_CENTER,
        if *visible {
            "Campo completo  ◉"
        } else {
            "Campo completo  ○"
        },
        egui::FontId::proportional(12.0),
        Color32::WHITE,
    );
    if ui
        .interact(
            toolbar,
            ui.id().with("overview_toggle"),
            egui::Sense::click(),
        )
        .clicked()
    {
        *visible = !*visible;
    }
    if !*visible {
        return;
    }
    let rect = egui::Rect::from_min_size(toolbar.left_bottom(), egui::vec2(w, h));
    ui.painter().image(
        texture.id(),
        rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    let Ok(geometry) = transform::overview_geometry(source.0, source.1, view) else {
        return;
    };
    let [left, top, crop_width, crop_height] = geometry.crop;
    let crop = egui::Rect::from_min_size(
        rect.min + egui::vec2(left * w, top * h),
        egui::vec2(crop_width * w, crop_height * h),
    );
    ui.painter().rect_stroke(
        crop,
        0.0,
        egui::Stroke::new(2.0_f32, Color32::LIGHT_GREEN),
        egui::StrokeKind::Inside,
    );
    let response = ui.interact(
        crop,
        ui.id().with("overview_drag"),
        if enabled {
            egui::Sense::drag()
        } else {
            egui::Sense::hover()
        },
    );
    if enabled && response.dragged() {
        let delta = ui.input(|i| i.pointer.delta());
        let (horizontal_flip, vertical_flip) = transform::effective_flips(view);
        if geometry.travel[0] > 1e-6 {
            view.x = (view.x
                + delta.x / (w * geometry.travel[0]) * if horizontal_flip { -1.0 } else { 1.0 })
            .clamp(0.0, 1.0);
        }
        if geometry.travel[1] > 1e-6 {
            view.y = (view.y
                + delta.y / (h * geometry.travel[1]) * if vertical_flip { -1.0 } else { 1.0 })
            .clamp(0.0, 1.0);
        }
    }
    if enabled && ui.rect_contains_pointer(rect) {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll != 0.0 {
            transform::set_zoom(view, view.zoom + scroll.signum() * 0.1);
        }
    }
}

include!("ui_studio.rs");
include!("ui_presets.rs");

impl PixyApp {
    fn studio_audio(&mut self, ui: &mut egui::Ui) {
        if let Some(meter) = &self.meter {
            let db = 20.0 * meter.level().max(0.00001).log10();
            ui.add(
                egui::ProgressBar::new(((db + 60.0) / 60.0).clamp(0.0, 1.0))
                    .text(format!("Microfone · {db:.0} dBFS")),
            );
            ui.small(meter.status());
        }
        if let Some(audio) = self.snapshot.audio.clone() {
            ui.label(RichText::new(audio.name).strong());
            if let Some(mut muted) = audio.muted {
                if ui.checkbox(&mut muted, "Silenciar microfone").changed() {
                    self.send(Command::Mute(muted));
                }
            }
            if let Some(current) = audio.volume {
                let mut volume = (current * 100.0).round();
                let r = ui.add(
                    egui::Slider::new(&mut volume, 0.0..=100.0)
                        .step_by(10.0)
                        .text("Entrada (%)"),
                );
                if r.changed() {
                    if let Some(audio) = &mut self.snapshot.audio {
                        audio.volume = Some(volume / 100.0);
                    }
                }
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    self.send(Command::Volume(volume / 100.0));
                }
            }
            if let Some(error) = audio.error {
                ui.weak(error);
            }
        }
        self.vendor_choice(
            ui,
            "audio_mode",
            "Modo do microfone",
            hid::commands::GET_MUSIC_MODE,
            hid::commands::SET_MUSIC_MODE,
            hid::AUDIO_MODES,
        );
        ui.label("O microfone USB continua disponível para os seus aplicativos.");
        ui.add_enabled(
            self.recorder.is_none(),
            egui::Checkbox::new(&mut self.record_audio, "Incluir microfone Pixy na gravação"),
        );
    }
    fn studio_device(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "Dispositivo");
        egui::Frame::NONE
            .fill(theme::CONTROL)
            .corner_radius(8.0)
            .inner_margin(16.0)
            .show(ui, |ui| {
                for (label, value) in [
                    ("Nome", "EMEET PIXY".to_owned()),
                    ("USB", "328F:00C0".to_owned()),
                    (
                        "Número de série",
                        self.snapshot.serial.clone().unwrap_or_else(|| "—".into()),
                    ),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(label).color(theme::MUTED));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(value);
                        });
                    });
                }
                for (i, version) in self.snapshot.firmware.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("Firmware {}", i + 1)).color(theme::MUTED));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(format!("v{version}"));
                        });
                    });
                }
                if ui
                    .small_button(RichText::new("Copiar número de série").color(theme::BLUE))
                    .clicked()
                {
                    if let Some(serial) = &self.snapshot.serial {
                        ui.ctx().copy_text(serial.clone());
                    }
                }
            });
        ui.add_space(24.0);
        if ui
            .add_enabled(
                !self.snapshot.controls.is_empty(),
                egui::Button::new(RichText::new("Remover da sessão").color(theme::BG))
                    .fill(theme::TEXT)
                    .corner_radius(20.0)
                    .min_size(egui::vec2(ui.available_width(), 40.0)),
            )
            .clicked()
        {
            self.stop_virtual();
            if self.preview.is_some() {
                self.toggle_preview();
            }
            self.meter = None;
            self.send(Command::Disconnect);
        }
        ui.add_space(10.0);
        if ui
            .add_enabled(
                self.snapshot.hid_connected,
                egui::Button::new("Restaurar câmera de fábrica…")
                    .fill(egui::Color32::TRANSPARENT)
                    .stroke(egui::Stroke::new(1.0_f32, theme::MUTED))
                    .corner_radius(20.0)
                    .min_size(egui::vec2(ui.available_width(), 40.0)),
            )
            .clicked()
        {
            self.confirm_reset = true;
        }
        ui.add_space(20.0);
        ui.collapsing("Informações técnicas", |ui| {
            ui.small(format!(
                "UVC: {} controles · HID: {}",
                self.snapshot.controls.len(),
                if self.snapshot.hid_connected {
                    "conectado"
                } else {
                    "indisponível"
                }
            ));
            for c in &self.snapshot.controls {
                ui.small(format!("{}: {:?}", c.label, c.current));
            }
        });
    }
}

include!("ui_sharing.rs");
