#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoundedTexturePoolLimits {
    pub max_cached_bytes: u64,
    pub max_cached_entries: usize,
    pub max_idle_frames: u64,
    pub max_cached_globals: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OffscreenTexturePoolPolicy {
    #[default]
    Ephemeral,
    BoundedReuse(BoundedTexturePoolLimits),
}

pub(crate) const AMD_PCI_VENDOR_ID: u32 = 0x1002;

pub(crate) fn is_amd_vulkan(adapter_info: &wgpu::AdapterInfo) -> bool {
    adapter_info.backend == wgpu::Backend::Vulkan && adapter_info.vendor == AMD_PCI_VENDOR_ID
}

/// Whether this adapter draws its memory from system RAM rather than from a board of its own.
///
/// The distinction matters because every budget in this file was sized against a discrete card
/// with gigabytes to spare. An integrated adapter has no such headroom: it is spending the same
/// RAM the player's machine is running on, and it reports a device loss rather than paging when
/// that runs out. Ruffle carries an open class of exactly this crash on Intel integrated parts --
/// UHD 630, UHD 600, Iris Xe, Iris Plus 640, HD Gen11, across both Vulkan and Dx12 -- with the
/// closest report closed as not planned, so nothing is arriving from upstream to help.
pub(crate) fn is_integrated_gpu(adapter_info: &wgpu::AdapterInfo) -> bool {
    matches!(
        adapter_info.device_type,
        wgpu::DeviceType::IntegratedGpu | wgpu::DeviceType::Cpu | wgpu::DeviceType::Other
    )
}

/// What the general pool may cache, for an adapter that is spending the player's system RAM.
///
/// A quarter of the discrete budget. Sized to still clear the nested stack of stage-sized targets
/// that [`GENERAL_TEXTURE_POOL_MAX_CACHED_BYTES`] exists to retain -- an integrated part is not
/// running a 2560x1440 stage at 4x MSAA, so the pair this has to hold is far smaller than the
/// ~70 MiB that sizes the discrete budget.
pub(crate) const INTEGRATED_TEXTURE_POOL_MAX_CACHED_BYTES: u64 = 96 * 1024 * 1024;

/// Budget for the general pool, which holds whole-surface render targets rather than the small
/// filter intermediates the offscreen pool deals in.
///
/// Every `Command::Blend` and every mask in a frame builds its own `Surface` at the *stage* size,
/// and at `StageQuality::High` that means a 4x MSAA framebuffer alongside its resolved colour
/// texture. At 2560x1440 that pair is ~70 MiB. Sequential nodes drop their targets before the next
/// one asks for its own, so one set can serve an entire frame -- but only if the pool is permitted
/// to hold it in between. Deriving this budget as a fraction of the offscreen pool capped it at
/// 64 MiB, below the size of a single pair, so retention planning evicted 100% of what it was
/// handed and every node in the frame allocated fresh. Size it for a realistic nesting depth
/// instead; the cap still bounds the pool, it just bounds it somewhere useful.
pub(crate) const GENERAL_TEXTURE_POOL_MAX_CACHED_BYTES: u64 = 384 * 1024 * 1024;

/// Entries the general pool may cache, summed across every size bucket.
///
/// This was 128, derived as a quarter of the offscreen pool's 512. The number is a count across
/// *all* buckets, and quantising blend regions to a 128px grid yields hundreds of distinct sizes on
/// a 2560x1365 stage -- so 128 could not hold even one texture per size a frame touches, and the
/// hot buckets were evicted every frame while the 384 MiB budget went largely unspent. A measured
/// session peaked at 3.15 GB resident on a 10 GB card, so bytes were never the binding constraint;
/// the entry count was, and it was bounding the wrong thing. Size it so the byte budget is what
/// bounds the pool, because that is the limit that actually protects memory.
pub(crate) const GENERAL_TEXTURE_POOL_MAX_CACHED_ENTRIES: usize = 1024;

