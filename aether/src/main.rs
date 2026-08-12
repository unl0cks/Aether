#![deny(clippy::unwrap_used)]
// By default, Windows creates an additional console window for our program.
//
//
// This is silently ignored on non-windows systems.
// See https://docs.microsoft.com/en-us/cpp/build/reference/subsystem?view=msvc-160 for details.
#![windows_subsystem = "windows"]

#[cfg(feature = "metrics")]
mod aether_metrics;
mod aether_preset;
mod aether_settings;
mod app;
mod backends;
mod cli;
mod crash_report;
mod custom_event;
mod dbus;
mod gui;
mod input_latency;
mod log;
mod mouse_motion;
mod player;
mod preferences;
mod self_update;
#[cfg(feature = "tracy")]
mod tracy;
mod util;
#[cfg(windows)]
mod windows;

use crate::preferences::GlobalPreferences;
use anyhow::{Context, Error};
use app::App;
use clap::Parser;
use cli::Opt;
use rfd::MessageDialogResult;
use ruffle_core::StaticCallstack;
use std::cell::RefCell;
use std::env;
use std::fs::File;
use std::panic::PanicHookInfo;
use tracing_subscriber::fmt::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use url::Url;

thread_local! {
    static CALLSTACK: RefCell<Option<StaticCallstack>> = RefCell::default();
    static RENDER_INFO: RefCell<Option<String>> = RefCell::default();
    static SWF_INFO: RefCell<Option<String>> = RefCell::default();
}

#[cfg(feature = "tracy")]
#[global_allocator]
static GLOBAL: tracing_tracy::client::ProfiledAllocator<std::alloc::System> =
    tracing_tracy::client::ProfiledAllocator::new(std::alloc::System, 0);

static AETHER_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "-",
    env!("CFG_RELEASE_CHANNEL"),
    " (",
    env!("VERGEN_GIT_SHA"),
    " ",
    env!("VERGEN_GIT_COMMIT_DATE"),
    ")"
);

fn truncate_to_line_boundary(text: String, max_len: usize) -> String {
    if text.len() <= max_len {
        return text;
    }
    let mut truncated = String::with_capacity(max_len + 10);
    for line in text.lines() {
        if truncated.len() + line.len() > max_len {
            break;
        }
        truncated.push_str(line);
        truncated.push('\n');
    }
    format!("{truncated}[...]")
}

fn panic_hook(info: &PanicHookInfo) {
    CALLSTACK.with(|callstack| {
        if let Some(callstack) = &*callstack.borrow() {
            callstack.avm2(|callstack| println!("AVM2 stack trace: {callstack}"))
        }
    });

    let message = info.payload_as_str().unwrap_or("panic occurred");

    if crash_report::is_armed() {
        let mut avm2 = String::new();
        CALLSTACK.with(|callstack| {
            if let Some(callstack) = &*callstack.borrow() {
                callstack.avm2(|stack| avm2 = stack.to_string());
            }
        });
        let backtrace = std::backtrace::Backtrace::force_capture();
        if let Some(path) = crash_report::write(
            "panic",
            &info.to_string(),
            &[
                crash_report::Section::new("AVM2 Callstack", avm2),
                crash_report::Section::new("Backtrace", backtrace.to_string()),
            ],
        ) {
            eprintln!("Aether crash report written to {}", path.display());
        }
    }

    if rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("Aether")
        .set_description(format!(
            "Aether has encountered a fatal error.\n\n\
            {message}\n\n\
            Please report this to us so that we can fix it. Thank you!\n\
            Pressing Yes will open a browser window."
        ))
        .set_buttons(rfd::MessageButtons::YesNo)
        .show()
        == MessageDialogResult::Yes
    {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let panic_text = format!("```\n{info}\n\nStack trace:\n{backtrace}\n```");

        // The URL cannot be too long.
        let panic_text = truncate_to_line_boundary(panic_text, 2048);

        let mut params = vec![
            ("panic_text", panic_text),
            ("platform", "Desktop app".to_string()),
            ("operating_system", os_info::get().to_string()),
            ("aether_version", AETHER_VERSION.to_string()),
        ];
        let mut extra_info = vec![];
        SWF_INFO.with(|i| {
            if let Some(swf_name) = i.take() {
                extra_info.push(format!("Filename: {swf_name}\n"));
                params.push(("title", format!("Crash on {swf_name}")));
            }
        });
        CALLSTACK.with(|callstack| {
            if let Some(callstack) = &*callstack.borrow() {
                callstack.avm2(|callstack| {
                    extra_info.push(format!("### AVM2 Callstack\n```{callstack}\n```\n"));
                });
            }
        });
        RENDER_INFO.with(|i| {
            if let Some(render_info) = i.take() {
                extra_info.push(format!("### Render Info\n{render_info}\n"));
            }
        });
        if !extra_info.is_empty() {
            params.push(("extra_info", extra_info.join("\n")));
        }
        if let Ok(url) = Url::parse_with_params(
            "https://github.com/ruffle-rs/ruffle/issues/new?assignees=&labels=bug&template=crash_report.yml",
            &params,
        ) {
            let _ = webbrowser::open(url.as_str());
        }
    }
}

