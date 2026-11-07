//! Debug settings (the toolbar's `settings_debug`, Advanced › Debug
//! settings): every setting of `settings/registry.rs` by name, searched by
//! name or description, with an editor for its kind, "Reset to default" and
//! the `*` of the ones away from their default. Port of
//! LLFloaterSettingsDebug (indra/newview/llfloatersettingsdebug.cpp,
//! floater_settings_debug.xml, originally LGPL 2.1): the search over names
//! and comments, the list sorted by name, "Show changed settings only",
//! the selected setting's "name: comment", TRUE / FALSE for a boolean,
//! spinners for numbers and vectors, a swatch for a colour, a field for a
//! text, Copy (the name) and the default button, and the warning at the
//! first opening. Aurora's settings are typed, so a setting keeps the
//! editor of its Rust type (an enum offers its variants, a `u8` stops at
//! 255), and the names are Aurora's dotted paths; the Legacy name is
//! searched through the description.

use super::widgets::{self, ActionKind, Floater};
use crate::settings::Settings;
use crate::settings::registry::{self, Access, Kind, Registry};
use crate::theme::Palette;
use egui::{RichText, Sense, Vec2};
use serde_json::Value;

pub const WINDOW: &str = "debug_settings";
/// floater_settings_debug.xml: 450 by 215; wider and taller for Aurora's
/// widgets and the French buttons, the same size in every language.
pub const WIDTH: f32 = 520.0;
pub const HEIGHT: f32 = 380.0;
const ROW_H: f32 = 22.0;
/// The list's share of the width (the Legacy panels: 170 of 450).
const LIST_SHARE: f32 = 0.42;
/// How long "Copied" stays after the Copy button (s).
const COPIED_FOR: f64 = 1.2;

#[derive(Default)]
pub struct Out {
    /// The name of a setting that changed: apply the settings like a
    /// change of Preferences.
    pub changed: Option<String>,
}

#[derive(Default)]
pub struct DebugSettingsUi {
    query: String,
    changed_only: bool,
    selected: Option<String>,
    registry: Option<Registry>,
    /// When `registry` was read (`ctx.input().time`).
    refreshed: f64,
    /// The rows of the list (name, modified), filtered once per change of
    /// the search, of the filter or of the registry, not every frame.
    shown: Vec<(String, bool)>,
    shown_for: Option<(String, bool, f64)>,
    /// The text editor's buffer, and the name it belongs to.
    text: String,
    text_for: Option<String>,
    /// Why the last value was refused, under the editor.
    error: Option<String>,
    /// A change waits for the pointer's release to be saved.
    unsaved: bool,
    /// A search typed before the registry was read (the demo): select its
    /// first result once it is.
    select_pending: bool,
    /// The warning was answered this session.
    warned: bool,
    dont_warn_again: bool,
    copied_at: Option<f64>,
}

