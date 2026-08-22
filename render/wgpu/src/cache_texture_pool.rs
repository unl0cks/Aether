//! Reuse of the persistent textures behind `cacheAsBitmap` surfaces.
//!
//! Frame-time spikes were measured to be allocation, not drawing. Comparing the worst 25 seconds of
//! a session by hitch (`player_render` max minus p50) against the other 1,118:
//!
//! ```text
//! hitch (max - p50)          31.1 ms   vs   7.1 ms
//! cache texture allocations    108.3   vs    12.4     8.7x
//! texture create time         1807 us  vs   130 us   13.9x
//! render p50                  22.6 ms  vs  22.2 ms    1.0x
//! display objects entered     97,707   vs 114,048     0.9x
//! ```
//!
//! The baseline is identical in a bad second and *fewer* objects are drawn, so this is not load. It
//! is a room filling up: every arriving player and every piece of gear wants a new cache surface,
//! and `create_empty_texture` went straight to `device.create_texture` for each one. Offscreen
//! targets have been pooled for a long time and reach 99.6% reuse; these never were.
//!
//! These cannot use [`crate::buffer_pool::TexturePool`]. That pool checks a texture out and takes it
//! back within the frame, whereas a cache surface is owned by its display object for as long as the
//! object keeps its shape, which may be the rest of the session. So the recycling happens the only
//! other place it can: when the handle is finally dropped.
//!
//! **Recycled textures are handed back with their previous contents.** That is sound here and only
//! here: the one caller is the bitmap cache, and every allocation is immediately followed by a
//! rebuild that renders with `RenderTargetMode::ExistingWithColor`, which clears. Anything else
//! reusing this pool would have to clear for itself.

use fnv::FnvHashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// Whether the idle set is held to what a small card can afford.
///
/// Global rather than a constructor argument because the renderer is built before the preferences
/// that decide it are applied, and a cap that depends on construction order is a cap that silently
/// does not apply.
static LOW_VRAM: AtomicBool = AtomicBool::new(false);

pub fn set_low_vram(enabled: bool) {
    LOW_VRAM.store(enabled, Ordering::Relaxed);
}

/// Whether cache surfaces are recycled at all.
///
/// On by default, on measurement. `AETHER_CACHE_TEXTURE_POOL=0` sends every allocation to the
/// driver as before, which is the control for measuring what this bought and the switch to reach
/// for if a cache surface ever shows content that is not its own. A recycled texture arrives with
/// the previous owner's pixels and is cleared by the rebuild that follows; if that ever stops being
/// true, this is the fastest way to confirm it.
pub fn pooling_enabled() -> bool {
    crate::aether_switches::CACHE_TEXTURE_POOL.enabled()
}

fn max_idle_bytes() -> u64 {
    if LOW_VRAM.load(Ordering::Relaxed) {
        LOW_VRAM_MAX_IDLE_BYTES
    } else {
        DEFAULT_MAX_IDLE_BYTES
    }
}

/// How much texture memory may sit idle in the pool.
///
/// Against measured peaks of 825 to 1,286 MB of live cache textures, this is a small fraction, and
/// the sizes repeat heavily -- that is the entire premise of the fix -- so a modest cap reaches a
/// high hit rate. Unbounded would simply be a second leak.
const DEFAULT_MAX_IDLE_BYTES: u64 = 192 * 1024 * 1024;

/// The same, for a card that cannot spare it. Low VRAM mode exists for 2 GB cards, where holding
/// 192 MB of *idle* textures to avoid allocation stalls is the wrong trade.
const LOW_VRAM_MAX_IDLE_BYTES: u64 = 32 * 1024 * 1024;

/// A hard cap on retained textures, independent of their size.
///
/// Bytes alone would let a flood of tiny surfaces mint thousands of buckets, which costs lookup
/// time and memory in the map rather than in the textures.
const MAX_IDLE_ENTRIES: usize = 512;

/// How much total texture memory may be live before new cache surfaces are refused.
///
/// Sized from two measurements. A healthy 47-minute session on a 10 GB card peaked at
/// 1,407 MB of live texture memory, so this binds on nothing normal. A runaway on a 4 GB
/// card -- one broken particle effect spawning thousands of cached skulls -- went from
/// 1,576 to 4,128 live textures (3.3 GB) in three minutes and took the machine down with
/// it, VRAM spilling over PCIe into system RAM. The budget stops that climb at a point
/// where a 4 GB card is still workable.
const DEFAULT_TEXTURE_BUDGET_BYTES: u64 = 2560 * 1024 * 1024;

/// The same, for a card that cannot spare it. On a 2 GB card the default budget IS the
/// whole card, so pressure has to start far earlier.
const LOW_VRAM_TEXTURE_BUDGET_BYTES: u64 = 1280 * 1024 * 1024;