/// Keep a companion pool for the whole-surface render targets that blends and masks allocate,
/// whenever the caller explicitly enables bounded offscreen reuse. These targets repeat every
/// frame in animated Flash content, but retaining every size encountered across maps and login
/// sessions causes GPU memory to grow without a useful bound.
pub(crate) fn general_texture_pool_policy(
    offscreen_policy: OffscreenTexturePoolPolicy,
    adapter_info: &wgpu::AdapterInfo,
) -> OffscreenTexturePoolPolicy {
    match offscreen_policy {
        OffscreenTexturePoolPolicy::Ephemeral => OffscreenTexturePoolPolicy::Ephemeral,
        OffscreenTexturePoolPolicy::BoundedReuse(limits) => {
            OffscreenTexturePoolPolicy::BoundedReuse(BoundedTexturePoolLimits {
                max_cached_bytes: if is_integrated_gpu(adapter_info) {
                    INTEGRATED_TEXTURE_POOL_MAX_CACHED_BYTES
                } else {
                    GENERAL_TEXTURE_POOL_MAX_CACHED_BYTES
                },
                max_cached_entries: GENERAL_TEXTURE_POOL_MAX_CACHED_ENTRIES,
                max_idle_frames: limits.max_idle_frames.min(1),
                max_cached_globals: limits.max_cached_globals.min(32),
            })
        }
    }
}

