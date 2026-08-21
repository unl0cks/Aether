use crate::aether_settings::OptionsHotkey;
use crate::preferences::SavedGlobalPreferences;
use ruffle_frontend_utils::parse::{
    DocumentHolder, ParseContext, ParseDetails, ParseWarning, ReadExt,
};
use toml_edit::DocumentMut;

/// Read the given preferences into a **guaranteed valid** `SavedGlobalPreferences`,
/// recording any possible warnings encountered along the way.
///
/// We wish to support backwards and forwards compatibility where possible,
/// so nothing is fatal in this function.
///
/// Default values are used wherever an unknown or invalid value is found;
/// this is to support the case of, for example, a later version having different supported
/// backends than an older version.
pub fn read_preferences(input: &str) -> ParseDetails<SavedGlobalPreferences> {
    let document = match input.parse::<DocumentMut>() {
        Ok(document) => document,
        Err(e) => {
            return ParseDetails {
                result: Default::default(),
                warnings: vec![ParseWarning::InvalidToml(e)],
            };
        }
    };

    let mut result = SavedGlobalPreferences::default();
    let mut cx = ParseContext::default();

    if let Some(value) = document.parse_from_str(&mut cx, "graphics_backend") {
        result.graphics_backend = value;
    };

    if let Some(value) = document.parse_from_str(&mut cx, "graphics_power_preference") {
        result.graphics_power_preference = value;
    };

    if let Some(value) = document.parse_from_str(&mut cx, "language") {
        result.language = value;
    };

    if let Some(value) = document.parse_from_str(&mut cx, "output_device") {
        result.output_device = Some(value);
    };

    if let Some(value) = document.get_float(&mut cx, "volume") {
        result.volume = value.clamp(0.0, 1.0) as f32;
    };

    if let Some(value) = document.get_bool(&mut cx, "mute") {
        result.mute = value;
    };

    if let Some(value) = document.get_bool(&mut cx, "enable_openh264") {
        result.enable_openh264 = value;
    };

    if let Some(value) = document.get_integer(&mut cx, "recent_limit") {
        result.recent_limit = value as usize;
    }

    if let Some(value) = document.parse_from_str(&mut cx, "theme") {
        result.theme_preference = value;
    }

    if let Some(value) = document.parse_from_str(&mut cx, "gamemode") {
        result.gamemode_preference = value;
    }

    if let Some(value) = document.parse_from_str(&mut cx, "open_url_mode") {
        result.open_url_mode = value;
    }

    document.get_table_like(&mut cx, "log", |cx, log| {
        if let Some(value) = log.parse_from_str(cx, "filename_pattern") {
            result.log.filename_pattern = value;
        };
    });

    document.get_table_like(&mut cx, "storage", |cx, storage| {
        if let Some(value) = storage.parse_from_str(cx, "backend") {
            result.storage.backend = value;
        }
    });

    document.get_table_like(&mut cx, "ime", |cx, ime| {
        result.ime_enabled = ime.get_bool(cx, "enabled");
    });

    // Everything the in-game options window writes lives under one table, so the Aether settings
    // stay visibly separate from the ones Ruffle itself understands.
    document.get_table_like(&mut cx, "aether", |cx, aether| {
        let settings = &mut result.aether;
        for (key, field) in [
            ("number_separators", &mut settings.number_separators),
            (
                "number_separator_space",
                &mut settings.number_separator_space,
            ),
            (
                "separators_from_ten_thousand",
                &mut settings.separators_from_ten_thousand,
            ),
            ("timeline_child_rebind", &mut settings.timeline_child_rebind),
            ("adaptive_avatar_cache", &mut settings.adaptive_avatar_cache),
            ("movement_stop_guard", &mut settings.movement_stop_guard),
            (
                "avm2_broadcast_fast_path",
                &mut settings.avm2_broadcast_fast_path,
            ),
            (
                "mouse_motion_coalescing",
                &mut settings.mouse_motion_coalescing,
            ),
            (
                "bounded_offscreen_pool",
                &mut settings.bounded_offscreen_pool,
            ),
            ("cache_texture_grid", &mut settings.cache_texture_grid),
            (
                "idle_gpu_upload_eviction",
                &mut settings.idle_gpu_upload_eviction,
            ),
            ("low_vram", &mut settings.low_vram),
            ("filter_atlas", &mut settings.filter_atlas),
            ("cache_texture_pool", &mut settings.cache_texture_pool),
            ("blend_batching", &mut settings.blend_batching),
            ("blend_reordering", &mut settings.blend_reordering),
            ("blendcheck", &mut settings.blendcheck),
            ("prerelease_updates", &mut settings.prerelease_updates),
            (
                "tooltips_follow_pointer",
                &mut settings.tooltips_follow_pointer,
            ),
            ("hide_skill_tooltips", &mut settings.hide_skill_tooltips),
            (
                "always_show_aura_tooltips",
                &mut settings.always_show_aura_tooltips,
            ),
            ("recolour_focus_aura", &mut settings.recolour_focus_aura),
            ("crash_report", &mut settings.crash_report),
            ("ui_font_all_text", &mut settings.ui_font_all_text),
        ] {
            if let Some(value) = aether.get_bool(cx, key) {
                *field = value;
            }
        }

        // The setting used to name the wrong half of the pair: it tinted Reckless, when the one
        // worth watching is Focus, the shorter of the two. Carrying the old key over means a player
        // who already turned it on does not silently lose it to the rename -- which is exactly how
        // the tooltip settings went missing once already. Read only when the new key is absent, so
        // the new one always wins once it has been written.
        if aether.get_bool(cx, "recolour_focus_aura").is_none()
            && let Some(value) = aether.get_bool(cx, "recolour_reckless_aura")
        {
            settings.recolour_focus_aura = value;
        }

        if let Some(value) = aether.parse_from_str(cx, "ui_font") {
            settings.ui_font = value;
        }
        if let Some(value) = aether.parse_from_str(cx, "focus_aura_colour") {
            settings.focus_aura_colour = value;
        }
        if let Some(value) = aether.parse_from_str(cx, "bitmap_cache_sweep") {
            settings.bitmap_cache_sweep = value;
        }

        if let Some(value) = aether.parse_from_str(cx, "quality") {
            settings.quality = value;
        }

        // Only 1, 2 and 4 are offered; anything else is a hand-edit and the renderer's own choice
        // is the safer answer than an unsupported sample count.
        if let Some(value) = aether.get_integer(cx, "msaa_samples") {
            settings.msaa_samples = matches!(value, 1 | 2 | 4).then_some(value as u8);
        }

        if let Some(value) = aether.get_float(cx, "max_fps") {
            // Zero or negative would stop the movie rather than uncap it.
            settings.max_fps = (value > 0.0).then_some(value);
        }

        // A binding that no longer parses leaves the default in place. Refusing to guess is what
        // stops a typo in this file from leaving the window with no way to open.
        if let Some(written) = aether.parse_from_str::<String>(cx, "options_hotkey")
            && let Some(hotkey) = OptionsHotkey::parse(&written)
        {
            result.aether_options_hotkey = hotkey;
        }
    });

    ParseDetails {
        warnings: cx.warnings,
        result: DocumentHolder::new(result, document),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{GameModePreference, OpenUrlMode};
    use crate::gui::ThemePreference;
    use crate::log::FilenamePattern;
    use crate::preferences::{LogPreferences, StoragePreferences, storage::StorageBackend};
    use fluent_templates::loader::langid;
    use ruffle_render_wgpu::clap::{GraphicsBackend, PowerPreference};

    #[test]
    fn invalid_toml() {
        let result = read_preferences("~~INVALID~~");

        assert_eq!(&SavedGlobalPreferences::default(), result.values());
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(
            "Invalid TOML: TOML parse error at line 1, column 12\n  |\n1 | ~~INVALID~~\n  |            ^\nkey with no value, expected `=`\n",
            result.warnings[0].to_string()
        );
    }

    #[test]
    fn empty_toml() {
        let result = read_preferences("");

        assert_eq!(&SavedGlobalPreferences::default(), result.values());
        assert_eq!(Vec::<ParseWarning>::new(), result.warnings);
    }

    /// A hotkey that no longer parses has to leave the default in place. Taking the binding away
    /// would leave the options window with no way to open, and the file that broke it is the one
    /// place a player could not then go to fix it.
    #[test]
    fn a_broken_hotkey_falls_back_to_f1() {
        for written in ["\"\"", "\"Meta+F1\"", "\"F99\"", "5", "true"] {
            let result = read_preferences(&format!("[aether]\noptions_hotkey = {written}"));
            assert_eq!(
                OptionsHotkey::default(),
                result.values().aether_options_hotkey,
                "{written}"
            );
        }
    }

    #[test]
    fn aether_settings_default_when_absent_or_wrong_typed() {
        let defaults = crate::aether_settings::AetherSettings::default();
        assert_eq!(defaults, read_preferences("").values().aether);
        assert_eq!(defaults, read_preferences("[aether]").values().aether);
        assert_eq!(
            defaults,
            read_preferences("[aether]\nnumber_separators = \"yes\"")
                .values()
                .aether
        );
    }

    /// One setting written by hand must not disturb the eleven around it.
    #[test]
    fn a_single_saved_setting_leaves_the_rest_alone() {
        let read = read_preferences("[aether]\ncache_texture_grid = false\n");
        let settings = read.values().aether;

        assert!(!settings.cache_texture_grid);
        assert!(settings.number_separators);
        assert!(settings.crash_report);
    }

    #[test]
    fn invalid_backend_type() {
        let result = read_preferences("graphics_backend = 5");

        assert_eq!(&SavedGlobalPreferences::default(), result.values());
        assert_eq!(
            vec![ParseWarning::UnexpectedType {
                expected: "string",
                actual: "integer",
                path: "graphics_backend".to_string()
            }],
            result.warnings
        );
    }

    #[test]
    fn invalid_backend_value() {
        let result = read_preferences("graphics_backend = \"fast\"");

        assert_eq!(&SavedGlobalPreferences::default(), result.values());
        assert_eq!(
            vec![ParseWarning::UnsupportedValue {
                value: "fast".to_string(),
                path: "graphics_backend".to_string()
            }],
            result.warnings
        );
    }

    #[test]
    fn correct_backend_value() {
        let result = read_preferences("graphics_backend = \"vulkan\"");

        assert_eq!(
            &SavedGlobalPreferences {
                graphics_backend: GraphicsBackend::Vulkan,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(Vec::<ParseWarning>::new(), result.warnings);
    }

    #[test]
    fn invalid_power_type() {
        let result = read_preferences("graphics_power_preference = 5");

        assert_eq!(&SavedGlobalPreferences::default(), result.values());
        assert_eq!(
            vec![ParseWarning::UnexpectedType {
                expected: "string",
                actual: "integer",
                path: "graphics_power_preference".to_string()
            }],
            result.warnings
        );
    }

    #[test]
    fn invalid_power_value() {
        let result = read_preferences("graphics_power_preference = \"fast\"");

        assert_eq!(&SavedGlobalPreferences::default(), result.values());
        assert_eq!(
            vec![ParseWarning::UnsupportedValue {
                value: "fast".to_string(),
                path: "graphics_power_preference".to_string()
            }],
            result.warnings
        );
    }

    #[test]
    fn correct_power_value() {
        let result = read_preferences("graphics_power_preference = \"low\"");

        assert_eq!(
            &SavedGlobalPreferences {
                graphics_power_preference: PowerPreference::Low,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(Vec::<ParseWarning>::new(), result.warnings);
    }

    #[test]
    fn invalid_language_value() {
        let result = read_preferences("language = \"???\"");

        assert_eq!(&SavedGlobalPreferences::default(), result.values());
        assert_eq!(
            vec![ParseWarning::UnsupportedValue {
                value: "???".to_string(),
                path: "language".to_string()
            }],
            result.warnings
        );
    }

    #[test]
    fn correct_language_value() {
        let result = read_preferences("language = \"en-US\"");

        assert_eq!(
            &SavedGlobalPreferences {
                language: langid!("en-US"),
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(Vec::<ParseWarning>::new(), result.warnings);
    }

    #[test]
    fn correct_output_device() {
        let result = read_preferences("output_device = \"Speakers\"");

        assert_eq!(
            &SavedGlobalPreferences {
                output_device: Some("Speakers".to_string()),
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(Vec::<ParseWarning>::new(), result.warnings);
    }

    #[test]
    fn invalid_output_device() {
        let result = read_preferences("output_device = 5");

        assert_eq!(
            &SavedGlobalPreferences {
                output_device: None,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(
            vec![ParseWarning::UnexpectedType {
                expected: "string",
                actual: "integer",
                path: "output_device".to_string()
            }],
            result.warnings
        );
    }

    #[test]
    fn mute() {
        let result = read_preferences("mute = \"false\"");
        assert_eq!(
            &SavedGlobalPreferences {
                mute: false,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(
            vec![ParseWarning::UnexpectedType {
                expected: "boolean",
                actual: "string",
                path: "mute".to_string()
            }],
            result.warnings
        );

        let result = read_preferences("mute = true");
        assert_eq!(
            &SavedGlobalPreferences {
                mute: true,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(Vec::<ParseWarning>::new(), result.warnings);

        let result = read_preferences("");
        assert_eq!(
            &SavedGlobalPreferences {
                mute: false,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(Vec::<ParseWarning>::new(), result.warnings);
    }

    #[test]
    fn volume() {
        let result = read_preferences("volume = \"0.5\"");
        assert_eq!(
            &SavedGlobalPreferences {
                volume: 1.0,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(
            vec![ParseWarning::UnexpectedType {
                expected: "float",
                actual: "string",
                path: "volume".to_string()
            }],
            result.warnings
        );

        let result = read_preferences("volume = 0.5");
        assert_eq!(
            &SavedGlobalPreferences {
                volume: 0.5,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(Vec::<ParseWarning>::new(), result.warnings);

        let result = read_preferences("volume = -1.0");
        assert_eq!(
            &SavedGlobalPreferences {
                volume: 0.0,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(Vec::<ParseWarning>::new(), result.warnings);
    }

    #[test]
    fn enable_openh264() {
        let result = read_preferences("enable_openh264 = \"true\"");
        assert_eq!(
            &SavedGlobalPreferences {
                enable_openh264: true,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(
            vec![ParseWarning::UnexpectedType {
                expected: "boolean",
                actual: "string",
                path: "enable_openh264".to_string()
            }],
            result.warnings
        );

        let result = read_preferences("enable_openh264 = false");
        assert_eq!(
            &SavedGlobalPreferences {
                enable_openh264: false,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(Vec::<ParseWarning>::new(), result.warnings);

        let result = read_preferences("");
        assert_eq!(
            &SavedGlobalPreferences {
                enable_openh264: true,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(Vec::<ParseWarning>::new(), result.warnings);
    }

    #[test]
    fn log_filename() {
        let result = read_preferences("log = {filename_pattern = 5}");
        assert_eq!(
            &SavedGlobalPreferences {
                log: LogPreferences {
                    ..Default::default()
                },
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(
            vec![ParseWarning::UnexpectedType {
                expected: "string",
                actual: "integer",
                path: "log.filename_pattern".to_string()
            }],
            result.warnings
        );

        let result = read_preferences("log = {filename_pattern = \"???\"}");
        assert_eq!(&SavedGlobalPreferences::default(), result.values());
        assert_eq!(
            vec![ParseWarning::UnsupportedValue {
                value: "???".to_string(),
                path: "log.filename_pattern".to_string()
            }],
            result.warnings
        );

        let result = read_preferences("log = {filename_pattern = \"with_timestamp\"}");
        assert_eq!(
            &SavedGlobalPreferences {
                log: LogPreferences {
                    filename_pattern: FilenamePattern::WithTimestamp,
                },
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(Vec::<ParseWarning>::new(), result.warnings);
    }

    #[test]
    fn log() {
        let result = read_preferences("log = \"yes\"");
        assert_eq!(&SavedGlobalPreferences::default(), result.values());
        assert_eq!(
            vec![ParseWarning::UnexpectedType {
                expected: "table",
                actual: "string",
                path: "log".to_string()
            }],
            result.warnings
        );
    }

    #[test]
    fn storage_backend() {
        let result = read_preferences("storage = {backend = 5}");
        assert_eq!(&SavedGlobalPreferences::default(), result.values());
        assert_eq!(
            vec![ParseWarning::UnexpectedType {
                expected: "string",
                actual: "integer",
                path: "storage.backend".to_string()
            }],
            result.warnings
        );

        let result = read_preferences("storage = {backend = \"???\"}");
        assert_eq!(&SavedGlobalPreferences::default(), result.values());
        assert_eq!(
            vec![ParseWarning::UnsupportedValue {
                value: "???".to_string(),
                path: "storage.backend".to_string()
            }],
            result.warnings
        );

        let result = read_preferences("storage = {backend = \"memory\"}");
        assert_eq!(
            &SavedGlobalPreferences {
                storage: StoragePreferences {
                    backend: StorageBackend::Memory,
                },
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(Vec::<ParseWarning>::new(), result.warnings);
    }

    #[test]
    fn storage() {
        let result = read_preferences("storage = \"no\"");
        assert_eq!(&SavedGlobalPreferences::default(), result.values());
        assert_eq!(
            vec![ParseWarning::UnexpectedType {
                expected: "table",
                actual: "string",
                path: "storage".to_string()
            }],
            result.warnings
        );
    }

    #[test]
    fn recent_limit() {
        let result = read_preferences("recent_limit = \"1\"");
        assert_eq!(
            &SavedGlobalPreferences {
                recent_limit: 10,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(
            vec![ParseWarning::UnexpectedType {
                expected: "integer",
                actual: "string",
                path: "recent_limit".to_string(),
            }],
            result.warnings
        );

        let result = read_preferences("recent_limit = 0.5");
        assert_eq!(
            &SavedGlobalPreferences {
                recent_limit: 10,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(
            vec![ParseWarning::UnexpectedType {
                expected: "integer",
                actual: "float",
                path: "recent_limit".to_string(),
            }],
            result.warnings
        );

        let result = read_preferences("recent_limit = 5");
        assert_eq!(
            &SavedGlobalPreferences {
                recent_limit: 5,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(Vec::<ParseWarning>::new(), result.warnings);
    }

    #[test]
    fn theme() {
        let result = read_preferences("theme = \"light\"");
        assert_eq!(
            &SavedGlobalPreferences {
                theme_preference: ThemePreference::Light,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(Vec::<ParseWarning>::new(), result.warnings);

        let result = read_preferences("theme = \"dark\"");
        assert_eq!(
            &SavedGlobalPreferences {
                theme_preference: ThemePreference::Dark,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(Vec::<ParseWarning>::new(), result.warnings);

        let result = read_preferences("theme = \"default\"");
        assert_eq!(
            &SavedGlobalPreferences {
                theme_preference: ThemePreference::System,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(
            vec![ParseWarning::UnsupportedValue {
                value: "default".to_string(),
                path: "theme".to_string(),
            }],
            result.warnings
        );

        let result = read_preferences("theme = 9");
        assert_eq!(
            &SavedGlobalPreferences {
                theme_preference: ThemePreference::System,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(
            vec![ParseWarning::UnexpectedType {
                expected: "string",
                actual: "integer",
                path: "theme".to_string(),
            }],
            result.warnings
        );
    }

    #[test]
    fn gamemode() {
        let result = read_preferences("gamemode = \"on\"");
        assert_eq!(
            &SavedGlobalPreferences {
                gamemode_preference: GameModePreference::On,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(Vec::<ParseWarning>::new(), result.warnings);

        let result = read_preferences("gamemode = \"off\"");
        assert_eq!(
            &SavedGlobalPreferences {
                gamemode_preference: GameModePreference::Off,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(Vec::<ParseWarning>::new(), result.warnings);

        let result = read_preferences("gamemode = \"default\"");
        assert_eq!(
            &SavedGlobalPreferences {
                gamemode_preference: GameModePreference::Default,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(
            vec![ParseWarning::UnsupportedValue {
                value: "default".to_string(),
                path: "gamemode".to_string(),
            }],
            result.warnings
        );

        let result = read_preferences("gamemode = 1");
        assert_eq!(
            &SavedGlobalPreferences {
                gamemode_preference: GameModePreference::Default,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(
            vec![ParseWarning::UnexpectedType {
                expected: "string",
                actual: "integer",
                path: "gamemode".to_string(),
            }],
            result.warnings
        );
    }

    #[test]
    fn open_url_mode() {
        let result = read_preferences("open_url_mode = \"allow\"");
        assert_eq!(
            &SavedGlobalPreferences {
                open_url_mode: OpenUrlMode::Allow,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(Vec::<ParseWarning>::new(), result.warnings);

        let result = read_preferences("open_url_mode = \"deny\"");
        assert_eq!(
            &SavedGlobalPreferences {
                open_url_mode: OpenUrlMode::Deny,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(Vec::<ParseWarning>::new(), result.warnings);

        let result = read_preferences("open_url_mode = \"confirm\"");
        assert_eq!(
            &SavedGlobalPreferences {
                open_url_mode: OpenUrlMode::Confirm,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(
            vec![ParseWarning::UnsupportedValue {
                value: "confirm".to_string(),
                path: "open_url_mode".to_string(),
            }],
            result.warnings
        );

        let result = read_preferences("open_url_mode = 1");
        assert_eq!(
            &SavedGlobalPreferences {
                open_url_mode: OpenUrlMode::Confirm,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(
            vec![ParseWarning::UnexpectedType {
                expected: "string",
                actual: "integer",
                path: "open_url_mode".to_string(),
            }],
            result.warnings
        );
    }

    #[test]
    fn ime_enabled() {
        let result = read_preferences("ime = {enabled = true}");
        assert_eq!(
            &SavedGlobalPreferences {
                ime_enabled: Some(true),
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(Vec::<ParseWarning>::new(), result.warnings);

        let result = read_preferences("ime.enabled = false");
        assert_eq!(
            &SavedGlobalPreferences {
                ime_enabled: Some(false),
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(Vec::<ParseWarning>::new(), result.warnings);

        let result = read_preferences("[ime]\nenabled = false\n");
        assert_eq!(
            &SavedGlobalPreferences {
                ime_enabled: Some(false),
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(Vec::<ParseWarning>::new(), result.warnings);

        let result = read_preferences("ime.enabled = \"x\"");
        assert_eq!(
            &SavedGlobalPreferences {
                ime_enabled: None,
                ..Default::default()
            },
            result.values()
        );
        assert_eq!(
            vec![ParseWarning::UnexpectedType {
                expected: "boolean",
                actual: "string",
                path: "ime.enabled".to_string(),
            }],
            result.warnings
        );
    }
}
