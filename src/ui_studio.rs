impl PixyApp {
    fn studio_layout(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::BG).inner_margin(8.0))
            .show(ctx, |ui| {
                let height = ui.available_height();
                let controls = if ui.available_width() >= 1300.0 {
                    400.0
                } else {
                    352.0
                };
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(controls, height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            studio_card().show(ui, |ui| {
                                ui.set_min_width(controls - 32.0);
                                ui.set_height(120.0);
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("Câmera").color(theme::TEXT).size(16.0));
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if theme::icon(ui, "plus", 14.0, "Reconectar câmera")
                                                .clicked()
                                            {
                                                self.send(Command::Refresh);
                                            }
                                            if theme::icon(
                                                ui,
                                                "settings",
                                                14.0,
                                                "Ajustes do dispositivo",
                                            )
                                            .clicked()
                                            {
                                                self.page = 3;
                                            }
                                        },
                                    );
                                });
                                ui.add_space(0.0);
                                ui.horizontal(|ui| {
                                    let (r, response) = ui.allocate_exact_size(
                                        egui::vec2(80.0, 80.0),
                                        egui::Sense::click(),
                                    );
                                    ui.painter().rect_filled(r, 8.0, theme::CONTROL);
                                    ui.painter().rect_stroke(
                                        r,
                                        8.0,
                                        egui::Stroke::new(2.0_f32, theme::BLUE),
                                        egui::StrokeKind::Inside,
                                    );
                                    draw_pixy(ui.painter(), r.shrink2(egui::vec2(15.0, 10.0)));
                                    ui.painter().rect_filled(
                                        egui::Rect::from_min_max(
                                            egui::pos2(r.left() + 2.0, r.bottom() - 22.0),
                                            r.right_bottom() - egui::vec2(2.0, 2.0),
                                        ),
                                        0.0,
                                        Color32::from_black_alpha(160),
                                    );
                                    ui.painter().text(
                                        r.center_bottom() - egui::vec2(0.0, 12.0),
                                        egui::Align2::CENTER_CENTER,
                                        "PIXY",
                                        egui::FontId::proportional(12.0),
                                        theme::TEXT,
                                    );
                                    if response
                                        .on_hover_text("Informações da EMEET PIXY")
                                        .clicked()
                                    {
                                        self.page = 3;
                                    }
                                    let (r, response) = ui.allocate_exact_size(
                                        egui::vec2(80.0, 80.0),
                                        egui::Sense::click(),
                                    );
                                    ui.painter().rect_filled(r, 8.0, theme::CONTROL);
                                    ui.painter().text(
                                        r.center(),
                                        egui::Align2::CENTER_CENTER,
                                        "+",
                                        egui::FontId::proportional(34.0),
                                        theme::MUTED,
                                    );
                                    if response.on_hover_text("Reconectar câmera USB").clicked() {
                                        self.send(Command::Refresh);
                                    }
                                });
                            });
                            studio_card()
                                .inner_margin(egui::Margin::symmetric(16, 0))
                                .show(ui, |ui| {
                                    ui.set_min_width(controls - 32.0);
                                    ui.set_height((height - 160.0).max(400.0));
                                    ui.horizontal(|ui| {
                                        let w = (ui.available_width() - 16.0) / 3.0;
                                        for (i, label) in
                                            ["Visão", "Efeitos", "Assistência"].iter().enumerate()
                                        {
                                            if theme::tab(ui, label, self.page == i, w).clicked() {
                                                self.page = i;
                                            }
                                        }
                                    });
                                    ui.separator();
                                    egui::ScrollArea::vertical()
                                        .id_salt(("studio_controls", self.page))
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            ui.set_max_width(ui.available_width());
                                            match self.page {
                                                0 => self.studio_view(ui),
                                                1 => self.studio_effects(ui),
                                                2 => self.studio_assistance(ui),
                                                _ => self.studio_device(ui),
                                            }
                                        });
                                });
                        },
                    );
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| self.studio_output(ui, height),
                    );
                });
            });
        if self.confirm_reset {
            egui::Window::new("Restaurar câmera")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label("Isso apaga os ajustes e as posições salvas na câmera.");
                    ui.horizontal(|ui| {
                        if ui.button("Cancelar").clicked() {
                            self.confirm_reset = false;
                        }
                        if ui.button("Restaurar de fábrica").clicked() {
                            self.send(Command::FactoryReset);
                            self.confirm_reset = false;
                        }
                    });
                });
        }
        if self.ptz_open {
            let mut open = true;
            let mut apply = false;
            egui::Window::new("Ajustes de PTZ").open(&mut open).collapsible(false).resizable(false).default_width(568.0).anchor(egui::Align2::CENTER_CENTER,egui::Vec2::ZERO).show(ctx,|ui|{
                ui.add_space(16.0);
                egui::Frame::NONE.fill(theme::CONTROL).corner_radius(8.0).inner_margin(16.0).show(ui,|ui|{ui.label("Ajuste a orientação dos controles conforme a posição de montagem da sua câmera.");});
                ui.add_space(20.0);
                studio_switch(ui,&mut self.ptz_draft.reverse,"Inverter controles horizontais");
                ui.add_space(20.0);
                ui.horizontal(|ui|{
                    let(r,_)=ui.allocate_exact_size(egui::vec2(260.0,170.0),egui::Sense::hover());
                    draw_pixy(ui.painter(),egui::Rect::from_center_size(r.center(),egui::vec2(104.0,134.0)));
                    let(r,_)=ui.allocate_exact_size(egui::vec2(260.0,170.0),egui::Sense::hover());let c=r.center();
                    ui.painter().circle_filled(c,80.0,theme::CONTROL);ui.painter().circle_filled(c,18.0,Color32::from_rgb(76,79,82));
                    for(index,label)in["↑","↓","←","→"].iter().enumerate(){let(axis,sign)=model::ptz_direction(index as u8,self.ptz_draft.reverse,self.ptz_draft.orientation).unwrap();let v=if axis==1{egui::vec2(-(sign as f32)*55.0,0.0)}else{egui::vec2(0.0,-(sign as f32)*55.0)};ui.painter().text(c+v,egui::Align2::CENTER_CENTER,label,egui::FontId::proportional(18.0),theme::MUTED);}
                });
                ui.add_space(12.0);
                ui.horizontal(|ui|{ui.label(format!("Montagem: {}°",self.ptz_draft.orientation as u16*90));if ui.button("Girar 90°").clicked(){self.ptz_draft.orientation=(self.ptz_draft.orientation+1)%4;}
if ui.button("Repor").clicked(){self.ptz_draft=model::PtzPreferences::default();self.step_draft=5;}});
                ui.collapsing("Velocidade do controle",|ui|{ui.add(egui::Slider::new(&mut self.step_draft,1..=15));});
                ui.add_space(24.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{apply=ui.add_sized([160.0,40.0],egui::Button::new(RichText::new("Confirmar").color(theme::BG)).fill(theme::TEXT).corner_radius(20.0)).clicked();if ui.add_sized([160.0,40.0],egui::Button::new("Cancelar").corner_radius(20.0)).clicked(){self.ptz_open=false;}});
            });
            self.ptz_open &= open;
            if apply {
                self.ptz = model::PtzPreferences {
                    reverse: self.ptz_draft.reverse,
                    orientation: self.ptz_draft.orientation,
                };
                self.step = self.step_draft;
                if let Err(e) = model::save_json(&data_dir().join("ptz.json"), &self.ptz) {
                    self.notice = e;
                }
                self.ptz_open = false;
            }
        }
    }
    fn studio_view(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        self.vendor_choice(
            ui,
            "mode",
            "Modo da câmera",
            hid::commands::GET_DEVICE_MODE,
            hid::commands::SET_DEVICE_MODE,
            hid::DEVICE_MODES,
        );
        ui.add_space(20.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Controle PTZ").color(theme::TEXT).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon(ui, "refresh", 22.0, "Centralizar câmera").clicked() {
                    self.send(Command::Center);
                }
            });
        });
        ui.add_space(16.0);
        ui.add_enabled_ui(self.snapshot.motor.is_some(), |ui| {
            ui.horizontal(|ui| {
                ui.add_space((ui.available_width() - 164.0).max(0.0) / 2.0);
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(164.0, 164.0), egui::Sense::click_and_drag());
                let center = rect.center();
                let p = ui.painter();
                p.circle_filled(center, 80.0, theme::CONTROL);
                for d in [
                    egui::vec2(0.0, -56.0),
                    egui::vec2(0.0, 56.0),
                    egui::vec2(-56.0, 0.0),
                    egui::vec2(56.0, 0.0),
                ] {
                    p.circle_filled(center + d, 1.8, theme::MUTED);
                }
                let mut offset = egui::Vec2::ZERO;
                if response.dragged() {
                    if let Some(pos) = response.interact_pointer_pos() {
                        offset = pos - center;
                        if offset.length() > 55.0 {
                            offset = offset.normalized() * 55.0;
                        }
                    }
                }
                p.circle_filled(
                    center + offset,
                    18.0,
                    if response.dragged() {
                        theme::BLUE
                    } else {
                        Color32::from_rgb(76, 79, 82)
                    },
                );
                let response = response
                    .on_hover_text("Arraste para mover. Clique no centro para centralizar.");
                if response.clicked() {
                    if let Some(pos) = response.interact_pointer_pos() {
                        let d = pos - center;
                        if d.length() < 25.0 {
                            self.send(Command::Center);
                        } else {
                            self.move_direction(if d.x.abs() > d.y.abs() {
                                if d.x < 0.0 {
                                    2
                                } else {
                                    3
                                }
                            } else if d.y < 0.0 {
                                0
                            } else {
                                1
                            });
                        }
                    }
                }
                if response.dragged()
                    && !self.hardware_busy
                    && self.last_jog.elapsed() > Duration::from_millis(120)
                {
                    let (mut h, mut v) = (-offset.x / 55.0, -offset.y / 55.0);
                    if self.ptz.reverse {
                        h = -h;
                    }
                    (h, v) = match self.ptz.orientation {
                        1 => (-v, -h),
                        2 => (-h, -v),
                        3 => (v, h),
                        _ => (h, v),
                    };
                    let speed = self.step as f32 * 0.15;
                    self.send(Command::Jog(h * speed, v * speed));
                    self.last_jog = std::time::Instant::now();
                }
            });
        });
        ui.add_space(20.0);
        let (r, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 40.0), egui::Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "Ajustes de PTZ")
        });
        ui.painter().rect_filled(r, 8.0, theme::CONTROL);
        ui.painter().text(
            r.left_center() + egui::vec2(16.0, 0.0),
            egui::Align2::LEFT_CENTER,
            "Ajustes de PTZ",
            egui::FontId::proportional(14.0),
            theme::MUTED,
        );
        ui.painter().text(
            r.right_center() - egui::vec2(16.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            "›",
            egui::FontId::proportional(20.0),
            theme::MUTED,
        );
        if response.clicked() {
            self.ptz_draft = model::PtzPreferences {
                reverse: self.ptz.reverse,
                orientation: self.ptz.orientation,
            };
            self.step_draft = self.step;
            self.ptz_open = true;
        }
        ui.add_space(20.0);
        self.studio_presets(ui);
    }
    fn studio_effects(&mut self, ui: &mut egui::Ui) {
        self.blur_controls(ui);
        ui.add_space(10.0);
        theme::section(ui, "Lente");
        self.control(ui, ControlId::FocusAuto);
        self.control(ui, ControlId::Focus);
        ui.add_space(20.0);
        theme::section(ui, "Efeitos de imagem");
        for id in [
            ControlId::ExposureAuto,
            ControlId::Exposure,
            ControlId::Gain,
            ControlId::PowerLineFrequency,
            ControlId::WhiteBalanceAuto,
            ControlId::WhiteBalance,
            ControlId::Brightness,
            ControlId::Contrast,
            ControlId::Hue,
            ControlId::Saturation,
            ControlId::Sharpness,
        ] {
            self.control(ui, id);
            ui.add_space(10.0);
        }
        ui.collapsing("Ajustes avançados", |ui| {
            for id in [ControlId::BacklightCompensation, ControlId::Gamma] {
                self.control(ui, id);
            }
            let allowed = !matches!(
                model::FORMATS[self.format],
                (3840, 2160, 30) | (1920, 1080, 60)
            );
            ui.add_enabled_ui(allowed, |ui| self.control(ui, ControlId::Zoom));
            self.vendor_toggle(
                ui,
                "mirror",
                "Espelhar imagem na câmera",
                hid::commands::GET_REVERSE,
                hid::commands::SET_REVERSE,
            );
            self.vendor_toggle(
                ui,
                "flip",
                "Inverter imagem na câmera",
                hid::commands::GET_REVERSE,
                hid::commands::SET_REVERSE,
            );
            for (key, label, get, set) in [
                (
                    "wb_lock",
                    "Travar balanço de branco",
                    [9, 4, 0, 10],
                    [9, 4, 0, 9],
                ),
                ("focus_lock", "Travar foco", [9, 4, 0, 14], [9, 4, 0, 13]),
            ] {
                self.vendor_choice(ui, key, label, get, set, &[(0, "Desligado"), (1, "Ligado")]);
            }
        });
        ui.add_space(16.0);
        if ui
            .add_enabled(
                self.frame_ready(),
                egui::Button::new("Restaurar ajustes de imagem")
                    .min_size(egui::vec2(ui.available_width(), 36.0)),
            )
            .clicked()
        {
            self.send(Command::RestoreImage);
        }
    }
    fn studio_assistance(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        theme::section(ui, "Áudio");
        if let Some(mut muted) = self.snapshot.audio.as_ref().and_then(|a| a.muted) {
            if studio_switch(ui, &mut muted, "Silenciar microfone").changed() {
                self.send(Command::Mute(muted));
            }
        }
        ui.add_space(12.0);
        self.vendor_choice(
            ui,
            "audio_mode",
            "Modo do microfone",
            hid::commands::GET_MUSIC_MODE,
            hid::commands::SET_MUSIC_MODE,
            hid::AUDIO_MODES,
        );
        ui.add_space(24.0);
        ui.separator();
        ui.add_space(20.0);
        if let Some(data) = self
            .snapshot
            .vendor
            .get("privacy")
            .cloned()
            .filter(|v| v.len() == 4)
        {
            let current = u32::from_le_bytes(data.try_into().unwrap());
            ui.label("Privacidade automática");
            egui::ComboBox::from_id_salt("privacy_time")
                .width(ui.available_width())
                .selected_text(
                    hid::PRIVACY_TIMES
                        .iter()
                        .find(|(v, _)| *v == current)
                        .map(|(_, l)| *l)
                        .unwrap_or("Desconhecido"),
                )
                .show_ui(ui, |ui| {
                    for &(value, label) in hid::PRIVACY_TIMES {
                        if ui.selectable_label(value == current, label).clicked() {
                            self.send(Command::Vendor {
                                get: hid::commands::GET_PRIVACY_TIME,
                                set: hid::commands::SET_PRIVACY_TIME,
                                query: vec![],
                                payload: value.to_le_bytes().to_vec(),
                            });
                        }
                    }
                });
        }
        ui.add_space(24.0);
        ui.separator();
        ui.add_space(20.0);
        self.vendor_toggle(
            ui,
            "palm",
            "Gesto de palma",
            hid::commands::GET_GESTURE,
            hid::commands::SET_GESTURE,
        );
        ui.add_space(20.0);
        self.vendor_toggle(
            ui,
            "auto_rotate",
            "Rotação automática quando invertida",
            hid::commands::GET_REVERSE,
            hid::commands::SET_REVERSE,
        );
        ui.add_space(20.0);
        self.vendor_choice(
            ui,
            "osd",
            "Indicadores na imagem",
            [9, 4, 0, 6],
            [9, 4, 0, 5],
            &[(0, "Desligados"), (1, "Ligados")],
        );
    }
    fn studio_output(&mut self, ui: &mut egui::Ui, height: f32) {
        let width = ui.available_width();
        let ph = (width * 9.0 / 16.0 + 28.0).min(height - 176.0).max(260.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, ph), egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, 0.0, Color32::from_rgb(13, 14, 15));
        if let Some(texture) = self.texture.as_ref().filter(|_| self.preview_visible) {
            let size = texture.size_vec2();
            let scale = (rect.width() / size.x).min((rect.height() - 28.0) / size.y);
            let ir = egui::Rect::from_center_size(rect.center(), size * scale);
            ui.painter().image(
                texture.id(),
                ir,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            if let Some(quad) = &mut self.quad_edit {
                quad_handles(ui, ir, quad);
            } else if self.view.zoom > 1.0 && self.view.quad.is_none() {
                if let Some(overview) = &self.overview_texture {
                    digital_ptz(
                        ui,
                        ir,
                        overview,
                        &mut self.view,
                        &mut self.overview_visible,
                        self.overview_source.unwrap_or((1920, 1080)),
                        self.recorder.is_none(),
                    );
                }
            }
        } else {
            ui.painter().text(
                rect.center() - egui::vec2(0.0, 16.0),
                egui::Align2::CENTER_CENTER,
                if self.manual_privacy_lock || self.camera_mode() == Some(2) {
                    "Câmera em privacidade"
                } else if self.preview_visible {
                    "EMEET PIXY"
                } else {
                    "Visor oculto"
                },
                egui::FontId::proportional(22.0),
                theme::MUTED,
            );
            let br = egui::Rect::from_center_size(
                rect.center() + egui::vec2(0.0, 30.0),
                egui::vec2(190.0, 36.0),
            );
            let mut placeholder = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("preview_placeholder")
                    .max_rect(br),
            );
            if placeholder
                .add_enabled(
                    self.preview.is_none() || !self.preview_visible,
                    egui::Button::new(if self.manual_privacy_lock {
                        "Controles da câmera"
                    } else if self.camera_mode() == Some(2) {
                        "Mostrar prévia"
                    } else if self.preview.is_none() {
                        "Iniciar prévia"
                    } else if !self.preview_visible {
                        "Mostrar visor"
                    } else {
                        "Conectando câmera…"
                    })
                    .min_size(br.size()),
                )
                .clicked()
            {
                if self.manual_privacy_lock || self.camera_mode() == Some(2) {
                    self.page = 0;
                    self.preview_visible = true;
                } else if self.preview.is_some() {
                    self.preview_visible = true;
                } else {
                    self.toggle_preview();
                }
            }
        }
        if self.texture.is_none() && self.preview_visible {
            if let Some(preview) = &self.preview {
                ui.painter().text(
                    rect.center() + egui::vec2(0.0, 74.0),
                    egui::Align2::CENTER_CENTER,
                    preview.status(),
                    egui::FontId::proportional(12.0),
                    theme::MUTED,
                );
            }
        }
        let baseline = rect.bottom() - 72.0;
        let left = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 16.0, baseline),
            egui::vec2(150.0, 48.0),
        );
        studio_overlay(ui, left, |ui| {
            studio_pill().show(ui, |ui| {
                let old = self.format;
                ui.add_enabled_ui(self.recorder.is_none(), |ui| {
                    egui::ComboBox::from_id_salt("format")
                        .width(116.0)
                        .selected_text(short_format(self.format))
                        .show_ui(ui, |ui| {
                            for i in 0..model::FORMATS.len() {
                                ui.selectable_value(&mut self.format, i, format_label(i));
                            }
                        });
                });
                if old != self.format && self.preview.is_some() {
                    self.resume_mode = self.camera_mode().filter(|m| *m < 2);
                    if let Some(p) = &mut self.preview {
                        p.stop();
                    }
                    self.stopping_preview = true;
                    self.frame = None;
                    self.texture = None;
                    self.restart_preview = true;
                }
            });
        });
        let barwidth = if width > 700.0 { 290.0 } else { 250.0 };
        let center = egui::Rect::from_min_size(
            egui::pos2(rect.center().x - barwidth / 2.0, baseline),
            egui::vec2(barwidth, 48.0),
        );
        studio_overlay(ui, center, |ui| {
            studio_pill().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.add_enabled_ui(self.recorder.is_none() && self.quad_edit.is_none(), |ui| {
                        if theme::icon(ui, "rotate", 22.0, "Girar 90°").clicked() {
                            self.view.rotation = (self.view.rotation + 1) % 4;
                        }
                        ui.menu_button(
                            if self.view.aspect == Aspect::Portrait {
                                "9:16"
                            } else {
                                "16:9"
                            },
                            |ui| {
                                for (v, l) in [
                                    (Aspect::Original, "Original"),
                                    (Aspect::Landscape, "16:9"),
                                    (Aspect::Portrait, "9:16"),
                                ] {
                                    if ui.selectable_value(&mut self.view.aspect, v, l).clicked() {
                                        ui.close_menu();
                                    }
                                }
                            },
                        );
                        if theme::icon(ui, "flip", 22.0, "Inverter verticalmente").clicked() {
                            self.view.flip = !self.view.flip;
                        }
                        if theme::icon(ui, "mirror", 22.0, "Espelhar horizontalmente").clicked() {
                            self.view.mirror = !self.view.mirror;
                        }
                        if ui.button("Zoom").clicked() {
                            self.zoom_open = !self.zoom_open;
                        }
                    });
                });
            });
        });
        let right = egui::Rect::from_min_size(
            egui::pos2(rect.right() - 106.0, baseline),
            egui::vec2(98.0, 48.0),
        );
        studio_overlay(ui, right, |ui| {
            studio_pill().show(ui, |ui| {
                ui.horizontal(|ui| {
                    if theme::icon(ui, "crop", 22.0, "Recorte de quatro cantos").clicked()
                        && self.recorder.is_none()
                        && self.frame_ready()
                    {
                        self.quad_edit = Some(self.view.quad.unwrap_or(crate::quad::FULL_QUAD));
                    }
                    if theme::icon(
                        ui,
                        "eye",
                        22.0,
                        if self.preview_visible {
                            "Ocultar visor"
                        } else {
                            "Mostrar visor"
                        },
                    )
                    .clicked()
                    {
                        self.preview_visible = !self.preview_visible;
                    }
                });
            });
        });
        if self.zoom_open {
            let zr = egui::Rect::from_min_size(
                egui::pos2(rect.center().x - 135.0, baseline - 56.0),
                egui::vec2(270.0, 48.0),
            );
            studio_overlay(ui, zr, |ui| {
                studio_pill().show(ui, |ui| {
                    ui.add_enabled_ui(self.recorder.is_none() && self.quad_edit.is_none(), |ui| {
                        let mut zoom = self.view.zoom;
                        if ui
                            .add(
                                egui::Slider::new(&mut zoom, 1.0..=4.0)
                                    .step_by(0.1)
                                    .suffix("×"),
                            )
                            .changed()
                        {
                            transform::set_zoom(&mut self.view, zoom);
                        }
                    });
                });
            });
        }
        if self.quad_edit.is_some() {
            let qr = egui::Rect::from_min_size(
                rect.left_top() + egui::vec2(16.0, 16.0),
                egui::vec2(420.0, 44.0),
            );
            studio_overlay(ui, qr, |ui| {
                studio_pill().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let q = self.quad_edit.unwrap();
                        if ui
                            .add_enabled(
                                crate::quad::validate_quad(&q).is_ok(),
                                egui::Button::new("Aplicar perspectiva"),
                            )
                            .clicked()
                        {
                            self.view.quad = Some(q);
                            self.quad_edit = None;
                            self.frame = None;
                        }
                        if ui.button("Cancelar recorte").clicked() {
                            self.quad_edit = None;
                            self.frame = None;
                        }
                    });
                });
            });
        }
        ui.add_space(0.0);
        studio_card().inner_margin(0.0).show(ui, |ui| {
            ui.set_width(width);
            ui.set_min_height((height - ph - 8.0).max(160.0));
            egui::Frame::NONE
                .fill(theme::BG)
                .inner_margin(egui::Margin::symmetric(10, 0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for (i, l) in ["Live", "Áudio", "V-Cam"].iter().enumerate() {
                            if theme::tab(ui, l, self.output_page == i, 76.0).clicked() {
                                self.output_page = i;
                            }
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .button(RichText::new("Ajustes  ›").color(theme::BLUE))
                                .clicked()
                            {
                                self.settings_open = true;
                            }
                            ui.add_enabled_ui(
                                self.frame_ready() || self.recorder.is_some(),
                                |ui| {
                                    ui.scope(|ui| {
                                        ui.visuals_mut().override_text_color =
                                            Some(Color32::from_rgb(245, 90, 92));
                                        let active = self.recorder.is_some();
                                        if theme::icon(
                                            ui,
                                            if active { "stop" } else { "record" },
                                            20.0,
                                            if active { "Parar gravação" } else { "Gravar" },
                                        )
                                        .clicked()
                                        {
                                            self.record();
                                        }
                                    });
                                },
                            );
                            ui.add_enabled_ui(self.frame_ready(), |ui| {
                                if theme::icon(ui, "camera", 20.0, "Foto").clicked() {
                                    self.capture();
                                }
                            });
                            if theme::icon(ui, "fullscreen", 18.0, "Tela cheia").clicked() {
                                self.fullscreen = !self.fullscreen;
                                ui.ctx()
                                    .send_viewport_cmd(egui::ViewportCommand::Fullscreen(
                                        self.fullscreen,
                                    ));
                            }
                        });
                    });
                });
            egui::Frame::NONE.inner_margin(16.0).show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("output_detail")
                    .max_height((height - ph - 86.0).max(72.0))
                    .show(ui, |ui| {
                        ui.set_width(width - 32.0);
                        match self.output_page {
                            1 => self.studio_audio(ui),
                            2 => {
                                self.sharing_controls(ui);
                            }
                            _ => {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new("Filtros de imagem").color(theme::MUTED),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui
                                                .small_button(if self.preview_visible {
                                                    "Ocultar prévia"
                                                } else {
                                                    "Mostrar prévia"
                                                })
                                                .clicked()
                                            {
                                                self.preview_visible = !self.preview_visible;
                                            }
                                        },
                                    );
                                });
                                ui.add_space(8.0);
                                ui.horizontal_wrapped(|ui| {
                                    for (i, f) in model::filters().iter().enumerate() {
                                        if ui
                                            .add_enabled(
                                                self.frame_ready(),
                                                egui::Button::new(f.name)
                                                    .min_size(egui::vec2(100.0, 42.0)),
                                            )
                                            .clicked()
                                        {
                                            self.send(Command::Filter(i));
                                        }
                                    }
                                });
                            }
                        }
                        if let Some(error) = &self.preview_error {
                            ui.colored_label(Color32::LIGHT_RED, error);
                        }
                        if let Some(r) = &self.recorder {
                            ui.small(r.status());
                        }
                        if !self.notice.is_empty() {
                            ui.label(RichText::new(&self.notice).size(11.0).color(theme::MUTED));
                        }
                    });
            });
        });
    }
}
fn studio_card() -> egui::Frame {
    egui::Frame::NONE
        .fill(theme::PANEL)
        .corner_radius(12.0)
        .inner_margin(16.0)
}
fn studio_pill() -> egui::Frame {
    egui::Frame::NONE
        .fill(Color32::from_rgba_unmultiplied(25, 27, 29, 190))
        .corner_radius(24.0)
        .inner_margin(egui::Margin::symmetric(10, 6))
}
fn short_format(i: usize) -> String {
    let (w, h, f) = model::FORMATS[i];
    if w == 3840 {
        format!("4K/{f}FPS")
    } else if w == 1920 {
        format!("1080P/{f}FPS")
    } else {
        format!("{w}×{h}/{f}")
    }
}
fn draw_pixy(p: &egui::Painter, r: egui::Rect) {
    let c = r.center();
    let w = r.width();
    let h = r.height();
    p.rect_filled(
        egui::Rect::from_center_size(
            c + egui::vec2(0.0, h * 0.29),
            egui::vec2(w * 0.66, h * 0.32),
        ),
        5.0,
        Color32::from_rgb(77, 81, 85),
    );
    p.rect_filled(
        egui::Rect::from_center_size(c - egui::vec2(0.0, h * 0.12), egui::vec2(w, h * 0.52)),
        7.0,
        Color32::from_rgb(128, 133, 138),
    );
    p.rect_filled(
        egui::Rect::from_center_size(
            c - egui::vec2(0.0, h * 0.12),
            egui::vec2(w * 0.87, h * 0.40),
        ),
        5.0,
        Color32::from_rgb(43, 46, 50),
    );
    for x in [-0.22, 0.22] {
        let cc = c + egui::vec2(w * x, -h * 0.12);
        p.circle_stroke(cc, w * 0.14, egui::Stroke::new(1.3_f32, theme::MUTED));
        p.circle_filled(cc, w * 0.075, Color32::from_rgb(15, 19, 24));
        p.circle_filled(
            cc + egui::vec2(-1.5, -1.5),
            w * 0.025,
            Color32::from_rgb(162, 176, 186),
        );
    }
}

