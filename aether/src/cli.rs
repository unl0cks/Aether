use crate::AETHER_VERSION;
use crate::preferences::storage::StorageBackend;
use anyhow::{Error, anyhow};
use clap::{Parser, ValueEnum};
use ruffle_core::backend::navigator::SocketMode;
use ruffle_core::config::Letterbox;
use ruffle_core::events::{GamepadButton, KeyCode};
use ruffle_core::{LoadBehavior, PlayerRuntime, StageAlign, StageScaleMode};
use ruffle_render::quality::StageQuality;
use ruffle_render_wgpu::clap::{GraphicsBackend, PowerPreference};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;
use url::Url;

fn get_default_save_directory() -> std::path::PathBuf {
    dirs::data_local_dir()
        .expect("Couldn't find a valid data_local dir")
        .join("aether")
        .join("SharedObjects")
}

fn get_default_config_directory() -> std::path::PathBuf {
    dirs::config_local_dir()
        .expect("Couldn't find a valid config_local dir")
        .join("aether")
}

fn get_default_cache_directory() -> std::path::PathBuf {
    dirs::cache_dir()
        .expect("Couldn't find a valid cache dir")
        .join("aether")
}

#[derive(Parser, Debug, Clone)]
#[clap(
    name = "Aether",
    author,
    version = AETHER_VERSION,
)]
pub struct Opt {
    /// Path or URL of a Flash movie (SWF) to play.
    #[clap(name = "FILE", value_parser(parse_movie_file_or_url))]
    pub movie_url: Option<Url>,

    /// Start as a generic Ruffle player instead of loading AQW automatically.
    #[clap(long)]
    pub generic: bool,

    /// Keep Ruffle's menu bar visible in AQW mode.
    #[clap(long)]
    pub show_menu: bool,

    /// Write one-second frame-time and render-counter summaries as JSON Lines.
    #[clap(long = "metrics", alias = "aether-metrics")]
    pub aether_metrics: bool,

    /// Override the metrics output path. The default is inside Aether's cache directory.
    #[clap(long = "metrics-file", alias = "aether-metrics-file")]
    pub aether_metrics_file: Option<std::path::PathBuf>,

    /// Record diagnostic host/core mouse events as JSON Lines. Requires a metrics build.
    #[clap(long = "input-trace", alias = "aether-input-trace")]
    pub aether_input_trace: bool,

    /// Override the input trace output path. The default is inside Aether's cache directory.
    #[clap(long = "input-trace-file", alias = "aether-input-trace-file")]
    pub aether_input_trace_file: Option<std::path::PathBuf>,

    /// Record targeted AVM2 goto/frame-construction diagnostics as JSON Lines. Requires a metrics build.
    #[clap(long = "timeline-trace", alias = "aether-timeline-trace")]
    pub aether_timeline_trace: bool,

    /// Override the timeline trace output path. The default is inside Aether's cache directory.
    #[clap(long = "timeline-trace-file", alias = "aether-timeline-trace-file")]
    pub aether_timeline_trace_file: Option<std::path::PathBuf>,

