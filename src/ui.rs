use crate::{
    driver::usb::{DeviceMonitor, UsbDevice},
    engine::Engine,
    model::*,
};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use std::{
    collections::{HashMap, VecDeque},
    sync::atomic::Ordering,
    time::Duration,
};

mod theme;
use theme::*;
const PINK: Color32 = AMBER;
const HISTORY: usize = 180;

// Même référence pour les deux vues ; les mesures DSP restent intactes.
fn display_power(
    s: &Spectrum,
    normalized: bool,
    blank_dc: bool,
    noise_blank: Option<f32>,
) -> Vec<f32> {
    let mut values: Vec<_> = s
        .power_db
        .iter()
        .zip(&s.floor_db)
        .map(|(power, floor)| if normalized { power - floor } else { *power })
        .collect();
    if blank_dc {
        blank_dc_bins(&mut values);
    }
    if normalized {
        if let Some(threshold) = noise_blank {
            for value in &mut values {
                if *value <= threshold {
                    *value = 0.0;
                }
            }
        }
    }
    values
}

fn blank_dc_bins(power: &mut [f32]) {
    if power.len() < 7 {
        return;
    }
    let center = power.len() / 2;
    let left = power[center - 3];
    let right = power[center + 3];
    for (i, value) in power[center - 2..=center + 2].iter_mut().enumerate() {
        *value = left + (right - left) * (i + 1) as f32 / 6.0;
    }
}

pub struct QuasarApp {
    engine: Option<Engine>,
    settings: Settings,
    snapshot: Option<Snapshot>,
    waterfall: VecDeque<Vec<f32>>,
    texture: Option<egui::TextureHandle>,
    dirty: bool,
    selected: u32,
    next_id: u32,
    normalized: bool,
    noise_blank: bool,
    noise_margin: f32,
    error: Option<String>,
    monitor: DeviceMonitor,
    devices: Vec<UsbDevice>,
    selected_device: Option<UsbDevice>,
    driver_error: Option<String>,
    rf_dirty: bool,
    rf_steps: Vec<f32>,
    rf_applied: Option<f32>,
    frequency_inputs: HashMap<u32, String>,
    center_input: String,
    frequency_error: Option<String>,
    pan_origin: Option<f64>,
    blank_dc: bool,
    meter_level: f32,
    meter_peak: f32,
    meter_hold_until: f64,
    meter_vfo: u32,
}

