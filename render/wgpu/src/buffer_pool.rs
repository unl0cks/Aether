use crate::descriptors::Descriptors;
use crate::globals::Globals;
use crate::texture_pool_policy::{
    BoundedTexturePoolLimits, OffscreenTexturePoolPolicy, PoolMaintenanceReport,
    TextureRetentionCandidate, estimate_texture_bytes, plan_globals_retention,
    plan_texture_retention,
};
use fnv::FnvHashMap;
use std::fmt::{Debug, Formatter};
use std::ops::Deref;
use std::sync::{Arc, Mutex, Weak};

type PoolInner<T> = Mutex<Vec<T>>;
type Constructor<Type, Description> = Box<dyn Fn(&Descriptors, &Description) -> Type>;

#[derive(Debug)]
struct TextureBucket {
    pool: BufferPool<(wgpu::Texture, wgpu::TextureView), AlwaysCompatible>,
    bytes_per_entry: Option<u64>,
    last_used_frame: u64,
    /// Lifetime request count, used to rank this size against others last touched on the same
    /// frame. Without it, retention picks between them by hash-map iteration order.
    uses: u64,
}

#[derive(Debug)]
struct GlobalsEntry {
    globals: Arc<Globals>,
    last_used_frame: u64,
}

#[derive(Debug)]
pub struct TexturePool {
    pools: FnvHashMap<TextureKey, TextureBucket>,
    globals_cache: FnvHashMap<GlobalsKey, GlobalsEntry>,
    policy: OffscreenTexturePoolPolicy,
    submitted_frame: u64,
    #[cfg(feature = "aether_metrics")]
    metrics_kind: crate::aether_metrics::TexturePoolKind,
}

impl TexturePool {
    pub fn new(
        policy: OffscreenTexturePoolPolicy,
        #[cfg(feature = "aether_metrics")] metrics_kind: crate::aether_metrics::TexturePoolKind,
    ) -> Self {
        Self {
            pools: FnvHashMap::default(),
            globals_cache: FnvHashMap::default(),
            policy,
            submitted_frame: 0,
            #[cfg(feature = "aether_metrics")]
            metrics_kind,
        }
    }

    pub fn reset(&mut self) {
        #[cfg(feature = "aether_metrics")]
        crate::aether_metrics::record_pool_reset(
            self.metrics_kind,
            self.pools.values().fold(0_usize, |total, bucket| {
                total.saturating_add(bucket.pool.available_len())
            }),
        );

        let policy = self.policy;
        #[cfg(feature = "aether_metrics")]
        let metrics_kind = self.metrics_kind;
        *self = Self::new(
            policy,
            #[cfg(feature = "aether_metrics")]
            metrics_kind,
        );
    }

    pub fn get_texture(
        &mut self,
        descriptors: &Descriptors,
        size: wgpu::Extent3d,
        usage: wgpu::TextureUsages,
        format: wgpu::TextureFormat,
        sample_count: u32,
    ) -> PoolEntry<(wgpu::Texture, wgpu::TextureView), AlwaysCompatible> {
        let key = TextureKey {
            size,
            usage,
            format,
            sample_count,
        };
        let submitted_frame = self.submitted_frame;
        #[cfg(feature = "aether_metrics")]
        let metrics_kind = self.metrics_kind;
        let bucket = self.pools.entry(key).or_insert_with(|| {
            let label = if cfg!(feature = "render_debug_labels") {
                use std::sync::atomic::{AtomicU32, Ordering};
                static ID_COUNT: AtomicU32 = AtomicU32::new(0);
                let id = ID_COUNT.fetch_add(1, Ordering::Relaxed);
                create_debug_label!("Pooled texture {}", id)
            } else {
                None
            };
            TextureBucket {
                pool: BufferPool::new(Box::new(move |descriptors, _description| {
                    #[cfg(feature = "aether_metrics")]
                    let creation_started = std::time::Instant::now();
                    let texture = descriptors.device.create_texture(&wgpu::TextureDescriptor {
                        label: label.as_deref(),
                        size,
                        mip_level_count: 1,
                        sample_count,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        view_formats: &[format],
                        usage,
                    });
                    #[cfg(feature = "aether_metrics")]
                    crate::aether_metrics::record_texture_creation(creation_started.elapsed());
                    #[cfg(feature = "aether_metrics")]
                    crate::aether_metrics::record_texture_created(
                        crate::aether_metrics::TextureOrigin::Pool,
                        size.width,
                        size.height,
                        sample_count,
                        estimate_texture_bytes(size, format, 1, sample_count).unwrap_or(0),
                    );
                    let view = texture.create_view(&Default::default());
                    (texture, view)
                })),
                bytes_per_entry: estimate_texture_bytes(size, format, 1, sample_count),
                last_used_frame: submitted_frame,
                uses: 0,
            }
        });
        bucket.last_used_frame = submitted_frame;
        bucket.uses = bucket.uses.saturating_add(1);
        #[cfg(feature = "aether_metrics")]
        {
            let (entry, reused) = bucket.pool.take_with_reuse(descriptors, AlwaysCompatible);
            crate::aether_metrics::record_texture_request(
                metrics_kind,
                reused,
                bucket.bytes_per_entry,
            );
            entry
        }

        #[cfg(not(feature = "aether_metrics"))]
        {
            bucket.pool.take(descriptors, AlwaysCompatible)
        }
    }

