mod commands;
pub mod target;

use crate::backend::RenderTargetMode;
use crate::blend::ComplexBlend;
use crate::buffer_pool::TexturePool;
use crate::dynamic_transforms::DynamicTransforms;
use crate::filters::FilterSource;
use crate::mesh::Mesh;
use crate::pixel_bender::{ShaderMode, run_pixelbender_shader_impl};
use crate::surface::commands::{Chunk, CommandRenderer, chunk_blends};
use crate::utils::supported_sample_count;
use crate::{Descriptors, MaskState, Pipelines};
use ruffle_render::commands::CommandList;
use ruffle_render::pixel_bender_support::{ImageInputTexture, PixelBenderShaderArgument};
use ruffle_render::quality::StageQuality;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use target::CommandTarget;
use tracing::instrument;

use crate::utils::run_copy_pipeline;

pub use crate::surface::commands::LayerRef;

use self::commands::ChunkBlendMode;

const LARGE_TARGET_MSAA_PIXEL_THRESHOLD: u64 = 2560 * 1440;

static BACKEND_MSAA_OVERRIDE: AtomicU8 = AtomicU8::new(0);

pub(crate) fn set_backend_msaa_override(sample_count: Option<u32>) {
    let sample_count = sample_count.unwrap_or(0);
    debug_assert!(matches!(sample_count, 0 | 2 | 4));
    BACKEND_MSAA_OVERRIDE.store(sample_count as u8, Ordering::Relaxed);
}

fn backend_msaa_override() -> Option<u32> {
    match BACKEND_MSAA_OVERRIDE.load(Ordering::Relaxed) {
        2 => Some(2),
        4 => Some(4),
        _ => None,
    }
}

fn backend_sample_count_for_target(
    quality: StageQuality,
    width: u32,
    height: u32,
    sample_count_override: Option<u32>,
) -> u32 {
    if let Some(sample_count) = sample_count_override {
        return sample_count;
    }

    let requested = quality.sample_count();
    let pixel_count = u64::from(width) * u64::from(height);

    if pixel_count >= LARGE_TARGET_MSAA_PIXEL_THRESHOLD {
        // AQW creates a stage-sized multisampled target for every blend and mask. At 1440p,
        // even 2x MSAA turns Medium into a severe bandwidth/allocation cliff on 4 GiB GPUs.
        // StageQuality remains unchanged for ActionScript and tessellation; only the backend
        // framebuffer sample count is capped for this target size.
        requested.min(1)
    } else {
        requested
    }
}

#[derive(Debug)]
pub struct Surface {
    size: wgpu::Extent3d,
    quality: StageQuality,
    sample_count: u32,
    pipelines: Arc<Pipelines>,
    format: wgpu::TextureFormat,
}

impl Surface {
    pub fn new(
        descriptors: &Descriptors,
        quality: StageQuality,
        width: u32,
        height: u32,
        frame_buffer_format: wgpu::TextureFormat,
    ) -> Self {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };

