//! The in-game options window.
//!
//! Everything here was a command-line flag first. The window exists so that none of it needs a
//! shortcut with arguments baked into it, and so that a setting survives a restart.
//!
//! Two rules shape what appears:
//!
//! Anything that can expose credentials or exists only for debugging stays on the command line.
//! `--avm-trace` prints the game's own trace output, which includes login and session data, and is
//! the one with a real disclosure risk. `--input-trace` and `--timeline-trace` are development
//! tools that write large files and mean nothing to a player; they record mouse events and AVM2
//! frame construction respectively, not anything typed. `--socket-allow` and `--tcp-connections`
//! are network policy. None of them should be one click away in a window opened by accident.
//!
//! A setting the command line pinned is shown, greyed, rather than hidden. Hiding it would leave
//! a player looking at a window that disagrees with their game and no way to see why.

use crate::aether_settings::{
    AetherSettings, OptionsHotkey, Resolved, ResolvedAetherSettings, ResolvedSetting,
};
use crate::preferences::GlobalPreferences;
use egui::{
    Align2, Button, Checkbox, ComboBox, Grid, Key, Modifiers, RichText, Ui, Widget, Window,
};
use ruffle_core::Player;
use ruffle_render::quality::StageQuality;
use unic_langid::LanguageIdentifier;

#[derive(Clone, Copy, PartialEq, Eq)]
enum OptionsTab {
    Gameplay,
    Visuals,
    Advanced,
}

/// One row of the Advanced tab.
type AdvancedRow<'a> = (&'a mut bool, ResolvedSetting, &'static str, &'static str);

pub struct AetherOptionsDialog {
    preferences: GlobalPreferences,

    /// What applies right now, including which settings the command line pinned.
    resolved: ResolvedAetherSettings,

    /// The working copy the controls edit. Nothing is written until Save.
    settings: AetherSettings,

    hotkey: OptionsHotkey,

    /// While true the next key press becomes the new binding instead of doing what it normally
    /// would.
    rebinding: bool,

    tab: OptionsTab,

    /// What the renderer and player were actually built with, taken from process start rather
    /// than from the saved file. Comparing against the file would forget the moment Save wrote to
    /// it, and the warning would vanish while the old value was still the one running.
    applied_at_open: AetherSettings,

    aqw_mode: bool,
}

impl AetherOptionsDialog {
    pub fn new(preferences: GlobalPreferences) -> Self {
        let settings = preferences.aether_saved();
        Self {
            resolved: preferences.aether(),
            hotkey: preferences.aether_options_hotkey(),
            settings,
            applied_at_open: preferences.aether_at_startup(),
            rebinding: false,
            tab: OptionsTab::Gameplay,
            aqw_mode: preferences.aqw_mode(),
            preferences,
        }
    }

    pub fn is_rebinding(&self) -> bool {
        self.rebinding
    }

