//! Low-overhead runtime switches for measured Aether optimizations.

use std::sync::atomic::{AtomicBool, Ordering};

static AVM2_BROADCAST_FAST_PATH: AtomicBool = AtomicBool::new(false);
static FILTERLESS_HOT_CACHE_BYPASS: AtomicBool = AtomicBool::new(false);
static ADAPTIVE_AVATAR_CACHE: AtomicBool = AtomicBool::new(false);
static CACHE_TEXTURE_GRID: AtomicBool = AtomicBool::new(false);
static IDLE_GPU_UPLOAD_EVICTION: AtomicBool = AtomicBool::new(false);
static CACHE_VIEWPORT_CLIP: AtomicBool = AtomicBool::new(true);
static BITMAP_CACHE: AtomicBool = AtomicBool::new(true);

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

/// Confine a cache that is several times the viewport to the part of it that can be seen.
///
/// Defaults on, because leaving it off costs 627 megapixels a second of offscreen drawing on AQW's
/// map layers. It exists as a switch only so that the world map flicker can be bisected: both this
/// and [`filterless_hot_cache_bypass_enabled`] change what a huge, frequently invalidated cache
/// draws, and the map is the object both were written for.
#[inline]
pub fn set_cache_viewport_clip_enabled(enabled: bool) {
    CACHE_VIEWPORT_CLIP.store(enabled, Ordering::Relaxed);
}

#[inline]
pub fn cache_viewport_clip_enabled() -> bool {
    CACHE_VIEWPORT_CLIP.load(Ordering::Relaxed)
}

/// Honour `cacheAsBitmap` at all.
///
/// Defaults on: this is ordinary Flash behaviour, not an Aether optimization, and turning it off
/// makes the game draw everything from vectors every frame. It is switchable purely as the broad
/// half of a bisect, to answer "is this the cache subsystem or not" in one run rather than by
/// eliminating behaviours inside it one at a time.
#[inline]
pub fn set_bitmap_cache_enabled(enabled: bool) {
    BITMAP_CACHE.store(enabled, Ordering::Relaxed);
}

#[inline]
pub fn bitmap_cache_enabled() -> bool {
    BITMAP_CACHE.load(Ordering::Relaxed)
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
    fn texture_pressure_makes_the_sweep_eager_but_respects_off() {
        // Balanced patience collapses to one missed sweep under pressure...
        assert_eq!(effective_sweep_limit(Some(15), true), Some(1));
        assert_eq!(effective_sweep_limit(Some(5), true), Some(1));
        // ...and stays untouched without it.
        assert_eq!(effective_sweep_limit(Some(15), false), Some(15));
        // Off is an explicit user choice, kept even under pressure: the allocation
        // budget bounds the damage without releasing anything behind their back.
        assert_eq!(effective_sweep_limit(None, true), None);
        assert_eq!(effective_sweep_limit(None, false), None);
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

/// How many idle `cacheAsBitmap` surfaces have been released.
///
/// Reported by the memory census. These are invisible in every other counter: the texture pools
/// show them as checked out and the library census cannot see them at all, so without this there
/// is no way to tell an effective sweep from one that never fires.
static BITMAP_CACHES_SWEPT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn note_bitmap_caches_swept(count: usize) {
    if count > 0 {
        BITMAP_CACHES_SWEPT.fetch_add(count, std::sync::atomic::Ordering::Relaxed);
    }
}

pub fn bitmap_caches_swept() -> usize {
    BITMAP_CACHES_SWEPT.load(std::sync::atomic::Ordering::Relaxed)
}

/// How long an idle `cacheAsBitmap` surface is kept before its GPU texture is released.
///
/// A cache only goes idle when its object stops being *rendered* -- removed from the render list,
/// made invisible, orphaned, or left behind by a map change. Occlusion does not count: something
/// hidden behind a panel is still drawn. In AQW that makes the idle set closed UI panels, players
/// who have left, and the previous room's content, and those differ in one way that decides the
/// default: a panel is often reopened within seconds, a departed player never comes back.
///
/// So the default keeps a surface across a stretch of stillness and releases it after that.
///
/// **These timings started far too short, and the reason is worth keeping.** The first cut was
/// 2/6/16 seconds, reasoning that "a panel is often reopened within seconds". Two things were
/// wrong with it.
///
/// The first is that seconds is not the scale a player browses on. Clicking through armours in a
/// shop, coming back to one you looked at eight seconds ago is completely ordinary, and at six
/// seconds it had already been released.
///
/// The second is that the rebuild was priced as "one rebuild from the display list", and it is
/// worse than that. A cached object's shapes are only ever drawn during a rebuild, so between
/// rebuilds nothing draws them -- and `MovieLibrary::sweep_idle_gpu_uploads`, which runs on this
/// same timer with no reprieve at all, drops a `Graphic`'s tessellated `ShapeHandle` after a
/// single idle sweep. So by the time a cache surface is released, the shapes it would be rebuilt
/// from are already cold, and the rebuild pays re-tessellation for every one of them. A crowded
/// AQW map holds ~30,000 graphics against 36 bitmaps, so that is the dominant cost and it was not
/// in the original accounting at all.
///
/// Releasing therefore costs a full cold rebuild -- re-tessellate, render offscreen, re-run the
/// filter chain -- the next time the object draws. Keeping costs a texture, measured at 2.25 GB
/// across 9,324 live textures in a crowded room, back when filter intermediates were also churning
/// 781 GB. That churn is fixed and peak resident GPU memory fell from 5.60 GB to 1.98 GB, so the
/// pressure that justified being aggressive here is largely gone.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BitmapCacheSweep {
    /// Never release. What every build before this did.
    Off,
    /// Thirty sweeps, about a minute.
    Relaxed,
    /// Fifteen sweeps, about thirty seconds.
    #[default]
    Balanced,
    /// Five sweeps, about ten seconds.
    Eager,
}

impl BitmapCacheSweep {
    /// Every choice, in the order they are offered.
    pub const ALL: [Self; 4] = [Self::Off, Self::Relaxed, Self::Balanced, Self::Eager];

    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Relaxed => "relaxed",
            Self::Balanced => "balanced",
            Self::Eager => "eager",
        }
    }

    /// What the options menu calls it, with the delay spelled out.
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off (never release)",
            Self::Relaxed => "Relaxed (about a minute)",
            Self::Balanced => "Balanced (about 30 seconds)",
            Self::Eager => "Eager (about 10 seconds)",
        }
    }

    /// Consecutive sweeps a surface may go undrawn before it is released, or `None` for never.
    ///
    /// Multiply by `GPU_UPLOAD_SWEEP_INTERVAL` (two seconds) for the wall-clock delay.
    pub fn sweeps_before_release(self) -> Option<u8> {
        match self {
            Self::Off => None,
            Self::Relaxed => Some(30),
            Self::Balanced => Some(15),
            Self::Eager => Some(5),
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|choice| choice.name().eq_ignore_ascii_case(name.trim()))
    }
}

