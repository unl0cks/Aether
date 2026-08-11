use crate::cli::Opt;
use anyhow::{Context, Result};
use ruffle_core::LoadBehavior;
use ruffle_core::backend::navigator::SocketMode;
use ruffle_core::config::Letterbox;
use ruffle_render::quality::StageQuality;
use ruffle_render_wgpu::clap::{GraphicsBackend, PowerPreference};
use ruffle_render_wgpu::texture_pool_policy::BoundedTexturePoolLimits;
use url::Url;

pub const AQW_LOADER_URL: &str = "https://game.aq.com/game/gamefiles/Loader_Spider.swf";
pub const AQW_BASE_URL: &str = "https://game.aq.com/game/gamefiles/";
pub const AQW_PAGE_URL: &str = "https://game.aq.com/";
/// Retention limits for both texture pools, as specified in the bounded-offscreen-pool design.
///
/// The design called for 256 MiB / 1,024 entries / 120 idle frames. What shipped was 128 MiB
/// with `max_idle_frames: 1`, and the general pool then quartered and clamped that to 32 MiB --
/// two of AQW's ~12 MB render targets, which made the pool a no-op and produced 48 GB/second
/// of allocation churn. Raising it far past the design instead (768 MiB per pool) restored
/// reuse but pushed a 10 GB card into a device-lost fault, so this is the documented budget.
pub const AQW_OFFSCREEN_TEXTURE_POOL_LIMITS: BoundedTexturePoolLimits = BoundedTexturePoolLimits {
    // Measured: with 128 MiB the offscreen pool reused 62.2% of its requests and threw away 99,073
    // entries for budget against 28,372 for age, so the byte limit was the dominant reason a
    // texture did not survive to be reused. The general pool showed the same signature before its
    // own limit was raised and now reuses 99.7%. Peak resident GPU memory in that session was
    // 5.45 GB on a 10 GB card, so there is room to let this pool keep what it is being handed.
    max_cached_bytes: 512 * 1024 * 1024,
    max_cached_entries: 512,
    // Raising the byte budget moved the bottleneck rather than removing it. Budget evictions fell
    // from 99,073 to 7,380 and reuse rose from 62.2% to 74.7%, but age evictions rose from 28,372
    // to 52,573 and the pool ended the session holding 9 entries across 6 buckets. A one-frame
    // window only keeps a size that is used on consecutive frames, and AQW's animations revisit a
    // size every few frames as a loop comes back around, so the window was throwing away work that
    // was about to be asked for again. Four frames is still short enough that a size which stops
    // being used is gone within a tenth of a second.
    max_idle_frames: 4,
    max_cached_globals: 128,
};