    /// Experimentally retry construction of missing AQW timeline children before a frame script runs.
    #[clap(
        long = "frame-construction-retry",
        alias = "aether-aqw-frame-construction-retry"
    )]
    pub aether_aqw_frame_construction_retry: bool,

    /// Enable AQW's targeted Settings timeline-child compatibility repair.
    #[clap(
        long = "timeline-child-rebind",
        alias = "aether-aqw-timeline-child-rebind",
        conflicts_with = "no_aether_aqw_timeline_child_rebind"
    )]
    pub aether_aqw_timeline_child_rebind: bool,

    /// Disable AQW's targeted Settings timeline-child compatibility repair.
    #[clap(
        long = "no-timeline-child-rebind",
        alias = "no-aether-aqw-timeline-child-rebind",
        conflicts_with = "aether_aqw_timeline_child_rebind"
    )]
    pub no_aether_aqw_timeline_child_rebind: bool,

    /// Disable internal bitmap reuse for visually stable AQW AvatarMC roots.
    #[clap(
        long = "no-adaptive-avatar-cache",
        alias = "no-aether-aqw-adaptive-avatar-cache",
        conflicts_with = "aether_aqw_adaptive_avatar_cache"
    )]
    pub no_aether_aqw_adaptive_avatar_cache: bool,

    /// Enable internal bitmap reuse for visually stable AQW AvatarMC roots.
    #[clap(
        long = "adaptive-avatar-cache",
        alias = "aether-aqw-adaptive-avatar-cache",
        conflicts_with = "no_aether_aqw_adaptive_avatar_cache"
    )]
    pub aether_aqw_adaptive_avatar_cache: bool,

    /// Disable eviction of GPU uploads for bitmaps that are no longer being drawn.
    #[clap(
        long = "no-idle-gpu-upload-eviction",
        alias = "no-aether-aqw-idle-gpu-upload-eviction"
    )]
    pub no_aether_aqw_idle_gpu_upload_eviction: bool,

    /// Resolved launch-mode setting; populated by the AQW preset.
    #[clap(skip)]
    pub aether_aqw_idle_gpu_upload_eviction: bool,

    /// Disable stale live-entry pruning in AQW's AVM2 broadcast registry.
    #[clap(
        long = "no-avm2-broadcast-fast-path",
        alias = "no-aether-aqw-avm2-broadcast-fast-path"
    )]
    pub no_aether_aqw_avm2_broadcast_fast_path: bool,

    /// Resolved launch-mode setting; populated by the AQW preset.
    #[clap(skip)]
    pub aether_aqw_avm2_broadcast_fast_path: bool,

    /// Enable protection against false premature AvatarMC stopWalking calls during AQW movement.
    #[clap(
        long = "movement-stop-guard",
        alias = "aether-aqw-movement-stop-guard",
        conflicts_with = "no_aether_aqw_movement_stop_guard"
    )]
    pub aether_aqw_movement_stop_guard: bool,

    /// Disable protection against false premature AvatarMC stopWalking calls during AQW movement.
    #[clap(
        long = "no-movement-stop-guard",
        alias = "no-aether-aqw-movement-stop-guard",
        conflicts_with = "aether_aqw_movement_stop_guard"
    )]
    pub no_aether_aqw_movement_stop_guard: bool,

    /// Keep every host mouse position instead of coalescing movement to rendered frames.
    #[clap(
        long = "no-mouse-motion-coalescing",
        aliases = [
            "no-aether-aqw-mouse-motion-coalescing",
            "aether-no-mouse-motion-coalescing"
        ],
        conflicts_with = "aether_aqw_mouse_motion_coalescing"
    )]
    pub no_aether_aqw_mouse_motion_coalescing: bool,

    /// Coalesce continuous AQW mouse positions to rendered frames.
    #[clap(
        long = "mouse-motion-coalescing",
        alias = "aether-aqw-mouse-motion-coalescing",
        conflicts_with = "no_aether_aqw_mouse_motion_coalescing"
    )]
    pub aether_aqw_mouse_motion_coalescing: bool,

    /// Experimentally enable bounded cross-frame reuse of offscreen textures in AQW mode.
    #[clap(
        long = "bounded-offscreen-pool",
        alias = "aether-aqw-bounded-offscreen-pool",
        conflicts_with = "no_aether_aqw_bounded_offscreen_pool"
    )]
    pub aether_aqw_bounded_offscreen_pool: bool,

    /// Disable bounded cross-frame reuse of offscreen textures in AQW mode.
    #[clap(
        long = "no-bounded-offscreen-pool",
        alias = "no-aether-aqw-bounded-offscreen-pool",
        conflicts_with = "aether_aqw_bounded_offscreen_pool"
    )]
    pub no_aether_aqw_bounded_offscreen_pool: bool,

    /// Resolved launch-mode setting; populated by the AQW preset.
    #[clap(skip)]
    pub aether_bounded_offscreen_pool: bool,

    /// Round cache texture sizes up to a grid so animating objects stop asking for a new size
    /// every frame. Cuts texture creation by roughly 32x.
    #[clap(
        long = "cache-texture-grid",
        alias = "aether-aqw-cache-texture-grid",
        conflicts_with = "no_aether_aqw_cache_texture_grid"
    )]
    pub aether_aqw_cache_texture_grid: bool,

    /// Allocate cache textures at their exact size, as Ruffle does by default.
    #[clap(
        long = "no-cache-texture-grid",
        alias = "no-aether-aqw-cache-texture-grid",
        conflicts_with = "aether_aqw_cache_texture_grid"
    )]
    pub no_aether_aqw_cache_texture_grid: bool,

    /// Group AQW health numbers with thousands separators, as 1,250,000. On by default.
    #[clap(long = "hp-separators", conflicts_with = "no_aether_aqw_hp_separators")]
    pub aether_aqw_hp_separators: bool,

    /// Show AQW health numbers exactly as the game writes them, with no separators.
    #[clap(long = "no-hp-separators", conflicts_with = "aether_aqw_hp_separators")]
    pub no_aether_aqw_hp_separators: bool,

    /// Separate AQW health numbers with spaces rather than commas, as 1 250 000.
    #[clap(long = "hp-separator-space")]
    pub aether_aqw_hp_separator_space: bool,

    /// Enable raw ActionScript trace output. This may contain account or session data.
    #[clap(long = "avm-trace", alias = "aether-avm-trace")]
    pub aether_avm_trace: bool,

    /// Write a self-contained crash report when Aether stops on a fatal error. Covers a lost
    /// graphics device as well as a panic; the former exits gracefully and is otherwise invisible
    /// to crash handlers.
    #[clap(
        long = "crash-report",
        alias = "aether-crash-report",
        conflicts_with = "no_aether_crash_report"
    )]
    pub aether_crash_report: bool,

    /// Disable self-contained crash reports.
    #[clap(long = "no-crash-report", conflicts_with = "aether_crash_report")]
    pub no_aether_crash_report: bool,

    /// Directory to write crash reports into. Defaults to Aether's log directory.
    #[clap(
        long = "crash-report-dir",
        alias = "aether-crash-report-dir",
        conflicts_with = "no_aether_crash_report"
    )]
    pub aether_crash_report_dir: Option<PathBuf>,

    /// A "flashvars" parameter to provide to the movie.
    /// This can be repeated multiple times, for example -Pkey=value -Pfoo=bar.
    #[clap(short = 'P', action = clap::ArgAction::Append)]
    parameters: Vec<String>,

    /// Type of graphics backend to use. Not all options may be supported by your current system.
    ///
    /// Default will attempt to pick the most supported graphics backend.
    /// This option temporarily overrides any stored preference.
    #[clap(long, short)]
    pub graphics: Option<GraphicsBackend>,

    /// Power preference for the graphics device used. High power usage tends to prefer dedicated GPUs,
    /// whereas a low power usage tends prefer integrated GPUs.
    ///
    /// Default preference is high (likely dedicated GPU).
    /// This option temporarily overrides any stored preference.
    #[clap(long, short)]
    pub power: Option<PowerPreference>,

    /// GameMode preference.
    ///
    /// This allows enabling or disabling GameMode manually.
    /// When enabled, GameMode will be requested only when a movie is loaded.
    ///
    /// The default preference enables GameMode when power preference is set to high.
    /// This option temporarily overrides any stored preference.
    ///
    /// See <https://github.com/FeralInteractive/gamemode>.
    #[clap(long)]
    #[cfg_attr(not(target_os = "linux"), clap(hide = true))]
    pub gamemode: Option<GameModePreference>,

    /// Type of storage backend to use. This determines where local storage data is saved (e.g. shared objects).
    ///
    /// This option temporarily overrides any stored preference.
    #[clap(long)]
    pub storage: Option<StorageBackend>,

    /// Width of window in pixels.
    #[clap(long, display_order = 1)]
    pub width: Option<f64>,

    /// Height of window in pixels.
    #[clap(long, display_order = 2)]
    pub height: Option<f64>,

    /// Maximum number of seconds a script can run before scripting is disabled.
    #[clap(long, short, value_parser(parse_duration_seconds))]
    pub max_execution_duration: Option<Duration>,

    /// Base directory or URL used to resolve all relative path statements in the SWF file.
    /// The default is the current directory.
    #[clap(long)]
    pub base: Option<Url>,

    /// Default quality of the movie.
    #[clap(long, short)]
    pub quality: Option<StageQuality>,

    /// Force 2x backend multisample antialiasing without changing Flash StageQuality.
    #[clap(long, conflicts_with = "msaa4x")]
    pub msaa2x: bool,

    /// Force 4x backend multisample antialiasing without changing Flash StageQuality.
    #[clap(long, conflicts_with = "msaa2x")]
    pub msaa4x: bool,

    /// The alignment of the stage.
    #[clap(long, short, value_parser(parse_align))]
    pub align: Option<StageAlign>,

    /// Prevent movies from changing the stage alignment.
    #[clap(long, action)]
    pub force_align: bool,

    /// The scale mode of the stage.
    #[clap(long, short)]
    pub scale: Option<StageScaleMode>,

    /// Audio volume as a number between 0 (muted) and 1 (full volume). Default is 1.
    #[clap(long, short)]
    pub volume: Option<f32>,

    /// Prevent movies from changing the stage scale mode.
    #[clap(long, action)]
    pub force_scale: bool,

    /// Location to store save data for games.
    ///
    /// This option has no effect if `storage` is not `disk`.
    #[clap(long, default_value_os_t=get_default_save_directory())]
    pub save_directory: std::path::PathBuf,

    /// Location of a directory to store Ruffle configuration.
    #[clap(long, default_value_os_t=get_default_config_directory())]
    pub config: std::path::PathBuf,

    /// Directory that contains non-essential files created by Ruffle.
    ///
    /// This directory can be deleted without affecting functionality.
    #[clap(long, default_value_os_t=get_default_cache_directory())]
    pub cache_directory: std::path::PathBuf,

    /// Proxy to use when loading movies via URL.
    #[clap(long)]
    pub proxy: Option<Url>,

    /// Add an endpoint (`[host]:[port]`) to the socket whitelist.
    #[clap(long = "socket-allow", number_of_values = 1, action = clap::ArgAction::Append)]
    pub socket_allow: Vec<String>,

    /// Define how to deal with TCP Socket connections.
    #[clap(long = "tcp-connections")]
    pub tcp_connections: Option<SocketMode>,

    /// Replace all embedded HTTP URLs with HTTPS.
    #[clap(long, action)]
    pub upgrade_to_https: bool,

    /// Start application in fullscreen.
    #[clap(long, action)]
    pub fullscreen: bool,

    #[clap(long)]
    pub load_behavior: Option<LoadBehavior>,

    /// Specify how Ruffle should handle areas outside the movie stage.
    #[clap(long)]
    pub letterbox: Option<Letterbox>,

    /// Spoofs the root SWF URL provided to ActionScript.
    #[clap(long, value_parser)]
    pub spoof_url: Option<Url>,

    /// Spoofs the HTTP referer header.
    #[clap(long, value_parser)]
    pub referer: Option<Url>,

    /// Spoofs the HTTP cookie header.
    /// This is a string of the form "name1=value1; name2=value2".
    #[clap(long)]
    pub cookie: Option<String>,

    /// The version of the player to emulate
    #[clap(long)]
    pub player_version: Option<u8>,

    /// The runtime to emulate (Flash Player or Adobe AIR)
    #[clap(long)]
    pub player_runtime: Option<PlayerRuntime>,

    /// Set and lock the player's frame rate, overriding the movie's frame rate.
    #[clap(long = "maxfps", alias = "frame-rate")]
    pub frame_rate: Option<f64>,

    /// The handling mode of links opening a new website.
    #[clap(long)]
    pub open_url_mode: Option<OpenUrlMode>,

    /// How to handle non-interactive filesystem access.
    #[clap(long, default_value = "ask")]
    pub filesystem_access_mode: FilesystemAccessMode,

    /// Provide a dummy (completely empty) External Interface to the movie.
    /// This may break some movies that expect an External Interface to be functional,
    /// but may fix others that always require an External Interface.
    #[clap(long)]
    pub dummy_external_interface: bool,

    /// Hides the menu bar (the bar at the top of the window).
    #[clap(long)]
    pub no_gui: bool,

    /// Remaps a specific button on a gamepad to a keyboard key.
    /// This can be used to add new gamepad support to existing games, for example mapping
    /// the D-pad to the arrow keys with -B d-pad-up=up -B d-pad-down=down etc.
    ///
    /// A case-insensitive list of supported gamepad-buttons is:
    /// - north, east, south, west
    /// - d-pad-up, d-pad-down, d-pad-left, d-pad-right
    /// - left-trigger, left-trigger2
    /// - right-trigger, right-trigger2
    /// - select, start
    ///
    /// A case-insensitive (non-exhaustive) list of common key-names is:
    /// - a, b, c, ..., z
    /// - up, down, left, right
    /// - return
    /// - space
    /// - comma, semicolon
    /// - key0, key1, ..., key9
    ///
    /// The complete list of supported key-names can be found by using -B start=nonexistent.
    #[clap(
        long,
        short = 'B',
        value_parser(parse_gamepad_button),
        verbatim_doc_comment,
        value_name = "GAMEPAD BUTTON>=<KEY NAME"
    )]
    pub gamepad_button: Vec<(GamepadButton, KeyCode)>,

    /// Disable AVM2 optimizer.
    /// Note that some early opcode conversions
    /// (like inlining constant pool entries) can't be disabled.
    #[clap(long)]
    pub no_avm2_optimizer: bool,
}