impl DebugSettingsUi {
    pub fn show(&mut self, ctx: &egui::Context, p: &Palette, open: &mut bool, settings: &mut Settings) -> Out {
        let mut out = Out::default();
        if !*open {
            self.registry = None;
            return out;
        }
        let now = ctx.input(|i| i.time);
        if self.registry.is_none() || now - self.refreshed > 0.5 {
            self.registry = Some(Registry::of(settings));
            self.refreshed = now;
        }
        if std::mem::take(&mut self.select_pending) {
            self.select_first_match();
        }
        let screen = ctx.content_rect();
        let pos = egui::pos2(screen.center().x - WIDTH / 2.0, screen.top() + 120.0);
        Floater::new(
            WINDOW,
            aurora_i18n::tr!("toolbar-command-debug-settings"),
            pos,
            Vec2::new(WIDTH, HEIGHT),
        )
        .show(ctx, p, open, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
            self.body(ui, p, settings, &mut out);
        });
        if settings.debug_settings_warning && !self.warned {
            self.warning(ctx, p, settings);
        }
        if self.unsaved && !ctx.input(|i| i.pointer.any_down()) {
            self.unsaved = false;
            settings.save();
        }
        out
    }

    /// `AURORA_DEMO_DEBUG_SETTINGS`: the search filled in, the warning
    /// answered.
    pub fn demo(&mut self, query: &str) {
        self.query = query.to_owned();
        self.select_pending = !query.is_empty();
        self.warned = true;
    }

    /// DebugSettingsWarning (notifications.xml), with its ignore box.
    fn warning(&mut self, ctx: &egui::Context, p: &Palette, settings: &mut Settings) {
        let mut answered = false;
        let modal = widgets::Modal::new(egui::Id::unique("debug_settings_warning")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.label(
                RichText::new(aurora_i18n::tr!("debug-settings-warning-title"))
                    .size(15.0)
                    .strong()
                    .color(p.ink),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new(aurora_i18n::tr!("debug-settings-warning-body"))
                    .size(12.5)
                    .color(p.muted),
            );
            ui.add_space(8.0);
            widgets::checkbox(
                ui,
                p,
                &mut self.dont_warn_again,
                RichText::new(aurora_i18n::tr!("ui-context-do-not-show-again"))
                    .size(12.5)
                    .color(p.muted),
            );
            ui.add_space(8.0);
            if widgets::default_button(widgets::flat_button(ui, p, aurora_i18n::tr!("common-ok"))).clicked() {
                answered = true;
            }
        });
        if answered || modal.should_close() {
            self.warned = true;
            if self.dont_warn_again {
                settings.debug_settings_warning = false;
                settings.save();
            }
        }
    }

    fn body(&mut self, ui: &mut egui::Ui, p: &Palette, settings: &mut Settings, out: &mut Out) {
        let hint = aurora_i18n::tr!("debug-settings-search-hint");
        if widgets::search_field(ui, &mut self.query, hint, ui.available_width()).changed() {
            self.select_first_match();
        }
        let width = ui.available_width();
        let height = ui.available_height();
        let list_w = (width * LIST_SHARE).round();
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(Vec2::new(list_w, height), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_min_height(height);
                self.list(ui, p);
            });
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_min_height(height);
                    self.editor(ui, p, settings, out);
                },
            );
        });
    }

    /// A search selects the first of its results, as the Legacy list does.
    fn select_first_match(&mut self) {
        if self.query.trim().is_empty() {
            return;
        }
        let changed_only = self.changed_only;
        let first = self.registry.as_ref().and_then(|r| {
            r.matching(&self.query)
                .find(|e| !changed_only || e.modified())
                .map(|e| e.name.clone())
        });
        if first.is_some() {
            self.select(first);
        }
    }

    fn select(&mut self, name: Option<String>) {
        if self.selected != name {
            self.selected = name;
            self.text_for = None;
            self.error = None;
            self.copied_at = None;
        }
    }

    /// The rows of the list, filtered again only when something changed.
    fn shown_rows(&mut self) {
        let key = (self.query.clone(), self.changed_only, self.refreshed);
        if self.shown_for.as_ref() == Some(&key) {
            return;
        }
        let changed_only = self.changed_only;
        self.shown = match &self.registry {
            Some(registry) => registry
                .matching(&self.query)
                .filter(|e| !changed_only || e.modified())
                .map(|e| (e.name.clone(), e.modified()))
                .collect(),
            None => Vec::new(),
        };
        self.shown_for = Some(key);
    }

    fn list(&mut self, ui: &mut egui::Ui, p: &Palette) {
        if self.registry.is_none() {
            return;
        }
        self.shown_rows();
        let shown = std::mem::take(&mut self.shown);
        let mut clicked = None;
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            // DebugSettingsHideDefault, kept for the window's lifetime
            widgets::checkbox(
                ui,
                p,
                &mut self.changed_only,
                RichText::new(aurora_i18n::tr!("debug-settings-changed-only")).size(12.0),
            );
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                widgets::header_row(ui, p, &[(aurora_i18n::tr!("debug-settings-column-setting"), ui.available_width())]);
                let star_w = 12.0;
                egui::ScrollArea::vertical()
                    .id_salt(WINDOW)
                    .auto_shrink([false, false])
                    .show_rows(ui, ROW_H, shown.len(), |ui, range| {
                        if shown.is_empty() {
                            ui.label(RichText::new(aurora_i18n::tr!("debug-settings-no-match")).size(12.0).color(p.muted));
                        }
                        for (name, modified) in &shown[range] {
                            let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
                            let selected = self.selected.as_deref() == Some(name.as_str());
                            if selected || resp.hovered() {
                                let color = if selected {
                                    p.violet.gamma_multiply(0.30)
                                } else {
                                    p.raised.gamma_multiply(0.6)
                                };
                                ui.painter().rect_filled(rect, ui.visuals().window_corner_radius, color);
                            }
                            if *modified {
                                // the Legacy list's changed_setting column
                                ui.painter().text(
                                    egui::pos2(rect.left() + 4.0, rect.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    "*",
                                    egui::FontId::proportional(12.5),
                                    p.violet_light,
                                );
                            }
                            widgets::paint_elided(
                                ui,
                                name,
                                egui::pos2(rect.left() + 4.0 + star_w, rect.center().y),
                                rect.width() - star_w - 8.0,
                                egui::FontId::proportional(12.5),
                                p.ink,
                            );
                            resp.clone().on_hover_text(name);
                            if resp.clicked() {
                                clicked = Some(name.clone());
                            }
                        }
                    });
            });
        });
        self.shown = shown;
        if clicked.is_some() {
            self.select(clicked);
        }
    }

    fn editor(&mut self, ui: &mut egui::Ui, p: &Palette, settings: &mut Settings, out: &mut Out) {
        let Some(entry) = self.selected.as_deref().and_then(|name| self.registry.as_ref()?.get(name)).cloned() else {
            ui.label(
                RichText::new(aurora_i18n::tr!("debug-settings-select-one"))
                    .size(12.5)
                    .color(p.muted),
            );
            return;
        };
        let editable = entry.access == Access::Editable;
        // the comment_text box: "name: comment"
        let comment = if entry.description.is_empty() {
            entry.name.clone()
        } else {
            format!("{}: {}", entry.name, entry.description)
        };
        let comment_h = (ui.available_height() * 0.38).max(60.0);
        egui::ScrollArea::vertical()
            .id_salt((WINDOW, "comment"))
            .max_height(comment_h)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_height(comment_h);
                ui.add(
                    egui::Label::new(RichText::new(comment).size(12.0).color(p.muted))
                        .wrap()
                        .selectable(true),
                );
            });
        ui.add_space(4.0);
        let mut value = None;
        ui.add_enabled_ui(editable, |ui| {
            value = self.value_editor(ui, p, &entry);
        });
        if !editable {
            ui.label(
                RichText::new(aurora_i18n::tr!("debug-settings-read-only"))
                    .size(11.5)
                    .color(p.muted),
            );
        }
        if let Some(error) = &self.error {
            ui.label(RichText::new(error.as_str()).size(11.5).color(p.danger));
        }
        if let Some(value) = value {
            self.apply(settings, &entry.name, value, out);
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.horizontal(|ui| {
                let size = Vec2::new(0.0, 24.0);
                if widgets::action_button(ui, p, "copy", aurora_i18n::tr!("common-copy"), ActionKind::Flat, size).clicked() {
                    ui.ctx().copy_text(entry.name.clone());
                    self.copied_at = Some(ui.input(|i| i.time));
                }
                let reset = aurora_i18n::tr!("debug-settings-reset-default");
                if widgets::action_button_if(ui, p, "restore", reset, ActionKind::Flat, size, editable).clicked() {
                    match Registry::reset(settings, &entry.name) {
                        Ok(()) => {
                            self.text_for = None;
                            self.error = None;
                            self.refresh(settings, &entry.name, out);
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                let now = ui.input(|i| i.time);
                if let Some(at) = self.copied_at.filter(|at| now - at < COPIED_FOR) {
                    let fade = (1.0 - ((now - at) / COPIED_FOR) as f32).clamp(0.0, 1.0);
                    ui.label(
                        RichText::new(aurora_i18n::tr!("debug-settings-copied"))
                            .size(11.5)
                            .color(p.muted.gamma_multiply(fade)),
                    );
                    ui.ctx().request_repaint();
                }
            });
        });
    }

    /// The editor of the entry's kind; Some(value) when it set one.
    fn value_editor(&mut self, ui: &mut egui::Ui, p: &Palette, entry: &registry::Entry) -> Option<Value> {
        let ty = entry.ty();
        match &entry.kind {
            Kind::Bool => {
                let mut on = entry.value.as_bool().unwrap_or(false);
                let mut changed = false;
                ui.horizontal(|ui| {
                    // boolean_combo: TRUE / FALSE, not translated in the
                    // Legacy window either
                    changed |= widgets::radio_value(ui, p, &mut on, true, "TRUE").changed();
                    changed |= widgets::radio_value(ui, p, &mut on, false, "FALSE").changed();
                });
                changed.then_some(Value::Bool(on))
            }
            Kind::Integer { min, max } => {
                let mut v = entry.value.as_f64().unwrap_or(0.0);
                let resp = spinner(ui, &mut v, *min..=*max, 1.0, 0);
                resp.changed().then(|| integer_value(v))
            }
            Kind::Float => {
                let mut v = entry.value.as_f64().unwrap_or(0.0);
                let resp = spinner(ui, &mut v, f64::MIN..=f64::MAX, 0.1, 3);
                resp.changed().then(|| float_value(v))
            }
            Kind::Vector(n) => {
                let mut parts: Vec<f64> = entry
                    .value
                    .as_array()
                    .map(|a| a.iter().map(|v| v.as_f64().unwrap_or(0.0)).collect())
                    .unwrap_or_default();
                let integers = entry.value.as_array().is_some_and(|a| a.iter().all(|v| v.is_i64() || v.is_u64()));
                let mut changed = false;
                ui.horizontal_wrapped(|ui| {
                    for v in parts.iter_mut().take(*n) {
                        let (speed, decimals) = if integers { (1.0, 0) } else { (0.1, 3) };
                        changed |= spinner(ui, v, f64::MIN..=f64::MAX, speed, decimals).changed();
                    }
                });
                changed.then(|| {
                    Value::Array(
                        parts
                            .iter()
                            .map(|&v| if integers { integer_value(v) } else { float_value(v) })
                            .collect(),
                    )
                })
            }
            Kind::Color => {
                let mut color: [u8; 4] = [0; 4];
                for (slot, v) in color.iter_mut().zip(entry.value.as_array().into_iter().flatten()) {
                    *slot = v.as_u64().unwrap_or(0).min(255) as u8;
                }
                let before = color;
                ui.horizontal(|ui| {
                    widgets::color_button(ui, p, (WINDOW, entry.name.as_str()), &mut color, true);
                    ui.label(RichText::new(widgets::color::format_hex(color, true)).size(12.0).color(p.muted));
                });
                (color != before).then(|| Value::Array(color.iter().map(|&c| Value::from(c)).collect()))
            }
            Kind::Choice(variants) => {
                let mut current = entry.value.as_str().unwrap_or("").to_owned();
                let mut changed = false;
                egui::ComboBox::from_id_salt((WINDOW, "choice"))
                    .width(ui.available_width().min(220.0))
                    .selected_text(current.clone())
                    .show_ui(ui, |ui| {
                        for variant in variants {
                            changed |= ui.selectable_value(&mut current, variant.clone(), variant.as_str()).changed();
                        }
                    });
                changed.then(|| Value::String(current))
            }
            Kind::Text => {
                if self.text_for.as_deref() != Some(entry.name.as_str()) {
                    self.text = registry::display(&entry.value, ty);
                    self.text_for = Some(entry.name.clone());
                }
                let resp = ui.add(egui::TextEdit::singleline(&mut self.text).desired_width(ui.available_width()));
                let committed = resp.lost_focus() && self.text != registry::display(&entry.value, ty);
                committed
                    .then(|| registry::parse(&entry.kind, &self.text))
                    .and_then(|parsed| match parsed {
                        Ok(value) => Some(value),
                        Err(error) => {
                            self.error = Some(error);
                            None
                        }
                    })
            }
            Kind::Null => {
                ui.label(RichText::new(aurora_i18n::tr!("debug-settings-not-set")).size(12.0).color(p.muted));
                None
            }
        }
    }

    fn apply(&mut self, settings: &mut Settings, name: &str, value: Value, out: &mut Out) {
        match Registry::set(settings, name, value) {
            Ok(()) => {
                self.error = None;
                self.refresh(settings, name, out);
            }
            Err(error) => self.error = Some(error),
        }
    }

    /// After a change: the list read again at once, the change reported
    /// and saved on release.
    fn refresh(&mut self, settings: &Settings, name: &str, out: &mut Out) {
        self.registry = Some(Registry::of(settings));
        out.changed = Some(name.to_owned());
        self.unsaved = true;
    }
}

