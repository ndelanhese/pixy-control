impl PixyApp {
    fn camera_mode(&self) -> Option<u8> {
        self.snapshot
            .hid_connected
            .then(|| {
                self.snapshot
                    .vendor
                    .get("mode")?
                    .first()
                    .copied()
                    .map(hid::normalize_device_mode)
            })
            .flatten()
    }
    fn current_settings(&self) -> Settings {
        Settings {
            view: self.view,
            format: self.format,
            audio: self.record_audio,
            folder: self.output_folder.clone(),
            segment: self.segment_seconds,
            virtual_enabled: self.virtual_enabled,
            auto_privacy: self.auto_privacy_enabled,
            auto_privacy_restore: self.auto_privacy.owned_restore_mode(),
            auto_privacy_hint: self.auto_privacy.retained_mode(),
            manual_privacy_lock: self.manual_privacy_lock,
        }
    }
    fn persist_settings(&mut self) {
        if self.settings_error.is_some() {
            return;
        }
        let settings = self.current_settings();
        if let Ok(encoded) = serde_json::to_vec(&settings) {
            if self.last_settings.as_ref() != Some(&encoded) {
                match model::save_json(&data_dir().join("settings.json"), &settings) {
                    Ok(()) => self.last_settings = Some(encoded),
                    Err(e) => self.settings_error = Some(e),
                }
            }
        }
    }
    fn pause_capture_for_privacy(&mut self) {
        self.pause_virtual();
        self.privacy_blocked = true;
        if let Some(p) = &self.preview {
            p.suspend_processing();
        }
        self.restart_preview = false;
        self.retry_at = None;
        self.frame = None;
        self.texture = None;
        self.overview_texture = None;
        if let Some(r) = &self.recorder {
            r.request_stop();
        }
    }
    fn update_sharing(&mut self, ctx: &egui::Context) {
        use crate::auto_privacy::{Action, Input, Usage};
        let now = self.started.elapsed();
        if self
            .virtual_camera
            .as_ref()
            .is_some_and(|c| c.is_finished())
        {
            if let Some(camera) = self.virtual_camera.take() {
                self.virtual_error = Some(camera.status());
            }
            self.virtual_view = None;
            self.virtual_retry_at = std::time::Instant::now() + Duration::from_secs(2);
        }
        if self.virtual_enabled
            && self.virtual_camera.is_none()
            && self.virtual_start.is_none()
            && std::time::Instant::now() >= self.virtual_retry_at
        {
            self.ensure_virtual();
        }

        for (mode, result) in self.controller.auto_mode_results() {
            let action = if mode == 2 {
                Action::EnterPrivacy
            } else {
                Action::Restore(mode)
            };
            self.auto_privacy
                .command_finished(now, action, result.is_ok());
            if let Err(error) = result {
                self.notice = format!("Privacidade automática: {error}");
            }
        }
        if let Some((mode, started)) = self.wake_mode {
            if self
                .preview
                .as_ref()
                .is_some_and(|p| p.timing().frames > 0 && !p.is_finished())
            {
                self.wake_mode = None;
                if self.controller.send(Command::AutoMode(mode)).is_err() {
                    self.hardware_busy = false;
                    self.auto_privacy
                        .command_finished(now, Action::Restore(mode), false);
                }
            } else if started.elapsed() > Duration::from_secs(8) {
                self.wake_mode = None;
                self.hardware_busy = false;
                self.auto_privacy
                    .command_finished(now, Action::Restore(mode), false);
                self.notice = "Aguardando conexão USB para sair da privacidade".into();
            }
        }
        self.usage_sample = self
            .usage_monitor
            .latest()
            .filter(|s| s.sampled_at.elapsed() <= Duration::from_secs(2));
        let (physical, virtual_active) = self
            .usage_sample
            .as_ref()
            .map(|sample| {
                let physical = if sample.physical == Usage::Active {
                    Usage::Active
                } else if self.preview.is_some() || sample.sampled_at < self.transport_closed_at {
                    Usage::Unknown
                } else {
                    sample.physical_running
                };
                (physical, sample.virtual_consumers.active())
            })
            .unwrap_or((Usage::Unknown, None));
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(false));
        let local_preview = !self.lifecycle.hidden && focused && self.preview_visible;
        let usage = crate::sharing_usage::aggregate(
            physical,
            if self.virtual_enabled {
                virtual_active
            } else {
                Some(false)
            },
            local_preview,
            self.recorder.is_some(),
        );
        self.usage_status = if self.recorder.is_some() {
            "Gravação local em uso"
        } else if self.virtual_enabled && virtual_active == Some(true) {
            "Câmera virtual em uso por outro app"
        } else if physical == Usage::Active {
            "Câmera USB em uso por outro app"
        } else if local_preview {
            "Prévia local em uso para ajustes"
        } else if usage == Usage::Idle {
            "Nenhum app usando a câmera"
        } else {
            "Aguardando confirmação de uso da câmera"
        }
        .into();
        if !self.hardware_busy && !self.manual_privacy_lock {
            if usage == Usage::Active {
                if let Some(mode @ 0..=1) = self.auto_privacy.restore_mode().or(self.camera_mode())
                {
                    self.last_active_mode = Some(mode);
                }
            } else if usage == Usage::Idle && self.auto_privacy_enabled {
                if let Some(mode @ 0..=1) = self.last_active_mode.or(self.camera_mode()) {
                    self.auto_privacy.retain_idle_mode(mode);
                }
            }
        }
        if !self.hardware_busy && !self.manual_privacy_lock {
            let input = Input {
                enabled: self.auto_privacy_enabled,
                mode: self.camera_mode(),
                usage,
            };
            if let Some(action) = self.auto_privacy.update(now, input) {
                let mode = match action {
                    Action::EnterPrivacy => 2,
                    Action::Restore(mode) => mode,
                };
                if mode == 2 {
                    self.pause_capture_for_privacy();
                }
                if mode != 2 && self.preview.as_ref().is_none_or(Preview::is_finished) {
                    // This firmware requires a running video transport before
                    // accepting wake/tracking commands. Its head stays private;
                    // neither raw frames nor a stale picture are exported.
                    let (w, h, fps) = model::FORMATS[self.format];
                    self.preview = Some(Preview::start(w, h, fps));
                    self.stopping_preview = false;
                    self.restart_preview = false;
                    self.resume_mode = None;
                    self.wake_mode = Some((mode, std::time::Instant::now()));
                    self.hardware_busy = true;
                } else {
                    match self.controller.send(Command::AutoMode(mode)) {
                        Ok(()) => self.hardware_busy = true,
                        Err(_) => self.auto_privacy.command_finished(now, action, false),
                    }
                }
            }
        }
        if self.camera_mode() == Some(2)
            && self.auto_privacy.owned_restore_mode().is_none()
            && !self.hardware_busy
            && self.wake_mode.is_none()
        {
            // A gesture or another controller may have selected Privacy. A
            // remembered framing mode is not permission to open that privacy.
            self.manual_privacy_lock = true;
            self.auto_privacy.user_override();
        }
        let idle_probe = self.auto_privacy_enabled
            && (virtual_active == Some(false) || !self.virtual_enabled)
            && !local_preview
            && self.recorder.is_none();
        let idle_probe = self.capture_idle.ready(now, idle_probe);
        if idle_probe
            && !self.hardware_busy
            && self.wake_mode.is_none()
            && self.preview.is_some()
            && !self.stopping_preview
        {
            if !self.manual_privacy_lock {
                if let Some(mode @ 0..=1) = self.camera_mode() {
                    self.auto_privacy.retain_idle_mode(mode);
                }
            }
            self.pause_virtual();
            if let Some(preview) = &mut self.preview {
                preview.stop();
            }
            self.stopping_preview = true;
            self.restart_preview = false;
            self.frame = None;
            self.texture = None;
        }
        let capture_needed = local_preview
            || self.recorder.is_some()
            || (self.virtual_enabled
                && (virtual_active == Some(true) || !self.auto_privacy_enabled));
        if self.manual_privacy_lock || self.camera_mode() == Some(2) {
            self.pause_capture_for_privacy();
            self.pause_virtual();
            if self.wake_mode.is_none() && !self.hardware_busy {
                if let Some(preview) = &mut self.preview {
                    preview.stop();
                    self.stopping_preview = true;
                }
            }
        } else if capture_needed
            && self.camera_mode().is_some()
            && self.preview.is_none()
            && self.capture_retries < 3
            && self
                .retry_at
                .is_none_or(|at| std::time::Instant::now() >= at)
        {
            self.retry_at = None;
            self.toggle_preview();
        }
        if !self.manual_privacy_lock
            && self.camera_mode().is_some_and(|m| m != 2)
            && !self.hardware_busy
        {
            self.privacy_blocked = false;
        }
        if !self.virtual_enabled {
            self.pause_virtual();
        }
    }
    fn write_sharing_report(&mut self) {
        if self.telemetry_at.elapsed() < Duration::from_millis(500) {
            return;
        }
        self.telemetry_at = std::time::Instant::now();
        if let Some(path) = &self.telemetry_path {
            let timing = self.preview.as_ref().map(Preview::timing);
            let data = serde_json::json!({"timestamp_ms":timestamp(), "mode":self.camera_mode(),
                "window_hidden":self.lifecycle.hidden,"window_visible":crate::lifecycle::window_visible(),
                "auto_privacy":self.auto_privacy_enabled,"restore_mode":self.auto_privacy.restore_mode(), "owned_restore_mode":self.auto_privacy.owned_restore_mode(),
                "virtual_enabled":self.virtual_enabled,"virtual_status":self.virtual_camera.as_ref().map(|c|c.status()), "virtual_metrics":self.virtual_camera.as_ref().map(|c|c.metrics()),
                "virtual_consumers":self.usage_sample.as_ref().map(|s|&s.virtual_consumers),
                "physical_usage":self.usage_sample.as_ref().map(|s|format!("{:?}",s.physical)),
                "physical_running":self.usage_sample.as_ref().map(|s|format!("{:?}",s.physical_running)),
                "usage":self.usage_status,"view":self.view,"format":self.format,
                "preview_status":self.preview.as_ref().map(Preview::status),"frame_ready":self.frame_ready(),"manual_privacy_lock":self.manual_privacy_lock,
                "processing_samples":self.preview.as_ref().map(|p|p.processing_samples()),"capture_frames":timing.map(|t|t.frames),"capture_width":timing.map(|t|t.width),"capture_height":timing.map(|t|t.height),
                "capture_retries":self.capture_retries,"notice":self.notice});
            static WRITER: std::sync::OnceLock<crate::diagnostics::Writer> = std::sync::OnceLock::new();
            WRITER.get_or_init(crate::diagnostics::Writer::new).submit(path.clone(), data);
        }
    }
    fn sharing_controls(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Câmera para outros apps").size(16.0).strong());
        ui.small("No Teams, Meet ou Zoom, selecione EMEET STUDIO Virtual Camera.");
        ui.small(
            "Enquadramento e blur locais · saída 1080p / 30 fps. Mantenha o Pixy Control aberto.",
        );
        ui.add_space(8.0);
        if ui
            .button(if self.virtual_enabled {
                "Desativar câmera virtual"
            } else {
                "Ativar câmera virtual"
            })
            .clicked()
        {
            self.toggle_virtual();
        }
        if let Some(camera) = &self.virtual_camera {
            ui.small(camera.status());
            // The per-session count is independently cached by the producer.
            if let crate::virtual_camera::ConsumerState::Known { count } = camera.consumer_state() {
                ui.small(format!("Aplicativos conectados: {count}"));
            }
        }
        if self.virtual_start.is_some() {
            ui.spinner();
        }
        ui.add_space(8.0);
        studio_switch(ui, &mut self.auto_privacy_enabled, "Privacidade automática");
        ui.small("Sem uso por 3 segundos: privacidade. Ao usar: volta ao modo anterior.");
        ui.small(&self.usage_status);
        if self.camera_mode() == Some(2) {
            ui.colored_label(
                theme::BLUE,
                if self.auto_privacy.restore_mode().is_some() {
                    "Privacidade automática ativa"
                } else {
                    "Privacidade manual · abertura automática bloqueada"
                },
            );
        }
        if let Some(error) = &self.virtual_error {
            ui.colored_label(Color32::LIGHT_RED, error);
            if ui.button("Tentar câmera virtual novamente").clicked() {
                self.virtual_error = None;
                self.ensure_virtual();
            }
        }
        if self.capture_retries >= 3 && ui.button("Reconectar captura").clicked() {
            self.capture_retries = 0;
            self.retry_at = None;
        }
    }
    fn blur_controls(&mut self, ui: &mut egui::Ui) {
        let mut enabled = self.view.background_blur > 0.0;
        ui.add_enabled_ui(self.recorder.is_none(), |ui| {
            if studio_switch(ui, &mut enabled, "Desfocar fundo").changed() {
                self.view.background_blur = if enabled { 0.65 } else { 0.0 };
            }
            if enabled {
                ui.label("Intensidade do desfoque");
                ui.add(egui::Slider::new(&mut self.view.background_blur, 0.1..=1.0).step_by(0.05));
            }
        });
        ui.small("Processamento local no Mac. Aplicado à prévia, fotos, vídeos e câmera virtual.");
        ui.add_space(16.0);
    }
}