    /// Globals for a target covering `[offset_x, offset_x + viewport_width]` by
    /// `[offset_y, offset_y + viewport_height]`. The offset is part of the cache key: two targets of
    /// the same size looking at different parts of the stage need different view matrices, and
    /// sharing one would draw every blend in the wrong place.
    pub fn get_globals(
        &mut self,
        descriptors: &Descriptors,
        offset_x: u32,
        offset_y: u32,
        viewport_width: u32,
        viewport_height: u32,
    ) -> Arc<Globals> {
        let submitted_frame = self.submitted_frame;
        let entry = self
            .globals_cache
            .entry(GlobalsKey {
                offset_x,
                offset_y,
                viewport_width,
                viewport_height,
            })
            .or_insert_with(|| GlobalsEntry {
                globals: Arc::new(Globals::new(
                    &descriptors.device,
                    &descriptors.bind_layouts.globals,
                    offset_x,
                    offset_y,
                    viewport_width,
                    viewport_height,
                )),
                last_used_frame: submitted_frame,
            });
        entry.last_used_frame = submitted_frame;
        entry.globals.clone()
    }

    pub(crate) fn finish_frame(&mut self) -> PoolMaintenanceReport {
        match self.policy {
            OffscreenTexturePoolPolicy::Ephemeral => {
                self.reset();
                PoolMaintenanceReport::default()
            }
            OffscreenTexturePoolPolicy::BoundedReuse(limits) => {
                self.submitted_frame = self.submitted_frame.saturating_add(1);
                self.maintain_bounded(limits)
            }
        }
    }

    fn maintain_bounded(&mut self, limits: BoundedTexturePoolLimits) -> PoolMaintenanceReport {
        let texture_keys: Vec<TextureKey> = self.pools.keys().copied().collect();
        let candidates: Vec<TextureRetentionCandidate> = texture_keys
            .iter()
            .map(|key| {
                let bucket = &self.pools[key];
                TextureRetentionCandidate {
                    last_used_frame: bucket.last_used_frame,
                    available_entries: bucket.pool.available_len(),
                    bytes_per_entry: bucket.bytes_per_entry,
                    uses: bucket.uses,
                }
            })
            .collect();
        let textures = plan_texture_retention(self.submitted_frame, limits, &candidates);
        for (key, keep) in texture_keys.iter().zip(&textures.keep_entries) {
            if let Some(bucket) = self.pools.get(key) {
                bucket.pool.truncate_available(*keep);
            }
        }
        // A bucket with nothing available is not necessarily a cold bucket -- it is also what the
        // hottest size in the frame looks like when every one of its textures is still checked out
        // at maintenance time. Dropping it here drops the `Arc` behind the free list, so the
        // `Weak` upgrade in `PoolEntry::drop` fails and those textures are destroyed instead of
        // returned, guaranteeing a fresh allocation next frame for exactly the size that is reused
        // the most. Keep buckets that are still within their idle window regardless.
        let current_frame = self.submitted_frame;
        self.pools.retain(|_, bucket| {
            bucket.pool.available_len() != 0
                || current_frame.saturating_sub(bucket.last_used_frame) <= limits.max_idle_frames
        });

        let globals_keys: Vec<GlobalsKey> = self.globals_cache.keys().copied().collect();
        let globals_last_used: Vec<u64> = globals_keys
            .iter()
            .map(|key| self.globals_cache[key].last_used_frame)
            .collect();
        let globals = plan_globals_retention(
            self.submitted_frame,
            limits.max_idle_frames,
            limits.max_cached_globals,
            &globals_last_used,
        );
        for (key, keep) in globals_keys.iter().zip(&globals.keep) {
            if !keep {
                self.globals_cache.remove(key);
            }
        }

        PoolMaintenanceReport {
            bucket_count: u64::try_from(self.pools.len()).unwrap_or(u64::MAX),
            available_entries: textures.available_entries,
            available_bytes: textures.available_bytes,
            age_evicted_entries: textures.age_evicted_entries,
            age_evicted_bytes: textures.age_evicted_bytes,
            budget_evicted_entries: textures.budget_evicted_entries,
            budget_evicted_bytes: textures.budget_evicted_bytes,
            unknown_size_evicted_entries: textures.unknown_size_evicted_entries,
            globals_available_entries: globals.available_entries,
            globals_age_evictions: globals.age_evictions,
            globals_budget_evictions: globals.budget_evictions,
        }
    }
}