    pub fn show(
        &mut self,
        _locale: &LanguageIdentifier,
        egui_ctx: &egui::Context,
        player: Option<&mut Player>,
    ) -> bool {
        let mut keep_open = true;
        let mut should_close = false;
        // Deferred out of the closure: the player borrow cannot cross into the window body.
        let mut saving = false;

        if self.rebinding {
            self.capture_rebind(egui_ctx);
        }

        Window::new("Aether Options")
            .open(&mut keep_open)
            // Centred on first open, then wherever it is dragged to. An `anchor` would re-pin it
            // every frame, which is what stops a window being movable at all, and this one covers
            // the middle of the screen where the game is.
            .pivot(Align2::CENTER_CENTER)
            .default_pos(egui_ctx.content_rect().center())
            .collapsible(false)
            .resizable(false)
            .show(egui_ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tab, OptionsTab::Gameplay, "Gameplay");
                    ui.selectable_value(
                        &mut self.tab,
                        OptionsTab::Visuals,
                        "Visuals & Performance",
                    );
                    ui.selectable_value(&mut self.tab, OptionsTab::Advanced, "Advanced");
                });
                ui.separator();

                match self.tab {
                    OptionsTab::Gameplay => self.show_gameplay_tab(ui),
                    OptionsTab::Visuals => self.show_visuals_tab(ui),
                    OptionsTab::Advanced => self.show_advanced_tab(ui),
                }

                ui.separator();

                if self.restart_required() {
                    ui.colored_label(
                        ui.style().visuals.warn_fg_color,
                        "Some of these changes apply the next time Aether starts",
                    );
                }

                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if Button::new("Save").ui(ui).clicked() {
                            saving = true;
                            should_close = true;
                        }
                        if Button::new("Reset to defaults").ui(ui).clicked() {
                            self.settings = AetherSettings::default();
                            self.hotkey = OptionsHotkey::default();
                        }
                    })
                });
            });

        if saving {
            self.save(player);
        }

        keep_open && !should_close
    }

    /// What the game does, as opposed to how it looks or how fast it runs.
    fn show_gameplay_tab(&mut self, ui: &mut Ui) {
        ui.label("Numbers");
        toggle(
            ui,
            &mut self.settings.number_separators,
            self.resolved.number_separators,
            "Group large numbers",
            "Health, gold, experience, reputation, quest progress and item counts are written as 1,250,000 rather than 1250000.",
        );

        // Gated on what will actually apply, not on the checkbox above. When the command line has
        // pinned grouping off, the box above is greyed and unticked, and the two choices about how
        // to group are equally moot.
        let grouping_on = if self.resolved.number_separators.from_command_line {
            self.resolved.number_separators.value
        } else {
            self.settings.number_separators
        };

        ui.add_enabled_ui(grouping_on, |ui| {
            ui.indent("separator-style", |ui| {
                toggle(
                    ui,
                    &mut self.settings.number_separator_space,
                    self.resolved.number_separator_space,
                    "Separate with spaces instead of commas",
                    "1 250 000 rather than 1,250,000. A comma can read as a decimal point, which much of the world writes that way.",
                );
                toggle(
                    ui,
                    &mut self.settings.separators_from_ten_thousand,
                    self.resolved.separators_from_ten_thousand,
                    "Only from ten thousand upwards",
                    "2750 is left as it is, while 12,750 is still grouped.",
                );
            });
        });

        ui.add_space(8.0);
        ui.label("Tooltips");
        toggle(
            ui,
            &mut self.settings.tooltips_follow_pointer,
            self.resolved.tooltips_follow_pointer,
            "Show tooltips above the pointer",
            "AQW pins a skill's tooltip to the bottom-right corner, over the bag icon and away from the skill it describes. This puts it above whatever you are hovering instead. The account-safety warning is left where it is.",
        );
        toggle(
            ui,
            &mut self.settings.hide_skill_tooltips,
            self.resolved.hide_skill_tooltips,
            "Hide skill tooltips",
            "Stops a skill's tooltip appearing at all, which is what covers the screen and gets clicked through during a fight. Tooltips for buffs and auras still appear, so you can still read what is on you.",
        );
        toggle(
            ui,
            &mut self.settings.always_show_aura_tooltips,
            self.resolved.always_show_aura_tooltips,
            "Always show buff and aura tooltips",
            "Keeps the tooltip that names a buff or aura on screen and next to the pointer, whatever else is set. AQW has its own switch for these under Class Actives/Auras UI; if that one is off, nothing here can bring them back.",
        );
    }

    fn show_visuals_tab(&mut self, ui: &mut Ui) {
        ui.label("Display");
        Grid::new("aether-options-display")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("Quality");
                choice(
                    ui,
                    "aether-quality",
                    &mut self.settings.quality,
                    self.resolved.quality,
                    &[
                        (StageQuality::Low, "Low"),
                        (StageQuality::Medium, "Medium"),
                        (StageQuality::High, "High"),
                        (StageQuality::Best, "Best"),
                    ],
                    "Flash's own quality setting. It decides how vectors are antialiased and whether bitmaps are smoothed. This one takes effect straight away.",
                );
                ui.end_row();

                ui.label("Antialiasing");
                choice(
                    ui,
                    "aether-msaa",
                    &mut self.settings.msaa_samples,
                    self.resolved.msaa_samples,
                    &[
                        (None, "Automatic"),
                        (Some(1), "Off"),
                        (Some(2), "2x"),
                        (Some(4), "4x"),
                    ],
                    "Multisampling in the renderer, separate from the quality setting above. Automatic lets the renderer choose, which drops to off at high resolutions where the cost outweighs what it buys.",
                );
                ui.end_row();

                ui.label("Frame rate");
                choice(
                    ui,
                    "aether-max-fps",
                    &mut self.settings.max_fps,
                    self.resolved.max_fps,
                    &[
                        (None, "As the game asks"),
                        (Some(30.0), "30"),
                        (Some(60.0), "60"),
                        (Some(120.0), "120"),
                        (Some(144.0), "144"),
                    ],
                    "Overrides the rate the movie asks for, which is why 60 is the default here rather than the 24 AQW requests.",
                );
                ui.end_row();
            });

        ui.add_space(8.0);
        ui.label("Opening this window");
        ui.horizontal(|ui| {
            ui.label("Shortcut:");
            let label = if self.rebinding {
                "Press a key...".to_owned()
            } else {
                self.hotkey.to_string()
            };
            if Button::new(label).ui(ui).clicked() {
                self.rebinding = true;
            }
            if self.hotkey != OptionsHotkey::default() && Button::new("Reset").ui(ui).clicked() {
                self.hotkey = OptionsHotkey::default();
                self.rebinding = false;
            }
        });
    }

    fn show_advanced_tab(&mut self, ui: &mut Ui) {
        ui.label(
            RichText::new(
                "These work around specific faults in how AQW drives Flash, or trade memory for speed. Turning one off brings the original behaviour back, including the bug it was written for.",
            )
            .small()
            .weak(),
        );
        ui.add_space(4.0);

        // Grouped by what a player would be trying to fix, rather than by which subsystem the code
        // happens to live in. Someone whose mouse feels wrong should not have to know that the
        // repair for it sits next to a texture allocator.
        for (heading, rows) in self.advanced_groups() {
            ui.label(heading);
            for (value, resolved, label, hint) in rows {
                toggle(ui, value, resolved, label, hint);
            }
            ui.add_space(8.0);
        }

        ui.label(
            RichText::new(
                "The graphics backend and power preference live in the main Preferences window, under File. They are not repeated here so the two cannot disagree.",
            )
            .small()
            .weak(),
        );
        ui.label(
            RichText::new(
                "Trace logging stays on the command line. The ActionScript trace prints what the game itself logs, which includes login and session data.",
            )
            .small()
            .weak(),
        );
    }

    /// The Advanced tab's contents, as headings and the rows under them.
    ///
    /// Built as data so the ordering is one readable list rather than a run of near-identical
    /// blocks, and so a setting cannot end up drawn twice or not at all.
    fn advanced_groups(&mut self) -> [(&'static str, Vec<AdvancedRow<'_>>); 6] {
        let settings = &mut self.settings;
        let resolved = &self.resolved;

        [
            (
                "Mouse and movement",
                vec![
                    (
                        &mut settings.mouse_motion_coalescing,
                        resolved.mouse_motion_coalescing,
                        "Coalesce mouse motion",
                        "Delivers one mouse move per drawn frame rather than one per hardware event, so a high polling rate mouse cannot flood a busy map.",
                    ),
                    (
                        &mut settings.movement_stop_guard,
                        resolved.movement_stop_guard,
                        "Movement stop guard",
                        "Stops a walk animation from sticking when the square you clicked is already occupied.",
                    ),
                ],
            ),
            (
                "Avatars",
                vec![(
                    &mut settings.adaptive_avatar_cache,
                    resolved.adaptive_avatar_cache,
                    "Adaptive avatar cache",
                    "Reuses a slightly oversized cache texture as an avatar moves, instead of reallocating every time its bounds drift.",
                )],
            ),
            (
                "Textures and memory",
                vec![
                    (
                        &mut settings.cache_texture_grid,
                        resolved.cache_texture_grid,
                        "Cache texture grid",
                        "Allocates cache textures at rounded sizes so per-frame bounds drift stops churning GPU memory.",
                    ),
                    (
                        &mut settings.bounded_offscreen_pool,
                        resolved.bounded_offscreen_pool,
                        "Bounded offscreen pool",
                        "Keeps a fixed budget of offscreen render targets for reuse rather than an unbounded one.",
                    ),
                    (
                        &mut settings.idle_gpu_upload_eviction,
                        resolved.idle_gpu_upload_eviction,
                        "Idle upload eviction",
                        "Releases upload buffers that have gone unused instead of holding them for the whole session.",
                    ),
                    (
                        &mut settings.low_vram,
                        resolved.low_vram,
                        "Low VRAM mode",
                        "Keeps a much smaller budget of textures for reuse. Costs frames on a card with memory to spare, and gains them on one without, because a card that cannot hold the full budget spills into system memory instead. Try it if you see coloured noise or streaks.",
                    ),
                ],
            ),
            (
                "Interface",
                vec![(
                    &mut settings.tooltips_follow_pointer,
                    resolved.tooltips_follow_pointer,
                    "Tooltips above the pointer",
                    "AQW pins a skill's tooltip to the bottom-right corner, over the bag icon and far from the skill it describes. This puts it above whatever you are hovering instead. The account-safety warning is left where it is.",
                )],
            ),
            (
                "Performance",
                vec![(
                    &mut settings.avm2_broadcast_fast_path,
                    resolved.avm2_broadcast_fast_path,
                    "Broadcast fast path",
                    "Skips per-frame event dispatch to objects that have no listener for it.",
                )],
            ),
            (
                "Compatibility and diagnostics",
                vec![
                    (
                        &mut settings.timeline_child_rebind,
                        resolved.timeline_child_rebind,
                        "Timeline child rebind",
                        "Keeps the in-game Settings panel working when AQW rebuilds it from the timeline.",
                    ),
                    (
                        &mut settings.crash_report,
                        resolved.crash_report,
                        "Write a crash report",
                        "On a fatal error or a lost graphics device, save a report describing what the renderer was holding at the time.",
                    ),
                ],
            ),
        ]
    }

    /// Turn the next key press into the new binding.
    ///
    /// Escape cancels rather than binding, because a window opened by accident needs a way out
    /// that does not first take the shortcut away.
    fn capture_rebind(&mut self, egui_ctx: &egui::Context) {
        let pressed = egui_ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => Some((*key, *modifiers)),
                _ => None,
            })
        });

        let Some((key, modifiers)) = pressed else {
            return;
        };

        // Escape cancels, and the keys that move focus around the window are left alone. Binding
        // one of those would make the window hard to use with the keyboard.
        if modifiers.is_none()
            && matches!(
                key,
                Key::Escape | Key::Tab | Key::Enter | Key::Space | Key::Backspace
            )
        {
            if key == Key::Escape {
                self.rebinding = false;
            }
            return;
        }

        self.hotkey = OptionsHotkey {
            modifiers: Modifiers {
                command: modifiers.command || modifiers.ctrl,
                alt: modifiers.alt,
                shift: modifiers.shift,
                ..Modifiers::NONE
            },
            key,
        };
        self.rebinding = false;
    }

    /// Whether anything changed that the running renderer or player cannot pick up.
    ///
    /// The separator settings and the quality are absent on purpose. Separators are read per text
    /// assignment and quality is a property of the running stage, so both are already live by the
    /// time this is asked.
    fn restart_required(&self) -> bool {
        let before = self.applied_at_open;
        let after = self.settings;

        before.timeline_child_rebind != after.timeline_child_rebind
            || before.adaptive_avatar_cache != after.adaptive_avatar_cache
            || before.movement_stop_guard != after.movement_stop_guard
            || before.avm2_broadcast_fast_path != after.avm2_broadcast_fast_path
            || before.mouse_motion_coalescing != after.mouse_motion_coalescing
            || before.bounded_offscreen_pool != after.bounded_offscreen_pool
            || before.cache_texture_grid != after.cache_texture_grid
            || before.idle_gpu_upload_eviction != after.idle_gpu_upload_eviction
            || before.crash_report != after.crash_report
            // Both read once, while the renderer and the player are built.
            || before.msaa_samples != after.msaa_samples
            || before.max_fps != after.max_fps
    }

    fn save(&mut self, player: Option<&mut Player>) {
        let settings = self.settings;
        let hotkey = self.hotkey;

        if let Err(error) = self.preferences.write_preferences(|writer| {
            writer.set_aether_settings(settings);
            writer.set_aether_options_hotkey(hotkey);
        }) {
            tracing::warn!("Could not save Aether options: {error}");
            return;
        }

        // Re-resolve rather than assuming the saved value won: a setting the command line pinned
        // is still pinned, and applying the working copy would override the flag for this session.
        self.resolved = self.preferences.aether();
        crate::aether_settings::apply_live_settings(&self.resolved, self.aqw_mode, player);
    }
}

