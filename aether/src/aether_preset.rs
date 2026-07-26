use crate::cli::Opt;
use anyhow::{Context, Result};
use ruffle_core::LoadBehavior;
use ruffle_core::backend::navigator::SocketMode;
use ruffle_core::config::Letterbox;
use ruffle_render::quality::StageQuality;
use ruffle_render_wgpu::clap::PowerPreference;
use ruffle_render_wgpu::texture_pool_policy::BoundedTexturePoolLimits;
use url::Url;

pub const AQW_LOADER_URL: &str = "https://game.aq.com/game/gamefiles/Loader_Spider.swf";
pub const AQW_BASE_URL: &str = "https://game.aq.com/game/gamefiles/";
pub const AQW_PAGE_URL: &str = "https://game.aq.com/";
pub const AQW_OFFSCREEN_TEXTURE_POOL_LIMITS: BoundedTexturePoolLimits = BoundedTexturePoolLimits {
    max_cached_bytes: 128 * 1024 * 1024,
    max_cached_entries: 512,
    max_idle_frames: 1,
    max_cached_globals: 128,
};

/// Apply conservative AQW defaults while preserving explicit CLI overrides.
///
/// Native Ruffle can open TCP sockets directly. Aether therefore does not use Aquastar's
/// Electron/WebSocket relay. Socket access is enabled only for the dedicated AQW preset; generic
/// Ruffle mode retains the normal Ruffle behavior.
pub fn apply(opt: &mut Opt) -> Result<bool> {
    if opt.generic || opt.movie_url.is_some() {
        return Ok(false);
    }

    opt.aether_aqw_timeline_child_rebind = !opt.no_aether_aqw_timeline_child_rebind;
    opt.aether_aqw_mouse_motion_coalescing = !opt.no_aether_aqw_mouse_motion_coalescing;
    opt.aether_aqw_avm2_broadcast_fast_path = !opt.no_aether_aqw_avm2_broadcast_fast_path;
    opt.aether_aqw_cache_hit_fast_path = !opt.no_aether_aqw_cache_hit_fast_path;
    opt.aether_aqw_adaptive_avatar_cache = !opt.no_aether_aqw_adaptive_avatar_cache;
    opt.aether_aqw_movement_stop_guard = !opt.no_aether_aqw_movement_stop_guard;

    // Retain exact-sized offscreen surfaces long enough for repeating equipment and combat-filter
    // animation cycles to reuse them. The 256 MiB / 2,048-entry hard limits remain authoritative,
    // so extending the age window reduces allocation churn without restoring unbounded lifetime
    // accumulation.
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
    fn aqw_preset_enables_bitmap_cache_hit_fast_path_by_default() {
        let opt = parse_and_apply(&["aether"]);
        assert!(opt.aether_aqw_cache_hit_fast_path);
    }

    #[test]
    fn aqw_preset_allows_bitmap_cache_hit_fast_path_opt_out() {
        let opt = parse_and_apply(&["aether", "--no-aether-aqw-cache-hit-fast-path"]);
        assert!(!opt.aether_aqw_cache_hit_fast_path);
    }

    #[test]
    fn aqw_preset_accepts_retained_positive_bitmap_cache_hit_fast_path_flag() {
        let opt = parse_and_apply(&["aether", "--aether-aqw-cache-hit-fast-path"]);
        assert!(opt.aether_aqw_cache_hit_fast_path);
    }

    #[test]
    fn bitmap_cache_hit_fast_path_flags_conflict() {
        let parsed = Opt::try_parse_from([
            "aether",
            "--aether-aqw-cache-hit-fast-path",
            "--no-aether-aqw-cache-hit-fast-path",
        ]);
        assert!(parsed.is_err());
    }

    #[test]
    fn generic_and_explicit_movies_do_not_enable_bitmap_cache_hit_fast_path() {
        let generic = parse_and_apply(&["aether", "--generic"]);
        assert!(!generic.aether_aqw_cache_hit_fast_path);

        let movie = parse_and_apply(&["aether", "https://example.invalid/movie.swf"]);
        assert!(!movie.aether_aqw_cache_hit_fast_path);
    }

    #[test]
    fn aqw_preset_enables_adaptive_avatar_cache_by_default() {
        let opt = parse_and_apply(&["aether"]);
        assert!(opt.aether_aqw_adaptive_avatar_cache);
    }

    #[test]
    fn aqw_preset_allows_adaptive_avatar_cache_opt_out() {
        let opt = parse_and_apply(&["aether", "--no-aether-aqw-adaptive-avatar-cache"]);
        assert!(!opt.aether_aqw_adaptive_avatar_cache);
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
        assert_eq!(AQW_OFFSCREEN_TEXTURE_POOL_LIMITS.max_idle_frames, 1);
        assert!(AQW_OFFSCREEN_TEXTURE_POOL_LIMITS.max_cached_bytes <= 128 * 1024 * 1024);
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
        let opt = parse_and_apply(&["aether", "--no-aether-aqw-mouse-motion-coalescing"]);
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
        let opt = parse_and_apply(&["aether", "--frame-rate", "48"]);
        assert_eq!(opt.frame_rate, Some(48.0));
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