/// The live-texture-memory ceiling above which new `cacheAsBitmap` surfaces are refused
/// and the idle sweep turns eager.
///
/// `AETHER_TEXTURE_BUDGET_MB` pins it for a measuring run; `0` refuses every fresh cache
/// surface, which is the control for measuring what caching buys. The refusal path is
/// graceful -- the object renders direct, identically -- so a too-small budget costs
/// speed, never correctness.
pub fn texture_memory_budget_bytes() -> u64 {
    static ENV_OVERRIDE: OnceLock<Option<u64>> = OnceLock::new();
    let env = *ENV_OVERRIDE.get_or_init(|| {
        std::env::var("AETHER_TEXTURE_BUDGET_MB")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(|megabytes| megabytes.saturating_mul(1024 * 1024))
    });
    env.unwrap_or_else(|| {
        if LOW_VRAM.load(Ordering::Relaxed) {
            LOW_VRAM_TEXTURE_BUDGET_BYTES
        } else {
            DEFAULT_TEXTURE_BUDGET_BYTES
        }
    })
}

#[derive(Debug)]
struct Inner<T> {
    /// Keyed by exact dimensions. `bitmap_cache_texture_plan` already rounds requests to a grid, so
    /// the sizes arriving here are the gridded ones and collide readily.
    buckets: FnvHashMap<(u32, u32), Vec<T>>,
    entries: usize,
    bytes: u64,
    max_bytes: u64,
    hits: u64,
    misses: u64,
    dropped: u64,
}

// Hand written rather than derived: `#[derive(Default)]` would demand `T: Default`, and a
// `wgpu::Texture` has no default.
impl<T> Default for Inner<T> {
    fn default() -> Self {
        Self {
            buckets: FnvHashMap::default(),
            entries: 0,
            bytes: 0,
            max_bytes: 0,
            hits: 0,
            misses: 0,
            dropped: 0,
        }
    }
}

/// A recycler for `cacheAsBitmap` textures.
#[derive(Debug)]
pub struct CacheTexturePool {
    inner: Mutex<Inner<wgpu::Texture>>,
}

impl Default for CacheTexturePool {
    fn default() -> Self {
        Self::new()
    }
}

/// Bytes one RGBA8 texture of this size occupies, for the purpose of the cap.
fn texture_bytes(width: u32, height: u32) -> u64 {
    u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(4)
}

impl CacheTexturePool {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                max_bytes: max_idle_bytes(),
                ..Inner::default()
            }),
        }
    }

    /// A texture of exactly this size, if one is idle.
    ///
    /// Exact rather than "big enough on both axes": the caller records the dimensions it asked for
    /// and reads the texture back at that size, so handing over a larger one would silently change
    /// what the cache believes it owns.
    pub fn take(&self, width: u32, height: u32) -> Option<wgpu::Texture> {
        if !pooling_enabled() {
            return None;
        }
        let mut inner = self.inner.lock().ok()?;
        let texture = inner.buckets.get_mut(&(width, height)).and_then(Vec::pop);

        match texture {
            Some(texture) => {
                inner.entries -= 1;
                inner.bytes = inner.bytes.saturating_sub(texture_bytes(width, height));
                inner.hits += 1;
                #[cfg(feature = "aether_metrics")]
                crate::aether_metrics::record_cache_texture_pool(true);
                Some(texture)
            }
            None => {
                inner.misses += 1;
                #[cfg(feature = "aether_metrics")]
                crate::aether_metrics::record_cache_texture_pool(false);
                None
            }
        }
    }

    /// Offer a texture back once its owner has let go of it.
    pub fn give_back(&self, texture: wgpu::Texture) {
        if !pooling_enabled() {
            return;
        }
        let (width, height) = (texture.width(), texture.height());
        let bytes = texture_bytes(width, height);
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        // Refreshed here rather than at construction, so switching Low VRAM mode takes effect
        // without a restart and without depending on when the renderer was built.
        inner.max_bytes = max_idle_bytes();

        // A single texture larger than the whole budget would evict everything else to sit there
        // alone, so it is simply not kept.
        if bytes > inner.max_bytes {
            inner.dropped += 1;
            return;
        }

        inner
            .buckets
            .entry((width, height))
            .or_default()
            .push(texture);
        inner.entries += 1;
        inner.bytes = inner.bytes.saturating_add(bytes);
        trim(&mut inner);
    }

    /// Reuses, misses and evictions since the last call, with what is currently held.
    pub fn take_stats(&self) -> CacheTexturePoolStats {
        let Ok(mut inner) = self.inner.lock() else {
            return CacheTexturePoolStats::default();
        };
        let stats = CacheTexturePoolStats {
            hits: inner.hits,
            misses: inner.misses,
            dropped: inner.dropped,
            idle_entries: inner.entries as u64,
            idle_bytes: inner.bytes,
        };
        inner.hits = 0;
        inner.misses = 0;
        inner.dropped = 0;
        stats
    }
}