impl QuasarApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        let settings = Settings::default();
        Self {
            engine: None,
            settings,
            snapshot: None,
            waterfall: VecDeque::new(),
            texture: None,
            dirty: false,
            selected: 1,
            next_id: 2,
            normalized: true,
            noise_blank: false,
            noise_margin: 3.0,
            error: None,
            monitor: DeviceMonitor::default(),
            devices: Vec::new(),
            selected_device: None,
            driver_error: None,
            rf_dirty: false,
            rf_steps: vec![28.0],
            rf_applied: None,
            frequency_inputs: HashMap::new(),
            center_input: format!("{:.6}", CENTER_HZ / 1e6),
            frequency_error: None,
            pan_origin: None,
            blank_dc: true,
            meter_level: -120.0,
            meter_peak: -120.0,
            meter_hold_until: 0.0,
            meter_vfo: 0,
        }
    }

    fn receive(&mut self, ctx: &egui::Context) {
        while let Ok(update) = self.monitor.updates.try_recv() {
            match update {
                Ok(mut devices) => {
                    if self.engine.is_some() {
                        if let Some(active) = &self.selected_device {
                            UsbDevice::preserve_active_label(&mut devices, active);
                        }
                    }
                    self.devices = devices;
                    self.driver_error = None;
                }
                Err(error) => {
                    self.devices.clear();
                    self.driver_error = Some(error);
                }
            }
            if self
                .selected_device
                .as_ref()
                .is_some_and(|d| !self.devices.contains(d))
            {
                self.disconnect();
                self.selected_device = None;
            }
        }
        let Some(engine) = &self.engine else {
            return;
        };
        while let Ok((applied, steps)) = engine.rf_updates.try_recv() {
            self.rf_applied = Some(applied);
            self.rf_steps = steps;
        }
        if self.rf_dirty
            && engine
                .rf_commands
                .try_send(self.settings.rf_gain_db)
                .is_ok()
        {
            self.rf_dirty = false;
        }
        let mut updated = false;
        while let Some(snapshot) = engine.snapshots.pop() {
            if snapshot.center_hz != self.settings.center_hz {
                continue;
            }
            self.waterfall.push_front(display_power(
                &snapshot.spectrum,
                self.normalized,
                self.blank_dc,
                self.noise_blank.then_some(self.noise_margin),
            ));
            self.waterfall.truncate(HISTORY);
            self.snapshot = Some(snapshot);
            updated = true;
        }
        if let Ok(error) = engine.errors.try_recv() {
            self.disconnect();
            self.error = Some(error);
            return;
        }
        if updated {
            let mut image = egui::ColorImage::new([FFT_SIZE, HISTORY], Color32::BLACK);
            for (y, row) in self.waterfall.iter().enumerate() {
                for (x, db) in row.iter().enumerate() {
                    let t = if self.normalized {
                        (db + 5.0) / 40.0
                    } else {
                        (db + 100.0) / 85.0
                    }
                    .clamp(0.0, 1.0);
                    image.pixels[y * FFT_SIZE + x] = waterfall_color(t);
                }
            }
            if let Some(texture) = &mut self.texture {
                texture.set(image, egui::TextureOptions::LINEAR);
            } else {
                self.texture =
                    Some(ctx.load_texture("waterfall", image, egui::TextureOptions::LINEAR));
            }
        }
        if self.dirty
            && self.pan_origin.is_none()
            && self
                .engine
                .as_ref()
                .is_some_and(|e| e.commands.try_send(self.settings.clone()).is_ok())
        {
            self.dirty = false;
        }
    }

    fn disconnect(&mut self) {
        self.engine = None;
        self.snapshot = None;
        self.waterfall.clear();
        self.texture = None;
        self.error = None;
        self.rf_applied = None;
        self.rf_dirty = false;
        self.pan_origin = None;
    }

    fn connect(&mut self) {
        self.disconnect();
        if let Some(device) = self.selected_device.clone() {
            self.engine = Some(Engine::start(self.settings.clone(), device));
        }
    }

    fn clear_plot(&mut self) {
        self.snapshot = None;
        self.waterfall.clear();
        self.texture = None;
    }

    fn tune_vfo(&mut self, id: u32, hz: f64) {
        let before = self.settings.center_hz;
        self.settings.tune_vfo(id, hz);
        if let Some(vfo) = self.settings.vfos.iter().find(|v| v.id == id) {
            self.frequency_inputs
                .insert(id, format!("{:.6}", vfo.frequency_hz / 1e6));
        }
        self.center_input = format!("{:.6}", self.settings.center_hz / 1e6);
        if before != self.settings.center_hz {
            self.clear_plot();
        }
        self.frequency_error = None;
        self.dirty = true;
    }

    fn plot_input(&mut self, response: &egui::Response) {
        if !response.enabled() {
            return;
        }
        if response.drag_started() {
            self.pan_origin = Some(self.settings.center_hz);
        }
        if response.dragged() {
            if let Some(origin) = self.pan_origin {
                let center = pan_center(origin, response.drag_delta().x, response.rect.width());
                if center != self.settings.center_hz {
                    self.settings.center_hz = center;
                    self.center_input = format!("{:.6}", center / 1e6);
                    self.clear_plot();
                }
            }
        }
        if response.drag_stopped() {
            self.pan_origin = None;
            self.dirty = true;
        }
        if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let hz = self.settings.center_hz
                    + ((pos.x - response.rect.left()) / response.rect.width() - 0.5) as f64
                        * IQ_RATE as f64;
                self.tune_vfo(self.selected, hz.round());
            }
        }
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (logo, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
            let p = ui.painter();
            p.circle_stroke(logo.center(), 10.0, Stroke::new(1.5_f32, CYAN));
            p.line_segment(
                [
                    logo.left_bottom() + Vec2::new(3.0, -3.0),
                    logo.right_top() + Vec2::new(-3.0, 3.0),
                ],
                Stroke::new(2.0_f32, CYAN),
            );
            p.circle_filled(logo.center(), 3.0, AMBER);
            ui.label(
                egui::RichText::new("QUASAR")
                    .strong()
                    .size(19.0)
                    .color(TEXT),
            );
            caption(ui, "SDR");
            ui.separator();
            if ui.available_width() > 1050.0 {
                caption(ui, "L'ESSENTIEL DU SIGNAL");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let running = self.engine.is_some();
                if ui
                    .add_enabled(
                        self.selected_device.is_some(),
                        egui::Button::new(
                            egui::RichText::new(if running { "RX ACTIF" } else { "DÉMARRER RX" })
                                .size(11.0)
                                .color(if running { GREEN } else { MUTED }),
                        ),
                    )
                    .on_hover_text("Démarrer ou arrêter la réception USB")
                    .clicked()
                {
                    if running {
                        self.disconnect();
                    } else {
                        self.connect();
                    }
                }
                let old = self.selected_device.clone();
                ui.add_enabled_ui(!self.devices.is_empty(), |ui| {
                    egui::ComboBox::from_id_salt("usb-device")
                        .width(300.0)
                        .selected_text(
                            self.selected_device
                                .as_ref()
                                .map_or_else(String::new, UsbDevice::label),
                        )
                        .show_ui(ui, |ui| {
                            for device in &self.devices {
                                ui.selectable_value(
                                    &mut self.selected_device,
                                    Some(device.clone()),
                                    device.label(),
                                );
                            }
                        });
                });
                if old != self.selected_device {
                    self.connect();
                }
                caption(ui, "SOURCE USB");
            });
        });
        ui.add_space(5.0);
        let enabled = self.engine.is_some() && self.rf_applied.is_some();
        let frequency_width = (ui.available_width() * 0.32 - 24.0).max(280.0);
        let meter_width = (ui.available_width() * 0.27 - 24.0).max(210.0);
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal_top(|ui| {
                card().show(ui, |ui| { ui.vertical(|ui| {
                    ui.set_width(frequency_width);
                    ui.set_min_height(132.0);
                    if let Some(vfo) = self.settings.vfos.iter().find(|v| v.id == self.selected).cloned() {
                        ui.horizontal(|ui| {
                            caption(ui, format!("FRÉQUENCE · VFO {:02}", vfo.id));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { caption(ui, vfo.mode.label()); });
                        });
                        let input = self.frequency_inputs.entry(vfo.id).or_insert_with(|| format!("{:.6}", vfo.frequency_hz / 1e6));
                        let response = ui.add(egui::TextEdit::singleline(input).font(egui::FontId::monospace(35.0))
                            .text_color(AMBER).frame(false).desired_width(frequency_width - 10.0));
                        let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        let mut submit = enter;
                        ui.horizontal(|ui| {
                            caption(ui, if vfo.frequency_hz == 156_800_000.0 { "MHz · VHF MARINE 16" } else { "MHz · SAISIE DIRECTE" });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                submit |= chip(ui, "ACCORDER", false).clicked();
                            });
                        });
                        if submit {
                            match parse_mhz(input) { Ok(hz) => self.tune_vfo(vfo.id, hz), Err(error) => self.frequency_error = Some(error) }
                        }
                    } else {
                        caption(ui, "FRÉQUENCE");
                        ui.label(egui::RichText::new("— — — . — — —").monospace().size(30.0).color(MUTED));
                        caption(ui, "AJOUTEZ UN CANAL");
                    }
                }); });
                card().show(ui, |ui| { ui.vertical(|ui| { ui.set_width(meter_width); ui.set_min_height(132.0); self.signal_meter(ui); }); });
                card().show(ui, |ui| { ui.vertical(|ui| {
                    ui.set_width(ui.available_width().max(225.0));
                    ui.set_min_height(132.0);
                    ui.horizontal_wrapped(|ui| {
                        caption(ui, "GAIN RF");
                        let previous = self.settings.rf_gain_db;
                        egui::ComboBox::from_id_salt("rf-gain").width(76.0).selected_text(format!("{:+.1} dB", self.settings.rf_gain_db)).show_ui(ui, |ui| {
                            for gain in &self.rf_steps { ui.selectable_value(&mut self.settings.rf_gain_db, *gain, format!("{gain:+.1} dB")); }
                        });
                        self.rf_dirty |= previous != self.settings.rf_gain_db;
                        caption(ui, "VOL");
                        ui.scope(|ui| { ui.spacing_mut().slider_width = 75.0;
                            self.dirty |= ui.add(egui::Slider::new(&mut self.settings.master_gain, 0.0..=1.0).show_value(false)).on_hover_text("Volume général").changed();
                        });
                    });
                    ui.horizontal_wrapped(|ui| {
                        self.dirty |= toggle(ui, &mut self.settings.adaptive_squelch, "SQL REL.", "Actif : seuil au-dessus du bruit local. Inactif : seuil absolu en dBFS.");
                        ui.scope(|ui| {
                            ui.spacing_mut().slider_width = 90.0;
                            self.dirty |= if self.settings.adaptive_squelch {
                                ui.add(egui::Slider::new(&mut self.settings.sql_relative_db, 0.0..=40.0).suffix(" dB").fixed_decimals(0)).changed()
                            } else { ui.add(egui::Slider::new(&mut self.settings.sql_dbfs, -120.0..=0.0).suffix(" dBFS").fixed_decimals(0)).changed() };
                        });
                    });
                    ui.horizontal_wrapped(|ui| {
                        caption(ui, "CENTRE");
                        let response = ui.add(egui::TextEdit::singleline(&mut self.center_input).font(egui::TextStyle::Monospace).desired_width(91.0));
                        caption(ui, "MHz");
                        if (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) || chip(ui, "CALER", false).on_hover_text("Accorder la fréquence centrale du matériel").clicked() {
                            match parse_mhz(&self.center_input) {
                                Ok(hz) => { self.settings.center_hz = hz; self.center_input = format!("{:.6}", hz / 1e6); self.clear_plot(); self.dirty = true; self.frequency_error = None; }
                                Err(error) => self.frequency_error = Some(error),
                            }
                        }
                    });
                }); });
            });
        });
        if self.devices.is_empty() {
            ui.label(
                egui::RichText::new("AUCUN SDR COMPATIBLE CONNECTÉ")
                    .color(RED)
                    .size(11.0),
            );
        } else if self.selected_device.is_none() {
            ui.label(
                egui::RichText::new("Sélectionnez votre SDR USB pour commencer l'écoute.")
                    .color(MUTED)
                    .size(11.0),
            );
        }
        for error in [&self.driver_error, &self.frequency_error, &self.error]
            .into_iter()
            .flatten()
        {
            ui.label(egui::RichText::new(error).color(RED).size(11.0));
        }
    }

    fn signal_meter(&mut self, ui: &mut egui::Ui) {
        caption(ui, "NIVEAU DU CANAL");
        let status = self
            .snapshot
            .as_ref()
            .and_then(|s| s.vfos.iter().find(|v| v.id == self.selected && v.in_band));
        let (dt, now) = ui.input(|i| (i.stable_dt.clamp(0.001, 0.1), i.time));
        if let Some(s) = status {
            let target = s.level_dbfs.clamp(-120.0, 0.0);
            if self.meter_vfo != self.selected {
                self.meter_level = target;
                self.meter_peak = target;
                self.meter_hold_until = now + 1.0;
                self.meter_vfo = self.selected;
            }
            let tau = if target > self.meter_level {
                0.035
            } else {
                0.35
            };
            self.meter_level += (target - self.meter_level) * (1.0 - (-dt / tau).exp());
            if target >= self.meter_peak {
                self.meter_peak = target;
                self.meter_hold_until = now + 1.0;
            } else if now > self.meter_hold_until {
                self.meter_peak = (self.meter_peak - 12.0 * dt).max(target);
            }
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{:.1}", s.level_dbfs))
                        .monospace()
                        .size(27.0)
                        .color(TEXT),
                );
                caption(ui, "dBFS");
            });
        } else {
            self.meter_vfo = 0;
            self.meter_level = -120.0;
            self.meter_peak = -120.0;
            ui.label(
                egui::RichText::new("—  dBFS")
                    .monospace()
                    .size(27.0)
                    .color(MUTED),
            );
        }
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 33.0), Sense::hover());
        response.on_hover_text("Puissance intégrée du VFO en dBFS, non calibrée en dBm. Montée 35 ms, descente 350 ms, crête maintenue 1 s.");
        let width = rect.width() / 36.0;
        for i in 0..36 {
            let threshold = -120.0 + i as f32 * 120.0 / 36.0;
            let color = if status.is_none() || self.meter_level <= threshold {
                BORDER
            } else if threshold > -15.0 {
                RED
            } else if threshold > -40.0 {
                AMBER
            } else {
                CYAN
            };
            ui.painter().rect_filled(
                Rect::from_min_size(
                    Pos2::new(rect.left() + i as f32 * width, rect.top()),
                    Vec2::new((width - 2.0).max(1.0), 11.0),
                ),
                1.0,
                color,
            );
        }
        if status.is_some() {
            let x = rect.left() + (self.meter_peak + 120.0) / 120.0 * rect.width();
            ui.painter().line_segment(
                [
                    Pos2::new(x, rect.top() - 2.0),
                    Pos2::new(x, rect.top() + 13.0),
                ],
                Stroke::new(1.5_f32, TEXT),
            );
        }
        for i in 0..=4 {
            ui.painter().text(
                Pos2::new(
                    rect.left() + i as f32 / 4.0 * rect.width(),
                    rect.top() + 17.0,
                ),
                if i == 0 {
                    egui::Align2::LEFT_TOP
                } else if i == 4 {
                    egui::Align2::RIGHT_TOP
                } else {
                    egui::Align2::CENTER_TOP
                },
                format!("{}", -120 + i * 30),
                egui::FontId::monospace(9.0),
                MUTED,
            );
        }
        ui.horizontal(|ui| {
            if let Some(s) = status {
                ui.label(
                    egui::RichText::new(format!("Δ {:+.1} dB / bruit", s.snr_db))
                        .size(10.0)
                        .color(MUTED),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(if s.squelch_open {
                            "SQL OUVERT"
                        } else {
                            "SQL FERMÉ"
                        })
                        .size(9.0)
                        .color(if s.squelch_open { GREEN } else { MUTED }),
                    );
                });
            } else {
                caption(ui, "EN ATTENTE DU SIGNAL");
            }
        });
    }
    fn controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Canaux");
            caption(
                ui,
                format!("{:02} / {:02}", self.settings.vfos.len(), MAX_VFOS),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        self.settings.vfos.len() < MAX_VFOS,
                        egui::Button::new("+ VFO"),
                    )
                    .clicked()
                {
                    let mut vfo = VfoConfig::new(self.next_id, 0.0, Mode::Nfm);
                    vfo.frequency_hz = self.settings.center_hz;
                    self.settings.vfos.push(vfo);
                    self.selected = self.next_id;
                    self.next_id += 1;
                    self.dirty = true;
                }
            });
        });
        ui.horizontal(|ui| {
            if chip(ui, "AUTO-SWAP", !self.settings.mix)
                .on_hover_text("Écouter le canal actif le plus prioritaire")
                .clicked()
            {
                self.settings.mix = false;
                self.dirty = true;
            }
            if chip(ui, "MIX", self.settings.mix)
                .on_hover_text("Mixer les canaux actifs")
                .clicked()
            {
                self.settings.mix = true;
                self.dirty = true;
            }
            self.dirty |= toggle(
                ui,
                &mut self.settings.use_vad,
                "VAD",
                "Conditionner l'écoute au détecteur d'activité audio",
            );
        });
        ui.horizontal(|ui| {
            self.dirty |= toggle(ui, &mut self.settings.voice_denoise, "VOIX IA", "RNNoise local : réduire le souffle. Peut altérer les voix faibles ; désactiver pour comparer.");
            if self.settings.voice_denoise {
                ui.scope(|ui| { ui.spacing_mut().slider_width = 70.0;
                    self.dirty |= ui.add(egui::Slider::new(&mut self.settings.voice_strength, 0.0..=1.0).show_value(false)).on_hover_text("Intensité de la réduction de bruit").changed();
                });
                caption(ui, format!("{:.0} %", self.settings.voice_strength * 100.0));
            } else { caption(ui, "AUDIO ORIGINAL"); }
        });
        ui.add_space(4.0);
        let mut remove = None;
        let mut recenter = None;
        let center = self.settings.center_hz;
        for config in &mut self.settings.vfos {
            let selected = self.selected == config.id;
            let status = self
                .snapshot
                .as_ref()
                .and_then(|s| s.vfos.iter().find(|v| v.id == config.id));
            let (label, color) = match status {
                _ if self.engine.is_none() => ("HORS LIGNE", MUTED),
                _ if !config.in_band(center) => ("HORS BANDE", MUTED),
                Some(s) if s.audible => ("ÉCOUTE", GREEN),
                Some(s) if s.squelch_open => ("ACTIF", CYAN),
                _ => ("VEILLE", MUTED),
            };
            card().fill(if selected { CARD } else { PANEL })
                .stroke(Stroke::new(1.0_f32, if selected { Color32::from_rgb(135, 112, 72) } else { BORDER }))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        if chip(ui, &format!("VFO {:02}", config.id), selected).clicked() { self.selected = config.id; }
                        ui.label(egui::RichText::new(label).size(10.0).color(color));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(egui::Button::new("×").frame(false)).on_hover_text("Retirer ce VFO").clicked() { remove = Some(config.id); }
                        });
                    });
                    if ui.add(egui::Button::new(egui::RichText::new(format!("{:.6}", config.frequency_hz / 1e6))
                        .monospace().size(25.0).color(if selected { AMBER } else { TEXT })).frame(false))
                        .on_hover_text("Sélectionner ce canal ; saisir la fréquence dans l'afficheur principal").clicked() { self.selected = config.id; }
                    ui.horizontal(|ui| {
                        caption(ui, if config.frequency_hz == 156_800_000.0 { "VHF MARINE · CANAL 16".to_owned() }
                            else { format!("MHz · OFFSET {:+.1} kHz", config.offset_hz(center) / 1000.0) });
                    });
                    let (meter, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 4.0), Sense::hover());
                    ui.painter().rect_filled(meter, 2.0, BG);
                    if let Some(s) = status {
                        let width = meter.width() * (s.snr_db / 40.0).clamp(0.0, 1.0);
                        ui.painter().rect_filled(Rect::from_min_size(meter.min, Vec2::new(width, 4.0)), 2.0, color);
                    }
                    ui.horizontal(|ui| {
                        self.dirty |= toggle(ui, &mut config.muted, "MUTE", "Couper ce canal");
                        self.dirty |= toggle(ui, &mut config.solo, "SOLO", "Écouter uniquement les canaux en solo");
                        let old = config.mode;
                        egui::ComboBox::from_id_salt(("mode", config.id)).width(80.0).selected_text(config.mode.label()).show_ui(ui, |ui| {
                            for mode in Mode::ALL { ui.selectable_value(&mut config.mode, mode, mode.label()); }
                        });
                        self.dirty |= old != config.mode;
                    });
                    if !config.in_band(center) && ui.button("Recentrer la réception").clicked() { recenter = Some(config.frequency_hz); }
                    egui::CollapsingHeader::new("Réglages du canal").id_salt(("details", config.id)).show(ui, |ui| {
                        self.dirty |= ui.add(egui::Slider::new(&mut config.gain, 0.0..=2.0).text("Gain audio")).changed();
                        self.dirty |= ui.add(egui::Slider::new(&mut config.squelch_db, -10.0..=20.0).text("Correction SQL").suffix(" dB")).changed();
                        ui.horizontal(|ui| {
                            caption(ui, "PRIORITÉ");
                            self.dirty |= ui.add(egui::DragValue::new(&mut config.priority).range(0..=255)).on_hover_text("La valeur la plus basse gagne").changed();
                            let mut tone = config.ctcss_hz.is_some();
                            if toggle(ui, &mut tone, "CTCSS", "Filtrer par tonalité sub-audible") {
                                config.ctcss_hz = tone.then_some(100.0); self.dirty = true;
                            }
                        });
                        if let Some(hz) = &mut config.ctcss_hz {
                            self.dirty |= ui.add(egui::DragValue::new(hz).range(67.0..=254.1).speed(0.1).suffix(" Hz")).changed();
                        }
                        if let Some(s) = status {
                            ui.label(egui::RichText::new(format!("Δ bruit {:+.1} dB  ·  {:.1} dBFS", s.snr_db, s.level_dbfs)).small().color(MUTED));
                        }
                    });
                });
            ui.add_space(4.0);
        }
        if let Some(hz) = recenter {
            self.settings.center_hz = hz;
            self.center_input = format!("{:.6}", hz / 1e6);
            self.clear_plot();
            self.dirty = true;
        }
        if let Some(id) = remove {
            self.frequency_inputs.remove(&id);
            self.settings.vfos.retain(|c| c.id != id);
            if self.selected == id {
                self.selected = self.settings.vfos.first().map_or(0, |c| c.id);
            }
            self.dirty = true;
        }
        if self.settings.vfos.is_empty() {
            ui.add_space(20.0);
            ui.label(egui::RichText::new("Votre espace d'écoute").color(TEXT));
            ui.label(
                egui::RichText::new("Ajoutez un VFO pour surveiller une fréquence.").color(MUTED),
            );
        }
    }
    fn spectrum(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            caption(ui, "SPECTRE");
            let mut changed = toggle(
                ui,
                &mut self.normalized,
                "RELATIF",
                "Niveau au-dessus du bruit local, dans le spectre et le waterfall",
            );
            changed |= toggle(
                ui,
                &mut self.blank_dc,
                "DC",
                "Masquer visuellement le pic central du SDR",
            );
            ui.add_enabled_ui(self.normalized, |ui| {
                changed |= toggle(
                    ui,
                    &mut self.noise_blank,
                    "FOND",
                    "Masquer visuellement le bruit sous la marge choisie",
                );
            });
            if chip(ui, "AUTO ZÉRO", false)
                .on_hover_text("Mesurer le bruit pendant 2 secondes, puis suivre sa variation")
                .clicked()
            {
                self.settings.noise_zero_request = self.settings.noise_zero_request.wrapping_add(1);
                self.normalized = true;
                self.noise_blank = true;
                self.dirty = true;
                changed = true;
            }
            if changed {
                self.clear_plot();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                caption(ui, "2,40 MHz · BANDE INSTANTANÉE");
            });
        });
        ui.horizontal_wrapped(|ui| {
            if let Some(s) = &self.snapshot {
                ui.label(
                    egui::RichText::new(if s.zero_progress < 1.0 {
                        format!("Mesure du bruit  {:.0} %", s.zero_progress * 100.0)
                    } else {
                        "Bruit normalisé · suivi actif".to_owned()
                    })
                    .size(11.0)
                    .color(if s.zero_progress < 1.0 { AMBER } else { MUTED }),
                );
            } else {
                caption(ui, "EN ATTENTE DU FLUX RX");
            }
            if self.noise_blank && self.normalized {
                ui.separator();
                caption(ui, "MARGE DU FOND");
                ui.scope(|ui| {
                    ui.spacing_mut().slider_width = 75.0;
                    if ui
                        .add(egui::Slider::new(&mut self.noise_margin, 0.0..=10.0).suffix(" dB"))
                        .changed()
                    {
                        self.clear_plot();
                    }
                });
            }
        });
        let (response, painter) = ui.allocate_painter(
            Vec2::new(
                ui.available_width(),
                (ui.available_height() * 0.43).clamp(140.0, 290.0),
            ),
            Sense::click_and_drag(),
        );
        let rect = response.rect;
        painter.rect_filled(rect, 4.0, Color32::from_rgb(6, 11, 20));
        let (min, max) = if self.normalized {
            (-10.0, 40.0)
        } else {
            (-110.0, 0.0)
        };
        for i in 1..5 {
            let y = rect.top() + rect.height() * i as f32 / 5.0;
            painter.line_segment(
                [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
                Stroke::new(1.0_f32, Color32::from_gray(35)),
            );
            painter.text(
                Pos2::new(rect.left() + 4.0, y),
                egui::Align2::LEFT_BOTTOM,
                format!("{:.0} dB", max - (max - min) * i as f32 / 5.0),
                egui::FontId::monospace(10.0),
                Color32::GRAY,
            );
        }
        if let Some(s) = &self.snapshot {
            let line = |values: &[f32], floor: bool| -> Vec<Pos2> {
                values
                    .iter()
                    .enumerate()
                    .map(|(i, db)| {
                        let value = if self.normalized && floor { 0.0 } else { *db };
                        Pos2::new(
                            rect.left() + i as f32 / (FFT_SIZE - 1) as f32 * rect.width(),
                            rect.bottom()
                                - ((value - min) / (max - min)).clamp(0.0, 1.0) * rect.height(),
                        )
                    })
                    .collect()
            };
            let trace = line(
                &display_power(
                    &s.spectrum,
                    self.normalized,
                    self.blank_dc,
                    self.noise_blank.then_some(self.noise_margin),
                ),
                false,
            );
            // Dégradé discret sous la courbe, calculé uniquement sur le vrai spectre.
            let mut mesh = egui::Mesh::default();
            for point in &trace {
                mesh.colored_vertex(*point, Color32::from_rgba_unmultiplied(53, 219, 239, 42));
                mesh.colored_vertex(Pos2::new(point.x, rect.bottom()), Color32::TRANSPARENT);
            }
            for i in 0..trace.len().saturating_sub(1) {
                let j = (i * 2) as u32;
                mesh.add_triangle(j, j + 1, j + 2);
                mesh.add_triangle(j + 1, j + 3, j + 2);
            }
            painter.add(egui::Shape::mesh(mesh));
            painter.add(egui::Shape::line(trace, Stroke::new(1.25_f32, CYAN)));
            painter.add(egui::Shape::line(
                line(&s.spectrum.floor_db, true),
                Stroke::new(1.0_f32, Color32::from_rgb(55, 83, 106)),
            ));
        }
        if self.snapshot.is_none() {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                if self.engine.is_none() {
                    "Votre prochaine fréquence commence ici."
                } else {
                    "Acquisition du signal…"
                },
                egui::FontId::proportional(16.0),
                MUTED,
            );
        }
        if self.snapshot.is_some() && self.normalized && self.settings.adaptive_squelch {
            let y =
                rect.bottom() - (self.settings.sql_relative_db - min) / (max - min) * rect.height();
            painter.line_segment(
                [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
                Stroke::new(1.0_f32, Color32::LIGHT_RED),
            );
            painter.text(
                Pos2::new(rect.right() - 4.0, y),
                egui::Align2::RIGHT_BOTTOM,
                format!("SQL +{:.0} dB / bruit", self.settings.sql_relative_db),
                egui::FontId::monospace(11.0),
                Color32::LIGHT_RED,
            );
        }
        for config in &self.settings.vfos {
            if !config.in_band(self.settings.center_hz) {
                continue;
            }
            let x = rect.left()
                + (config.offset_hz(self.settings.center_hz) / IQ_RATE as f32 + 0.5) * rect.width();
            let width = config.mode.bandwidth() / IQ_RATE as f32 * rect.width();
            painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(x - width / 2.0, rect.top()),
                    Pos2::new(x + width / 2.0, rect.bottom()),
                ),
                0.0,
                Color32::from_rgba_unmultiplied(35, 224, 220, 18),
            );
            let color = if config.id == self.selected {
                PINK
            } else {
                Color32::WHITE
            };
            painter.line_segment(
                [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                Stroke::new(1.0_f32, color),
            );
            painter.text(
                Pos2::new(x + 4.0, rect.top() + 6.0),
                egui::Align2::LEFT_TOP,
                format!("V{}", config.id),
                egui::FontId::monospace(12.0),
                color,
            );
        }
        self.plot_input(&response);
        let (axis, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 25.0), Sense::hover());
        for i in 0..=8 {
            let x = axis.left() + axis.width() * i as f32 / 8.0;
            ui.painter().line_segment(
                [Pos2::new(x, axis.top()), Pos2::new(x, axis.top() + 4.0)],
                Stroke::new(1.0_f32, BORDER),
            );
            if i % 2 == 0 {
                let align = if i == 0 {
                    egui::Align2::LEFT_TOP
                } else if i == 8 {
                    egui::Align2::RIGHT_TOP
                } else {
                    egui::Align2::CENTER_TOP
                };
                ui.painter().text(
                    Pos2::new(x, axis.top() + 7.0),
                    align,
                    format!(
                        "{:.3}",
                        (self.settings.center_hz + (i as f64 / 8.0 - 0.5) * IQ_RATE as f64) / 1e6
                    ),
                    egui::FontId::monospace(10.0),
                    MUTED,
                );
            }
        }
        let size = Vec2::new(
            ui.available_width(),
            (ui.available_height() - 25.0).max(100.0),
        );
        // Toujours allouer la même surface, même pendant un réaccord : le drag
        // doit conserver son widget jusqu'au relâchement de la souris.
        let (waterfall_response, painter) = ui.allocate_painter(size, Sense::click_and_drag());
        if let Some(texture) = &self.texture {
            painter.image(
                texture.id(),
                waterfall_response.rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            painter.rect_filled(waterfall_response.rect, 0.0, Color32::from_rgb(6, 11, 20));
        }
        for config in &self.settings.vfos {
            if self.snapshot.is_none() || !config.in_band(self.settings.center_hz) {
                continue;
            }
            let x = waterfall_response.rect.left()
                + (config.offset_hz(self.settings.center_hz) / IQ_RATE as f32 + 0.5)
                    * waterfall_response.rect.width();
            let color = if config.id == self.selected {
                AMBER
            } else {
                MUTED
            };
            painter.line_segment(
                [
                    Pos2::new(x, waterfall_response.rect.top()),
                    Pos2::new(x, waterfall_response.rect.bottom()),
                ],
                Stroke::new(1.0_f32, color.gamma_multiply(0.45)),
            );
        }
        self.plot_input(&waterfall_response);
        ui.label(
            egui::RichText::new("CLIC  Accorder le canal    /    GLISSER  Explorer la bande")
                .size(10.0)
                .color(MUTED),
        );
    }
}

