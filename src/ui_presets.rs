impl PixyApp {
    fn studio_preset_menu(
        &mut self,
        ui: &mut egui::Ui,
        preset: &Preset,
        enabled: bool,
        remove_id: &mut Option<u64>,
    ) {
        if ui
            .add_enabled(enabled, egui::Button::new("Atualizar posição"))
            .clicked()
        {
            self.send(Command::CapturePreset(preset.name.clone(), Some(preset.id)));
            ui.close_menu();
        }
        if ui
            .add_enabled(!self.hardware_busy, egui::Button::new("Excluir posição"))
            .clicked()
        {
            *remove_id = Some(preset.id);
            ui.close_menu();
        }
    }

    fn studio_presets(&mut self, ui: &mut egui::Ui) {
        let tab_width = (ui.available_width() - 8.0) / 2.0;
        ui.horizontal(|ui| {
            if theme::tab(ui, "Posições salvas", !self.startup_tab, tab_width).clicked() {
                self.startup_tab = false;
            }
            if theme::tab(ui, "Posição ao ligar", self.startup_tab, tab_width).clicked() {
                self.startup_tab = true;
            }
        });
        ui.add_space(14.0);
        if self.startup_tab {
            let saved = self
                .snapshot
                .vendor
                .get("startup")
                .and_then(|value| value.first())
                .is_some_and(|value| *value == 1);
            let enabled = !self.hardware_busy
                && self.snapshot.hid_connected
                && self.snapshot.vendor.contains_key("startup");
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), 186.0),
                egui::Sense::click(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    enabled,
                    "Salvar posição atual ao ligar",
                )
            });
            let fill = if enabled && response.hovered() {
                Color32::from_rgb(49, 51, 54)
            } else {
                theme::CONTROL
            };
            ui.painter().rect_filled(rect, 8.0, fill);
            ui.painter().text(
                rect.center() - egui::vec2(0.0, 18.0),
                egui::Align2::CENTER_CENTER,
                if saved { "✓" } else { "+" },
                egui::FontId::proportional(36.0),
                if saved { theme::BLUE } else { theme::MUTED },
            );
            ui.painter().text(
                rect.center() + egui::vec2(0.0, 25.0),
                egui::Align2::CENTER_CENTER,
                if saved {
                    "Atualizar posição ao ligar"
                } else {
                    "Salvar posição atual ao ligar"
                },
                egui::FontId::proportional(12.0),
                theme::MUTED,
            );
            if enabled && response.clicked() {
                self.send(Command::Startup(true));
            }
            response.on_hover_text(if enabled {
                "A câmera usará esta posição quando for ligada."
            } else {
                "Aguardando a câmera ficar disponível."
            });
            if saved {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Posição armazenada na câmera")
                            .size(11.0)
                            .color(theme::MUTED),
                    );
                    if ui
                        .add_enabled(enabled, egui::Button::new("Remover").small())
                        .clicked()
                    {
                        self.send(Command::Startup(false));
                    }
                });
            }
            return;
        }

        if let Some(error) = &self.presets_error {
            ui.colored_label(
                Color32::LIGHT_RED,
                format!("Não foi possível carregar as posições: {error}"),
            );
            return;
        }
        let width = ((ui.available_width() - 12.0) / 2.0).max(1.0);
        let tile_height = width * 9.0 / 16.0 + 30.0;
        let enabled = !self.hardware_busy && self.snapshot.hid_connected;
        let presets = self.presets.clone();
        let mut remove_id = None;
        let tile_count = presets.len() + usize::from(presets.len() < 8);
        egui::Grid::new("studio_preset_tiles")
            .num_columns(2)
            .spacing(egui::vec2(12.0, 12.0))
            .show(ui, |ui| {
                for index in 0..tile_count {
                    if let Some(preset) = presets.get(index) {
                        if let Some(name) = &preset.thumbnail {
                            if !self.preset_textures.contains_key(name) {
                                if let Ok(image) =
                                    image::open(data_dir().join("thumbnails").join(name))
                                {
                                    let image = image.into_rgba8();
                                    let color = egui::ColorImage::from_rgba_unmultiplied(
                                        [image.width() as usize, image.height() as usize],
                                        &image,
                                    );
                                    self.preset_textures.insert(
                                        name.clone(),
                                        ui.ctx().load_texture(
                                            name,
                                            color,
                                            egui::TextureOptions::LINEAR,
                                        ),
                                    );
                                }
                            }
                        }
                        let (rect, response) = ui.allocate_exact_size(
                            egui::vec2(width, tile_height),
                            egui::Sense::click(),
                        );
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                enabled,
                                &preset.name,
                            )
                        });
                        ui.painter().rect_filled(
                            rect,
                            6.0,
                            if enabled && response.hovered() {
                                Color32::from_rgb(49, 51, 54)
                            } else {
                                theme::CONTROL
                            },
                        );
                        let image_rect = egui::Rect::from_min_size(
                            rect.min,
                            egui::vec2(width, width * 9.0 / 16.0),
                        );
                        if let Some(texture) = preset
                            .thumbnail
                            .as_ref()
                            .and_then(|name| self.preset_textures.get(name))
                        {
                            ui.painter().image(
                                texture.id(),
                                image_rect,
                                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        } else {
                            ui.painter().rect_filled(image_rect, 6.0, theme::BG);
                            ui.painter().text(
                                image_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "Posição salva",
                                egui::FontId::proportional(12.0),
                                theme::MUTED,
                            );
                        }
                        let caption_rect = egui::Rect::from_min_max(
                            egui::pos2(rect.left() + 9.0, image_rect.bottom()),
                            rect.max - egui::vec2(32.0, 0.0),
                        );
                        let galley = ui.painter().layout(
                            preset.name.clone(),
                            egui::FontId::proportional(12.0),
                            theme::TEXT,
                            caption_rect.width(),
                        );
                        ui.painter().with_clip_rect(caption_rect).galley(
                            egui::pos2(caption_rect.left(), caption_rect.center().y - 7.0),
                            galley,
                            theme::TEXT,
                        );
                        let menu_rect = egui::Rect::from_min_max(
                            egui::pos2(rect.right() - 30.0, image_rect.bottom()),
                            rect.max,
                        );
                        // Explicitly exclude the menu target from Recall, even
                        // when egui reports the enclosing tile click as well.
                        let menu_hit = response
                            .interact_pointer_pos()
                            .is_some_and(|position| menu_rect.contains(position));
                        if enabled && response.clicked() && !menu_hit {
                            self.send(Command::Recall(preset.clone()));
                        }
                        response.context_menu(|ui| {
                            self.studio_preset_menu(ui, preset, enabled, &mut remove_id);
                        });
                        // This child overlays an already allocated tile. It
                        // must not advance the grid cursor into another cell.
                        let mut menu_ui = ui.new_child(
                            egui::UiBuilder::new()
                                .id_salt(("preset_actions", preset.id))
                                .max_rect(menu_rect)
                                .layout(egui::Layout::centered_and_justified(
                                    egui::Direction::LeftToRight,
                                )),
                        );
                        menu_ui.add_enabled_ui(!self.hardware_busy, |ui| {
                            ui.menu_button("⋯", |ui| {
                                self.studio_preset_menu(ui, preset, enabled, &mut remove_id);
                            })
                            .response
                            .on_hover_text("Ações da posição");
                        });
                    } else {
                        let (rect, response) = ui.allocate_exact_size(
                            egui::vec2(width, tile_height),
                            egui::Sense::click(),
                        );
                        let can_add = !self.hardware_busy && self.snapshot.motor.is_some();
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                can_add,
                                "Adicionar posição",
                            )
                        });
                        ui.painter().rect_filled(
                            rect,
                            6.0,
                            if can_add && response.hovered() {
                                Color32::from_rgb(49, 51, 54)
                            } else {
                                theme::CONTROL
                            },
                        );
                        ui.painter().text(
                            rect.center() - egui::vec2(0.0, 12.0),
                            egui::Align2::CENTER_CENTER,
                            "+",
                            egui::FontId::proportional(34.0),
                            theme::MUTED,
                        );
                        ui.painter().text(
                            egui::pos2(rect.center().x, rect.bottom() - 16.0),
                            egui::Align2::CENTER_CENTER,
                            "Adicionar posição",
                            egui::FontId::proportional(11.0),
                            theme::MUTED,
                        );
                        if can_add && response.clicked() {
                            self.preset_name.clear();
                            self.preset_dialog = true;
                            ui.memory_mut(|memory| {
                                memory.request_focus(egui::Id::new("preset_name_entry"))
                            });
                        }
                    }
                    if index % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
        if let Some(id) = remove_id {
            self.presets.retain(|preset| preset.id != id);
            self.save_presets();
        }
        if self.preset_dialog {
            let mut open = true;
            let mut capture = false;
            let mut cancel = false;
            egui::Window::new("Salvar posição")
                .id(egui::Id::new("preset_name_dialog"))
                .collapsible(false)
                .resizable(false)
                .default_width(300.0)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .open(&mut open)
                .show(ui.ctx(), |ui| {
                    ui.label("Dê um nome ao enquadramento atual.");
                    let name = ui.add(
                        egui::TextEdit::singleline(&mut self.preset_name)
                            .id(egui::Id::new("preset_name_entry"))
                            .char_limit(120)
                            .hint_text("Nome da posição")
                            .desired_width(f32::INFINITY),
                    );
                    let can_save = !self.hardware_busy
                        && self.snapshot.motor.is_some()
                        && self.presets.len() < 8
                        && !self.preset_name.trim().is_empty();
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        cancel = ui.button("Cancelar").clicked();
                        capture = ui
                            .add_enabled(can_save, egui::Button::new("Salvar").fill(theme::BLUE))
                            .clicked();
                    });
                    capture |= can_save
                        && name.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    cancel |= ui.input(|input| input.key_pressed(egui::Key::Escape));
                });
            self.preset_dialog = open && !capture && !cancel;
            if capture {
                self.send(Command::CapturePreset(
                    self.preset_name.trim().to_string(),
                    None,
                ));
                self.preset_name.clear();
            }
        }
    }
}