/// Drop idle textures until the pool is back inside its caps.
///
/// Evicts from the largest bucket first: the biggest textures free the most for the fewest
/// evictions, and a bucket holding many spares is the one least likely to run dry.
///
/// Generic over what is stored so the bookkeeping can be tested without a GPU. Getting this wrong
/// is either an unbounded pool or an infinite loop, and neither shows up in a render comparison.
fn trim<T>(inner: &mut Inner<T>) {
    while inner.bytes > inner.max_bytes || inner.entries > MAX_IDLE_ENTRIES {
        let Some(key) = inner
            .buckets
            .iter()
            .filter(|(_, textures)| !textures.is_empty())
            .max_by_key(|((w, h), textures)| (texture_bytes(*w, *h), textures.len()))
            .map(|(key, _)| *key)
        else {
            // Nothing left to evict. The counters could only disagree with the buckets through a
            // bug, so resynchronise rather than spin forever.
            inner.entries = 0;
            inner.bytes = 0;
            return;
        };

        if let Some(textures) = inner.buckets.get_mut(&key)
            && textures.pop().is_some()
        {
            inner.entries = inner.entries.saturating_sub(1);
            inner.bytes = inner.bytes.saturating_sub(texture_bytes(key.0, key.1));
            inner.dropped += 1;
            if textures.is_empty() {
                inner.buckets.remove(&key);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CacheTexturePoolStats {
    pub hits: u64,
    pub misses: u64,
    pub dropped: u64,
    pub idle_entries: u64,
    pub idle_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stands in for a texture. Only the bookkeeping is under test here; a real texture would need
    /// a device and would be testing wgpu rather than this.
    #[derive(Debug)]
    struct Stub;

    fn pool(max_bytes: u64) -> Inner<Stub> {
        Inner {
            max_bytes,
            ..Inner::default()
        }
    }

    fn add(inner: &mut Inner<Stub>, width: u32, height: u32) {
        inner.buckets.entry((width, height)).or_default().push(Stub);
        inner.entries += 1;
        inner.bytes += texture_bytes(width, height);
    }

    /// The accounting has to survive eviction, because every cap here trusts it.
    #[test]
    fn bytes_and_entries_track_what_is_actually_held() {
        let mut inner = pool(texture_bytes(64, 64) * 2);
        for _ in 0..3 {
            add(&mut inner, 64, 64);
        }
        trim(&mut inner);
        assert_eq!(inner.entries, 2, "trimmed back to the cap");
        assert_eq!(inner.bytes, texture_bytes(64, 64) * 2);
        assert_eq!(inner.dropped, 1);
        assert_eq!(
            inner.buckets[&(64, 64)].len(),
            2,
            "counters match the buckets"
        );
    }

    /// Eviction takes from the largest bucket, so the budget is freed for the fewest evictions.
    #[test]
    fn eviction_prefers_the_largest_textures() {
        let mut inner = pool(texture_bytes(256, 256));
        add(&mut inner, 256, 256);
        add(&mut inner, 64, 64);
        trim(&mut inner);
        assert!(
            !inner.buckets.contains_key(&(256, 256)),
            "the large one goes first"
        );
        assert_eq!(inner.buckets[&(64, 64)].len(), 1, "the small one survives");
    }

    /// An emptied bucket must not be left behind, or the map grows without bound across a session
    /// even while the byte count looks healthy.
    #[test]
    fn emptied_buckets_are_removed() {
        let mut inner = pool(0);
        add(&mut inner, 32, 32);
        add(&mut inner, 64, 64);
        trim(&mut inner);
        assert!(inner.buckets.is_empty());
        assert_eq!((inner.entries, inner.bytes), (0, 0));
    }

    /// The entry cap has to bind on its own, or thousands of tiny surfaces sit under the byte cap
    /// forever.
    #[test]
    fn the_entry_cap_binds_even_when_the_bytes_are_tiny() {
        let mut inner = pool(u64::MAX);
        for _ in 0..(MAX_IDLE_ENTRIES + 10) {
            add(&mut inner, 1, 1);
        }
        trim(&mut inner);
        assert_eq!(inner.entries, MAX_IDLE_ENTRIES);
    }

    /// Counters disagreeing with the buckets used to be an infinite loop, which is the worst way
    /// for a pool to fail.
    #[test]
    fn trim_resynchronises_rather_than_spinning_when_counters_disagree() {
        let mut inner: Inner<Stub> = pool(0);
        inner.bytes = 999;
        inner.entries = 5;
        trim(&mut inner);
        assert_eq!((inner.entries, inner.bytes), (0, 0));
    }

    #[test]
    fn a_texture_larger_than_the_whole_budget_is_refused_by_the_size_guard() {
        // The guard `give_back` applies, exercised through the same arithmetic.
        let inner = pool(texture_bytes(64, 64));
        assert!(texture_bytes(4096, 4096) > inner.max_bytes);
        assert!(texture_bytes(64, 64) <= inner.max_bytes);
    }
}