/// Bound the amount of cache/filter work recorded into one GPU command submission. The generic
/// renderer keeps its established batching, while Aether's bounded AQW path retains a safety
/// margin below that established limit. AMD's Windows Vulkan driver needs a smaller batch because
/// it has reported device OOM while resident texture memory remained low and one dense AQW frame
/// accumulated hundreds of passes in a single command submission.
pub(crate) fn max_cache_entries_per_submission(
    policy: OffscreenTexturePoolPolicy,
    adapter_info: &wgpu::AdapterInfo,
) -> u32 {
    match policy {
        OffscreenTexturePoolPolicy::Ephemeral => 100,
        // An integrated part gets the same conservative batch AMD Vulkan gets, for the same
        // measured reason: fewer resources alive at once, and the driver given more chances to
        // retire the ones that are. That AMD case reported device OOM while resident texture
        // memory stayed low, which is the shape of the Intel reports too.
        OffscreenTexturePoolPolicy::BoundedReuse(_)
            if is_amd_vulkan(adapter_info) || is_integrated_gpu(adapter_info) =>
        {
            16
        }
        OffscreenTexturePoolPolicy::BoundedReuse(_) => 64,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TextureRetentionCandidate {
    pub last_used_frame: u64,
    pub available_entries: usize,
    pub bytes_per_entry: Option<u64>,
    /// How many times this size has ever been requested. Breaks ties between buckets last used on
    /// the same frame, which in a busy frame is nearly all of them.
    pub uses: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct TextureRetentionDecision {
    pub keep_entries: Vec<usize>,
    pub age_evicted_entries: u64,
    pub age_evicted_bytes: u64,
    pub budget_evicted_entries: u64,
    pub budget_evicted_bytes: u64,
    pub unknown_size_evicted_entries: u64,
    pub available_entries: u64,
    pub available_bytes: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct GlobalsRetentionDecision {
    pub keep: Vec<bool>,
    pub age_evictions: u64,
    pub budget_evictions: u64,
    pub available_entries: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct PoolMaintenanceReport {
    /// Distinct size buckets the pool is tracking. The entry cap is shared across all of them, so
    /// this is what says whether that cap can hold anything useful per bucket.
    pub bucket_count: u64,
    pub available_entries: u64,
    pub available_bytes: u64,
    pub age_evicted_entries: u64,
    pub age_evicted_bytes: u64,
    pub budget_evicted_entries: u64,
    pub budget_evicted_bytes: u64,
    pub unknown_size_evicted_entries: u64,
    pub globals_available_entries: u64,
    pub globals_age_evictions: u64,
    pub globals_budget_evictions: u64,
}

/// Smallest cell a pooled texture's dimension is rounded up to.
///
/// Matches `quantise_cache_texture_size` in core, so a filter target and the cache texture it was
/// derived from land in the same bucket instead of two adjacent ones.
const POOL_TEXTURE_SIZE_FLOOR: u32 = 64;

/// How many cells span a dimension, which is what keeps the slack proportional.
const POOL_TEXTURE_GRID_STEPS: u32 = 16;

/// Round a pooled texture's dimension up so a few pixels of drift reuse the same bucket.
fn quantise_pool_dimension(value: u32, limit: u32) -> u32 {
    if value == 0 {
        return 0;
    }
    let cell = (value.next_power_of_two() / POOL_TEXTURE_GRID_STEPS).max(POOL_TEXTURE_SIZE_FLOOR);
    let rounded = value.div_ceil(cell).saturating_mul(cell).max(value);
    // Never past what the device can allocate. A size that rounds over the limit stays exact:
    // failing to allocate is worse than missing the bucket.
    if rounded > limit { value } else { rounded }
}

/// Round a pool request out to a grid so near-identical requests share one bucket.
///
/// The pool keys buckets on exact dimensions. A filter's target is sized to the region being
/// filtered, and an animating object's bounds move a pixel or two every frame, so one glow asks for
/// 231x343, then 232x344, then 230x341. Each is a bucket that is allocated once and never matched
/// again -- a session census measured 781 GB of offscreen texture creation against 4096 size
/// buckets, all of them full, with 774 GB thrown away for budget before anything could reuse it.
///
/// Growing a texture is safe because the caller keeps its logical size separately and confines
/// drawing to it with a viewport; see `FilterRegion::for_target`, which exists for exactly this.
pub(crate) fn quantise_pool_texture_size(
    size: wgpu::Extent3d,
    max_dimension: u32,
) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: quantise_pool_dimension(size.width, max_dimension),
        height: quantise_pool_dimension(size.height, max_dimension),
        depth_or_array_layers: size.depth_or_array_layers,
    }
}

pub(crate) fn estimate_texture_bytes(
    size: wgpu::Extent3d,
    format: wgpu::TextureFormat,
    mip_level_count: u32,
    sample_count: u32,
) -> Option<u64> {
    let bytes_per_block = u64::from(format.block_copy_size(None)?);
    let (block_width, block_height) = format.block_dimensions();
    let layers = u64::from(size.depth_or_array_layers.max(1));
    let samples = u64::from(sample_count.max(1));
    let mut width = size.width.max(1);
    let mut height = size.height.max(1);
    let mut total = 0_u64;

    for _ in 0..mip_level_count.max(1) {
        let blocks_wide = u64::from(width.div_ceil(block_width));
        let blocks_high = u64::from(height.div_ceil(block_height));
        let mip_bytes = blocks_wide
            .checked_mul(blocks_high)?
            .checked_mul(layers)?
            .checked_mul(bytes_per_block)?
            .checked_mul(samples)?;
        total = total.checked_add(mip_bytes)?;
        width = (width / 2).max(1);
        height = (height / 2).max(1);
    }

    Some(total)
}

pub(crate) fn plan_texture_retention(
    current_frame: u64,
    limits: BoundedTexturePoolLimits,
    candidates: &[TextureRetentionCandidate],
) -> TextureRetentionDecision {
    let mut decision = TextureRetentionDecision {
        keep_entries: candidates
            .iter()
            .map(|candidate| candidate.available_entries)
            .collect(),
        ..Default::default()
    };

    for (index, candidate) in candidates.iter().enumerate() {
        let entries = u64::try_from(candidate.available_entries).unwrap_or(u64::MAX);
        let Some(bytes_per_entry) = candidate.bytes_per_entry else {
            decision.keep_entries[index] = 0;
            decision.unknown_size_evicted_entries = decision
                .unknown_size_evicted_entries
                .saturating_add(entries);
            continue;
        };

        if current_frame.saturating_sub(candidate.last_used_frame) > limits.max_idle_frames {
            decision.keep_entries[index] = 0;
            decision.age_evicted_entries = decision.age_evicted_entries.saturating_add(entries);
            decision.age_evicted_bytes = decision
                .age_evicted_bytes
                .saturating_add(entries.saturating_mul(bytes_per_entry));
        }
    }

    let totals = |keep: &[usize]| {
        candidates
            .iter()
            .zip(keep)
            .fold((0_u64, 0_u64), |(entries, bytes), (candidate, keep)| {
                let keep = u64::try_from(*keep).unwrap_or(u64::MAX);
                (
                    entries.saturating_add(keep),
                    bytes.saturating_add(
                        keep.saturating_mul(candidate.bytes_per_entry.unwrap_or_default()),
                    ),
                )
            })
    };

    let (mut available_entries, mut available_bytes) = totals(&decision.keep_entries);
    let max_entries = u64::try_from(limits.max_cached_entries).unwrap_or(u64::MAX);
    let mut order: Vec<usize> = (0..candidates.len()).collect();
    // Least recently used first, then least often used. The second key is load-bearing rather than
    // decorative: a frame touches hundreds of buckets, so hundreds share a `last_used_frame`, and
    // ordering those by array index alone means the choice of what to evict is arbitrary. A census
    // of a real session showed the consequence -- one 2560x1365 surface, requested every single
    // frame, allocated 141 times in twelve seconds because single-use buckets kept displacing it.
    order.sort_by_key(|index| {
        (
            candidates[*index].last_used_frame,
            candidates[*index].uses,
            *index,
        )
    });

    for index in order {
        if available_entries <= max_entries && available_bytes <= limits.max_cached_bytes {
            break;
        }
        let keep = u64::try_from(decision.keep_entries[index]).unwrap_or(u64::MAX);
        let bytes_per_entry = candidates[index].bytes_per_entry.unwrap_or_default();
        let entries_over = available_entries.saturating_sub(max_entries);
        let bytes_over = available_bytes.saturating_sub(limits.max_cached_bytes);
        let entries_for_bytes = if bytes_over == 0 {
            0
        } else if bytes_per_entry == 0 {
            keep
        } else {
            bytes_over.div_ceil(bytes_per_entry)
        };
        let remove = keep.min(entries_over.max(entries_for_bytes));
        decision.keep_entries[index] = usize::try_from(keep - remove).unwrap_or_default();
        available_entries = available_entries.saturating_sub(remove);
        let removed_bytes = remove.saturating_mul(bytes_per_entry);
        available_bytes = available_bytes.saturating_sub(removed_bytes);
        decision.budget_evicted_entries = decision.budget_evicted_entries.saturating_add(remove);
        decision.budget_evicted_bytes = decision.budget_evicted_bytes.saturating_add(removed_bytes);
    }

    decision.available_entries = available_entries;
    decision.available_bytes = available_bytes;
    decision
}

pub(crate) fn plan_globals_retention(
    current_frame: u64,
    max_idle_frames: u64,
    max_entries: usize,
    last_used_frames: &[u64],
) -> GlobalsRetentionDecision {
    let mut decision = GlobalsRetentionDecision {
        keep: vec![true; last_used_frames.len()],
        ..Default::default()
    };
    for (index, last_used) in last_used_frames.iter().copied().enumerate() {
        if current_frame.saturating_sub(last_used) > max_idle_frames {
            decision.keep[index] = false;
            decision.age_evictions = decision.age_evictions.saturating_add(1);
        }
    }

    let mut available = decision.keep.iter().filter(|keep| **keep).count();
    let mut order: Vec<usize> = (0..last_used_frames.len()).collect();
    order.sort_by_key(|index| (last_used_frames[*index], *index));
    for index in order {
        if available <= max_entries {
            break;
        }
        if decision.keep[index] {
            decision.keep[index] = false;
            available -= 1;
            decision.budget_evictions = decision.budget_evictions.saturating_add(1);
        }
    }
    decision.available_entries = u64::try_from(available).unwrap_or(u64::MAX);
    decision
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter_info(vendor: u32, backend: wgpu::Backend) -> wgpu::AdapterInfo {
        wgpu::AdapterInfo {
            name: String::new(),
            vendor,
            device: 0,
            device_type: wgpu::DeviceType::DiscreteGpu,
            driver: String::new(),
            driver_info: String::new(),
            backend,
        }
    }

    /// An Intel UHD on Vulkan, which is the adapter the reports keep naming.
    fn integrated_adapter_info() -> wgpu::AdapterInfo {
        wgpu::AdapterInfo {
            device_type: wgpu::DeviceType::IntegratedGpu,
            ..adapter_info(0x8086, wgpu::Backend::Vulkan)
        }
    }

    const LIMITS: BoundedTexturePoolLimits = BoundedTexturePoolLimits {
        max_cached_bytes: 300,
        max_cached_entries: 3,
        max_idle_frames: 2,
        max_cached_globals: 2,
    };

    /// An integrated adapter is spending the player's system RAM, so it must not be handed budgets
    /// that were sized against a discrete card. Ruffle's own tracker carries this crash on Intel
    /// UHD, Iris Xe and Iris Plus parts with no fix, so nothing catches it if this does not.
    #[test]
    fn an_integrated_adapter_gets_a_smaller_pool_than_a_discrete_one() {
        let integrated = integrated_adapter_info();
        let discrete = adapter_info(0x10de, wgpu::Backend::Vulkan);

        let OffscreenTexturePoolPolicy::BoundedReuse(integrated_limits) =
            general_texture_pool_policy(
                OffscreenTexturePoolPolicy::BoundedReuse(LIMITS),
                &integrated,
            )
        else {
            panic!("bounded offscreen reuse must derive a bounded general pool");
        };
        let OffscreenTexturePoolPolicy::BoundedReuse(discrete_limits) = general_texture_pool_policy(
            OffscreenTexturePoolPolicy::BoundedReuse(LIMITS),
            &discrete,
        ) else {
            panic!("bounded offscreen reuse must derive a bounded general pool");
        };

        assert_eq!(
            integrated_limits.max_cached_bytes,
            INTEGRATED_TEXTURE_POOL_MAX_CACHED_BYTES
        );
        assert!(
            integrated_limits.max_cached_bytes < discrete_limits.max_cached_bytes,
            "an integrated adapter caching {} bytes is not spending less than a discrete one at {}",
            integrated_limits.max_cached_bytes,
            discrete_limits.max_cached_bytes,
        );
    }

    /// Whatever the vendor, memory drawn from system RAM has no headroom to speculate into.
    #[test]
    fn integrated_and_software_adapters_are_all_treated_as_sharing_system_memory() {
        assert!(is_integrated_gpu(&integrated_adapter_info()));
        for device_type in [wgpu::DeviceType::Cpu, wgpu::DeviceType::Other] {
            assert!(is_integrated_gpu(&wgpu::AdapterInfo {
                device_type,
                ..adapter_info(0x8086, wgpu::Backend::Dx12)
            }));
        }
        assert!(!is_integrated_gpu(&adapter_info(
            0x10de,
            wgpu::Backend::Vulkan
        )));
    }

    /// The Intel reports span both backends, so this cannot key off Vulkan the way the AMD case does.
    #[test]
    fn an_integrated_adapter_splits_submissions_on_either_backend() {
        for backend in [wgpu::Backend::Vulkan, wgpu::Backend::Dx12] {
            let integrated = wgpu::AdapterInfo {
                device_type: wgpu::DeviceType::IntegratedGpu,
                ..adapter_info(0x8086, backend)
            };
            assert_eq!(
                max_cache_entries_per_submission(
                    OffscreenTexturePoolPolicy::BoundedReuse(LIMITS),
                    &integrated,
                ),
                16,
                "integrated adapters must split dense frames on {backend:?} as well"
            );
        }
    }

    #[test]
    fn bounded_aqw_work_limits_amd_vulkan_submission_pressure() {
        let amd_vulkan = adapter_info(0x1002, wgpu::Backend::Vulkan);
        let amd_dx12 = adapter_info(0x1002, wgpu::Backend::Dx12);
        let nvidia_vulkan = adapter_info(0x10de, wgpu::Backend::Vulkan);

        assert_eq!(
            max_cache_entries_per_submission(OffscreenTexturePoolPolicy::Ephemeral, &amd_vulkan),
            100
        );
        assert_eq!(
            max_cache_entries_per_submission(
                OffscreenTexturePoolPolicy::BoundedReuse(LIMITS),
                &amd_vulkan,
            ),
            16,
            "AMD Vulkan must split dense AQW frames before one submission reaches driver OOM pressure"
        );
        assert_eq!(
            max_cache_entries_per_submission(
                OffscreenTexturePoolPolicy::BoundedReuse(LIMITS),
                &amd_dx12,
            ),
            64
        );
        assert_eq!(
            max_cache_entries_per_submission(
                OffscreenTexturePoolPolicy::BoundedReuse(LIMITS),
                &nvidia_vulkan,
            ),
            64
        );
    }

    #[test]
    fn bounded_offscreen_reuse_derives_a_smaller_general_pool() {
        assert_eq!(
            general_texture_pool_policy(
                OffscreenTexturePoolPolicy::Ephemeral,
                &adapter_info(0x10de, wgpu::Backend::Vulkan),
            ),
            OffscreenTexturePoolPolicy::Ephemeral
        );
        assert_eq!(
            general_texture_pool_policy(
                OffscreenTexturePoolPolicy::BoundedReuse(BoundedTexturePoolLimits {
                    max_cached_bytes: 128 * 1024 * 1024,
                    max_cached_entries: 512,
                    max_idle_frames: 3,
                    max_cached_globals: 128,
                }),
                &adapter_info(0x10de, wgpu::Backend::Vulkan),
            ),
            OffscreenTexturePoolPolicy::BoundedReuse(BoundedTexturePoolLimits {
                max_cached_bytes: GENERAL_TEXTURE_POOL_MAX_CACHED_BYTES,
                max_cached_entries: GENERAL_TEXTURE_POOL_MAX_CACHED_ENTRIES,
                max_idle_frames: 1,
                max_cached_globals: 32,
            })
        );
    }

    #[test]
    fn the_general_pool_entry_cap_clears_one_target_per_size_a_frame_touches() {
        // Blend regions quantise to a 128px grid, so a 2560x1365 stage can ask for 20x11 distinct
        // sizes per sample count -- 440 before any unquantised or edge-clamped size joins them. An
        // entry cap below that cannot keep one texture per size, so the sizes reused every frame
        // get evicted by the ones used once, which is the churn this budget exists to stop.
        let widths = 2560_usize.div_ceil(128);
        let heights = 1365_usize.div_ceil(128);
        let sizes_per_sample_count = widths * heights;
        let distinct_sizes = sizes_per_sample_count * 2;

        assert!(
            GENERAL_TEXTURE_POOL_MAX_CACHED_ENTRIES >= distinct_sizes,
            "{} entries cannot hold one texture for each of {} reachable sizes",
            GENERAL_TEXTURE_POOL_MAX_CACHED_ENTRIES,
            distinct_sizes,
        );
    }

    #[test]
    fn general_pool_budget_retains_a_nested_stack_of_native_resolution_targets() {
        // Every `Blend` and mask node in a frame gets its own stage-sized render target, and at
        // StageQuality::High that means a 4x MSAA framebuffer plus its resolved colour texture.
        // Sequential nodes can share one set, but only if the pool is allowed to keep it between
        // nodes, so the budget has to clear a realistic nesting depth. A budget below the size of
        // a single pair makes the pool evict everything it is handed and every node allocates
        // fresh -- which is what drove resident texture memory past 39 GB at 2560x1440.
        let stage = wgpu::Extent3d {
            width: 2560,
            height: 1440,
            depth_or_array_layers: 1,
        };
        let msaa = estimate_texture_bytes(stage, wgpu::TextureFormat::Rgba8Unorm, 1, 4)
            .expect("msaa framebuffer is a known size");
        let resolved = estimate_texture_bytes(stage, wgpu::TextureFormat::Rgba8Unorm, 1, 1)
            .expect("resolved colour target is a known size");

        let OffscreenTexturePoolPolicy::BoundedReuse(general) = general_texture_pool_policy(
            OffscreenTexturePoolPolicy::BoundedReuse(BoundedTexturePoolLimits {
                max_cached_bytes: 128 * 1024 * 1024,
                max_cached_entries: 512,
                max_idle_frames: 1,
                max_cached_globals: 128,
            }),
            &adapter_info(0x10de, wgpu::Backend::Vulkan),
        ) else {
            panic!("bounded offscreen reuse must derive a bounded general pool");
        };

        assert!(
            general.max_cached_bytes >= (msaa + resolved) * 4,
            "general budget of {} bytes cannot retain four nested {}+{} byte targets",
            general.max_cached_bytes,
            msaa,
            resolved,
        );
    }

    fn extent(width: u32, height: u32) -> wgpu::Extent3d {
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        }
    }

    /// The measured failure: an animating glow asks for three sizes a pixel apart on consecutive
    /// frames, and each one is a bucket that is allocated once and never matched again.
    #[test]
    fn a_run_of_drifting_sizes_collapses_into_one_bucket() {
        const LIMIT: u32 = 8192;
        let snapped: Vec<_> = [(230, 341), (231, 343), (232, 344)]
            .into_iter()
            .map(|(w, h)| quantise_pool_texture_size(extent(w, h), LIMIT))
            .collect();

        assert_eq!(snapped[0], snapped[1]);
        assert_eq!(snapped[1], snapped[2]);

        // And the big ones, which is where the bytes are: 2326x2371, 2324x2371, 2328x2371 were
        // 4,434 allocations of 22 MB apiece in a seven minute session.
        let big: Vec<_> = [(2326, 2371), (2324, 2371), (2328, 2371)]
            .into_iter()
            .map(|(w, h)| quantise_pool_texture_size(extent(w, h), LIMIT))
            .collect();
        assert_eq!(big[0], big[1]);
        assert_eq!(big[1], big[2]);
    }

    /// Rounding may only ever grow a texture. Anything smaller than the region would clip content.
    #[test]
    fn quantising_never_returns_a_texture_smaller_than_the_region() {
        const LIMIT: u32 = 8192;
        for (width, height) in [
            (1, 1),
            (63, 65),
            (64, 64),
            (100, 200),
            (1023, 1025),
            (2560, 1365),
            (8192, 8192),
        ] {
            let snapped = quantise_pool_texture_size(extent(width, height), LIMIT);
            assert!(
                snapped.width >= width && snapped.height >= height,
                "{snapped:?} is smaller than {width}x{height}",
            );
        }
        assert_eq!(
            quantise_pool_texture_size(extent(0, 0), LIMIT),
            extent(0, 0)
        );
    }

    /// The slack stays proportional, so a small avatar layer is not rounded out to many times its
    /// own size the way a flat grid would round it.
    #[test]
    fn the_slack_stays_proportional_to_the_region() {
        const LIMIT: u32 = 8192;
        for (width, height) in [(231, 343), (700, 500), (2326, 2371)] {
            let snapped = quantise_pool_texture_size(extent(width, height), LIMIT);
            let asked = u64::from(width) * u64::from(height);
            let got = u64::from(snapped.width) * u64::from(snapped.height);
            assert!(
                got <= asked * 3 / 2,
                "{width}x{height} rounded to {snapped:?}, more than half again the pixels",
            );
        }
    }

    /// A size that would round past what the device can allocate has to stay exact: missing the
    /// bucket costs a texture, failing to allocate costs the frame.
    ///
    /// Rounding lands at most on the value's own power-of-two bracket, so with the power-of-two
    /// limits real drivers report this can never bite -- 8100 against 8192 rounds to exactly the
    /// limit. The guard is here for a driver that reports something else.
    #[test]
    fn quantising_never_rounds_past_the_device_limit() {
        assert_eq!(
            quantise_pool_texture_size(extent(8100, 4000), 8192),
            extent(8192, 4096),
        );
        let snapped = quantise_pool_texture_size(extent(5900, 5900), 6000);
        assert_eq!(snapped, extent(5900, 5900), "{snapped:?} rounded past 6000");
    }

    #[test]
    fn texture_byte_estimator_checks_blocks_mips_layers_samples_and_unknown_formats() {
        assert_eq!(
            estimate_texture_bytes(
                wgpu::Extent3d {
                    width: 7,
                    height: 5,
                    depth_or_array_layers: 1,
                },
                wgpu::TextureFormat::Bc1RgbaUnorm,
                1,
                1,
            ),
            Some(32),
        );
        assert_eq!(
            estimate_texture_bytes(
                wgpu::Extent3d {
                    width: 8,
                    height: 8,
                    depth_or_array_layers: 1,
                },
                wgpu::TextureFormat::Rgba8Unorm,
                4,
                1,
            ),
            Some(340),
        );
        assert_eq!(
            estimate_texture_bytes(
                wgpu::Extent3d {
                    width: 4,
                    height: 4,
                    depth_or_array_layers: 3,
                },
                wgpu::TextureFormat::Rgba8Unorm,
                1,
                4,
            ),
            Some(768),
        );
        assert_eq!(
            estimate_texture_bytes(
                wgpu::Extent3d {
                    width: 4,
                    height: 4,
                    depth_or_array_layers: 1,
                },
                wgpu::TextureFormat::Depth24Plus,
                1,
                1,
            ),
            None,
        );
        assert_eq!(
            estimate_texture_bytes(
                wgpu::Extent3d {
                    width: u32::MAX,
                    height: u32::MAX,
                    depth_or_array_layers: u32::MAX,
                },
                wgpu::TextureFormat::Rgba32Float,
                1,
                4,
            ),
            None,
        );
    }

    #[test]
    fn retention_rejects_unknown_and_expired_entries_before_budget_eviction() {
        let decision = plan_texture_retention(
            10,
            LIMITS,
            &[
                TextureRetentionCandidate {
                    last_used_frame: 10,
                    available_entries: 2,
                    bytes_per_entry: Some(100),
                    uses: 1,
                },
                TextureRetentionCandidate {
                    last_used_frame: 7,
                    available_entries: 1,
                    bytes_per_entry: Some(50),
                    uses: 1,
                },
                TextureRetentionCandidate {
                    last_used_frame: 10,
                    available_entries: 1,
                    bytes_per_entry: None,
                    uses: 1,
                },
            ],
        );

        assert_eq!(decision.keep_entries, vec![2, 0, 0]);
        assert_eq!(decision.age_evicted_entries, 1);
        assert_eq!(decision.age_evicted_bytes, 50);
        assert_eq!(decision.unknown_size_evicted_entries, 1);
        assert_eq!(decision.budget_evicted_entries, 0);
        assert_eq!(decision.available_entries, 2);
        assert_eq!(decision.available_bytes, 200);
    }

    #[test]
    fn retention_trims_oldest_buckets_until_byte_and_entry_limits_pass() {
        let decision = plan_texture_retention(
            10,
            LIMITS,
            &[
                TextureRetentionCandidate {
                    last_used_frame: 8,
                    available_entries: 3,
                    bytes_per_entry: Some(100),
                    uses: 1,
                },
                TextureRetentionCandidate {
                    last_used_frame: 10,
                    available_entries: 2,
                    bytes_per_entry: Some(100),
                    uses: 1,
                },
            ],
        );

        assert_eq!(decision.keep_entries, vec![1, 2]);
        assert_eq!(decision.budget_evicted_entries, 2);
        assert_eq!(decision.budget_evicted_bytes, 200);
        assert_eq!(decision.available_entries, 3);
        assert_eq!(decision.available_bytes, 300);
    }

    #[test]
    fn a_bucket_used_every_frame_outranks_one_used_once_on_the_same_frame() {
        // The failure this exists to prevent: a busy frame touches hundreds of sizes, so they all
        // share a last_used_frame and the tie broke on array index. A census of a real session
        // caught the result -- the 2560x1365 stage surface, requested every frame, allocated 141
        // times in twelve seconds because one-shot buckets kept winning the coin flip.
        let limits = BoundedTexturePoolLimits {
            max_cached_bytes: 100,
            max_cached_entries: 8,
            max_idle_frames: 1,
            max_cached_globals: 8,
        };
        // The hot bucket is listed FIRST, so index order alone would evict it.
        let decision = plan_texture_retention(
            5,
            limits,
            &[
                TextureRetentionCandidate {
                    last_used_frame: 5,
                    available_entries: 1,
                    bytes_per_entry: Some(100),
                    uses: 900,
                },
                TextureRetentionCandidate {
                    last_used_frame: 5,
                    available_entries: 1,
                    bytes_per_entry: Some(100),
                    uses: 1,
                },
            ],
        );

        assert_eq!(
            decision.keep_entries,
            vec![1, 0],
            "the size requested every frame must survive and the one-shot must go"
        );
    }

    #[test]
    fn retention_partially_trims_one_hot_bucket() {
        let decision = plan_texture_retention(
            4,
            LIMITS,
            &[TextureRetentionCandidate {
                last_used_frame: 4,
                available_entries: 6,
                bytes_per_entry: Some(100),
                uses: 1,
            }],
        );

        assert_eq!(decision.keep_entries, vec![3]);
        assert_eq!(decision.budget_evicted_entries, 3);
        assert_eq!(decision.available_bytes, 300);
    }

    #[test]
    fn globals_retention_applies_age_then_lru_count_limit() {
        let decision = plan_globals_retention(10, 2, 2, &[10, 7, 9, 8]);
        assert_eq!(decision.keep, vec![true, false, true, false]);
        assert_eq!(decision.age_evictions, 1);
        assert_eq!(decision.budget_evictions, 1);
        assert_eq!(decision.available_entries, 2);
    }
}