fn parse_movie_file_or_url(path: &str) -> Result<Url, Error> {
    crate::util::parse_url(Path::new(path))
}

fn parse_duration_seconds(value: &str) -> Result<Duration, Error> {
    Ok(Duration::from_secs_f64(value.parse()?))
}

fn parse_align(value: &str) -> Result<StageAlign, Error> {
    value
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid stage alignment"))
}

fn parse_gamepad_button(mapping: &str) -> Result<(GamepadButton, KeyCode), Error> {
    let pos = mapping.find('=').ok_or_else(|| {
        anyhow!("invalid <gamepad button>=<key name>: no `=` found in `{mapping}`")
    })?;

    fn to_aliases<T: ValueEnum>(variants: &[T]) -> String {
        let aliases: Vec<String> = variants
            .iter()
            .map(|variant| {
                variant
                    .to_possible_value()
                    .expect("Must have a PossibleValue")
                    .get_name_and_aliases()
                    .next()
                    .expect("Must have one alias")
                    .to_owned()
            })
            .collect();
        aliases.join(", ")
    }

    let button = <GamepadButton as ValueEnum>::from_str(&mapping[..pos], true).map_err(|err| {
        anyhow!(
            "Could not parse <gamepad button>: {err}\n  The possible values are: {}",
            to_aliases(GamepadButton::value_variants())
        )
    })?;
    let key_code = NamedKeyCode::from_str(&mapping[pos + 1..], true).map_err(|err| {
        anyhow!(
            "Could not parse <key name>: {err}\n  The possible values are: {}",
            to_aliases(NamedKeyCode::value_variants())
        )
    })?;
    Ok((button, KeyCode::from_code(key_code as u32)))
}