/// The tracing filter Aether uses when `RUST_LOG` is unset.
///
/// `aether=info` covers the whole crate rather than naming each diagnostic target. It used to list
/// them one at a time, which meant every new target was emitted and then silently dropped until
/// somebody remembered to add it here. That happened to the click latency probe: it measured every
/// click correctly and threw all of them away, which is indistinguishable from a broken feature.
///
/// `warn` for everything else keeps other crates quiet; `ruffle=info` is what makes the ordinary
/// session log useful.
fn default_log_filter(avm_trace: bool) -> String {
    let avm_trace = if avm_trace { "info" } else { "off" };
    format!("warn,ruffle=info,aether=info,avm_trace={avm_trace}")
}

/// Ask GitHub whether there is a newer release, with a short leash.
///
/// Bounded at three seconds because this sits in front of the window. A slow or blocked connection
/// must cost a moment, not a wait: an update check that delays every launch is a worse feature than
/// no update check.
fn check_for_update() -> Option<self_update::Release> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;

    let release = runtime.block_on(async {
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            self_update::fetch_latest_release(),
        )
        .await
        .ok()
        .flatten()
    })?;

    self_update::is_newer(&release.version, env!("CARGO_PKG_VERSION")).then_some(release)
}

fn main() -> Result<(), Error> {
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        prev_hook(info);
        panic_hook(info);
    }));

    #[cfg(windows)]
    let _console = windows::Console::attach();
    // Before anything is measured or rendered, because both inherit from whoever started us.
    #[cfg(windows)]
    windows::restore_foreground_scheduling();

    let mut opt = Opt::parse();
    // Before the preset, not after: it is what collapses each --x / --no-x pair into one bool and
    // erases the difference between "switched off" and "not mentioned". A setting nobody mentioned
    // is one the options window is free to decide.
    let aether_explicit = aether_settings::AetherExplicitFlags::capture(&opt);
    let aqw_mode = aether_preset::apply(&mut opt)?;
    let preferences = GlobalPreferences::load(opt, aether_explicit, aqw_mode)?;
    // Resolved rather than read straight off the flags, so a sample count chosen in the options
    // window applies on the next start. Read once while the renderer is built, which is why the
    // window says a restart is needed for it.
    ruffle_render_wgpu::set_backend_msaa_override(
        preferences.aether().msaa_samples.value.map(u32::from),
    );

    // Before the window, so the dialogs are the only thing on screen rather than sitting over a
    // half-drawn game. A local build never checks: offering it an "update" to the release it was
    // built from would be noise every launch.
    if let Ok(current_exe) = env::current_exe() {
        self_update::clean_stale_backup(&current_exe);
    }
    if !preferences.cli.no_aether_update_check
        && self_update::version_is_released(&self_update::build_version(
            env!("CARGO_PKG_VERSION"),
            env!("CFG_RELEASE_CHANNEL"),
        ))
        && let Some(release) = check_for_update()
        && let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        && self_update::offer_update(&release, &runtime)
    {
        self_update::restart();
        return Ok(());
    }

    let logs_path = &preferences.cli.cache_directory.join("log");
    let log_path = preferences.log_filename_pattern().create_path(logs_path);
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Err(err) = migrate_logs(&preferences, logs_path) {
        tracing::warn!("Failed to migrate logs: {}", err);
    }

    // Armed before the subscriber exists so the ring buffer catches startup as well.
    if preferences.cli.aether_crash_report {
        crash_report::arm(
            preferences
                .cli
                .aether_crash_report_dir
                .clone()
                .unwrap_or_else(|| logs_path.clone()),
        );
    }

    // [NA] `_guard` cannot be `_` or it'll immediately drop
    // https://docs.rs/tracing-appender/latest/tracing_appender/non_blocking/index.html
    let (non_blocking_file, _file_guard) = tracing_appender::non_blocking(File::create(log_path)?);
    let (non_blocking_stdout, _stdout_guard) = tracing_appender::non_blocking(std::io::stdout());

    let default_log_filter = default_log_filter(preferences.cli.aether_avm_trace);

    let env_filter = tracing_subscriber::EnvFilter::builder().parse_lossy(
        env::var("RUST_LOG")
            .as_deref()
            .unwrap_or(&default_log_filter),
    );

    let subscriber = tracing_subscriber::registry()
        .with(env_filter)
        .with(crash_report::RecentLogLayer)
        .with(Layer::new().with_writer(non_blocking_stdout))
        .with(Layer::new().with_writer(non_blocking_file).with_ansi(false));

    #[cfg(feature = "tracy")]
    let subscriber = {
        let tracy_subscriber = tracing_tracy::TracyLayer::new(tracy::RuffleTracyConfig::default());
        subscriber.with(tracy_subscriber)
    };

    subscriber.init();

    if preferences.cli.aether_avm_trace {
        tracing::warn!(
            "Raw ActionScript trace logging is enabled and may contain credentials or session data"
        );
    }

    #[cfg(feature = "metrics")]
    if preferences.cli.aether_input_trace {
        let path = preferences
            .cli
            .aether_input_trace_file
            .clone()
            .unwrap_or_else(|| {
                preferences
                    .cli
                    .cache_directory
                    .join("metrics")
                    .join("input.jsonl")
            });
        match ruffle_core::aether_diagnostics::configure_input_trace(&path) {
            Ok(()) => tracing::info!("Aether input trace enabled: {}", path.display()),
            Err(error) => tracing::warn!("Failed to enable Aether input trace: {error}"),
        }
    }
    #[cfg(not(feature = "metrics"))]
    if preferences.cli.aether_input_trace {
        tracing::warn!(
            "--input-trace was requested, but this binary was built without the `metrics` feature"
        );
    }

    #[cfg(feature = "metrics")]
    if preferences.cli.aether_timeline_trace {
        let path = preferences
            .cli
            .aether_timeline_trace_file
            .clone()
            .unwrap_or_else(|| {
                preferences
                    .cli
                    .cache_directory
                    .join("metrics")
                    .join("timeline.jsonl")
            });
        match ruffle_core::aether_diagnostics::configure_timeline_trace(&path) {
            Ok(()) => tracing::info!("Aether timeline trace enabled: {}", path.display()),
            Err(error) => tracing::warn!("Failed to enable Aether timeline trace: {error}"),
        }
    }
    #[cfg(not(feature = "metrics"))]
    if preferences.cli.aether_timeline_trace {
        tracing::warn!(
            "--timeline-trace was requested, but this binary was built without the `metrics` feature"
        );
    }

    #[cfg(feature = "metrics")]
    {
        let retry_enabled = aqw_mode && preferences.cli.aether_aqw_frame_construction_retry;
        ruffle_core::aether_diagnostics::set_frame_construction_retry_enabled(retry_enabled);
        if retry_enabled {
            tracing::warn!(
                "Experimental AQW frame-construction retry is enabled; use only for A/B compatibility testing"
            );
        }
    }
    #[cfg(not(feature = "metrics"))]
    if preferences.cli.aether_aqw_frame_construction_retry {
        tracing::warn!(
            "--frame-construction-retry was requested, but this binary was built without the `metrics` feature"
        );
    }

    let cache_texture_grid = aqw_mode && preferences.cli.aether_aqw_cache_texture_grid;
    ruffle_core::aether_performance::set_cache_texture_grid_enabled(cache_texture_grid);

    // The same call the options window makes when it saves, so a setting changed in game and a
    // setting read from the file at startup cannot end up meaning different things.
    aether_settings::apply_live_settings(&preferences.aether(), aqw_mode, None);

    let rebind_enabled = aqw_mode && preferences.cli.aether_aqw_timeline_child_rebind;
    ruffle_core::aether_compatibility::set_timeline_child_rebind_enabled(rebind_enabled);
    if rebind_enabled {
        tracing::info!(
            target: "ruffle_core::aether_compatibility",
            "AQW Settings timeline-child compatibility repair is enabled"
        );
    }

    let adaptive_avatar_cache_enabled =
        aqw_mode && preferences.cli.aether_aqw_adaptive_avatar_cache;
    ruffle_core::aether_performance::set_adaptive_avatar_cache_enabled(
        adaptive_avatar_cache_enabled,
    );
    if adaptive_avatar_cache_enabled {
        tracing::info!(
            "AQW stable AvatarMC output caching is enabled; active animation frames remain uncached"
        );
    }

    let idle_gpu_upload_eviction = aqw_mode && preferences.cli.aether_aqw_idle_gpu_upload_eviction;
    ruffle_core::aether_performance::set_idle_gpu_upload_eviction_enabled(idle_gpu_upload_eviction);
    if idle_gpu_upload_eviction {
        tracing::info!(
            "AQW idle GPU uploads are evicted periodically; bitmaps re-upload when drawn again"
        );
    }

    ruffle_core::aether_performance::set_filterless_hot_cache_bypass_enabled(aqw_mode);
    if aqw_mode {
        tracing::info!(
            "AQW adaptive direct rendering for repeatedly invalidated filterless caches is enabled"
        );
    }

    let avm2_broadcast_fast_path_enabled =
        aqw_mode && preferences.cli.aether_aqw_avm2_broadcast_fast_path;
    ruffle_core::aether_performance::set_avm2_broadcast_fast_path_enabled(
        avm2_broadcast_fast_path_enabled,
    );
    if avm2_broadcast_fast_path_enabled {
        tracing::info!("AQW AVM2 broadcast cleanup and dispatch fast path is enabled");
    }

    let movement_stop_guard_enabled = aqw_mode && preferences.cli.aether_aqw_movement_stop_guard;
    ruffle_core::aether_diagnostics::set_movement_stop_guard_enabled(movement_stop_guard_enabled);
    if movement_stop_guard_enabled {
        tracing::warn!(
            "Experimental AQW premature movement-stop guard is enabled; use only for compatibility testing"
        );
    }

    if preferences.cli.aether_aqw_mouse_motion_coalescing {
        tracing::info!("AQW mouse motion is coalesced to rendered frames during gameplay");
    } else if aqw_mode {
        tracing::info!("AQW mouse motion is dispatched immediately for movement and dragging");
    }

    if let Some(sample_count) = preferences.aether().msaa_samples.value {
        tracing::info!(sample_count, "Backend MSAA override is enabled");
    }

    tracing::info!(
        mode = if aqw_mode { "aqw" } else { "generic" },
        version = AETHER_VERSION,
        "Starting Aether"
    );
    if preferences.cli.aether_metrics {
        #[cfg(feature = "metrics")]
        {
            let path = preferences
                .cli
                .aether_metrics_file
                .clone()
                .unwrap_or_else(|| {
                    preferences
                        .cli
                        .cache_directory
                        .join("metrics")
                        .join("frames.jsonl")
                });
            tracing::info!("Aether frame metrics enabled: {}", path.display());
        }
        #[cfg(not(feature = "metrics"))]
        tracing::warn!(
            "--metrics was requested, but this binary was built without the `metrics` feature"
        );
    }

    let result = App::new(preferences).and_then(|(mut app, event_loop)| {
        event_loop.run_app(&mut app).context("Event loop failure")
    });

    #[cfg(feature = "metrics")]
    {
        ruffle_core::aether_diagnostics::shutdown_input_trace();
        ruffle_core::aether_diagnostics::shutdown_timeline_trace();
        ruffle_core::aether_diagnostics::set_frame_construction_retry_enabled(false);

        // The census used to print only on a fault, which made the pool's own reuse figures
        // reachable only by crashing -- so a session that survived taught us nothing. Emit it at
        // the end of every run instead; the counters are process globals and outlive the device.
        for line in ruffle_render_wgpu::aether_metrics::texture_census_report(20) {
            tracing::info!(target: "aether::session", "{line}");
        }
    }
    ruffle_core::aether_diagnostics::set_movement_stop_guard_enabled(false);
    ruffle_core::aether_compatibility::set_timeline_child_rebind_enabled(false);

    #[cfg(windows)]
    if let Err(error) = &result {
        eprintln!("{:?}", error)
    }

    result
}