        let sample_count = supported_sample_count(
            &descriptors.adapter,
            backend_sample_count_for_target(quality, width, height, backend_msaa_override()),
            frame_buffer_format,
        );
        // The sample count is what decides whether a resolve is attached to a pass at all, and it is
        // reached by two different routes -- `StageQuality`, which AQW changes at runtime, and the
        // `--msaa` override. Reading it back from the surface itself is the only way to tell which
        // one won, and a perf capture is not interpretable without it.
        let pipelines = descriptors.pipelines(sample_count, frame_buffer_format);
        tracing::info!(
            "Surface {}x{} at quality {} using {}x MSAA",
            width,
            height,
            quality,
            sample_count,
        );
        Self {
            size,
            quality,
            sample_count,
            pipelines,
            format: frame_buffer_format,
        }
    }

    #[expect(clippy::too_many_arguments)]
    #[instrument(level = "debug", skip_all)]
    pub fn draw_commands_and_copy_to<'frame, 'global: 'frame>(
        &self,
        frame_view: &wgpu::TextureView,
        render_target_mode: RenderTargetMode,
        descriptors: &'global Descriptors,
        staging_belt: &'frame mut wgpu::util::StagingBelt,
        dynamic_transforms: &'global DynamicTransforms,
        draw_encoder: &'frame mut wgpu::CommandEncoder,
        meshes: &'global Vec<Mesh>,
        commands: CommandList,
        layer: LayerRef,
        texture_pool: &mut TexturePool,
    ) {
        let target = self.draw_commands(
            render_target_mode,
            descriptors,
            meshes,
            commands,
            staging_belt,
            dynamic_transforms,
            draw_encoder,
            layer,
            texture_pool,
        );

        run_copy_pipeline(
            descriptors,
            self.format,
            frame_view,
            target.color_view(),
            target.whole_frame_bind_group(descriptors),
            target.globals(),
            1,
            draw_encoder,
        );
    }

    #[expect(clippy::too_many_arguments)]
    #[instrument(level = "debug", skip_all)]
    pub fn draw_commands<'frame, 'global: 'frame>(
        &self,
        render_target_mode: RenderTargetMode,
        descriptors: &'global Descriptors,
        meshes: &'global Vec<Mesh>,
        commands: CommandList,
        staging_belt: &'global mut wgpu::util::StagingBelt,
        dynamic_transforms: &'global DynamicTransforms,
        draw_encoder: &'frame mut wgpu::CommandEncoder,
        nearest_layer: LayerRef<'frame>,
        texture_pool: &mut TexturePool,
    ) -> CommandTarget {
        self.draw_commands_at(
            (0, 0),
            render_target_mode,
            descriptors,
            meshes,
            commands,
            staging_belt,
            dynamic_transforms,
            draw_encoder,
            nearest_layer,
            texture_pool,
        )
    }

    /// As `draw_commands`, but the target covers only this surface's size starting at `origin` in
    /// the space the commands were recorded in. Blends use this to get a target the size of the
    /// blended object rather than the size of the stage.
    #[expect(clippy::too_many_arguments)]
    #[instrument(level = "debug", skip_all)]
    pub fn draw_commands_at<'frame, 'global: 'frame>(
        &self,
        origin: (u32, u32),
        render_target_mode: RenderTargetMode,
        descriptors: &'global Descriptors,
        meshes: &'global Vec<Mesh>,
        commands: CommandList,
        staging_belt: &'global mut wgpu::util::StagingBelt,
        dynamic_transforms: &'global DynamicTransforms,
        draw_encoder: &'frame mut wgpu::CommandEncoder,
        nearest_layer: LayerRef<'frame>,
        texture_pool: &mut TexturePool,
    ) -> CommandTarget {
        let target = CommandTarget::new_at(
            origin,
            descriptors,
            texture_pool,
            self.size,
            self.format,
            self.sample_count,
            render_target_mode,
            draw_encoder,
        );

        let mut num_masks = 0;
        let mut mask_state = MaskState::NoMask;
        let chunks = chunk_blends(
            commands,
            descriptors,
            staging_belt,
            dynamic_transforms,
            draw_encoder,
            meshes,
            self.quality,
            origin,
            target.width(),
            target.height(),
            match nearest_layer {
                LayerRef::Current => LayerRef::Parent(&target),
                layer => layer,
            },
            texture_pool,
        );

        // Peekable so a complex blend can gather the run of blends that follow it, which is what
        // lets a batch of them share one pass.
        let mut chunks = chunks.into_iter().peekable();
        while let Some(chunk) = chunks.next() {
            match chunk {
                Chunk::Draw {
                    chunk,
                    needs_stencil,
                    transforms,
                } => {
                    #[cfg(feature = "aether_metrics")]
                    crate::aether_metrics::record_encoded_chunk(chunk.len() as u64, false);
                    transforms.copy_to(
                        staging_belt,
                        &descriptors.device,
                        draw_encoder,
                        &dynamic_transforms.buffer,
                    );
                    let mut render_pass =
                        draw_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: create_debug_label!(
                                "Chunked draw calls {}",
                                if needs_stencil {
                                    "(with stencil)"
                                } else {
                                    "(Stencilless)"
                                }
                            )
                            .as_deref(),
                            color_attachments: &[target.color_attachments()],
                            depth_stencil_attachment: if needs_stencil {
                                target.stencil_attachment(descriptors, texture_pool)
                            } else {
                                None
                            },
                            ..Default::default()
                        });
                    render_pass.set_bind_group(0, target.globals().bind_group(), &[]);
                    let mut renderer = CommandRenderer::new(
                        &self.pipelines,
                        descriptors,
                        dynamic_transforms,
                        render_pass,
                        num_masks,
                        mask_state,
                        needs_stencil,
                    );

                    for command in &chunk {
                        renderer.execute(command);
                    }

                    num_masks = renderer.num_masks();
                    mask_state = renderer.mask_state();
                }
                Chunk::Blend {
                    texture,
                    blend_mode: ChunkBlendMode::Shader(shader),
                    needs_stencil,
                    // PixelBender blends run through their own shader, which does not read the
                    // region transform, so those are still allocated full-surface upstream.
                    region: _,
                } => {
                    assert!(!needs_stencil, "Shader blend mode not implemented in masks");
                    let parent_blend_buffer =
                        target.update_blend_buffer(descriptors, texture_pool, draw_encoder, None);
                    run_pixelbender_shader_impl(
                        descriptors,
                        shader,
                        ShaderMode::Filter,
                        &[
                            PixelBenderShaderArgument::ImageInput {
                                index: 0,
                                channels: 0xFF,
                                name: "background".to_string(),
                                texture: Some(ImageInputTexture::TextureRef(
                                    parent_blend_buffer.texture(),
                                )),
                            },
                            PixelBenderShaderArgument::ImageInput {
                                index: 1,
                                channels: 0xff,
                                name: "foreground".to_string(),
                                texture: Some(ImageInputTexture::TextureRef(texture.texture())),
                            },
                        ],
                        parent_blend_buffer.texture(),
                        draw_encoder,
                        target.color_attachments(),
                        target.sample_count(),
                        &FilterSource {
                            texture: texture.texture(),
                            view: texture.view().clone(),
                            point: (0, 0),
                            size: (texture.texture().width(), texture.texture().height()),
                        },
                    )
                    .expect("Failed to run PixelBender blend mode");
                }
                Chunk::Blend {
                    texture,
                    blend_mode: ChunkBlendMode::Complex(blend_mode),
                    needs_stencil,
                    region,
                } => {
                    let parent = match blend_mode {
                        ComplexBlend::Alpha | ComplexBlend::Erase => {
                            match nearest_layer {
                                LayerRef::None => {
                                    // An Alpha or Erase with no Layer above it should be ignored
                                    continue;
                                }
                                LayerRef::Current => &target,
                                LayerRef::Parent(layer) => layer,
                            }
                        }
                        _ => &target,
                    };

                    // Gather the run of blends this one can share a pass with.
                    //
                    // Frame time on this renderer is render passes, not draws, and a crowded AQW
                    // map spends about three passes on every complex blend. Blends that do not
                    // overlap cannot read what the others wrote, so a run of them composites
                    // identically whether it happens in one pass or several. Two modes account for
                    // 98% of the blends AQW issues, so such runs are the common case rather than a
                    // lucky one.
                    //
                    // Alpha and Erase stay out of it: they resolve their parent through the nearest
                    // layer and may be skipped entirely, which a batch would have to special-case
                    // for no gain, since between them they are a rounding error of AQW's blends.
                    let mut batch = vec![(texture, region)];
                    if crate::surface::commands::blend_batching_enabled()
                        && !matches!(blend_mode, ComplexBlend::Alpha | ComplexBlend::Erase)
                    {
                        while let Some(Chunk::Blend {
                            blend_mode: ChunkBlendMode::Complex(next_mode),
                            needs_stencil: next_needs_stencil,
                            region: next_region,
                            ..
                        }) = chunks.peek()
                        {
                            // Same pipeline, same stencil setup, and nothing it would need to read
                            // back from a blend already in the batch.
                            if std::mem::discriminant(next_mode)
                                != std::mem::discriminant(&blend_mode)
                                || *next_needs_stencil != needs_stencil
                                || batch.iter().any(|(_, region)| {
                                    crate::surface::commands::blend_regions_overlap(
                                        *region,
                                        *next_region,
                                    )
                                })
                            {
                                break;
                            }

                            let Some(Chunk::Blend { texture, region, .. }) = chunks.next() else {
                                unreachable!("peek just matched a complex blend")
                            };
                            batch.push((texture, region));
                        }
                    }

                    #[cfg(feature = "aether_metrics")]
                    {
                        // One pass for the batch, but each blend is still a blend.
                        crate::aether_metrics::record_encoded_chunk(0, true);
                        for _ in 0..batch.len() {
                            crate::aether_metrics::record_complex_blend(blend_mode.metrics_index());
                        }
                    }

                    // Every region is snapshotted before anything draws, so each blend reads the
                    // parent as it stood before the batch began. That is the same backdrop it would
                    // have read one pass at a time, precisely because none of them overlap.
                    let mut parent_blend_buffer = None;
                    for (_, region) in &batch {
                        parent_blend_buffer = Some(parent.update_blend_buffer(
                            descriptors,
                            texture_pool,
                            draw_encoder,
                            *region,
                        ));
                    }
                    let parent_blend_buffer =
                        parent_blend_buffer.expect("a batch always holds at least one blend");

                    let blend_bind_groups = batch
                        .iter()
                        .map(|(texture, _)| {
                            #[cfg(feature = "aether_metrics")]
                            crate::aether_metrics::record_bind_group_created();
                            descriptors
                                .device
                                .create_bind_group(&wgpu::BindGroupDescriptor {
                                    label: create_debug_label!(
                                        "Complex blend binds {:?} {}",
                                        blend_mode,
                                        if needs_stencil {
                                            "(with stencil)"
                                        } else {
                                            "(Stencilless)"
                                        }
                                    )
                                    .as_deref(),
                                    layout: &descriptors.bind_layouts.blend,
                                    entries: &[
                                        wgpu::BindGroupEntry {
                                            binding: 0,
                                            resource: wgpu::BindingResource::TextureView(
                                                parent_blend_buffer.view(),
                                            ),
                                        },
                                        wgpu::BindGroupEntry {
                                            binding: 1,
                                            resource: wgpu::BindingResource::TextureView(
                                                texture.view(),
                                            ),
                                        },
                                        wgpu::BindGroupEntry {
                                            binding: 2,
                                            resource: wgpu::BindingResource::Sampler(
                                                descriptors
                                                    .bitmap_samplers
                                                    .get_sampler(false, false),
                                            ),
                                        },
                                    ],
                                })
                        })
                        .collect::<Vec<_>>();

                    // A child covering only part of the target needs its quad placed there and its
                    // own UV remap; a full-surface child keeps the cached whole-frame group.
                    let region_bind_groups = batch
                        .iter()
                        .map(|(texture, region)| {
                            region.map(|region| {
                                target.region_frame_bind_group(
                                    descriptors,
                                    region,
                                    texture.texture().size(),
                                )
                            })
                        })
                        .collect::<Vec<_>>();

                    let mut render_pass =
                        draw_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: create_debug_label!(
                                "Complex blend {:?} {}",
                                blend_mode,
                                if needs_stencil {
                                    "(with stencil)"
                                } else {
                                    "(Stencilless)"
                                }
                            )
                            .as_deref(),
                            color_attachments: &[target.color_attachments()],
                            depth_stencil_attachment: if needs_stencil {
                                target.stencil_attachment(descriptors, texture_pool)
                            } else {
                                None
                            },
                            ..Default::default()
                        });
                    render_pass.set_bind_group(0, target.globals().bind_group(), &[]);

                    if needs_stencil {
                        match mask_state {
                            MaskState::NoMask => {}
                            MaskState::DrawMaskStencil => {
                                render_pass.set_stencil_reference(num_masks - 1);
                            }
                            MaskState::DrawMaskedContent => {
                                render_pass.set_stencil_reference(num_masks);
                            }
                            MaskState::ClearMaskStencil => {
                                render_pass.set_stencil_reference(num_masks);
                            }
                        }
                        render_pass.set_pipeline(
                            self.pipelines.complex_blends[blend_mode].pipeline_for(mask_state),
                        );
                    } else {
                        render_pass.set_pipeline(
                            self.pipelines.complex_blends[blend_mode].stencilless_pipeline(),
                        );
                    }

                    render_pass.set_vertex_buffer(0, descriptors.quad.vertices_pos.slice(..));
                    render_pass.set_index_buffer(
                        descriptors.quad.indices.slice(..),
                        wgpu::IndexFormat::Uint32,
                    );

                    // The geometry and pipeline are set once for the batch; only the two bind
                    // groups that identify a particular child change between draws.
                    for (blend_bind_group, region_bind_group) in
                        blend_bind_groups.iter().zip(&region_bind_groups)
                    {
                        render_pass.set_bind_group(
                            1,
                            region_bind_group
                                .as_deref()
                                .unwrap_or_else(|| target.whole_frame_bind_group(descriptors)),
                            &[0],
                        );
                        render_pass.set_bind_group(2, blend_bind_group, &[]);
                        render_pass.draw_indexed(0..6, 0, 0..1);
                    }
                }
            }

            // The pass is encoded and its borrow of the encoder has ended, so this is the only
            // point in the loop where the command buffer can be handed over. A crowded map encodes
            // hundreds of passes here and every one of them used to reach the driver as a single
            // submission; see `submission_splitter` for why that is suspected of losing the device.
            crate::submission_splitter::note_pass_and_maybe_split(
                descriptors,
                draw_encoder,
                staging_belt,
            );
        }

        // If nothing happened, ensure it's cleared so we don't operate on garbage data
        target.ensure_cleared(draw_encoder);

        // The target is finished, and every caller reads it through the resolved texture: the frame
        // copy, a blend or mask taking it as a child, the backend handing it to a BitmapData. This
        // is the one place that knows drawing has stopped, which is what makes the resolve
        // deferrable at all -- see `CommandTarget::deferred_resolve`.
        target.resolve_now(draw_encoder);

        target
    }

    pub fn quality(&self) -> StageQuality {
        self.quality
    }

    pub fn sample_count(&self) -> u32 {
        self.sample_count
    }

    pub fn size(&self) -> wgpu::Extent3d {
        self.size
    }
}

#[cfg(test)]
mod tests {
    use super::backend_sample_count_for_target;
    use ruffle_render::quality::StageQuality;

    #[test]
    fn backend_sample_count_avoids_the_msaa_bandwidth_cliff_at_1440p() {
        assert_eq!(
            backend_sample_count_for_target(StageQuality::High, 2560, 1440, None),
            1
        );
        assert_eq!(
            backend_sample_count_for_target(StageQuality::Medium, 2560, 1440, None),
            1
        );
    }

    #[test]
    fn backend_sample_count_preserves_smaller_and_lower_quality_targets() {
        assert_eq!(
            backend_sample_count_for_target(StageQuality::High, 1920, 1080, None),
            4
        );
        assert_eq!(
            backend_sample_count_for_target(StageQuality::Low, 2560, 1440, None),
            1
        );
    }

    #[test]
    fn explicit_msaa_overrides_the_adaptive_large_target_cap() {
        assert_eq!(
            backend_sample_count_for_target(StageQuality::High, 2560, 1440, Some(2)),
            2
        );
        assert_eq!(
            backend_sample_count_for_target(StageQuality::Low, 2560, 1440, Some(4)),
            4
        );
    }
}
