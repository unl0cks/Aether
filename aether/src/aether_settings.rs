//! The Aether-specific settings the in-game options window can change.
//!
//! These began as command-line flags with no persistence: `aether_preset::apply` collapsed each
//! `--x` / `--no-x` pair into a single `bool` on `Opt` and everything downstream read that. The
//! options window needs somewhere for a chosen setting to live between sessions, and it needs to
//! know whether the command line has already spoken for a setting so it can show that rather than
//! silently disagree with it.
//!
//! So there are three sources, in priority order:
//!
//! 1. An explicit `--x` or `--no-x` on the command line. Captured before the preset runs, because
//!    the preset is what erases the difference between "not mentioned" and "switched off".
//! 2. The saved preference, written by the options window.
//! 3. The built-in default, which is what the preset would have chosen.
//!
//! This is the same order the stock preferences dialog already uses for graphics and storage, and
//! it is why those controls grey out when the matching flag is present.

use crate::cli::Opt;
use egui::{Key, Modifiers};
use ruffle_render::quality::StageQuality;
use std::fmt;

/// The key that opens the options window.
///
/// Stored in `preferences.toml` as the text a player would recognise, `F1` or `Ctrl+O`, rather than
/// as a keycode. Someone editing that file by hand should be able to see what it says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OptionsHotkey {
    pub modifiers: Modifiers,
    pub key: Key,
}

impl Default for OptionsHotkey {
    fn default() -> Self {
        // F1 is the conventional "help or settings" key and AQW itself does not use it, so binding
        // it takes nothing away from the game.
        Self {
            modifiers: Modifiers::NONE,
            key: Key::F1,
        }
    }
}

impl OptionsHotkey {
    /// Read a hotkey back from the preferences file.
    ///
    /// Returns `None` for anything that is not a hotkey, so a hand-edited file with a typo in it
    /// falls back to the default instead of leaving the window with no way to open.
    pub fn parse(text: &str) -> Option<Self> {
        let mut modifiers = Modifiers::NONE;
        let mut parts = text.split('+').map(str::trim).peekable();
        let mut key = None;

        while let Some(part) = parts.next() {
            if part.is_empty() {
                return None;
            }

            // The last part is the key; everything before it has to be a modifier.
            if parts.peek().is_none() {
                key = Key::from_name(part);
                break;
            }

            let modifier = match part.to_ascii_lowercase().as_str() {
                // `COMMAND` rather than `CTRL` so the binding reads as Ctrl on Windows and Linux
                // and as Cmd on macOS, which is what the rest of the menu shortcuts already do.
                "ctrl" => Modifiers::COMMAND,
                "alt" => Modifiers::ALT,
                "shift" => Modifiers::SHIFT,
                _ => return None,
            };

            // A modifier named twice is a malformed binding, not a stronger one.
            if modifiers.contains(modifier) {
                return None;
            }
            modifiers |= modifier;
        }

        Some(Self {
            modifiers,
            key: key?,
        })
    }

    pub fn shortcut(self) -> egui::KeyboardShortcut {
        egui::KeyboardShortcut::new(self.modifiers, self.key)
    }
}

impl fmt::Display for OptionsHotkey {
    /// Written back in the same order [`OptionsHotkey::parse`] expects, so a round trip through the
    /// preferences file is lossless.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (present, name) in [
            (self.modifiers.command || self.modifiers.ctrl, "Ctrl"),
            (self.modifiers.alt, "Alt"),
            (self.modifiers.shift, "Shift"),
        ] {
            if present {
                write!(formatter, "{name}+")?;
            }
        }
        formatter.write_str(self.key.name())
    }
}

/// A setting the options window can change, resolved from all three sources.
///
/// `None` at capture time means the command line did not mention it either way, which is what
/// leaves the saved preference free to apply.
type Explicit = Option<bool>;