/// Apply conservative AQW defaults while preserving explicit CLI overrides.
///
/// Native Ruffle can open TCP sockets directly. Aether therefore does not use Aquastar's
/// Electron/WebSocket relay. Socket access is enabled only for the dedicated AQW preset; generic
/// Ruffle mode retains the normal Ruffle behavior.
pub fn apply(opt: &mut Opt) -> Result<bool> {
    opt.aether_crash_report = !opt.no_aether_crash_report;

    if opt.generic || opt.movie_url.is_some() {
        return Ok(false);
    }

    opt.aether_aqw_timeline_child_rebind = !opt.no_aether_aqw_timeline_child_rebind;
    // Immediate dispatch runs Flash mouse callbacks for every host cursor event. Keep the
    // rendered-frame coalescer on by default so high-rate pointer input cannot flood busy maps.
    opt.aether_aqw_mouse_motion_coalescing = !opt.no_aether_aqw_mouse_motion_coalescing;
    opt.aether_aqw_avm2_broadcast_fast_path = !opt.no_aether_aqw_avm2_broadcast_fast_path;
    // The adaptive avatar cache is what turns on BitmapCacheTexturePolicy::BoundedReuse, which keeps
    // an existing, slightly oversized cache texture when an object's bounds drift instead of
    // reallocating. Everything it corrupted -- glow and drop-shadow layers on fire effects, weapon
    // and armour trims, damage and healing numbers, NPC avatars, the Book of Lore's badges -- was a
    // filter drawn onto a texture larger than its contents.
    //
    // Those filters sample two textures at once: the source, and a separately allocated blurred
    // layer the size of the filtered region. Both sets of UVs were normalised against the source
    // texture, which is right only while that texture is exactly its own region. Oversize it and
    // the glow is sampled at region/texture of its correct scale, so it renders shrunk toward the
    // top left of the object it belongs to. Fixed in filters.rs; see the tests there.
    opt.aether_aqw_adaptive_avatar_cache = !opt.no_aether_aqw_adaptive_avatar_cache;
    opt.aether_aqw_movement_stop_guard = !opt.no_aether_aqw_movement_stop_guard;
    // The grid is a large win: it took texture creation from 7.0 GB/s to 0.22 GB/s and peak
    // resident memory from 4.67 GB to 1.12 GB, because it collapses the per-frame bounds drift that
    // no pool setting could absorb. It cost nothing in frame rate either way, which is what ruled
    // allocation out as the reason frames are slow.
    //
    // It is also what located the fault. The grid only ever allocates a texture larger than its
    // contents; it never reuses one across a bounds change. Corrupting the same things the avatar
    // cache does proved the oversizing was to blame rather than the reuse, and that is where the
    // filter UV bug turned out to be.
    opt.aether_aqw_cache_texture_grid = !opt.no_aether_aqw_cache_texture_grid;
    opt.aether_aqw_idle_gpu_upload_eviction = !opt.no_aether_aqw_idle_gpu_upload_eviction;

    // Requested by a dyslexic player: boss health runs to eight and nine digits and changes
    // several times a second, and without separators there is nothing to anchor where the
    // millions place sits. The same is true of the gold total, the experience and reputation
    // bars, quest progress and item counts, so it covers every number the game displays. Display
    // only, so it is on unless asked otherwise.
    opt.aether_aqw_hp_separators = !opt.no_aether_aqw_hp_separators;

    // Retain exact-sized offscreen surfaces only long enough for immediately repeating equipment
    // and combat-filter work to reuse them. The renderer also derives a smaller 32 MiB companion
    // budget for transient filter/surface textures, so changing maps or logging in repeatedly
    // cannot accumulate every GPU allocation size encountered during the session.
    opt.aether_bounded_offscreen_pool = !opt.no_aether_aqw_bounded_offscreen_pool;

    opt.movie_url = Some(Url::parse(AQW_LOADER_URL).context("Invalid built-in AQW loader URL")?);

    if opt.base.is_none() {
        opt.base = Some(Url::parse(AQW_BASE_URL).context("Invalid built-in AQW base URL")?);
    }
    if opt.referer.is_none() {
        opt.referer = Some(Url::parse(AQW_PAGE_URL).context("Invalid built-in AQW page URL")?);
    }
    if opt.power.is_none() {
        opt.power = Some(PowerPreference::High);
    }
    if opt.graphics.is_none() {
        // Measured A/B on identical scenes and builds: Vulkan held 15-30 fps in a populated
        // Battleon and never exceeded 3.0 GB of texture memory, while DX12 fell to 0.8-2.7 fps
        // and reached 17.4 GB. wgpu's DX12 backend does not return texture memory promptly, so
        // AQW's cache churn overcommits it into system RAM and every access then pages over
        // PCIe. Left to itself wgpu picks DX12 on Windows. `--graphics` still overrides this.
        opt.graphics = Some(GraphicsBackend::Vulkan);
    }
    if opt.quality.is_none() {
        // Preserve vector sharpness by default. Lower quality remains available through --quality.
        opt.quality = Some(StageQuality::High);
    }
    if opt.load_behavior.is_none() {
        opt.load_behavior = Some(LoadBehavior::Streaming);
    }
    if opt.letterbox.is_none() {
        opt.letterbox = Some(Letterbox::On);
    }
    if opt.player_version.is_none() {
        opt.player_version = Some(32);
    }
    if opt.width.is_none() {
        opt.width = Some(1280.0);
    }
    if opt.height.is_none() {
        opt.height = Some(720.0);
    }
    if opt.frame_rate.is_none() {
        opt.frame_rate = Some(60.0);
    }
    if opt.tcp_connections.is_none() {
        opt.tcp_connections = Some(SocketMode::Allow);
    }

    // Loader_Spider reads this FlashVar while resolving the rest of AQW's client assets.
    opt.push_parameter("base", AQW_BASE_URL);

    if !opt.show_menu {
        opt.no_gui = true;
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse_and_apply(args: &[&str]) -> Opt {
        let mut opt = Opt::try_parse_from(args.iter().copied()).unwrap();
        apply(&mut opt).unwrap();
        opt
    }

    #[test]
    fn aqw_preset_enables_timeline_child_rebind_by_default() {
        let opt = parse_and_apply(&["aether"]);
        assert!(opt.aether_aqw_timeline_child_rebind);
    }

    #[test]
    fn aqw_preset_allows_timeline_child_rebind_opt_out() {
        let parsed = Opt::try_parse_from(["aether", "--no-aether-aqw-timeline-child-rebind"]);
        assert!(parsed.is_ok());
        let opt = parse_and_apply(&["aether", "--no-aether-aqw-timeline-child-rebind"]);
        assert!(!opt.aether_aqw_timeline_child_rebind);
    }

    #[test]
    fn aqw_preset_accepts_retained_positive_timeline_child_rebind_flag() {
        let opt = parse_and_apply(&["aether", "--aether-aqw-timeline-child-rebind"]);
        assert!(opt.aether_aqw_timeline_child_rebind);
    }

    #[test]
    fn timeline_child_rebind_flags_conflict() {
        let parsed = Opt::try_parse_from([
            "aether",
            "--aether-aqw-timeline-child-rebind",
            "--no-aether-aqw-timeline-child-rebind",
        ]);
        assert!(parsed.is_err());
    }

    #[test]
    fn generic_and_explicit_movies_do_not_enable_timeline_child_rebind() {
        let generic = parse_and_apply(&["aether", "--generic"]);
        assert!(!generic.aether_aqw_timeline_child_rebind);
        let movie = parse_and_apply(&["aether", "https://example.invalid/movie.swf"]);
        assert!(!movie.aether_aqw_timeline_child_rebind);
    }

    #[test]
    fn aqw_preset_enables_adaptive_avatar_cache_by_default() {
        let opt = parse_and_apply(&["aether"]);
        assert!(opt.aether_aqw_adaptive_avatar_cache);
    }

    #[test]
    fn aqw_preset_allows_adaptive_avatar_cache_opt_out() {
        let opt = parse_and_apply(&["aether", "--no-adaptive-avatar-cache"]);
        assert!(!opt.aether_aqw_adaptive_avatar_cache);
    }

    #[test]
    fn aqw_preset_enables_idle_gpu_upload_eviction_by_default() {
        let opt = parse_and_apply(&["aether"]);
        assert!(opt.aether_aqw_idle_gpu_upload_eviction);
    }

    #[test]
    fn aqw_preset_allows_idle_gpu_upload_eviction_opt_out() {
        let opt = parse_and_apply(&["aether", "--no-aether-aqw-idle-gpu-upload-eviction"]);
        assert!(!opt.aether_aqw_idle_gpu_upload_eviction);
    }

    #[test]
    fn generic_and_explicit_movies_do_not_enable_adaptive_avatar_cache() {
        let generic = parse_and_apply(&["aether", "--generic"]);
        assert!(!generic.aether_aqw_adaptive_avatar_cache);

        let movie = parse_and_apply(&["aether", "https://example.invalid/movie.swf"]);
        assert!(!movie.aether_aqw_adaptive_avatar_cache);
    }

    #[test]
    fn aqw_preset_keeps_only_immediately_reusable_offscreen_textures() {
        let opt = parse_and_apply(&["aether"]);
        assert!(opt.aether_bounded_offscreen_pool);

        // A one-frame window was pinned here on the reasoning that it is what makes the pool
        // "immediately reusable". Measurement retired that too: once the byte budget stopped being
        // the binding limit, age became the dominant eviction by 52,573 to 7,380, and the pool held
        // 9 entries. The window still has to be short, because its job is to drop sizes that have
        // stopped being asked for, but one frame was short enough to drop sizes still in use.
        assert!(AQW_OFFSCREEN_TEXTURE_POOL_LIMITS.max_idle_frames >= 1);
        assert!(AQW_OFFSCREEN_TEXTURE_POOL_LIMITS.max_idle_frames <= 8);

        // The byte budget used to be pinned at 128 MiB as well, on the theory that a short idle
        // window made a small budget harmless. Measurement disagreed: at 128 MiB the pool reused
        // 62.2% of requests and discarded 99,073 entries for budget against 28,372 for age, so the
        // budget, not the idle window, was deciding what got reused. It still has to stay far below
        // the card, because a pool that can hold everything is not a bounded pool.
        assert!(AQW_OFFSCREEN_TEXTURE_POOL_LIMITS.max_cached_bytes <= 1024 * 1024 * 1024);
        assert!(AQW_OFFSCREEN_TEXTURE_POOL_LIMITS.max_cached_bytes >= 256 * 1024 * 1024);
    }

    #[test]
    fn aqw_preset_selects_vulkan_because_dx12_overcommits_gpu_memory() {
        // Measured A/B on the same scene and build: Vulkan never exceeded 3.0 GB of texture
        // memory and held 15-30 fps in a populated Battleon; DX12 reached 17.4 GB and fell to
        // 0.8-2.7 fps. wgpu's DX12 backend does not return texture memory promptly, so it
        // overcommits into system RAM and the card pages on every access. Vulkan hits a real
        // allocation limit instead, which is recoverable; silent 20x paging is not.
        let opt = parse_and_apply(&["aether"]);
        assert_eq!(opt.graphics, Some(GraphicsBackend::Vulkan));
    }

    #[test]
    fn an_explicit_graphics_backend_still_wins() {
        let opt = parse_and_apply(&["aether", "--graphics", "dx12"]);
        assert_eq!(opt.graphics, Some(GraphicsBackend::Dx12));
    }

    #[test]
    fn generic_and_explicit_movies_do_not_force_a_graphics_backend() {
        assert_eq!(parse_and_apply(&["aether", "--generic"]).graphics, None);
        assert_eq!(
            parse_and_apply(&["aether", "https://example.invalid/movie.swf"]).graphics,
            None
        );
    }

    #[test]
    fn aqw_preset_allows_bounded_offscreen_pool_opt_in() {
        let opt = parse_and_apply(&["aether", "--aether-aqw-bounded-offscreen-pool"]);
        assert!(opt.aether_bounded_offscreen_pool);
    }

    #[test]
    fn aqw_preset_allows_bounded_offscreen_pool_opt_out() {
        let opt = parse_and_apply(&["aether", "--no-aether-aqw-bounded-offscreen-pool"]);
        assert!(!opt.aether_bounded_offscreen_pool);
    }

    #[test]
    fn generic_and_explicit_movie_modes_remain_ephemeral() {
        let generic = parse_and_apply(&["aether", "--generic"]);
        assert!(!generic.aether_bounded_offscreen_pool);

        let movie = parse_and_apply(&["aether", "https://example.invalid/movie.swf"]);
        assert!(!movie.aether_bounded_offscreen_pool);
    }

    #[test]
    fn aqw_preset_enables_mouse_motion_coalescing_by_default() {
        let opt = parse_and_apply(&["aether"]);
        assert!(opt.aether_aqw_mouse_motion_coalescing);
    }

    #[test]
    fn aqw_preset_allows_mouse_motion_coalescing_opt_out() {
        let opt = parse_and_apply(&["aether", "--no-mouse-motion-coalescing"]);
        assert!(!opt.aether_aqw_mouse_motion_coalescing);
    }

    #[test]
    fn generic_and_explicit_movie_modes_do_not_coalesce_mouse_motion() {
        let generic = parse_and_apply(&["aether", "--generic"]);
        assert!(!generic.aether_aqw_mouse_motion_coalescing);

        let movie = parse_and_apply(&["aether", "https://example.invalid/movie.swf"]);
        assert!(!movie.aether_aqw_mouse_motion_coalescing);
    }

    #[test]
    fn aqw_preset_enables_avm2_broadcast_fast_path_by_default() {
        let opt = parse_and_apply(&["aether"]);
        assert!(opt.aether_aqw_avm2_broadcast_fast_path);
    }

    #[test]
    fn aqw_preset_allows_avm2_broadcast_fast_path_opt_out() {
        let opt = parse_and_apply(&["aether", "--no-aether-aqw-avm2-broadcast-fast-path"]);
        assert!(!opt.aether_aqw_avm2_broadcast_fast_path);
    }

    #[test]
    fn generic_and_explicit_movie_modes_do_not_enable_avm2_broadcast_fast_path() {
        let generic = parse_and_apply(&["aether", "--generic"]);
        assert!(!generic.aether_aqw_avm2_broadcast_fast_path);

        let movie = parse_and_apply(&["aether", "https://example.invalid/movie.swf"]);
        assert!(!movie.aether_aqw_avm2_broadcast_fast_path);
    }

    #[test]
    fn aqw_preset_targets_60_fps_by_default() {
        let opt = parse_and_apply(&["aether"]);
        assert_eq!(opt.frame_rate, Some(60.0));
    }

    #[test]
    fn aqw_preset_preserves_explicit_frame_rate_override() {
        let opt = parse_and_apply(&["aether", "--maxfps", "48"]);
        assert_eq!(opt.frame_rate, Some(48.0));
    }

    #[test]
    fn legacy_frame_rate_alias_remains_accepted() {
        let opt = parse_and_apply(&["aether", "--frame-rate", "48"]);
        assert_eq!(opt.frame_rate, Some(48.0));
    }

    #[test]
    fn crash_reports_are_enabled_by_default_in_every_mode() {
        assert!(parse_and_apply(&["aether"]).aether_crash_report);
        assert!(parse_and_apply(&["aether", "--generic"]).aether_crash_report);
    }

    #[test]
    fn crash_reports_can_be_disabled() {
        assert!(!parse_and_apply(&["aether", "--no-crash-report"]).aether_crash_report);
    }

    #[test]
    fn aqw_preset_groups_numbers_by_default() {
        assert!(parse_and_apply(&["aether"]).aether_aqw_hp_separators);
    }

    #[test]
    fn aqw_preset_allows_separator_opt_out() {
        assert!(!parse_and_apply(&["aether", "--no-number-separators"]).aether_aqw_hp_separators);
    }

    /// The flags shipped as `--hp-separators` before the feature covered gold, experience,
    /// reputation and quest text as well. Anyone whose shortcut still says that keeps working.
    #[test]
    fn the_original_hp_flag_names_still_work() {
        assert!(!parse_and_apply(&["aether", "--no-hp-separators"]).aether_aqw_hp_separators);
        assert!(parse_and_apply(&["aether", "--hp-separator-space"]).aether_aqw_hp_separator_space);
    }

    #[test]
    fn aqw_preset_enables_texture_grid_by_default() {
        assert!(parse_and_apply(&["aether"]).aether_aqw_cache_texture_grid);
    }

    #[test]
    fn aqw_preset_allows_texture_grid_opt_out() {
        assert!(
            !parse_and_apply(&["aether", "--no-cache-texture-grid"]).aether_aqw_cache_texture_grid
        );
    }

    #[test]
    fn generic_and_explicit_movie_modes_do_not_override_frame_rate() {
        let generic = parse_and_apply(&["aether", "--generic"]);
        assert_eq!(generic.frame_rate, None);

        let movie = parse_and_apply(&["aether", "https://example.invalid/movie.swf"]);
        assert_eq!(movie.frame_rate, None);
    }

    #[test]
    fn aqw_preset_enables_movement_stop_guard_by_default() {
        let opt = parse_and_apply(&["aether"]);
        assert!(opt.aether_aqw_movement_stop_guard);
    }

    #[test]
    fn normal_build_contains_movement_stop_guard_runtime() {
        ruffle_core::aether_diagnostics::set_movement_stop_guard_enabled(true);
        assert!(ruffle_core::aether_diagnostics::movement_stop_guard_enabled());
        ruffle_core::aether_diagnostics::set_movement_stop_guard_enabled(false);
    }

    #[test]
    fn aqw_preset_allows_movement_stop_guard_opt_out() {
        let opt = parse_and_apply(&["aether", "--no-aether-aqw-movement-stop-guard"]);
        assert!(!opt.aether_aqw_movement_stop_guard);
    }

    #[test]
    fn movement_stop_guard_flags_conflict() {
        let parsed = Opt::try_parse_from([
            "aether",
            "--aether-aqw-movement-stop-guard",
            "--no-aether-aqw-movement-stop-guard",
        ]);
        assert!(parsed.is_err());
    }

    #[test]
    fn generic_and_explicit_movie_modes_do_not_enable_movement_stop_guard() {
        let generic = parse_and_apply(&["aether", "--generic"]);
        assert!(!generic.aether_aqw_movement_stop_guard);

        let movie = parse_and_apply(&["aether", "https://example.invalid/movie.swf"]);
        assert!(!movie.aether_aqw_movement_stop_guard);
    }
}
