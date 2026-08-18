use crate::aether_settings::{AetherSettings, OptionsHotkey};
use crate::cli::{GameModePreference, OpenUrlMode};
use crate::gui::ThemePreference;
use crate::log::FilenamePattern;
use crate::preferences::storage::StorageBackend;
use crate::preferences::{GlobalPreferencesWatchers, SavedGlobalPreferences};
use ruffle_frontend_utils::parse::DocumentHolder;
use ruffle_render_wgpu::clap::{GraphicsBackend, PowerPreference};
use toml_edit::value;
use unic_langid::LanguageIdentifier;

pub struct PreferencesWriter<'a>(
    &'a mut DocumentHolder<SavedGlobalPreferences>,
    Option<&'a GlobalPreferencesWatchers>,
);

impl<'a> PreferencesWriter<'a> {
    pub(super) fn new(preferences: &'a mut DocumentHolder<SavedGlobalPreferences>) -> Self {
        Self(preferences, None)
    }

    pub(super) fn set_watchers(&mut self, watchers: &'a GlobalPreferencesWatchers) {
        self.1 = Some(watchers);
    }

    pub fn set_graphics_backend(&mut self, backend: GraphicsBackend) {
        self.0.edit(|values, toml_document| {
            toml_document["graphics_backend"] = value(backend.as_str());
            values.graphics_backend = backend;
        })
    }

    pub fn set_graphics_power_preference(&mut self, preference: PowerPreference) {
        self.0.edit(|values, toml_document| {
            toml_document["graphics_power_preference"] = value(preference.as_str());
            values.graphics_power_preference = preference;
        })
    }

    pub fn set_language(&mut self, language: LanguageIdentifier) {
        self.0.edit(|values, toml_document| {
            toml_document["language"] = value(language.to_string());
            values.language = language;
        })
    }

    pub fn set_output_device(&mut self, name: Option<String>) {
        self.0.edit(|values, toml_document| {
            if let Some(name) = &name {
                toml_document["output_device"] = value(name);
            } else {
                toml_document.remove("output_device");
            }
            values.output_device = name;
        })
    }

    pub fn set_mute(&mut self, mute: bool) {
        self.0.edit(|values, toml_document| {
            toml_document["mute"] = value(mute);
            values.mute = mute;
        })
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.0.edit(|values, toml_document| {
            toml_document["volume"] = value(volume as f64);
            values.volume = volume;
        })
    }

    pub fn set_enable_openh264(&mut self, enable: bool) {
        self.0.edit(|values, toml_document| {
            toml_document["enable_openh264"] = value(enable);
            values.enable_openh264 = enable;
        })
    }

    pub fn set_log_filename_pattern(&mut self, pattern: FilenamePattern) {
        self.0.edit(|values, toml_document| {
            toml_document["log"]["filename_pattern"] = value(pattern.as_str());
            values.log.filename_pattern = pattern;
        })
    }

    pub fn set_storage_backend(&mut self, backend: StorageBackend) {
        self.0.edit(|values, toml_document| {
            toml_document["storage"]["backend"] = value(backend.as_str());
            values.storage.backend = backend;
        })
    }

    pub fn set_recent_limit(&mut self, limit: usize) {
        self.0.edit(|values, toml_document| {
            toml_document["recent_limit"] = value(limit as i64);
            values.recent_limit = limit;
        })
    }

    pub fn set_theme_preference(&mut self, theme_preference: ThemePreference) {
        self.0.edit(|values, toml_document| {
            if let Some(theme_preference) = theme_preference.as_str() {
                toml_document["theme"] = value(theme_preference);
            } else {
                toml_document.remove("theme");
            }
            values.theme_preference = theme_preference;
        });
        if let Some(watcher) = self.1.map(|w| &w.theme_preference_watcher) {
            let _ = watcher.send(theme_preference);
        }
    }

    pub fn set_gamemode_preference(&mut self, gamemode_preference: GameModePreference) {
        self.0.edit(|values, toml_document| {
            if let Some(gamemode_preference) = gamemode_preference.as_str() {
                toml_document["gamemode"] = value(gamemode_preference);
            } else {
                toml_document.remove("gamemode");
            }
            values.gamemode_preference = gamemode_preference;
        });
    }