#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq)]
struct TextureKey {
    size: wgpu::Extent3d,
    usage: wgpu::TextureUsages,
    format: wgpu::TextureFormat,
    sample_count: u32,
}

#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq)]
struct GlobalsKey {
    offset_x: u32,
    offset_y: u32,
    viewport_width: u32,
    viewport_height: u32,
}

pub trait BufferDescription: Clone + Debug {
    type Cost: Ord;

    /// If the potential buffer represented by this description (`self`)
    /// fits another existing buffer and its description (`other`),
    /// return the cost to use that buffer instead of making a new one.
    ///
    /// Cost is an arbitrary unit, but lower is better.
    /// None means that the other buffer cannot be used in place of this one.
    fn cost_to_use(&self, other: &Self) -> Option<Self::Cost>;
}

#[derive(Clone, Debug)]
pub struct AlwaysCompatible;

impl BufferDescription for AlwaysCompatible {
    type Cost = ();

    fn cost_to_use(&self, _other: &Self) -> Option<()> {
        Some(())
    }
}

pub struct BufferPool<Type, Description: BufferDescription> {
    available: Arc<PoolInner<(Type, Description)>>,
    constructor: Constructor<Type, Description>,
}

impl<Type, Description: BufferDescription> Debug for BufferPool<Type, Description> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BufferPool").finish()
    }
}

impl<Type, Description: BufferDescription> BufferPool<Type, Description> {
    pub fn new(constructor: Constructor<Type, Description>) -> Self {
        Self {
            available: Arc::new(Mutex::new(vec![])),
            constructor,
        }
    }

    pub fn take(
        &self,
        descriptors: &Descriptors,
        description: Description,
    ) -> PoolEntry<Type, Description> {
        self.take_with_reuse(descriptors, description).0
    }

    fn take_with_reuse(
        &self,
        descriptors: &Descriptors,
        description: Description,
    ) -> (PoolEntry<Type, Description>, bool) {
        let mut guard = self
            .available
            .lock()
            .expect("Should not be able to lock recursively");
        let mut best: Option<(Description::Cost, usize)> = None;
        for i in 0..guard.len() {
            if let Some(cost) = description.cost_to_use(&guard[i].1) {
                if let Some(best) = &mut best {
                    if best.0 > cost {
                        *best = (cost, i);
                    }
                } else if best.is_none() {
                    best = Some((cost, i));
                }
            }
        }

        let (item, used_description, reused) = if let Some((_, best)) = best {
            let (item, used_description) = guard.swap_remove(best);
            (item, used_description, true)
        } else {
            let item = (self.constructor)(descriptors, &description);
            (item, description, false)
        };
        (
            PoolEntry {
                item: Some(item),
                description: used_description,
                pool: Arc::downgrade(&self.available),
            },
            reused,
        )
    }

    fn available_len(&self) -> usize {
        self.available
            .lock()
            .expect("Should not be able to lock recursively")
            .len()
    }

    fn truncate_available(&self, keep: usize) -> usize {
        let mut available = self
            .available
            .lock()
            .expect("Should not be able to lock recursively");
        let removed = available.len().saturating_sub(keep);
        available.truncate(keep);
        removed
    }
}

pub struct PoolEntry<Type, Description: BufferDescription> {
    item: Option<Type>,
    description: Description,
    pool: Weak<PoolInner<(Type, Description)>>,
}

impl<Type, Description: BufferDescription> Debug for PoolEntry<Type, Description>
where
    Type: Debug,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("PoolEntry").field(&self.item).finish()
    }
}

impl<Type, Description: BufferDescription> Drop for PoolEntry<Type, Description> {
    fn drop(&mut self) {
        if let Some(item) = self.item.take()
            && let Some(pool) = self.pool.upgrade()
        {
            pool.lock()
                .expect("Should not be able to lock recursively")
                .push((item, self.description.clone()))
        }
    }
}

impl<Type, Description: BufferDescription> Deref for PoolEntry<Type, Description> {
    type Target = Type;

