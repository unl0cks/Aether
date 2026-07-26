//! Low-overhead runtime switches for measured Aether optimizations.

use std::sync::atomic::{AtomicBool, Ordering};

static BITMAP_CACHE_HIT_FAST_PATH: AtomicBool = AtomicBool::new(false);
static AVM2_BROADCAST_FAST_PATH: AtomicBool = AtomicBool::new(false);
static FILTERLESS_HOT_CACHE_BYPASS: AtomicBool = AtomicBool::new(false);
static ADAPTIVE_AVATAR_CACHE: AtomicBool = AtomicBool::new(false);

/// Enable the experimental clean bitmap-cache hit fast path.
///
/// The optimization reuses cache geometry recorded at the last rebuild when the cache is
/// still clean and its non-translation transform and stage scale are unchanged. This avoids
/// recursively recomputing render bounds and preparing filters for a cache hit.
#[inline]
pub fn set_bitmap_cache_hit_fast_path_enabled(enabled: bool) {
    BITMAP_CACHE_HIT_FAST_PATH.store(enabled, Ordering::Relaxed);
}

#[inline]
pub fn bitmap_cache_hit_fast_path_enabled() -> bool {
    BITMAP_CACHE_HIT_FAST_PATH.load(Ordering::Relaxed)
}

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

    let path = movie_url
        .split_once(['?', '#'])
        .map_or(movie_url, |(path, _)| path);
    let file_name = path
        .rsplit_once(['/', '\\'])
        .map_or(path, |(_, file_name)| file_name)
        .as_bytes();

    file_name.eq_ignore_ascii_case(b"spider.swf")
        || file_name
            .get(file_name.len().saturating_sub(b"_spider.swf".len())..)
            .is_some_and(|suffix| suffix.eq_ignore_ascii_case(b"_spider.swf"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn performance_switches_are_independent() {
        set_bitmap_cache_hit_fast_path_enabled(false);
        set_avm2_broadcast_fast_path_enabled(true);
        set_filterless_hot_cache_bypass_enabled(false);
        set_adaptive_avatar_cache_enabled(false);
        assert!(avm2_broadcast_fast_path_enabled());
        assert!(!bitmap_cache_hit_fast_path_enabled());
        assert!(!filterless_hot_cache_bypass_enabled());
        assert!(!adaptive_avatar_cache_enabled());

        set_avm2_broadcast_fast_path_enabled(false);
        set_bitmap_cache_hit_fast_path_enabled(true);
        set_filterless_hot_cache_bypass_enabled(true);
        set_adaptive_avatar_cache_enabled(true);
        assert!(!avm2_broadcast_fast_path_enabled());
        assert!(bitmap_cache_hit_fast_path_enabled());
        assert!(filterless_hot_cache_bypass_enabled());
        assert!(adaptive_avatar_cache_enabled());

        set_bitmap_cache_hit_fast_path_enabled(false);
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
            "https://game.aq.com/game/gamefiles/Loader_SpIdEr.SwF?cache=1",
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