fn studio_switch(ui: &mut egui::Ui, value: &mut bool, label: &str) -> egui::Response {
    let (rect, mut response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), egui::Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *value, label)
    });
    ui.painter().text(
        rect.left_center(),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.0),
        theme::MUTED,
    );
    let switch = egui::Rect::from_center_size(
        rect.right_center() - egui::vec2(16.0, 0.0),
        egui::vec2(32.0, 16.0),
    );
    ui.painter().rect_filled(
        switch,
        8.0,
        if *value {
            theme::BLUE
        } else {
            Color32::from_rgb(57, 60, 64)
        },
    );
    ui.painter().circle_filled(
        egui::pos2(
            if *value {
                switch.right() - 8.0
            } else {
                switch.left() + 8.0
            },
            switch.center().y,
        ),
        6.0,
        theme::TEXT,
    );
    response
}

fn studio_overlay(ui: &mut egui::Ui, rect: egui::Rect, add: impl FnOnce(&mut egui::Ui)) {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt((
                "preview_overlay",
                rect.min.x.to_bits(),
                rect.min.y.to_bits(),
            ))
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    add(&mut child);
}

fn studio_slider<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut N,
    range: std::ops::RangeInclusive<N>,
    step: f64,
) -> egui::Response {
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 64.0).max(80.0);
        let slider = ui
            .scope(|ui| {
                ui.spacing_mut().slider_width = width;
                ui.spacing_mut().interact_size.y = 14.0;
                ui.style_mut().override_font_id = Some(egui::FontId::proportional(12.0));
                ui.visuals_mut().widgets.inactive.bg_fill = theme::TEXT;
                ui.visuals_mut().widgets.hovered.bg_fill = theme::TEXT;
                ui.visuals_mut().widgets.active.bg_fill = theme::BLUE;
                ui.add(
                    egui::Slider::new(value, range.clone())
                        .step_by(step)
                        .show_value(false),
                )
            })
            .inner;
        let numeric = ui.add_sized(
            [48.0, 24.0],
            egui::DragValue::new(value)
                .range(range)
                .speed(step)
                .fixed_decimals(0),
        );
        slider.union(numeric)
    })
    .inner
}