    pub fn set_open_url_mode(&mut self, open_url_mode: OpenUrlMode) {
        self.0.edit(|values, toml_document| {
            if let Some(open_url_mode) = open_url_mode.as_str() {
                toml_document["open_url_mode"] = value(open_url_mode);
            } else {
                toml_document.remove("open_url_mode");
            }
            values.open_url_mode = open_url_mode;
        });
    }

    pub fn set_ime_enabled(&mut self, ime_enabled: Option<bool>) {
        self.0.edit(|values, toml_document| {
            if let Some(ime_enabled) = ime_enabled {
                toml_document["ime"]["enabled"] = value(ime_enabled);
            } else {
                toml_document["ime"]["enabled"] = toml_edit::Item::None;
            }
            values.ime_enabled = ime_enabled;
        });
    }

    /// Write every Aether setting at once.
    ///
    /// The options window edits a working copy and saves it whole, so there is nothing to gain
    /// from a setter per toggle and a dozen of them to keep in step with the struct.
    pub fn set_aether_settings(&mut self, settings: AetherSettings) {
        self.0.edit(|values, toml_document| {
            aether_table(toml_document);
            let table = &mut toml_document["aether"];
            for (key, field) in [
                ("number_separators", settings.number_separators),
                ("number_separator_space", settings.number_separator_space),
                (
                    "separators_from_ten_thousand",
                    settings.separators_from_ten_thousand,
                ),
                ("timeline_child_rebind", settings.timeline_child_rebind),
                ("adaptive_avatar_cache", settings.adaptive_avatar_cache),
                ("movement_stop_guard", settings.movement_stop_guard),
                (
                    "avm2_broadcast_fast_path",
                    settings.avm2_broadcast_fast_path,
                ),
                ("mouse_motion_coalescing", settings.mouse_motion_coalescing),
                ("bounded_offscreen_pool", settings.bounded_offscreen_pool),
                ("cache_texture_grid", settings.cache_texture_grid),
                (
                    "idle_gpu_upload_eviction",
                    settings.idle_gpu_upload_eviction,
                ),
                ("low_vram", settings.low_vram),
                (
                    "tooltips_follow_pointer",
                    settings.tooltips_follow_pointer,
                ),
                ("hide_skill_tooltips", settings.hide_skill_tooltips),
                (
                    "always_show_aura_tooltips",
                    settings.always_show_aura_tooltips,
                ),
                ("recolour_focus_aura", settings.recolour_focus_aura),
                ("crash_report", settings.crash_report),
                ("ui_font_all_text", settings.ui_font_all_text),
            ] {
                table[key] = value(field);
            }
            table["quality"] = value(settings.quality.to_string());
            table["focus_aura_colour"] = value(settings.focus_aura_colour.to_string());
            table["ui_font"] = value(settings.ui_font.to_string());
            match settings.msaa_samples {
                Some(samples) => table["msaa_samples"] = value(i64::from(samples)),
                None => table["msaa_samples"] = toml_edit::Item::None,
            }
            // Zero, not an absent key. The default frame rate is 60, so leaving the key out would
            // read back as 60 and there would be no way to say "follow the movie's own rate". The
            // reader maps any non-positive value back to `None`.
            table["max_fps"] = value(settings.max_fps.unwrap_or(0.0));
            values.aether = settings;
        });
    }

    pub fn set_aether_options_hotkey(&mut self, hotkey: OptionsHotkey) {
        self.0.edit(|values, toml_document| {
            aether_table(toml_document);
            toml_document["aether"]["options_hotkey"] = value(hotkey.to_string());
            values.aether_options_hotkey = hotkey;
        });
    }
}