/// Which Aether toggles the command line set explicitly.
///
/// Captured from `Opt` before [`crate::aether_preset::apply`] fills in defaults. After that call
/// every paired flag reads as set, so this has to happen first.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AetherExplicitFlags {
    pub number_separators: Explicit,
    pub number_separator_space: Explicit,
    pub separators_from_ten_thousand: Explicit,
    pub timeline_child_rebind: Explicit,
    pub adaptive_avatar_cache: Explicit,
    pub movement_stop_guard: Explicit,
    pub avm2_broadcast_fast_path: Explicit,
    pub mouse_motion_coalescing: Explicit,
    pub bounded_offscreen_pool: Explicit,
    pub low_vram: Explicit,
    pub tooltips_follow_pointer: Explicit,
    pub hide_skill_tooltips: Explicit,
    pub always_show_aura_tooltips: Explicit,
    pub nine_slice: Explicit,
    pub cache_texture_grid: Explicit,
    pub idle_gpu_upload_eviction: Explicit,
    pub crash_report: Explicit,

    /// `--quality`, which is already a value rather than a pair.
    pub quality: Option<StageQuality>,
    /// `--msaa1x` / `--msaa2x` / `--msaa4x`, read as a sample count.
    pub msaa_samples: Option<Option<u8>>,
    /// `--maxfps`.
    pub max_fps: Option<Option<f64>>,
}

/// Read a `--x` / `--no-x` pair as one three-state answer.
///
/// Clap already rejects passing both, so the order of these two tests never decides anything.
fn pair(enabled: bool, disabled: bool) -> Explicit {
    match (enabled, disabled) {
        (true, _) => Some(true),
        (_, true) => Some(false),
        _ => None,
    }
}

impl AetherExplicitFlags {
    /// Capture what the command line asked for.
    ///
    /// Must be called on a freshly parsed `Opt`. Calling it after the preset has run reports every
    /// setting as explicitly set, which would make the options window read-only for everything.
    pub fn capture(opt: &Opt) -> Self {
        Self {
            number_separators: pair(
                opt.aether_aqw_hp_separators,
                opt.no_aether_aqw_hp_separators,
            ),
            // A lone switch, with no negative form to pair it with. Present means asked for.
            number_separator_space: opt.aether_aqw_hp_separator_space.then_some(true),
            separators_from_ten_thousand: opt
                .aether_aqw_separators_from_ten_thousand
                .then_some(true),
            timeline_child_rebind: pair(
                opt.aether_aqw_timeline_child_rebind,
                opt.no_aether_aqw_timeline_child_rebind,
            ),
            adaptive_avatar_cache: pair(
                opt.aether_aqw_adaptive_avatar_cache,
                opt.no_aether_aqw_adaptive_avatar_cache,
            ),
            movement_stop_guard: pair(
                opt.aether_aqw_movement_stop_guard,
                opt.no_aether_aqw_movement_stop_guard,
            ),
            avm2_broadcast_fast_path: pair(
                opt.aether_aqw_avm2_broadcast_fast_path,
                opt.no_aether_aqw_avm2_broadcast_fast_path,
            ),
            mouse_motion_coalescing: pair(
                opt.aether_aqw_mouse_motion_coalescing,
                opt.no_aether_aqw_mouse_motion_coalescing,
            ),
            bounded_offscreen_pool: pair(
                opt.aether_bounded_offscreen_pool,
                opt.no_aether_aqw_bounded_offscreen_pool,
            ),
            low_vram: pair(opt.aether_low_vram, opt.no_aether_low_vram),
            tooltips_follow_pointer: pair(
                opt.aether_tooltips_follow_pointer,
                opt.no_aether_tooltips_follow_pointer,
            ),
            hide_skill_tooltips: pair(
                opt.aether_hide_skill_tooltips,
                opt.no_aether_hide_skill_tooltips,
            ),
            always_show_aura_tooltips: pair(
                opt.aether_always_show_aura_tooltips,
                opt.no_aether_always_show_aura_tooltips,
            ),
            nine_slice: pair(opt.aether_nine_slice, opt.no_aether_nine_slice),
            cache_texture_grid: pair(
                opt.aether_aqw_cache_texture_grid,
                opt.no_aether_aqw_cache_texture_grid,
            ),
            idle_gpu_upload_eviction: pair(
                opt.aether_aqw_idle_gpu_upload_eviction,
                opt.no_aether_aqw_idle_gpu_upload_eviction,
            ),
            crash_report: pair(opt.aether_crash_report, opt.no_aether_crash_report),
            quality: opt.quality,
            // Three switches for one setting, and no switch at all means the saved value decides.
            msaa_samples: match (opt.msaa1x, opt.msaa2x, opt.msaa4x) {
                (true, _, _) => Some(Some(1)),
                (_, true, _) => Some(Some(2)),
                (_, _, true) => Some(Some(4)),
                _ => None,
            },
            max_fps: opt.frame_rate.map(Some),
        }
    }
}