impl std::str::FromStr for BitmapCacheSweep {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::from_name(text).ok_or(())
    }
}

impl std::fmt::Display for BitmapCacheSweep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

static BITMAP_CACHE_SWEEP: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(2);

pub fn set_bitmap_cache_sweep(choice: BitmapCacheSweep) {
    let index = BitmapCacheSweep::ALL
        .iter()
        .position(|candidate| *candidate == choice)
        .unwrap_or(2) as u8;
    BITMAP_CACHE_SWEEP.store(index, std::sync::atomic::Ordering::Relaxed);
}

/// The sweep patience actually applied, given whether the renderer is over its texture budget.
///
/// Under pressure an idle surface is released at the first sweep it misses -- roughly two to
/// four seconds -- because the alternative, measured on a 4 GB card, is VRAM spilling over
/// PCIe and taking the whole machine with it. `Off` stays `Off`: someone who chose never to
/// release keeps that choice, and the allocation budget still bounds the damage on its own.
pub fn effective_sweep_limit(configured: Option<u8>, over_texture_budget: bool) -> Option<u8> {
    match (configured, over_texture_budget) {
        (Some(limit), true) => Some(limit.min(1)),
        (configured, _) => configured,
    }
}

pub fn bitmap_cache_sweep() -> BitmapCacheSweep {
    let index = BITMAP_CACHE_SWEEP.load(std::sync::atomic::Ordering::Relaxed) as usize;
    BitmapCacheSweep::ALL
        .get(index)
        .copied()
        .unwrap_or(BitmapCacheSweep::Balanced)
}

#[cfg(test)]
mod bitmap_cache_sweep_tests {
    use super::*;

    /// The menu spells the delay out in words, and the policy counts sweeps. Those are two places
    /// to say the same thing, which is exactly how the menu came to offer "about 6 seconds" for a
    /// policy that had been retuned to thirty.
    #[test]
    fn every_label_states_the_delay_its_sweep_count_actually_produces() {
        const SWEEP_SECONDS: u32 = 2;
        for choice in BitmapCacheSweep::ALL {
            let label = choice.label();
            let Some(sweeps) = choice.sweeps_before_release() else {
                assert_eq!(choice, BitmapCacheSweep::Off);
                assert!(
                    label.contains("never"),
                    "{label:?} does not say that it never releases"
                );
                continue;
            };
            let seconds = u32::from(sweeps) * SWEEP_SECONDS;
            let stated = match seconds {
                60 => "a minute".to_string(),
                other => format!("{other} seconds"),
            };
            assert!(
                label.contains(&stated),
                "{label:?} does not state {stated:?}, which is what {sweeps} sweeps comes to"
            );
        }
    }

