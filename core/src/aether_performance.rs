//! Low-overhead runtime switches for measured Aether optimizations.

use std::sync::atomic::{AtomicBool, Ordering};

static AVM2_BROADCAST_FAST_PATH: AtomicBool = AtomicBool::new(false);
static FILTERLESS_HOT_CACHE_BYPASS: AtomicBool = AtomicBool::new(false);
static ADAPTIVE_AVATAR_CACHE: AtomicBool = AtomicBool::new(false);
static CACHE_TEXTURE_GRID: AtomicBool = AtomicBool::new(false);
static IDLE_GPU_UPLOAD_EVICTION: AtomicBool = AtomicBool::new(false);

#[inline]
pub fn set_avm2_broadcast_fast_path_enabled(enabled: bool) {
    AVM2_BROADCAST_FAST_PATH.store(enabled, Ordering::Relaxed);
}

#[inline]
pub fn avm2_broadcast_fast_path_enabled() -> bool {
    AVM2_BROADCAST_FAST_PATH.load(Ordering::Relaxed)
}

/// Enable adaptive direct rendering for explicit, filterless bitmap caches that are invalidated
/// on every rendered frame. This does not skip authored frames or reduce animation cadence.
#[inline]
pub fn set_filterless_hot_cache_bypass_enabled(enabled: bool) {
    FILTERLESS_HOT_CACHE_BYPASS.store(enabled, Ordering::Relaxed);
}

#[inline]
pub fn filterless_hot_cache_bypass_enabled() -> bool {
    FILTERLESS_HOT_CACHE_BYPASS.load(Ordering::Relaxed)
}

/// Enable internal bitmap caching for exact AQW `AvatarMC` roots after their complete visual
/// subtree has remained clean for several rendered frames.
#[inline]
pub fn set_adaptive_avatar_cache_enabled(enabled: bool) {
    ADAPTIVE_AVATAR_CACHE.store(enabled, Ordering::Relaxed);
}

#[inline]
pub fn adaptive_avatar_cache_enabled() -> bool {
    ADAPTIVE_AVATAR_CACHE.load(Ordering::Relaxed)
}

/// Round cache texture sizes up to a grid so an object whose bounds drift by a pixel per frame
/// keeps asking the texture pool for the same size instead of a new one every frame.
#[inline]
pub fn set_cache_texture_grid_enabled(enabled: bool) {
    CACHE_TEXTURE_GRID.store(enabled, Ordering::Relaxed);
}

#[inline]
pub fn cache_texture_grid_enabled() -> bool {
    CACHE_TEXTURE_GRID.load(Ordering::Relaxed)
}

/// Periodically drop GPU uploads for bitmaps that are no longer being drawn.
///
/// Uploads are lazy and, without this, were only released when a `Loader` was explicitly
/// unloaded -- which AQW never does, so every bitmap ever drawn stayed resident.
#[inline]
pub fn set_idle_gpu_upload_eviction_enabled(enabled: bool) {
    IDLE_GPU_UPLOAD_EVICTION.store(enabled, Ordering::Relaxed);
}

#[inline]
pub fn idle_gpu_upload_eviction_enabled() -> bool {
    IDLE_GPU_UPLOAD_EVICTION.load(Ordering::Relaxed)
}

/// Match the exact public AQW avatar class without allocating a qualified class name or parsing
/// the movie URL on the display-list hot path.
pub fn is_aqw_avatar_cache_candidate(
    movie_url: &str,
    class_local_name: &[u8],
    is_public_namespace: bool,
) -> bool {
    if !is_public_namespace || class_local_name != b"AvatarMC" {
        return false;
    }

    crate::aether_movie::is_aqw_game_movie(movie_url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn performance_switches_are_independent() {
        set_avm2_broadcast_fast_path_enabled(true);
        set_filterless_hot_cache_bypass_enabled(false);
        set_adaptive_avatar_cache_enabled(false);
        assert!(avm2_broadcast_fast_path_enabled());
        assert!(!filterless_hot_cache_bypass_enabled());
        assert!(!adaptive_avatar_cache_enabled());

        set_avm2_broadcast_fast_path_enabled(false);
        set_filterless_hot_cache_bypass_enabled(true);
        set_adaptive_avatar_cache_enabled(true);
        assert!(!avm2_broadcast_fast_path_enabled());
        assert!(filterless_hot_cache_bypass_enabled());
        assert!(adaptive_avatar_cache_enabled());

        set_filterless_hot_cache_bypass_enabled(false);
        set_adaptive_avatar_cache_enabled(false);
    }

    #[test]
    fn aqw_adaptive_avatar_candidate_requires_exact_public_class_and_spider_movie() {
        assert!(is_aqw_avatar_cache_candidate(
            "https://game.aq.com/game/gamefiles/spider.swf",
            b"AvatarMC",
            true,
        ));
        assert!(is_aqw_avatar_cache_candidate(
            "https://game.aq.com/game/gamefiles/SpIdEr.SwF?cache=1",
            b"AvatarMC",
            true,
        ));
        // The build the live loader picks, which is what AQW actually serves from 0.5.14 on.
        assert!(is_aqw_avatar_cache_candidate(
            "https://game.aq.com/game/gamefiles/Game3098r24.swf?ver=R0047",
            b"AvatarMC",
            true,
        ));
        // The loader defines no `AvatarMC`; it is the game's ancestor, not the game.
        assert!(!is_aqw_avatar_cache_candidate(
            "https://game.aq.com/game/gamefiles/Loader3.swf?ver=a",
            b"AvatarMC",
            true,
        ));

        assert!(!is_aqw_avatar_cache_candidate(
            "https://example.invalid/avatar.swf",
            b"AvatarMC",
            true,
        ));
        assert!(!is_aqw_avatar_cache_candidate(
            "https://game.aq.com/game/gamefiles/notspider.swf",
            b"AvatarMC",
            true,
        ));
        assert!(!is_aqw_avatar_cache_candidate(
            "https://game.aq.com/game/gamefiles/spider.swf.txt",
            b"AvatarMC",
            true,
        ));
        assert!(!is_aqw_avatar_cache_candidate(
            "https://game.aq.com/game/gamefiles/spider.swf",
            b"AvatarMCChild",
            true,
        ));
        assert!(!is_aqw_avatar_cache_candidate(
            "https://game.aq.com/game/gamefiles/spider.swf",
            b"AvatarMC",
            false,
        ));
    }
}