/// The saved half of the settings, as persisted to `preferences.toml`.
///
/// Every default here has to match what `aether_preset::apply` chooses, so that a fresh install
/// with no saved preferences behaves exactly as it did before the options window existed. The
/// tests below hold the two in step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AetherSettings {
    pub number_separators: bool,
    pub number_separator_space: bool,
    pub separators_from_ten_thousand: bool,
    pub timeline_child_rebind: bool,
    pub adaptive_avatar_cache: bool,
    pub movement_stop_guard: bool,
    pub avm2_broadcast_fast_path: bool,
    pub mouse_motion_coalescing: bool,
    pub bounded_offscreen_pool: bool,
    pub low_vram: bool,
    pub tooltips_follow_pointer: bool,
    pub hide_skill_tooltips: bool,
    pub always_show_aura_tooltips: bool,
    pub nine_slice: bool,
    pub cache_texture_grid: bool,
    pub idle_gpu_upload_eviction: bool,
    pub crash_report: bool,

    pub quality: StageQuality,

    /// Backend multisampling, independent of Flash's own StageQuality. `None` leaves the renderer
    /// to pick, which is what it did before this was settable.
    pub msaa_samples: Option<u8>,

    /// `None` follows the movie's own frame rate.
    pub max_fps: Option<f64>,
}

impl Default for AetherSettings {
    fn default() -> Self {
        Self {
            number_separators: true,
            number_separator_space: false,
            separators_from_ten_thousand: false,
            timeline_child_rebind: true,
            adaptive_avatar_cache: true,
            movement_stop_guard: true,
            avm2_broadcast_fast_path: true,
            mouse_motion_coalescing: true,
            bounded_offscreen_pool: true,
            // Off by default: it trades reuse for a smaller footprint, which only pays on a
            // card that cannot hold the full budget in the first place.
            low_vram: false,
            // Off by default: it overrules where the game puts its own tooltips.
            tooltips_follow_pointer: false,
            hide_skill_tooltips: false,
            // On: this is the tooltip that says what is on you, and turning it off is a
            // deliberate choice rather than a default.
            always_show_aura_tooltips: true,
            // Off: it fixes stretched corners and has also moved content that was correct.
            nine_slice: false,
            cache_texture_grid: true,
            idle_gpu_upload_eviction: true,
            crash_report: true,
            // These three mirror what the AQW preset picks, same as the toggles above.
            quality: StageQuality::High,
            msaa_samples: None,
            max_fps: Some(60.0),
        }
    }
}

/// A setting as it applies right now, and where that answer came from.
///
/// The window shows the value either way; the source decides whether the control is editable. A
/// setting the command line pinned is shown greyed rather than hidden, so a flag that is quietly
/// overriding a saved choice is visible instead of confusing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Resolved<T> {
    pub value: T,
    pub from_command_line: bool,
}

/// The common case: a toggle.
pub type ResolvedSetting = Resolved<bool>;

impl<T> Resolved<T> {
    fn resolve(explicit: Option<T>, saved: T) -> Self {
        match explicit {
            Some(value) => Self {
                value,
                from_command_line: true,
            },
            None => Self {
                value: saved,
                from_command_line: false,
            },
        }
    }

    /// Whether the options window may edit this one.
    pub fn is_editable(&self) -> bool {
        !self.from_command_line
    }
}

/// Every Aether setting as it applies to this session.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedAetherSettings {
    pub number_separators: ResolvedSetting,
    pub number_separator_space: ResolvedSetting,
    pub separators_from_ten_thousand: ResolvedSetting,
    pub timeline_child_rebind: ResolvedSetting,
    pub adaptive_avatar_cache: ResolvedSetting,
    pub movement_stop_guard: ResolvedSetting,
    pub avm2_broadcast_fast_path: ResolvedSetting,
    pub mouse_motion_coalescing: ResolvedSetting,
    pub bounded_offscreen_pool: ResolvedSetting,
    pub low_vram: ResolvedSetting,
    pub tooltips_follow_pointer: ResolvedSetting,
    pub hide_skill_tooltips: ResolvedSetting,
    pub always_show_aura_tooltips: ResolvedSetting,
    pub nine_slice: ResolvedSetting,
    pub cache_texture_grid: ResolvedSetting,
    pub idle_gpu_upload_eviction: ResolvedSetting,
    pub crash_report: ResolvedSetting,
    pub quality: Resolved<StageQuality>,
    pub msaa_samples: Resolved<Option<u8>>,
    pub max_fps: Resolved<Option<f64>>,
}

