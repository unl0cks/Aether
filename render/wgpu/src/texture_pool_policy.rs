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

/// Keep a much smaller companion pool for transient filter and surface intermediates whenever
/// the caller explicitly enables bounded offscreen reuse. These textures repeat frequently in
/// animated Flash content, but retaining every size encountered across maps and login sessions
/// causes GPU memory to grow without a useful bound.
pub(crate) fn general_texture_pool_policy(
    offscreen_policy: OffscreenTexturePoolPolicy,
) -> OffscreenTexturePoolPolicy {
    match offscreen_policy {
        OffscreenTexturePoolPolicy::Ephemeral => OffscreenTexturePoolPolicy::Ephemeral,
        OffscreenTexturePoolPolicy::BoundedReuse(limits) => {
            OffscreenTexturePoolPolicy::BoundedReuse(BoundedTexturePoolLimits {
                max_cached_bytes: (limits.max_cached_bytes / 4)
                    .clamp(16 * 1024 * 1024, 64 * 1024 * 1024),
                max_cached_entries: (limits.max_cached_entries / 4).clamp(32, 128),
                max_idle_frames: limits.max_idle_frames.min(1),
                max_cached_globals: limits.max_cached_globals.min(32),
            })
        }
    }
}

/// Bound the amount of cache/filter work recorded into one GPU command submission. The generic
/// renderer keeps its established batching, while Aether's bounded AQW path retains a safety
/// margin below that established limit. Sixty-four entries prevents an unbounded command buffer
/// without turning one dense AQW frame into a dozen separate queue submissions.
pub(crate) fn max_cache_entries_per_submission(policy: OffscreenTexturePoolPolicy) -> u32 {
    match policy {
        OffscreenTexturePoolPolicy::Ephemeral => 100,
        OffscreenTexturePoolPolicy::BoundedReuse(_) => 64,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TextureRetentionCandidate {
    pub last_used_frame: u64,
    pub available_entries: usize,
    pub bytes_per_entry: Option<u64>,
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
    order.sort_by_key(|index| (candidates[*index].last_used_frame, *index));

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

    const LIMITS: BoundedTexturePoolLimits = BoundedTexturePoolLimits {
        max_cached_bytes: 300,
        max_cached_entries: 3,
        max_idle_frames: 2,
        max_cached_globals: 2,
    };

    #[test]
    fn bounded_aqw_work_keeps_safe_but_coarser_gpu_submissions() {
        assert_eq!(
            max_cache_entries_per_submission(OffscreenTexturePoolPolicy::Ephemeral),
            100
        );
        assert_eq!(
            max_cache_entries_per_submission(OffscreenTexturePoolPolicy::BoundedReuse(LIMITS)),
            64
        );
    }

    #[test]
    fn bounded_offscreen_reuse_derives_a_smaller_general_pool() {
        assert_eq!(
            general_texture_pool_policy(OffscreenTexturePoolPolicy::Ephemeral),
            OffscreenTexturePoolPolicy::Ephemeral
        );
        assert_eq!(
            general_texture_pool_policy(OffscreenTexturePoolPolicy::BoundedReuse(
                BoundedTexturePoolLimits {
                    max_cached_bytes: 128 * 1024 * 1024,
                    max_cached_entries: 512,
                    max_idle_frames: 3,
                    max_cached_globals: 128,
                }
            )),
            OffscreenTexturePoolPolicy::BoundedReuse(BoundedTexturePoolLimits {
                max_cached_bytes: 32 * 1024 * 1024,
                max_cached_entries: 128,
                max_idle_frames: 1,
                max_cached_globals: 32,
            })
        );
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
                },
                TextureRetentionCandidate {
                    last_used_frame: 7,
                    available_entries: 1,
                    bytes_per_entry: Some(50),
                },
                TextureRetentionCandidate {
                    last_used_frame: 10,
                    available_entries: 1,
                    bytes_per_entry: None,
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
                },
                TextureRetentionCandidate {
                    last_used_frame: 10,
                    available_entries: 2,
                    bytes_per_entry: Some(100),
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
    fn retention_partially_trims_one_hot_bucket() {
        let decision = plan_texture_retention(
            4,
            LIMITS,
            &[TextureRetentionCandidate {
                last_used_frame: 4,
                available_entries: 6,
                bytes_per_entry: Some(100),
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
