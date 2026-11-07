//! Debug settings (the toolbar's `settings_debug`, Advanced › Debug
//! settings): the window of `ui/debug_settings.rs` and what a change of it
//! sets going.
use super::*;

impl App {
    /// Each frame, after Preferences: a change is applied by the frame's
    /// settings pass like one of Preferences, the fonts and the interface
    /// scale included when their settings are the ones changed.
    pub(super) fn debug_settings_window(&mut self, ctx: &egui::Context, p: &crate::theme::Palette, a: &mut UiActions) {
        if self.demo {
            self.demo_debug_settings();
        }
        let mut open = self.panels.debug_settings;
        let out = self.debug_settings_ui.show(ctx, p, &mut open, &mut self.settings);
        self.panels.debug_settings = open;
        if let Some(name) = out.changed {
            a.settings_changed = true;
            // Preferences' Appearance page (fonts) and Layout page (scale)
            a.font_changed |= name.starts_with("font") || name.contains("font_scale");
            a.ui_scale_changed |= name == "ui_scale";
        }
    }

    /// AURORA_DEMO_DEBUG_SETTINGS=1|search=<text>: the window opened at
    /// the start, its warning answered, a search typed.
    fn demo_debug_settings(&mut self) {
        if self.debug_settings_demo_done {
            return;
        }
        let Some(value) = std::env::var_os("AURORA_DEMO_DEBUG_SETTINGS") else {
            return;
        };
        self.debug_settings_demo_done = true;
        let value = value.to_string_lossy();
        let query = value.strip_prefix("search=").unwrap_or("");
        self.debug_settings_ui.demo(query);
        ui::widgets::open_floater(&self.egui_ctx, ui::debug_settings::WINDOW, &mut self.panels.debug_settings);
    }
}