impl eframe::App for QuasarApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.receive(ctx);
        egui::TopBottomPanel::top("header")
            .frame(egui::Frame::new().fill(BG).inner_margin(12))
            .show(ctx, |ui| self.header(ui));
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(14, 8)),
            )
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let receiving = self.snapshot.is_some();
                    ui.label(
                        egui::RichText::new(if receiving {
                            "EN RÉCEPTION"
                        } else {
                            "EN ATTENTE"
                        })
                        .color(if receiving { GREEN } else { MUTED })
                        .size(10.0),
                    );
                    ui.separator();
                    caption(ui, "RX UNIQUEMENT");
                    if let Some(engine) = &self.engine {
                        let c = &engine.counters;
                        ui.separator();
                        let lost = c.iq_dropped.load(Ordering::Relaxed)
                            + c.usb_dropped.load(Ordering::Relaxed);
                        ui.label(
                            egui::RichText::new(format!(
                                "DSP {:.2} ms  ·  I/Q perdus {}",
                                self.snapshot.as_ref().map_or(0.0, |s| s.dsp_ms),
                                lost
                            ))
                            .size(10.0)
                            .color(if lost > 0 {
                                AMBER
                            } else {
                                MUTED
                            }),
                        )
                        .on_hover_text(format!(
                            "{}\nAudio écrasé {} · Sous-alimentation {} · Erreurs {}",
                            engine.audio_status,
                            c.audio_dropped.load(Ordering::Relaxed),
                            c.underruns.load(Ordering::Relaxed),
                            c.audio_errors.load(Ordering::Relaxed)
                        ));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        caption(ui, "QUASAR  /  48 kHz AUDIO");
                    });
                });
            });
        let enabled = self.engine.is_some() && self.rf_applied.is_some();
        egui::SidePanel::right("vfos")
            .default_width(324.0)
            .width_range(300.0..=440.0)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(12))
            .show(ctx, |ui| {
                ui.add_enabled_ui(enabled, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| self.controls(ui));
                });
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(12))
            .show(ctx, |ui| {
                ui.add_enabled_ui(enabled, |ui| self.spectrum(ui));
            });
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalized_display_blanks_dc_without_altering_measurements() {
        let floor: Vec<_> = (0..FFT_SIZE).map(|i| -120.0 + i as f32 * 0.02).collect();
        let mut s = Spectrum {
            power_db: floor.clone(),
            floor_db: floor,
        };
        s.power_db[FFT_SIZE / 2] += 50.0;
        s.power_db[100] += 12.0;
        let values = display_power(&s, true, true, Some(3.0));
        assert_eq!(values[FFT_SIZE / 2], 0.0);
        assert_eq!(values[100], 12.0);
        assert_eq!(s.power_db[FFT_SIZE / 2] - s.floor_db[FFT_SIZE / 2], 50.0);
        assert_eq!(display_power(&s, false, false, Some(3.0)), s.power_db);
    }
    #[test]
    fn dc_blanking_only_interpolates_the_five_central_bins() {
        let mut spectrum = vec![-80.0; FFT_SIZE];
        spectrum[FFT_SIZE / 2] = -5.0;
        spectrum[100] = -20.0;
        blank_dc_bins(&mut spectrum);
        assert_eq!(spectrum[FFT_SIZE / 2], -80.0);
        assert_eq!(spectrum[100], -20.0);
        assert_eq!(spectrum[FFT_SIZE / 2 + 3], -80.0);
    }
}