    /// Off keeps everything and Eager gives up soonest, with the rest in between. The menu lists
    /// them in this order, so a reordering that broke it would be quietly confusing.
    #[test]
    fn the_choices_run_from_most_patient_to_least() {
        let retention: Vec<u16> = BitmapCacheSweep::ALL
            .into_iter()
            .map(|choice| choice.sweeps_before_release().map_or(u16::MAX, u16::from))
            .collect();
        assert!(
            retention.windows(2).all(|pair| pair[0] > pair[1]),
            "{retention:?} is not strictly decreasing"
        );
    }

    /// Rebuilding is not cheap -- a released surface usually has to re-tessellate its shapes first,
    /// because `sweep_idle_gpu_uploads` drops those after a single idle sweep. Ten seconds is
    /// already short for that; anything less is the six-second default that made shop items take a
    /// beat to reappear.
    #[test]
    fn even_the_most_eager_choice_outlasts_a_glance_away() {
        assert!(
            BitmapCacheSweep::Eager.sweeps_before_release() >= Some(5),
            "Eager releases sooner than ten seconds"
        );
        assert_eq!(BitmapCacheSweep::default(), BitmapCacheSweep::Balanced);
    }

    /// The stored index and the list have to agree, or a saved preference reads back as a
    /// different policy than the one chosen.
    #[test]
    fn every_choice_survives_a_round_trip_through_the_global() {
        for choice in BitmapCacheSweep::ALL {
            set_bitmap_cache_sweep(choice);
            assert_eq!(bitmap_cache_sweep(), choice);
            assert_eq!(BitmapCacheSweep::from_name(choice.name()), Some(choice));
        }
        set_bitmap_cache_sweep(BitmapCacheSweep::default());
    }
}

/// Sprites skipped during a re-preload because the character was already defined.
///
/// Only a deduped movie reaches this: sharing an `Arc<SwfMovie>` by URL also shares its populated
/// `MovieLibrary`, so the second load's preload walks the same `DefineSprite` tags against
/// characters that already exist. Worth counting because this path used to end the preload pass on
/// every one of them, and `Player::run_frame` pumps preload once per frame -- so a few hundred
/// sprites in a class or armour was several seconds before it appeared. A large number here with no
/// visible delay is the fix working; a large number WITH a delay means something else stalls too.
static PRELOAD_SPRITES_ALREADY_DEFINED: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

pub fn note_preload_sprite_already_defined() {
    PRELOAD_SPRITES_ALREADY_DEFINED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

pub fn preload_sprites_already_defined() -> usize {
    PRELOAD_SPRITES_ALREADY_DEFINED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Weak-keyed `flash.utils.Dictionary` instances created, and dead keys swept out of them.
///
/// `new Dictionary(true)` used to be answered with a stub warning and strong keys, so both of these
/// were structurally zero. AQW keeps two *static* weak dictionaries in `fl.core.UIComponent` keyed
/// on the component itself, plus more in `StyleManager`, `FocusManager` and `World`; with the keys
/// stored strongly, every UI object ever registered stayed reachable, and through its class and
/// application domain so did the movie it came from.
///
/// Reported by the memory census. `created` says the path is being taken at all; `object-keyed`
/// says how many of those dictionaries the flag can actually do anything for, because `weakKeys`
/// weakens object keys only and several of AQW's are keyed by uid or name; `pruned` says the keys
/// are really dying, which is the thing that was impossible before.
static WEAK_DICTIONARIES_CREATED: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
static WEAK_DICTIONARIES_WITH_OBJECT_KEYS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
static DICTIONARY_KEYS_PRUNED: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

pub fn note_weak_dictionary_created() {
    WEAK_DICTIONARIES_CREATED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

pub fn weak_dictionaries_created() -> usize {
    WEAK_DICTIONARIES_CREATED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Called once per dictionary, the first time it is given a key that is actually held weakly.
pub fn note_weak_dictionary_took_an_object_key() {
    WEAK_DICTIONARIES_WITH_OBJECT_KEYS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

pub fn weak_dictionaries_with_object_keys() -> usize {
    WEAK_DICTIONARIES_WITH_OBJECT_KEYS.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn note_dictionary_keys_pruned(count: usize) {
    if count > 0 {
        DICTIONARY_KEYS_PRUNED.fetch_add(count, std::sync::atomic::Ordering::Relaxed);
    }
}

pub fn dictionary_keys_pruned() -> usize {
    DICTIONARY_KEYS_PRUNED.load(std::sync::atomic::Ordering::Relaxed)
}