/// val_spinner: a drag value with the type's range.
fn spinner(ui: &mut egui::Ui, value: &mut f64, range: std::ops::RangeInclusive<f64>, speed: f64, decimals: usize) -> egui::Response {
    ui.add(
        egui::DragValue::new(value)
            .range(range)
            .clamp_existing_to_range(false)
            .speed(speed)
            .max_decimals(decimals.max(3))
            .min_decimals(decimals),
    )
}

fn integer_value(v: f64) -> Value {
    if v < 0.0 {
        Value::from(v.round() as i64)
    } else {
        Value::from(v.round() as u64)
    }
}

fn float_value(v: f64) -> Value {
    serde_json::Number::from_f64(v).map(Value::Number).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_keeps_its_size() {
        let reset = aurora_i18n::tr!("debug-settings-reset-default");
        crate::ui::widgets::steady_tests::assert_window_steady(
            WINDOW,
            &[egui::vec2(WIDTH, HEIGHT), egui::vec2(640.0, 480.0)],
            reset,
            || {
                let mut ui = DebugSettingsUi::default();
                ui.warned = true;
                ui.selected = Some("draw_distance".into());
                (ui, Settings::default())
            },
            |(window, settings), ctx, p| {
                let mut open = true;
                window.show(ctx, p, &mut open, settings);
            },
        );
    }

    #[test]
    fn a_search_selects_its_first_result_and_the_changed_filter_narrows_it() {
        let mut settings = Settings::default();
        let mut window = DebugSettingsUi::default();
        window.registry = Some(Registry::of(&settings));
        window.query = "ui_scal".into();
        window.select_first_match();
        assert_eq!(window.selected.as_deref(), Some("ui_scale"));
        Registry::set(&mut settings, "show_huds", Value::Bool(false)).expect("set");
        window.registry = Some(Registry::of(&settings));
        window.changed_only = true;
        window.query = "show".into();
        window.select_first_match();
        assert_eq!(window.selected.as_deref(), Some("show_huds"));
    }
}