impl Opt {
    pub(crate) fn push_parameter(&mut self, key: &str, value: &str) {
        self.parameters.push(format!("{key}={value}"));
    }

    pub fn parameters(&self) -> impl '_ + Iterator<Item = (String, String)> {
        self.parameters.iter().map(|parameter| {
            let mut split = parameter.splitn(2, '=');
            if let (Some(key), Some(value)) = (split.next(), split.next()) {
                (key.to_owned(), value.to_owned())
            } else {
                (parameter.clone(), "".to_string())
            }
        })
    }
}

#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, clap::ValueEnum)]
pub enum GameModePreference {
    #[default]
    Default,
    On,
    Off,
}

impl GameModePreference {
    pub fn as_str(self) -> Option<&'static str> {
        match self {
            GameModePreference::Default => None,
            GameModePreference::On => Some("on"),
            GameModePreference::Off => Some("off"),
        }
    }
}

impl FromStr for GameModePreference {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "on" => Ok(GameModePreference::On),
            "off" => Ok(GameModePreference::Off),
            _ => Err(()),
        }
    }
}

#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, clap::ValueEnum)]
pub enum OpenUrlMode {
    #[default]
    Confirm,
    Allow,
    Deny,
}

impl OpenUrlMode {
    pub fn as_str(self) -> Option<&'static str> {
        match self {
            OpenUrlMode::Confirm => None,
            OpenUrlMode::Allow => Some("allow"),
            OpenUrlMode::Deny => Some("deny"),
        }
    }
}