impl ResolvedAetherSettings {
    pub fn resolve(explicit: AetherExplicitFlags, saved: AetherSettings) -> Self {
        Self {
            number_separators: ResolvedSetting::resolve(
                explicit.number_separators,
                saved.number_separators,
            ),
            number_separator_space: ResolvedSetting::resolve(
                explicit.number_separator_space,
                saved.number_separator_space,
            ),
            separators_from_ten_thousand: ResolvedSetting::resolve(
                explicit.separators_from_ten_thousand,
                saved.separators_from_ten_thousand,
            ),
            timeline_child_rebind: ResolvedSetting::resolve(
                explicit.timeline_child_rebind,
                saved.timeline_child_rebind,
            ),
            adaptive_avatar_cache: ResolvedSetting::resolve(
                explicit.adaptive_avatar_cache,
                saved.adaptive_avatar_cache,
            ),
            movement_stop_guard: ResolvedSetting::resolve(
                explicit.movement_stop_guard,
                saved.movement_stop_guard,
            ),
            avm2_broadcast_fast_path: ResolvedSetting::resolve(
                explicit.avm2_broadcast_fast_path,
                saved.avm2_broadcast_fast_path,
            ),
            mouse_motion_coalescing: ResolvedSetting::resolve(
                explicit.mouse_motion_coalescing,
                saved.mouse_motion_coalescing,
            ),
            bounded_offscreen_pool: ResolvedSetting::resolve(
                explicit.bounded_offscreen_pool,
                saved.bounded_offscreen_pool,
            ),
            low_vram: ResolvedSetting::resolve(explicit.low_vram, saved.low_vram),
            tooltips_follow_pointer: ResolvedSetting::resolve(
                explicit.tooltips_follow_pointer,
                saved.tooltips_follow_pointer,
            ),
            hide_skill_tooltips: ResolvedSetting::resolve(
                explicit.hide_skill_tooltips,
                saved.hide_skill_tooltips,
            ),
            always_show_aura_tooltips: ResolvedSetting::resolve(
                explicit.always_show_aura_tooltips,
                saved.always_show_aura_tooltips,
            ),
            nine_slice: ResolvedSetting::resolve(explicit.nine_slice, saved.nine_slice),
            cache_texture_grid: ResolvedSetting::resolve(
                explicit.cache_texture_grid,
                saved.cache_texture_grid,
            ),
            idle_gpu_upload_eviction: ResolvedSetting::resolve(
                explicit.idle_gpu_upload_eviction,
                saved.idle_gpu_upload_eviction,
            ),
            crash_report: ResolvedSetting::resolve(explicit.crash_report, saved.crash_report),
            quality: Resolved::resolve(explicit.quality, saved.quality),
            msaa_samples: Resolved::resolve(explicit.msaa_samples, saved.msaa_samples),
            max_fps: Resolved::resolve(explicit.max_fps, saved.max_fps),
        }
    }
}

/// Settings that take effect the moment they change, with no restart.
///
/// The separator settings are read fresh on every text assignment the game makes, so writing the
/// new value into the atomics is all it takes for the next frame to draw with it. Everything else
/// on the Advanced tab is read once while the renderer or the player is being built, which is why
/// the window has to say "restart" for those instead of quietly doing nothing.
///
/// `aqw_mode` is passed rather than assumed: in generic Ruffle mode the AQW preset never runs, and
/// a saved preference must not switch AQW behaviour on for an unrelated movie.
pub fn apply_live_settings(
    settings: &ResolvedAetherSettings,
    aqw_mode: bool,
    player: Option<&mut ruffle_core::Player>,
) {
    use ruffle_core::aether_compatibility::{
        NumberSeparator, set_number_separator, set_separator_min_digits,
    };

    let separator = match (
        aqw_mode && settings.number_separators.value,
        settings.number_separator_space.value,
    ) {
        (false, _) => NumberSeparator::None,
        (true, true) => NumberSeparator::Space,
        (true, false) => NumberSeparator::Comma,
    };
    set_number_separator(separator);

    // Four groups from a thousand, five from ten thousand.
    set_separator_min_digits(if settings.separators_from_ten_thousand.value {
        5
    } else {
        4
    });

    // Nothing is cached from this, so it takes effect on the next tooltip rather than the next
    // launch.
    ruffle_core::aether_compatibility::set_tooltip_follows_pointer_enabled(
        aqw_mode && settings.tooltips_follow_pointer.value,
    );
    ruffle_core::aether_compatibility::set_skill_tooltips_hidden(
        aqw_mode && settings.hide_skill_tooltips.value,
    );
    ruffle_core::aether_compatibility::set_aura_tooltips_always_shown(
        aqw_mode && settings.always_show_aura_tooltips.value,
    );
    ruffle_core::aether_performance::set_nine_slice_scaling_enabled(settings.nine_slice.value);

    // Quality is a property of the running stage, so it can change without a restart. MSAA and the
    // frame rate are read while the renderer and the player are built, and cannot.
    if let Some(player) = player {
        player.set_quality(settings.quality.value);
    }
}