/// Make sure `[aether]` is a real section rather than the inline table toml_edit would create.
///
/// Indexing a key that does not exist yet produces `aether = { a = true, b = true, ... }`, which
/// for a dozen settings is one unreadable line. Storing the hotkey as `F1` instead of a keycode
/// was for the benefit of anyone opening this file, and the layout should match that.
fn aether_table(toml_document: &mut toml_edit::DocumentMut) {
    if !toml_document.contains_key("aether") {
        toml_document["aether"] = toml_edit::table();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preferences::read::read_preferences;
    use fluent_templates::loader::langid;

    ruffle_frontend_utils::define_serialization_test_helpers!(
        read_preferences,
        SavedGlobalPreferences,
        PreferencesWriter
    );

    #[test]
    fn set_graphics_backend() {
        test(
            "",
            |writer| writer.set_graphics_backend(GraphicsBackend::Default),
            "graphics_backend = \"default\"\n",
        );

        test(
            "graphics_backend = \"fast\"",
            |writer| writer.set_graphics_backend(GraphicsBackend::Vulkan),
            "graphics_backend = \"vulkan\"\n",
        );
    }

    #[test]
    fn set_graphics_power_preference() {
        test(
            "",
            |writer| writer.set_graphics_power_preference(PowerPreference::High),
            "graphics_power_preference = \"high\"\n",
        );

        test(
            "graphics_power_preference = \"fast\"",
            |writer| writer.set_graphics_power_preference(PowerPreference::Low),
            "graphics_power_preference = \"low\"\n",
        );
    }

    #[test]
    fn set_language() {
        test(
            "",
            |writer| writer.set_language(langid!("en-US")),
            "language = \"en-US\"\n",
        );

        test(
            "language = \"???\"",
            |writer| writer.set_language(langid!("en-Latn-US-valencia")),
            "language = \"en-Latn-US-valencia\"\n",
        );
    }

    #[test]
    fn set_output_device() {
        test(
            "",
            |writer| writer.set_output_device(Some("Speakers".to_string())),
            "output_device = \"Speakers\"\n",
        );

        test(
            "output_device = \"Speakers\"",
            |writer| writer.set_output_device(None),
            "",
        );
    }

    #[test]
    fn set_volume() {
        test("", |writer| writer.set_volume(0.5), "volume = 0.5\n");
    }

    #[test]
    fn set_mute() {
        test("", |writer| writer.set_mute(true), "mute = true\n");
        test(
            "mute = true",
            |writer| writer.set_mute(false),
            "mute = false\n",
        );
    }

    #[test]
    fn set_enable_openh264() {
        test(
            "",
            |writer| writer.set_enable_openh264(false),
            "enable_openh264 = false\n",
        );
        test(
            "enable_openh264 = false",
            |writer| writer.set_enable_openh264(true),
            "enable_openh264 = true\n",
        );
    }

    #[test]
    fn set_log_filename_pattern() {
        test(
            "",
            |writer| writer.set_log_filename_pattern(FilenamePattern::WithTimestamp),
            "log = { filename_pattern = \"with_timestamp\" }\n",
        );
        test(
            "log = { filename_pattern = \"with_timestamp\" }\n",
            |writer| writer.set_log_filename_pattern(FilenamePattern::SingleFile),
            "log = { filename_pattern = \"single_file\" }\n",
        );
        test(
            "[log]\nfilename_pattern = \"with_timestamp\"\n",
            |writer| writer.set_log_filename_pattern(FilenamePattern::SingleFile),
            "[log]\nfilename_pattern = \"single_file\"\n",
        );
    }

    #[test]
    fn set_storage_backend() {
        test(
            "",
            |writer| writer.set_storage_backend(StorageBackend::Disk),
            "storage = { backend = \"disk\" }\n",
        );
        test(
            "storage = { backend = \"disk\" }\n",
            |writer| writer.set_storage_backend(StorageBackend::Memory),
            "storage = { backend = \"memory\" }\n",
        );
        test(
            "[storage]\nbackend = \"disk\"\n",
            |writer| writer.set_storage_backend(StorageBackend::Memory),
            "[storage]\nbackend = \"memory\"\n",
        );
    }

    #[test]
    fn set_recent_limit() {
        test(
            "",
            |writer| writer.set_recent_limit(40),
            "recent_limit = 40\n",
        );
        test(
            "recent_limit = 5",
            |writer| writer.set_recent_limit(15),
            "recent_limit = 15\n",
        );
    }

    #[test]
    fn set_theme() {
        test(
            "theme = 6\n",
            |writer| writer.set_theme_preference(ThemePreference::Dark),
            "theme = \"dark\"\n",
        );
        test(
            "theme = \"dark\"",
            |writer| writer.set_theme_preference(ThemePreference::System),
            "",
        );
    }

    #[test]
    fn set_gamemode() {
        test(
            "gamemode = 6\n",
            |writer| writer.set_gamemode_preference(GameModePreference::Off),
            "gamemode = \"off\"\n",
        );
        test(
            "gamemode = \"on\"",
            |writer| writer.set_gamemode_preference(GameModePreference::Default),
            "",
        );
    }

    #[test]
    fn set_open_url_mode() {
        test(
            "open_url_mode = 6\n",
            |writer| writer.set_open_url_mode(OpenUrlMode::Allow),
            "open_url_mode = \"allow\"\n",
        );
        test(
            "open_url_mode = \"deny\"",
            |writer| writer.set_open_url_mode(OpenUrlMode::Confirm),
            "",
        );
    }

    #[test]
    #[test]
    fn set_aether_settings() {
        test(
            "",
            |writer| {
                writer.set_aether_settings(AetherSettings {
                    number_separator_space: true,
                    cache_texture_grid: false,
                    ..Default::default()
                })
            },
            "[aether]
number_separators = true
number_separator_space = true
separators_from_ten_thousand = false
timeline_child_rebind = true
adaptive_avatar_cache = true
movement_stop_guard = true
avm2_broadcast_fast_path = true
mouse_motion_coalescing = true
bounded_offscreen_pool = true
cache_texture_grid = false
idle_gpu_upload_eviction = true
low_vram = false
tooltips_follow_pointer = false
hide_skill_tooltips = false
always_show_aura_tooltips = true
recolour_focus_aura = false
crash_report = true
ui_font_all_text = false
quality = \"high\"
focus_aura_colour = \"red\"
ui_font = \"default\"
max_fps = 60.0
",
        );
    }

    /// The four settings that were in the options window but in neither the reader nor the
    /// writer, so they were discarded on exit and came back as their defaults every launch.
    #[test]
    fn the_tooltip_and_vram_settings_are_read_back() {
        let saved = read_preferences(
            "[aether]
low_vram = true
tooltips_follow_pointer = true
hide_skill_tooltips = true
always_show_aura_tooltips = false
",
        )
        .result
        .aether;

        assert!(saved.low_vram);
        assert!(saved.tooltips_follow_pointer);
        assert!(saved.hide_skill_tooltips);
        assert!(!saved.always_show_aura_tooltips);
    }

    /// The two settings that can be unset have to survive being unset.
    ///
    /// They differ because their defaults do. MSAA defaults to "let the renderer decide", so an
    /// absent key already means that and the key is removed. The frame rate defaults to 60, so an
    /// absent key would read back as 60 and "follow the movie's own rate" would be unsayable;
    /// zero carries it instead.
    ///
    /// Keys already in the file keep their position, which is why `max_fps` stays at the top here.
    #[test]
    fn an_unset_msaa_and_frame_rate_survive_a_round_trip() {
        test(
            "[aether]\nmsaa_samples = 4\nmax_fps = 144.0\n",
            |writer| {
                writer.set_aether_settings(AetherSettings {
                    msaa_samples: None,
                    max_fps: None,
                    ..Default::default()
                })
            },
            "[aether]
max_fps = 0.0
number_separators = true
number_separator_space = false
separators_from_ten_thousand = false
timeline_child_rebind = true
adaptive_avatar_cache = true
movement_stop_guard = true
avm2_broadcast_fast_path = true
mouse_motion_coalescing = true
bounded_offscreen_pool = true
cache_texture_grid = true
idle_gpu_upload_eviction = true
low_vram = false
tooltips_follow_pointer = false
hide_skill_tooltips = false
always_show_aura_tooltips = true
recolour_focus_aura = false
crash_report = true
ui_font_all_text = false
quality = \"high\"
focus_aura_colour = \"red\"
ui_font = \"default\"
",
        );
    }

    #[test]
    fn set_aether_options_hotkey() {
        test(
            "",
            |writer| {
                writer.set_aether_options_hotkey(OptionsHotkey {
                    modifiers: egui::Modifiers::COMMAND,
                    key: egui::Key::F9,
                })
            },
            "[aether]
options_hotkey = \"Ctrl+F9\"
",
        );
    }

    #[test]
    fn set_ime_enabled() {
        test(
            "ime.enabled = true\n",
            |writer| writer.set_ime_enabled(Some(false)),
            "ime.enabled = false\n",
        );
        test(
            "ime = {}",
            |writer| writer.set_ime_enabled(Some(true)),
            "ime = { enabled = true }\n",
        );
        test(
            "ime.enabled = false",
            |writer| writer.set_ime_enabled(None),
            "",
        );
    }
}