/// One setting picked from a short list, drawn as a dropdown unless the command line decided it.
///
/// The same read-only rule as [`toggle`]: a pinned setting shows the value the flag forced, not
/// the saved one, so the window and the game agree.
fn choice<T: Copy + PartialEq>(
    ui: &mut Ui,
    id: &str,
    value: &mut T,
    resolved: Resolved<T>,
    options: &[(T, &str)],
    hint: &str,
) {
    let name = |wanted: T| {
        options
            .iter()
            .find(|(option, _)| *option == wanted)
            .map_or("Custom", |(_, label)| *label)
    };

    if !resolved.is_editable() {
        ui.add_enabled(false, egui::Label::new(name(resolved.value)))
            .on_disabled_hover_text(format!(
                "{hint}\n\nSet on the command line for this session."
            ));
        return;
    }

    ComboBox::from_id_salt(id)
        .selected_text(name(*value))
        .show_ui(ui, |ui| {
            for (option, label) in options {
                ui.selectable_value(value, *option, *label);
            }
        })
        .response
        .on_hover_text(hint);
}

/// One setting, drawn as a checkbox unless the command line has already decided it.
fn toggle(ui: &mut Ui, value: &mut bool, resolved: ResolvedSetting, label: &str, hint: &str) {
    if resolved.is_editable() {
        Checkbox::new(value, label).ui(ui).on_hover_text(hint);
    } else {
        // Show the value the flag forced, not the saved one, so the window agrees with the game.
        let mut pinned = resolved.value;
        ui.add_enabled(false, Checkbox::new(&mut pinned, label))
            .on_disabled_hover_text(format!(
                "{hint}\n\nSet on the command line for this session."
            ));
    }
}