#[cfg(test)]
mod tests {
    use super::{AetherExplicitFlags, AetherSettings, ResolvedAetherSettings, StageQuality};
    use crate::cli::Opt;
    use clap::Parser;

    fn capture(args: &[&str]) -> AetherExplicitFlags {
        AetherExplicitFlags::capture(&Opt::parse_from(args))
    }

    #[test]
    fn a_bare_command_line_leaves_every_setting_free() {
        assert_eq!(capture(&["aether"]), AetherExplicitFlags::default());
    }

    #[test]
    fn a_flag_pins_only_the_setting_it_names() {
        let captured = capture(&["aether", "--no-number-separators"]);
        assert_eq!(captured.number_separators, Some(false));
        assert_eq!(captured.cache_texture_grid, None);

        let captured = capture(&["aether", "--number-separators"]);
        assert_eq!(captured.number_separators, Some(true));
    }

    #[test]
    fn a_pinned_setting_wins_over_the_saved_one_and_cannot_be_edited() {
        let saved = AetherSettings {
            number_separators: true,
            ..Default::default()
        };
        let resolved =
            ResolvedAetherSettings::resolve(capture(&["aether", "--no-number-separators"]), saved);

        assert!(!resolved.number_separators.value);
        assert!(!resolved.number_separators.is_editable());
    }

    #[test]
    fn an_unpinned_setting_takes_the_saved_value_and_stays_editable() {
        let saved = AetherSettings {
            number_separators: false,
            ..Default::default()
        };
        let resolved = ResolvedAetherSettings::resolve(capture(&["aether"]), saved);

        assert!(!resolved.number_separators.value);
        assert!(resolved.number_separators.is_editable());
    }

    /// The window has to open on the same settings the game already had, so a fresh install with
    /// nothing saved must resolve to exactly what the preset chooses.
    #[test]
    fn the_saved_defaults_match_what_the_preset_would_pick() {
        let mut opt = Opt::parse_from(["aether"]);
        crate::aether_preset::apply(&mut opt).expect("AQW preset applies");

        let defaults = AetherSettings::default();
        assert_eq!(opt.aether_aqw_hp_separators, defaults.number_separators);
        assert_eq!(
            opt.aether_aqw_hp_separator_space,
            defaults.number_separator_space
        );
        assert_eq!(
            opt.aether_aqw_separators_from_ten_thousand,
            defaults.separators_from_ten_thousand
        );
        assert_eq!(
            opt.aether_aqw_timeline_child_rebind,
            defaults.timeline_child_rebind
        );
        assert_eq!(
            opt.aether_aqw_adaptive_avatar_cache,
            defaults.adaptive_avatar_cache
        );
        assert_eq!(
            opt.aether_aqw_movement_stop_guard,
            defaults.movement_stop_guard
        );
        assert_eq!(
            opt.aether_aqw_avm2_broadcast_fast_path,
            defaults.avm2_broadcast_fast_path
        );
        assert_eq!(
            opt.aether_aqw_mouse_motion_coalescing,
            defaults.mouse_motion_coalescing
        );
        assert_eq!(
            opt.aether_bounded_offscreen_pool,
            defaults.bounded_offscreen_pool
        );
        assert_eq!(
            opt.aether_aqw_cache_texture_grid,
            defaults.cache_texture_grid
        );
        assert_eq!(
            opt.aether_aqw_idle_gpu_upload_eviction,
            defaults.idle_gpu_upload_eviction
        );
        assert_eq!(opt.aether_crash_report, defaults.crash_report);
    }