impl FromStr for OpenUrlMode {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "allow" => Ok(OpenUrlMode::Allow),
            "deny" => Ok(OpenUrlMode::Deny),
            _ => Err(()),
        }
    }
}

#[cfg(test)]
mod aether_cli_tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    #[test]
    fn canonical_aether_flags_parse_without_the_product_prefix() {
        let opt = Opt::try_parse_from([
            "aether",
            "--metrics",
            "--input-trace",
            "--timeline-trace",
            "--frame-construction-retry",
            "--timeline-child-rebind",
            "--adaptive-avatar-cache",
            "--no-idle-gpu-upload-eviction",
            "--no-avm2-broadcast-fast-path",
            "--movement-stop-guard",
            "--mouse-motion-coalescing",
            "--bounded-offscreen-pool",
            "--cache-texture-grid",
            "--avm-trace",
            "--crash-report",
            "--crash-report-dir",
            "reports",
            "--maxfps",
            "75",
            "--msaa2x",
        ])
        .expect("canonical flags should parse");

        assert!(opt.aether_metrics);
        assert!(opt.aether_input_trace);
        assert!(opt.aether_timeline_trace);
        assert!(opt.aether_aqw_frame_construction_retry);
        assert!(opt.aether_aqw_timeline_child_rebind);
        assert!(opt.aether_aqw_adaptive_avatar_cache);
        assert!(opt.no_aether_aqw_idle_gpu_upload_eviction);
        assert!(opt.no_aether_aqw_avm2_broadcast_fast_path);
        assert!(opt.aether_aqw_movement_stop_guard);
        assert!(opt.aether_aqw_mouse_motion_coalescing);
        assert!(opt.aether_aqw_bounded_offscreen_pool);
        assert!(opt.aether_aqw_cache_texture_grid);
        assert!(opt.aether_avm_trace);
        assert!(opt.aether_crash_report);
        assert_eq!(opt.frame_rate, Some(75.0));
        assert!(opt.msaa2x);
        assert!(!opt.msaa4x);
    }

    #[test]
    fn legacy_aether_flag_spellings_remain_accepted() {
        let opt = Opt::try_parse_from([
            "aether",
            "--aether-metrics",
            "--aether-input-trace",
            "--aether-timeline-trace",
            "--aether-aqw-frame-construction-retry",
            "--aether-aqw-adaptive-avatar-cache",
            "--aether-aqw-cache-texture-grid",
            "--no-aether-aqw-mouse-motion-coalescing",
            "--aether-crash-report",
            "--frame-rate",
            "48",
        ])
        .expect("legacy aliases should parse");

        assert!(opt.aether_metrics);
        assert!(opt.aether_input_trace);
        assert!(opt.aether_timeline_trace);
        assert!(opt.aether_aqw_frame_construction_retry);
        assert!(opt.aether_aqw_adaptive_avatar_cache);
        assert!(opt.aether_aqw_cache_texture_grid);
        assert!(opt.no_aether_aqw_mouse_motion_coalescing);
        assert!(opt.aether_crash_report);
        assert_eq!(opt.frame_rate, Some(48.0));
    }

    #[test]
    fn legacy_names_are_hidden_from_normal_help() {
        let help = Opt::command().render_long_help().to_string();

        assert!(help.contains("--maxfps"));
        assert!(help.contains("--input-trace"));
        assert!(help.contains("--no-adaptive-avatar-cache"));
        assert!(!help.contains("--frame-rate"));
        assert!(!help.contains("--aether-input-trace"));
        assert!(!help.contains("--no-aether-aqw-adaptive-avatar-cache"));
    }

    #[test]
    fn msaa_override_flags_conflict() {
        assert!(Opt::try_parse_from(["aether", "--msaa2x", "--msaa4x"]).is_err());
    }
}