    fn deref(&self) -> &Self::Target {
        self.item.as_ref().expect("Item should exist until dropped")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "aether_metrics")]
    use crate::aether_metrics::TexturePoolKind;
    use crate::texture_pool_policy::{BoundedTexturePoolLimits, OffscreenTexturePoolPolicy};

    #[test]
    fn buffer_pool_truncates_only_available_entries() {
        let pool = BufferPool::<u32, AlwaysCompatible>::new(Box::new(|_, _| unreachable!()));
        {
            let mut available = pool.available.lock().unwrap();
            available.extend([
                (1, AlwaysCompatible),
                (2, AlwaysCompatible),
                (3, AlwaysCompatible),
            ]);
        }

        assert_eq!(pool.truncate_available(1), 2);
        assert_eq!(pool.available_len(), 1);
    }

    #[test]
    fn bounded_pool_configuration_survives_reset() {
        let limits = BoundedTexturePoolLimits {
            max_cached_bytes: 256,
            max_cached_entries: 4,
            max_idle_frames: 2,
            max_cached_globals: 2,
        };
        let mut pool = TexturePool::new(
            OffscreenTexturePoolPolicy::BoundedReuse(limits),
            #[cfg(feature = "aether_metrics")]
            TexturePoolKind::Offscreen,
        );

        pool.reset();

        assert_eq!(
            pool.policy,
            OffscreenTexturePoolPolicy::BoundedReuse(limits)
        );
        assert_eq!(pool.submitted_frame, 0);
    }

    #[test]
    fn ephemeral_pool_discards_descriptor_buckets_at_frame_end() {
        let mut pool = TexturePool::new(
            OffscreenTexturePoolPolicy::Ephemeral,
            #[cfg(feature = "aether_metrics")]
            TexturePoolKind::General,
        );
        pool.pools.insert(
            TextureKey {
                size: wgpu::Extent3d {
                    width: 64,
                    height: 64,
                    depth_or_array_layers: 1,
                },
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: wgpu::TextureFormat::Rgba8Unorm,
                sample_count: 1,
            },
            TextureBucket {
                pool: BufferPool::new(Box::new(|_, _| unreachable!())),
                bytes_per_entry: Some(64 * 64 * 4),
                last_used_frame: 0,
                uses: 1,
            },
        );

        pool.finish_frame();

        assert!(pool.pools.is_empty());
    }

    #[test]
    fn bounded_pool_keeps_a_hot_bucket_whose_textures_are_all_still_checked_out() {
        let mut pool = TexturePool::new(
            OffscreenTexturePoolPolicy::BoundedReuse(BoundedTexturePoolLimits {
                max_cached_bytes: 384 * 1024 * 1024,
                max_cached_entries: 128,
                max_idle_frames: 1,
                max_cached_globals: 32,
            }),
            #[cfg(feature = "aether_metrics")]
            TexturePoolKind::General,
        );
        let key = TextureKey {
            size: wgpu::Extent3d {
                width: 2560,
                height: 1440,
                depth_or_array_layers: 1,
            },
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: wgpu::TextureFormat::Rgba8Unorm,
            sample_count: 4,
        };
        // Used this very frame, but every texture is still handed out, so nothing is available.
        pool.pools.insert(
            key,
            TextureBucket {
                pool: BufferPool::new(Box::new(|_, _| unreachable!())),
                bytes_per_entry: Some(2560 * 1440 * 4 * 4),
                last_used_frame: 0,
                uses: 1,
            },
        );

        pool.finish_frame();

        assert!(
            pool.pools.contains_key(&key),
            "a bucket used this frame must survive so its textures can return to it"
        );
    }

    #[test]
    fn texture_key_requires_every_descriptor_field_to_match() {
        let base = TextureKey {
            size: wgpu::Extent3d {
                width: 64,
                height: 32,
                depth_or_array_layers: 1,
            },
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: wgpu::TextureFormat::Rgba8Unorm,
            sample_count: 1,
        };
        assert_ne!(
            base,
            TextureKey {
                sample_count: 4,
                ..base
            }
        );
        assert_ne!(
            base,
            TextureKey {
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                ..base
            }
        );
        assert_ne!(
            base,
            TextureKey {
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                ..base
            }
        );
        assert_ne!(
            base,
            TextureKey {
                size: wgpu::Extent3d {
                    width: 65,
                    ..base.size
                },
                ..base
            }
        );
    }
}

#[cfg(all(test, feature = "aether_metrics"))]
mod aether_metrics_tests {
    use super::*;
    use crate::aether_metrics::{TexturePoolKind, take_snapshot};

    #[test]
    fn texture_pool_reset_records_its_kind() {
        let _ = take_snapshot();
        let mut pool = TexturePool::new(
            crate::texture_pool_policy::OffscreenTexturePoolPolicy::Ephemeral,
            TexturePoolKind::Offscreen,
        );

        pool.reset();

        let snapshot = take_snapshot();
        assert_eq!(snapshot.general.resets, 0);
        assert_eq!(snapshot.offscreen.resets, 1);
        assert_eq!(snapshot.offscreen.discarded_available_entries, 0);
    }
}