    #[test]
    fn the_msaa_switches_read_as_one_sample_count() {
        assert_eq!(capture(&["aether"]).msaa_samples, None);
        assert_eq!(capture(&["aether", "--msaa1x"]).msaa_samples, Some(Some(1)));
        assert_eq!(capture(&["aether", "--msaa2x"]).msaa_samples, Some(Some(2)));
        assert_eq!(capture(&["aether", "--msaa4x"]).msaa_samples, Some(Some(4)));
    }

    #[test]
    fn the_msaa_switches_still_refuse_to_be_combined() {
        for pair in [
            ["--msaa1x", "--msaa2x"],
            ["--msaa1x", "--msaa4x"],
            ["--msaa2x", "--msaa4x"],
        ] {
            assert!(
                Opt::try_parse_from(["aether", pair[0], pair[1]]).is_err(),
                "{pair:?}"
            );
        }
    }

    #[test]
    fn quality_and_frame_rate_are_pinned_by_their_own_flags() {
        let captured = capture(&["aether", "--quality", "low", "--maxfps", "144"]);
        assert_eq!(captured.quality, Some(StageQuality::Low));
        assert_eq!(captured.max_fps, Some(Some(144.0)));

        let resolved = ResolvedAetherSettings::resolve(captured, AetherSettings::default());
        assert_eq!(resolved.quality.value, StageQuality::Low);
        assert!(!resolved.quality.is_editable());
        assert_eq!(resolved.max_fps.value, Some(144.0));
        assert!(!resolved.max_fps.is_editable());
    }

    /// The display settings default to what the preset picks, the same as the toggles.
    #[test]
    fn the_saved_display_defaults_match_the_preset() {
        let mut opt = Opt::parse_from(["aether"]);
        crate::aether_preset::apply(&mut opt).expect("AQW preset applies");

        let defaults = AetherSettings::default();
        assert_eq!(opt.quality, Some(defaults.quality));
        assert_eq!(opt.frame_rate, defaults.max_fps);
        assert!(!opt.msaa1x && !opt.msaa2x && !opt.msaa4x);
        assert_eq!(defaults.msaa_samples, None);
    }

    /// Capturing after the preset has run reports everything as pinned, which would leave the
    /// options window unable to change anything. This is the ordering bug, held down by a test.
    #[test]
    fn capturing_after_the_preset_would_pin_everything() {
        let mut opt = Opt::parse_from(["aether"]);
        crate::aether_preset::apply(&mut opt).expect("AQW preset applies");

        assert_ne!(
            AetherExplicitFlags::capture(&opt),
            AetherExplicitFlags::default(),
            "capture has to happen before the preset fills in defaults"
        );
    }
}

#[cfg(test)]
mod hotkey_tests {
    use super::OptionsHotkey;
    use egui::{Key, Modifiers};

    #[test]
    fn the_default_is_f1() {
        assert_eq!(OptionsHotkey::default().key, Key::F1);
        assert_eq!(OptionsHotkey::default().modifiers, Modifiers::NONE);
        assert_eq!(OptionsHotkey::default().to_string(), "F1");
    }

    #[test]
    fn a_hotkey_survives_a_trip_through_the_preferences_file() {
        for written in ["F1", "F9", "Ctrl+O", "Alt+Shift+P", "Ctrl+Alt+Shift+Delete"] {
            let parsed = OptionsHotkey::parse(written).expect(written);
            assert_eq!(parsed.to_string(), written, "{written}");
        }
    }

    #[test]
    fn parsing_is_forgiving_about_spacing_and_case() {
        // `COMMAND`, not `CTRL`, so the binding follows the platform the way the menu shortcuts do.
        let expected = OptionsHotkey {
            modifiers: Modifiers::COMMAND,
            key: Key::O,
        };
        for written in ["Ctrl+O", "ctrl+o", "CTRL + O", " ctrl+O "] {
            assert_eq!(OptionsHotkey::parse(written), Some(expected), "{written}");
        }
    }

    /// A preferences file edited by hand can say anything. Anything it says that is not a hotkey
    /// has to leave the default in place rather than take the window's shortcut away.
    #[test]
    fn nonsense_is_rejected_rather_than_guessed_at() {
        for written in ["", "Ctrl", "Ctrl+", "+F1", "Meta+F1", "F99", "Ctrl+Ctrl+F1"] {
            assert_eq!(OptionsHotkey::parse(written), None, "{written}");
        }
    }
}