// TODO The following enum exists in order to preserve
//   the behavior of mapping gamepad buttons,
//   We should probably do something smarter here.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, clap::ValueEnum)]
enum NamedKeyCode {
    Unknown = 0,
    MouseLeft = 1,
    MouseRight = 2,
    MouseMiddle = 4,
    Backspace = 8,
    Tab = 9,
    Return = 13,
    Command = 15,
    Shift = 16,
    Control = 17,
    Alt = 18,
    Pause = 19,
    CapsLock = 20,
    Numpad = 21,
    Escape = 27,
    Space = 32,
    PgUp = 33,
    PgDown = 34,
    End = 35,
    Home = 36,
    Left = 37,
    Up = 38,
    Right = 39,
    Down = 40,
    Insert = 45,
    Delete = 46,
    Key0 = 48,
    Key1 = 49,
    Key2 = 50,
    Key3 = 51,
    Key4 = 52,
    Key5 = 53,
    Key6 = 54,
    Key7 = 55,
    Key8 = 56,
    Key9 = 57,
    A = 65,
    B = 66,
    C = 67,
    D = 68,
    E = 69,
    F = 70,
    G = 71,
    H = 72,
    I = 73,
    J = 74,
    K = 75,
    L = 76,
    M = 77,
    N = 78,
    O = 79,
    P = 80,
    Q = 81,
    R = 82,
    S = 83,
    T = 84,
    U = 85,
    V = 86,
    W = 87,
    X = 88,
    Y = 89,
    Z = 90,
    Numpad0 = 96,
    Numpad1 = 97,
    Numpad2 = 98,
    Numpad3 = 99,
    Numpad4 = 100,
    Numpad5 = 101,
    Numpad6 = 102,
    Numpad7 = 103,
    Numpad8 = 104,
    Numpad9 = 105,
    Multiply = 106,
    Plus = 107,
    NumpadEnter = 108,
    NumpadMinus = 109,
    NumpadPeriod = 110,
    NumpadSlash = 111,
    F1 = 112,
    F2 = 113,
    F3 = 114,
    F4 = 115,
    F5 = 116,
    F6 = 117,
    F7 = 118,
    F8 = 119,
    F9 = 120,
    F10 = 121,
    F11 = 122,
    F12 = 123,
    F13 = 124,
    F14 = 125,
    F15 = 126,
    F16 = 127,
    F17 = 128,
    F18 = 129,
    F19 = 130,
    F20 = 131,
    F21 = 132,
    F22 = 133,
    F23 = 134,
    F24 = 135,
    NumLock = 144,
    ScrollLock = 145,
    Semicolon = 186,
    Equals = 187,
    Comma = 188,
    Minus = 189,
    Period = 190,
    Slash = 191,
    Grave = 192,
    LBracket = 219,
    Backslash = 220,
    RBracket = 221,
    Apostrophe = 222,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub enum FilesystemAccessMode {
    /// Always allow non-interactive access to the filesystem.
    Allow,

    /// Refuse all non-interactive access to the filesystem.
    Deny,

    /// Ask the user before accessing the filesystem non-interactively.
    Ask,
}