/// Move logs from config directory into proper log directory.
///
/// This exists because in older versions Ruffle created log files in the config directory.
///
/// TODO Remove this after some time.
fn migrate_logs(
    preferences: &GlobalPreferences,
    log_path: &std::path::Path,
) -> std::io::Result<()> {
    tracing::debug!("Migrating logs from config directory to log directory");
    let config_path = &preferences.cli.config;
    let paths = std::fs::read_dir(config_path)?;

    for dir in paths.flatten() {
        if dir.file_name().as_encoded_bytes().ends_with(b".log") {
            let src = dir.path();
            let dest = log_path.join(dir.file_name());
            tracing::info!(
                "Moving log file {} -> {}",
                src.to_string_lossy(),
                dest.to_string_lossy()
            );
            if let Err(err) = std::fs::rename(&src, &dest) {
                tracing::warn!(
                    "Failed to move log file {} -> {}: {err}",
                    src.to_string_lossy(),
                    dest.to_string_lossy()
                )
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod log_filter_tests {
    use super::default_log_filter;
    use tracing::Level;
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::layer::{Layer as _, SubscriberExt as _};

    /// Whether a target would actually be recorded, asked of a real `EnvFilter` rather than by
    /// looking for a substring. The bug this guards against was a filter that parsed perfectly and
    /// dropped the events anyway.
    fn admits(filter: &str, target: &str, level: Level) -> bool {
        let filter = EnvFilter::builder().parse_lossy(filter);
        let subscriber = tracing_subscriber::registry().with(filter.boxed());
        tracing::subscriber::with_default(subscriber, || {
            tracing::dispatcher::get_default(|dispatch| {
                dispatch.enabled(&tracing::Metadata::new(
                    "probe",
                    target,
                    level,
                    None,
                    None,
                    None,
                    tracing::field::FieldSet::new(&[], tracing::callsite::Identifier(&CALLSITE)),
                    tracing::metadata::Kind::EVENT,
                ))
            })
        })
    }

    struct Callsite;
    impl tracing::callsite::Callsite for Callsite {
        fn set_interest(&self, _: tracing::subscriber::Interest) {}
        fn metadata(&self) -> &tracing::Metadata<'_> {
            unreachable!("only the identifier is used")
        }
    }
    static CALLSITE: Callsite = Callsite;

    /// Every diagnostic Aether writes goes to an `aether::*` target. Naming them one at a time is
    /// what lost the click latency probe, so the filter has to admit the crate as a whole.
    #[test]
    fn the_default_filter_admits_every_aether_diagnostic() {
        let filter = default_log_filter(false);

        for target in [
            "aether::input_latency",
            "aether::session",
            "aether::perf",
            "aether::gui::controller",
        ] {
            assert!(
                admits(&filter, target, Level::INFO),
                "{target} would be dropped by {filter}"
            );
        }
    }

    /// The filter that shipped in 0.5.1, kept as a specimen. It parses cleanly and drops the
    /// probe, which is why nothing looked wrong until a whole test session came back empty.
    #[test]
    fn the_enumerated_filter_that_lost_the_probe_is_still_caught() {
        let shipped = "warn,ruffle=info,avm_trace=off,aether::session=info,aether::perf=info";

        assert!(admits(shipped, "aether::session", Level::INFO));
        assert!(
            !admits(shipped, "aether::input_latency", Level::INFO),
            "this filter is the bug; if it now admits the probe the test proves nothing"
        );
    }

    /// Other crates stay quiet, which is the reason the filter exists at all.
    #[test]
    fn the_default_filter_still_keeps_unrelated_crates_at_warn() {
        let filter = default_log_filter(false);

        assert!(!admits(&filter, "wgpu_core::device", Level::INFO));
        assert!(admits(&filter, "wgpu_core::device", Level::WARN));
    }

    /// The ActionScript trace carries session data, so it is off unless asked for by name.
    #[test]
    fn the_actionscript_trace_is_off_unless_requested() {
        assert!(!admits(
            &default_log_filter(false),
            "avm_trace",
            Level::INFO
        ));
        assert!(admits(&default_log_filter(true), "avm_trace", Level::INFO));
    }
}
