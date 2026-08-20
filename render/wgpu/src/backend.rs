use crate::buffer_builder::BufferBuilder;
use crate::buffer_pool::{BufferPool, TexturePool};
use crate::context3d::WgpuContext3D;
use crate::dynamic_transforms::DynamicTransforms;
use crate::filters::FilterSource;
use crate::mesh::{CommonGradient, GradientAtlas, Mesh, PendingDraw};
use crate::pixel_bender::{ShaderMode, run_pixelbender_shader_impl};
use crate::surface::target::CommandTarget;
use crate::surface::{LayerRef, Surface};
use crate::target::{MaybeOwnedBuffer, TextureTarget};
use crate::target::{RenderTargetFrame, TextureBufferInfo};
use crate::texture_pool_policy::{
    OffscreenTexturePoolPolicy, general_texture_pool_policy, is_amd_vulkan,
    max_cache_entries_per_submission,
};
use crate::utils::BufferDimensions;
use crate::{
    Descriptors, Error, QueueSyncHandle, RenderTarget, SwapChainTarget, Texture, as_texture,
    format_list, get_backend_names,
};
use image::imageops::FilterType;
use ruffle_render::backend::{
    BitmapCacheEntry, Context3D, Context3DProfile, PixelBenderOutput, PixelBenderTarget,
};
use ruffle_render::backend::{RenderBackend, ShapeHandle, ViewportDimensions};
use ruffle_render::bitmap::{
    Bitmap, BitmapFormat, BitmapHandle, BitmapSource, PixelRegion, RgbaBufRead, SyncHandle,
};
use ruffle_render::commands::CommandList;
use ruffle_render::error::Error as BitmapError;
use ruffle_render::filters::Filter;
use ruffle_render::pixel_bender::{PixelBenderShader, PixelBenderShaderHandle};
use ruffle_render::pixel_bender_support::PixelBenderShaderArgument;
use ruffle_render::quality::StageQuality;
use ruffle_render::shape_utils::DistilledShape;
use ruffle_render::tessellator::ShapeTessellator;
use std::any::Any;
use std::borrow::Cow;
use std::cell::Cell;
use std::collections::VecDeque;
use std::num::NonZeroU32;
use std::sync::Arc;
use swf::Color;
use tracing::instrument;
use wgpu::SubmissionIndex;

/// Creates a wgpu instance with Ruffle's required configuration.
///
/// This disables indirect call validation because wgpu's validation runs a compute
/// shader that uses `array<u32>`, which requires the `DYNAMIC_ARRAY_SIZE` feature.
/// However, wgpu runs this shader without first checking if the device supports
/// that feature, causing device creation to fail on GPUs that lack it.
/// Since Ruffle doesn't use indirect draws, disabling this validation has no
/// functional impact.
///
/// See <https://github.com/gfx-rs/wgpu/issues/8799>
pub fn create_wgpu_instance(
    backends: wgpu::Backends,
    backend_options: wgpu::BackendOptions,
) -> wgpu::Instance {
    wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends,
        flags: wgpu::InstanceFlags::default()
            .difference(wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL)
            .with_env(),
        backend_options,
        ..Default::default()
    })
}

pub struct WgpuRenderBackend<T: RenderTarget> {
    pub(crate) descriptors: Arc<Descriptors>,
    target: T,
    surface: Surface,
    meshes: Vec<Mesh>,
    shape_tessellator: ShapeTessellator,
    gradient_atlas: GradientAtlas,
    // This is currently unused - we just store it to report in
    // `get_viewport_dimensions`
    viewport_scale_factor: f64,
    texture_pool: TexturePool,
    offscreen_texture_pool: TexturePool,
    pub(crate) offscreen_buffer_pool: Arc<BufferPool<wgpu::Buffer, BufferDimensions>>,
    dynamic_transforms: DynamicTransforms,
    active_frame: ActiveFrame,
}

impl WgpuRenderBackend<SwapChainTarget> {
    #[cfg(target_family = "wasm")]
    pub async fn for_canvas(
        canvas: web_sys::HtmlCanvasElement,
        webgpu: bool,
    ) -> Result<Self, Error> {
        let backends = if webgpu {
            wgpu::Backends::BROWSER_WEBGPU
        } else {
            wgpu::Backends::GL
        };
        let instance = create_wgpu_instance(
            backends,
            wgpu::BackendOptions {
                gl: wgpu::GlBackendOptions {
                    // See <https://github.com/gfx-rs/wgpu/releases/tag/v25.0.0>
                    fence_behavior: wgpu::GlFenceBehavior::AutoFinish,
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        let surface = instance.create_surface(wgpu::SurfaceTarget::Canvas(canvas))?;
        let (adapter, device, queue) = request_adapter_and_device(
            backends,
            &instance,
            Some(&surface),
            wgpu::PowerPreference::HighPerformance,
        )
        .await?;
        let descriptors = Descriptors::new(instance, adapter, device, queue);
        let target =
            SwapChainTarget::new(surface, &descriptors.adapter, (1, 1), &descriptors.device);
        Self::new(Arc::new(descriptors), target)
    }

    /// # Safety
    ///  See [`wgpu::SurfaceTargetUnsafe`] variants for safety requirements.
    #[cfg(not(target_family = "wasm"))]
    pub unsafe fn for_window_unsafe(
        window: wgpu::SurfaceTargetUnsafe,
        size: (u32, u32),
        backend: wgpu::Backends,
        power_preference: wgpu::PowerPreference,
    ) -> Result<Self, Error> {
        if wgpu::Backends::SECONDARY.contains(backend) {
            tracing::warn!(
                "{} graphics backend support may not be fully supported.",
                format_list(&get_backend_names(backend), "and")
            );
        }
        let instance = create_wgpu_instance(backend, wgpu::BackendOptions::default());
        let surface = unsafe { instance.create_surface_unsafe(window)? };
        let (adapter, device, queue) = futures::executor::block_on(request_adapter_and_device(
            backend,
            &instance,
            Some(&surface),
            power_preference,
        ))?;
        let descriptors = Descriptors::new(instance, adapter, device, queue);
        let target = SwapChainTarget::new(surface, &descriptors.adapter, size, &descriptors.device);
        Self::new(Arc::new(descriptors), target)
    }

    /// # Safety
    ///  See [`wgpu::SurfaceTargetUnsafe`] variants for safety requirements.
    #[cfg(not(target_family = "wasm"))]
    pub unsafe fn recreate_surface_unsafe(
        &mut self,
        window: wgpu::SurfaceTargetUnsafe,
        size: (u32, u32),
    ) -> Result<(), Error> {
        let descriptors = &self.descriptors;
        let surface = unsafe { descriptors.wgpu_instance.create_surface_unsafe(window)? };
        self.target =
            SwapChainTarget::new(surface, &descriptors.adapter, size, &descriptors.device);
        Ok(())
    }
}

#[cfg(not(target_family = "wasm"))]
impl WgpuRenderBackend<crate::target::TextureTarget> {
    pub fn for_offscreen(
        size: (u32, u32),
        backend: wgpu::Backends,
        power_preference: wgpu::PowerPreference,
    ) -> Result<Self, Error> {
        if wgpu::Backends::SECONDARY.contains(backend) {
            tracing::warn!(
                "{} graphics backend support may not be fully supported.",
                format_list(&get_backend_names(backend), "and")
            );
        }
        let instance = create_wgpu_instance(backend, wgpu::BackendOptions::default());
        let (adapter, device, queue) = futures::executor::block_on(request_adapter_and_device(
            backend,
            &instance,
            None,
            power_preference,
        ))?;
        let descriptors = Descriptors::new(instance, adapter, device, queue);
        let target = crate::target::TextureTarget::new(&descriptors.device, size)?;
        Self::new(Arc::new(descriptors), target)
    }

    pub fn capture_frame(&self) -> Option<image::RgbaImage> {
        use crate::utils::buffer_to_image;
        if let Some(buffer) = &self.target.buffer {
            let (buffer, dimensions) = buffer.buffer.inner();
            buffer_to_image(
                &self.descriptors.device,
                buffer,
                dimensions,
                None,
                self.target.size,
            )
        } else {
            None
        }
    }
}

impl<T: RenderTarget> WgpuRenderBackend<T> {
    pub fn new(descriptors: Arc<Descriptors>, target: T) -> Result<Self, Error> {
        Self::new_with_offscreen_texture_pool_policy(
            descriptors,
            target,
            OffscreenTexturePoolPolicy::Ephemeral,
        )
    }

    pub fn new_with_offscreen_texture_pool_policy(
        descriptors: Arc<Descriptors>,
        target: T,
        offscreen_texture_pool_policy: OffscreenTexturePoolPolicy,
    ) -> Result<Self, Error> {
        if target.width() > descriptors.limits.max_texture_dimension_2d
            || target.height() > descriptors.limits.max_texture_dimension_2d
        {
            return Err(format!(
                "Render target texture cannot be larger than {}px on either dimension (requested {} x {})",
                descriptors.limits.max_texture_dimension_2d,
                target.width(),
                target.height()
            )
                .into());
        }

        let surface = Surface::new(
            &descriptors,
            StageQuality::Low,
            target.width(),
            target.height(),
            target.format(),
        );

        let offscreen_buffer_pool = BufferPool::new(Box::new(
            |descriptors: &Descriptors, dimensions: &BufferDimensions| {
                descriptors.device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: dimensions.size(),
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                })
            },
        ));

        let transforms = DynamicTransforms::new(&descriptors);
        let adapter_info = descriptors.adapter.get_info();
        let active_frame = ActiveFrame::new(
            &descriptors,
            max_cache_entries_per_submission(offscreen_texture_pool_policy, &adapter_info),
            submission_retirement_limit_for_adapter_info(&adapter_info),
        );

        Ok(Self {
            descriptors,
            target,
            surface,
            meshes: Vec::new(),
            shape_tessellator: ShapeTessellator::new(),
            gradient_atlas: GradientAtlas::default(),
            viewport_scale_factor: 1.0,
            texture_pool: TexturePool::new(
                general_texture_pool_policy(offscreen_texture_pool_policy, &adapter_info),
                // Its callers already snap blend and mask regions to a 128px grid, it reuses 99.8%
                // across a couple of dozen buckets, and it holds the surface that reaches the
                // window. Nothing to gain and a blit to get wrong.
                false,
                #[cfg(feature = "aether_metrics")]
                crate::aether_metrics::TexturePoolKind::General,
            ),
            offscreen_texture_pool: TexturePool::new(
                offscreen_texture_pool_policy,
                // Where the 1,450 size buckets lived. Its textures are mostly filter
                // intermediates -- `blur` takes a flip and a flop per pass, which is why the
                // allocations before the last fault came in identical pairs -- so collapsing them
                // required teaching the filters that a pooled target can be larger than the region
                // drawn into it: a viewport confines `position`, and `FilterRegion` stops the UVs
                // at the content.
                //
                // Still off, because that is necessary and not sufficient. Rendering the AQW loader
                // through the offscreen exporter with this on moves 603,515 pixels on the first
                // frame against a byte-identical baseline -- and the same SWF renders identically
                // twice in a row, so that is a regression and not noise. The filter fixes above are
                // pixel-identical to the code before them, so whatever still assumes a pooled
                // target is exactly its texture lives outside the filters: the remaining suspects
                // are `CommandTarget`'s whole-frame bind group and the blend and mask compositing
                // in `surface::commands`, both of which sample a target back by its texture.
                //
                // `_evidence/filtercheck` has the harness. Render before and after, compare, and
                // this is a measurement rather than a hope. For the blend and mask half, use
                // `_evidence/blend_corpus.swf` -- generated by `_tools/blend_corpus`, because no
                // SWF in the tree exercised more than four complex blends and AQW's own are
                // authored into armour and map files an offline export never loads.
                //
                // **Resolved 2026-08-19, and the suspicion above was right.** The filter subset is
                // now rounded on its own, at the one place that only filters reach
                // (`CommandTarget::new_for_filter`), and this pool-wide switch stays off. Measured
                // with the harness: 12 frames of `Game3098r24.swf` exercising 63 filter targets are
                // byte-identical, and stay byte-identical with 300 pixels of deliberate extra
                // padding on every filter target -- far past anything the grid produces. So the
                // viewport and `FilterRegion` really do confine a filter to its region, and
                // whatever moved those 603,515 pixels is in `new_at`'s callers, which place
                // geometry with the globals view matrix and set no viewport at all.
                false,
                #[cfg(feature = "aether_metrics")]
                crate::aether_metrics::TexturePoolKind::Offscreen,
            ),
            offscreen_buffer_pool: Arc::new(offscreen_buffer_pool),
            dynamic_transforms: transforms,
            active_frame,
        })
    }

    fn register_shape_internal(
        &mut self,
        shape: DistilledShape,
        bitmap_source: &dyn BitmapSource,
        scale: f32,
    ) -> Mesh {
        let shape_id = shape.id;
        // Taken before tessellation, which consumes the shape and keeps no record of its extent.
        let shape_bounds = shape.shape_bounds;
        let lyon_mesh =
            self.shape_tessellator
                .tessellate_shape_with_scale(shape, bitmap_source, scale);

        let mut draws = Vec::with_capacity(lyon_mesh.draws.len());
        let mut uniform_buffer = BufferBuilder::new_for_uniform(&self.descriptors.limits);
        let mut vertex_buffer = BufferBuilder::new_for_vertices(&self.descriptors.limits);
        let mut index_buffer = BufferBuilder::new_for_vertices(&self.descriptors.limits);
        let mut gradients = Vec::with_capacity(lyon_mesh.gradients.len());

        for gradient in lyon_mesh.gradients {
            gradients.push(CommonGradient::new(
                &self.descriptors,
                gradient,
                &mut self.gradient_atlas,
                &mut uniform_buffer,
            ));
        }

        for draw in lyon_mesh.draws {
            let draw_id = draws.len();
            if let Some(draw) = PendingDraw::new(
                self,
                bitmap_source,
                draw,
                shape_id,
                draw_id,
                &mut uniform_buffer,
                &mut vertex_buffer,
                &mut index_buffer,
            ) {
                draws.push(draw);
            }
        }

        let uniform_buffer = uniform_buffer.finish(
            &self.descriptors.device,
            create_debug_label!("Shape {} uniforms", shape_id),
            wgpu::BufferUsages::UNIFORM,
        );
        let vertex_buffer = vertex_buffer.finish(
            &self.descriptors.device,
            create_debug_label!("Shape {} vertices", shape_id),
            wgpu::BufferUsages::VERTEX,
        );
        let index_buffer = index_buffer.finish(
            &self.descriptors.device,
            create_debug_label!("Shape {} indices", shape_id),
            wgpu::BufferUsages::INDEX,
        );

        let draws = PendingDraw::finish_all(draws, &self.descriptors, &uniform_buffer, &gradients);

        Mesh {
            draws,
            vertex_buffer,
            index_buffer,
            shape_bounds,
        }
    }

    fn clamp_bitmap(&self, bitmap: &mut Bitmap) -> bool {
        let max_size = self.descriptors.limits.max_texture_dimension_2d;
        if bitmap.width() > max_size || bitmap.height() > max_size {
            let image =
                image::RgbaImage::from_raw(bitmap.width(), bitmap.height(), bitmap.data().to_vec())
                    .expect("Width and height of bitmap must match bitmap data");

            let ratio = bitmap.width() as f32 / bitmap.height() as f32;
            let mut width = bitmap.width();
            let mut height = bitmap.height();
            if width > max_size {
                width = max_size;
                height = (max_size as f32 / ratio) as u32;
            }
            if height > max_size {
                height = max_size;
                width = (max_size as f32 * ratio) as u32;
            }
            let resized = image::imageops::resize(&image, width, height, FilterType::CatmullRom);
            *bitmap = Bitmap::new(width, height, BitmapFormat::Rgba, resized.into_raw());
            true
        } else {
            false
        }
    }

    pub fn descriptors(&self) -> &Arc<Descriptors> {
        &self.descriptors
    }

    pub fn target(&self) -> &T {
        &self.target
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.descriptors.device
    }

    pub fn make_queue_sync_handle(
        &self,
        target: TextureTarget,
        index: Option<SubmissionIndex>,
        destination: BitmapHandle,
        copy_area: PixelRegion,
    ) -> Box<QueueSyncHandle> {
        match target.take_buffer() {
            None => Box::new(QueueSyncHandle::NotCopied {
                handle: destination,
                copy_area,
                descriptors: self.descriptors.clone(),
                pool: self.offscreen_buffer_pool.clone(),
            }),
            Some(TextureBufferInfo {
                buffer: MaybeOwnedBuffer::Borrowed(buffer, copy_dimensions),
                ..
            }) => Box::new(QueueSyncHandle::AlreadyCopied {
                index,
                buffer,
                copy_dimensions,
                descriptors: self.descriptors.clone(),
            }),
            Some(TextureBufferInfo {
                buffer: MaybeOwnedBuffer::Owned(..),
                ..
            }) => unreachable!("Buffer must be Borrowed as it was set to be Borrowed earlier"),
        }
    }
}

impl<T: RenderTarget + 'static> WgpuRenderBackend<T> {
    /// Render a run of cache entries that share a blur kernel, blurring all of them together.
    ///
    /// Three phases, and the order is the design. Every member is drawn first, because the atlas is
    /// built out of what they drew. Then one blur covers the lot. Only then does each member's
    /// filter run, reading its own slot of the blurred atlas.
    ///
    /// That middle phase is exactly why [`plan_cache_entry_groups`] refuses to group an entry that
    /// draws another member's texture: this defers every member's copy-back past every member's
    /// draw, and a parent compositing its child before the child's filter landed would flicker for
    /// one frame and then look correct again.
    ///
    /// Falls back to filtering each member on its own whenever the group cannot be served -- an
    /// atlas that will not fit, a multisampled source it cannot copy, a blur that turns out to be a
    /// no-op. That fallback is the ordinary path, not an error path.
    fn render_atlased_cache_group(&mut self, group: Vec<BitmapCacheEntry>) -> Vec<u64> {
        #[cfg(feature = "aether_metrics")]
        let group_started = std::time::Instant::now();
        #[cfg_attr(not(feature = "aether_metrics"), allow(unused_mut))]
        let mut signatures = Vec::new();

        // Phase A: draw every member, keeping its target so the atlas can be built from them.
        let mut drawn: Vec<DrawnCacheEntry> = Vec::with_capacity(group.len());
        for entry in group {
            #[cfg(feature = "aether_metrics")]
            crate::aether_metrics::record_cache_entry(entry.filters.len() as u64);
            let BitmapCacheEntry {
                handle,
                commands,
                clear,
                logical_width,
                logical_height,
                filters,
            } = entry;
            let cache_texture = as_texture(&handle).texture.clone();
            let logical = bitmap_cache_filter_source_size(
                (cache_texture.width(), cache_texture.height()),
                (logical_width, logical_height),
            );
            let surface = Surface::new(
                &self.descriptors,
                self.surface.quality(),
                cache_texture.width(),
                cache_texture.height(),
                wgpu::TextureFormat::Rgba8Unorm,
            );
            let target = surface.draw_commands(
                RenderTargetMode::ExistingWithColor(
                    cache_texture.clone(),
                    wgpu::Color {
                        r: f64::from(clear.r) / 255.0,
                        g: f64::from(clear.g) / 255.0,
                        b: f64::from(clear.b) / 255.0,
                        a: f64::from(clear.a) / 255.0,
                    },
                ),
                &self.descriptors,
                &self.meshes,
                commands,
                &mut self.active_frame.staging_belt,
                &self.dynamic_transforms,
                &mut self.active_frame.command_encoder,
                LayerRef::None,
                &mut self.offscreen_texture_pool,
            );
            // The planner only admits entries carrying exactly one filter.
            let Some(filter) = filters.into_iter().next() else {
                continue;
            };
            #[cfg(feature = "aether_metrics")]
            if let Some(signature) = crate::aether_metrics::atlasable_filter_signature(&filter) {
                signatures.push(signature);
            }
            drawn.push(DrawnCacheEntry {
                cache_texture,
                logical,
                filter,
                target,
            });
        }

        // Phase B: pack the sources and blur the whole atlas once.
        let atlased = self.blur_group_into_atlas(&drawn);

        // Phase C: each member's filter, reading either its atlas slot or nothing at all.
        for (index, entry) in drawn.iter().enumerate() {
            let pre_blurred = atlased.as_ref().map(|(blurred_view, texture_size, slots)| {
                let slot = slots[index];
                crate::filters::PreBlurred {
                    view: blurred_view,
                    region: crate::filters::FilterRegion {
                        texture_size: *texture_size,
                        point: (slot.x, slot.y),
                        size: (slot.width, slot.height),
                    },
                }
            });
            let output = self.descriptors.filters.apply_with_pre_blurred(
                &self.descriptors,
                &mut self.active_frame.command_encoder,
                &mut self.offscreen_texture_pool,
                &mut self.active_frame.staging_belt,
                FilterSource {
                    texture: entry.target.color_texture(),
                    view: entry.target.color_view().clone(),
                    point: (0, 0),
                    size: entry.logical,
                },
                entry.filter.clone(),
                pre_blurred,
            );
            self.active_frame.command_encoder.copy_texture_to_texture(
                output.color_texture().as_image_copy(),
                entry.cache_texture.as_image_copy(),
                wgpu::Extent3d {
                    width: entry.logical.0,
                    height: entry.logical.1,
                    depth_or_array_layers: 1,
                },
            );
        }

        #[cfg(feature = "aether_metrics")]
        crate::aether_metrics::record_cache_entry_time(true, group_started.elapsed());
        signatures
    }

    /// Pack the group's sources into one texture and blur it, returning where each source landed.
    ///
    /// `None` means this group has to be filtered one at a time after all, and every reason for
    /// that is a legitimate shape of content rather than a failure.
    fn blur_group_into_atlas(
        &mut self,
        drawn: &[DrawnCacheEntry],
    ) -> Option<(
        wgpu::TextureView,
        (u32, u32),
        Vec<crate::filters::atlas::AtlasSlot>,
    )> {
        let first = drawn.first()?;
        if drawn.len() < 2 {
            return None;
        }
        // A multisampled source cannot be copied into the atlas, and resolving one here would cost
        // more than the passes being saved.
        if drawn
            .iter()
            .any(|entry| entry.target.color_texture().sample_count() != 1)
        {
            return None;
        }

        let blur = crate::filters::filter_inner_blur(&first.filter)?;
        let (blur_x, blur_y, passes) = crate::filters::filter_shares_a_blur(&first.filter)?;
        let sizes: Vec<(u32, u32)> = drawn.iter().map(|entry| entry.logical).collect();
        let packing = crate::filters::atlas::pack_atlas(
            &sizes,
            crate::filters::atlas::blur_reach(blur_x, passes),
            crate::filters::atlas::blur_reach(blur_y, passes),
            self.descriptors.limits.max_texture_dimension_2d,
        )?;

        let atlas = CommandTarget::new_for_filter(
            &self.descriptors,
            &mut self.offscreen_texture_pool,
            wgpu::Extent3d {
                width: packing.width,
                height: packing.height,
                depth_or_array_layers: 1,
            },
            wgpu::TextureFormat::Rgba8Unorm,
            1,
            RenderTargetMode::FreshWithColor(wgpu::Color::TRANSPARENT),
            &mut self.active_frame.command_encoder,
        );
        // The padding between slots has to actually be transparent, or every object in the group
        // blurs whatever the pooled texture happened to contain into its own edges.
        atlas.ensure_cleared(&mut self.active_frame.command_encoder);

        for slot in &packing.slots {
            let mut destination = atlas.color_texture().as_image_copy();
            destination.origin = wgpu::Origin3d {
                x: slot.x,
                y: slot.y,
                z: 0,
            };
            self.active_frame.command_encoder.copy_texture_to_texture(
                drawn[slot.index].target.color_texture().as_image_copy(),
                destination,
                wgpu::Extent3d {
                    width: slot.width,
                    height: slot.height,
                    depth_or_array_layers: 1,
                },
            );
        }

        let blurred = self.descriptors.filters.blur_together(
            &self.descriptors,
            &mut self.offscreen_texture_pool,
            &mut self.active_frame.command_encoder,
            &mut self.active_frame.staging_belt,
            &FilterSource {
                texture: atlas.color_texture(),
                view: atlas.color_view().clone(),
                point: (0, 0),
                size: (packing.width, packing.height),
            },
            &blur,
        )?;
        blurred.ensure_cleared(&mut self.active_frame.command_encoder);

        let texture_size = (
            blurred.color_texture().width(),
            blurred.color_texture().height(),
        );
        #[cfg(feature = "aether_metrics")]
        crate::aether_metrics::record_filter_atlas(
            packing.slots.len() as u64,
            u64::from(packing.width) * u64::from(packing.height),
            sizes
                .iter()
                .map(|&(w, h)| u64::from(w) * u64::from(h))
                .sum(),
        );
        Some((blurred.color_view().clone(), texture_size, packing.slots))
    }
}

impl<T: RenderTarget + 'static> RenderBackend for WgpuRenderBackend<T> {
    fn set_viewport_dimensions(&mut self, dimensions: ViewportDimensions) {
        // Avoid panics from creating 0-sized framebuffers.
        // TODO: find a way to bubble an error when the size is too large
        let width = std::cmp::max(
            std::cmp::min(
                dimensions.width,
                self.descriptors.limits.max_texture_dimension_2d,
            ),
            1,
        );
        let height = std::cmp::max(
            std::cmp::min(
                dimensions.height,
                self.descriptors.limits.max_texture_dimension_2d,
            ),
            1,
        );
        // Dragging a window on Windows delivers one of these per frame for the whole drag, and
        // dragging it by the title bar delivers them with nothing changed at all. Everything below
        // is expensive, so the size it is already at costs nothing.
        if !viewport_change_is_real(
            self.surface.size(),
            self.viewport_scale_factor,
            width,
            height,
            dimensions.scale_factor,
        ) {
            return;
        }

        self.target.resize(&self.descriptors.device, width, height);

        self.surface = Surface::new(
            &self.descriptors,
            self.surface.quality(),
            width,
            height,
            self.target.format(),
        );
        report_stage_msaa(&self.surface);

        self.viewport_scale_factor = dimensions.scale_factor;

        // The pools are deliberately not reset here, and that is a change.
        //
        // A pooled texture is keyed by its size, usage, format and sample count, and a new viewport
        // invalidates none of those. The old stage-sized targets simply stop being asked for and age
        // out; the globals cache is keyed by viewport and ages out the same way. What a reset threw
        // away with them was everything else in both pools, which on a busy map is thousands of
        // small avatar and filter textures that have nothing to do with how big the window is.
        //
        // Emptying them per resize event is what turned a drag into a torrent: every frame of the
        // drag destroyed the whole cache and allocated a fresh one, and wgpu cannot reclaim a
        // destroyed texture until the GPU has finished with the submission that referenced it. Ask
        // faster than the GPU retires and the pending-destruction queue is the memory. A 10 GB card
        // reached 44.2 GB of texture memory that way, against 14.2 GB the renderer had actually
        // asked for, and died on the next large allocation.
    }

    fn create_context3d(
        &mut self,
        profile: Context3DProfile,
    ) -> Result<Box<dyn Context3D>, BitmapError> {
        Ok(Box::new(WgpuContext3D::new(
            self.descriptors.clone(),
            profile,
        )))
    }

    fn debug_info(&self) -> Cow<'static, str> {
        let mut result = vec![];
        result.push("Renderer: wgpu".to_string());

        let info = self.descriptors.adapter.get_info();
        result.push(format!("Adapter Backend: {:?}", info.backend));
        result.push(format!("Adapter Name: {:?}", info.name));
        result.push(format!("Adapter Device Type: {:?}", info.device_type));
        result.push(format!("Adapter Driver Name: {:?}", info.driver));
        result.push(format!("Adapter Driver Info: {:?}", info.driver_info));
        // The policy actually in force, override included. Reporting the default here would put a
        // label in the crash report that contradicts how the device was really created.
        result.push(format!(
            "Device Memory Policy: {:?}",
            effective_memory_hints(&info)
        ));

        let enabled_features = self.descriptors.device.features();
        let available_features = self.descriptors.adapter.features() - enabled_features;
        let current_limits = &self.descriptors.limits;

        result.push(format!("Enabled features: {enabled_features:?}"));
        result.push(format!("Available features: {available_features:?}"));
        result.push(format!("Current limits: {current_limits:?}"));
        result.push(format!("Surface quality: {}", self.surface.quality()));
        result.push(format!("Surface samples: {}", self.surface.sample_count()));
        result.push(format!("Surface size: {:?}", self.surface.size()));

        Cow::Owned(result.join("\n"))
    }

    fn resource_census(&self) -> Option<ruffle_render::backend::RenderResourceCensus> {
        // Plain atomic loads, which is why this is not behind the metrics feature: the sessions
        // that need answering are ordinary release builds.
        let hal = self.descriptors.device.get_internal_counters().hal;
        Some(ruffle_render::backend::RenderResourceCensus {
            textures: hal.textures.read().max(0) as u64,
            texture_bytes: hal.texture_memory.read().max(0) as u64,
            buffers: hal.buffers.read().max(0) as u64,
            buffer_bytes: hal.buffer_memory.read().max(0) as u64,
            memory_allocations: hal.memory_allocations.read().max(0) as u64,
        })
    }

    fn name(&self) -> &'static str {
        if cfg!(target_family = "wasm") {
            let info = self.descriptors.adapter.get_info();
            if info.backend == wgpu::Backend::BrowserWebGpu {
                "webgpu"
            } else {
                "wgpu-webgl"
            }
        } else {
            "wgpu"
        }
    }

    fn set_quality(&mut self, quality: StageQuality) {
        // Every pooled texture carries the sample count it was made with, so a change to it strands
        // the lot: not one of them can answer a request at the new count, and they sit in the
        // retention budget until they age out. Meanwhile a complete second set is allocated
        // alongside them.
        //
        // That is not merely wasteful. A session that switched quality was measured holding 14.2 GB
        // of textures live on a 10 GB card, where a fresh start at the same quality held far less,
        // and once past the card the driver pages over PCIe: identical pass and draw counts, ten
        // times the frame time, all of it waiting to present.
        //
        // The reported symptom is asymmetric, and that is the tell. Dropping from 4x samples to 1x
        // or 2x stays fast, because the set being allocated is the small one and the stranded 4x set
        // still fits alongside it. Going back up is what lags, because now the large set is the new
        // one and the stranded set is on top of it. Only the upward direction crosses the card.
        //
        // Dropping what cannot be used is correct on its own terms, and is what stops the two sets
        // coexisting in either direction.
        if quality_change_strands_pooled_textures(self.surface.quality(), quality) {
            self.texture_pool.reset();
            self.offscreen_texture_pool.reset();
        }

        self.surface = Surface::new(
            &self.descriptors,
            quality,
            self.surface.size().width,
            self.surface.size().height,
            self.target.format(),
        );
        report_stage_msaa(&self.surface);
    }

    fn viewport_dimensions(&self) -> ViewportDimensions {
        ViewportDimensions {
            width: self.target.width(),
            height: self.target.height(),
            scale_factor: self.viewport_scale_factor,
        }
    }

    #[instrument(level = "debug", skip_all)]
    fn register_shape(
        &mut self,
        shape: DistilledShape,
        bitmap_source: &dyn BitmapSource,
    ) -> ShapeHandle {
        let mesh = self.register_shape_internal(shape, bitmap_source, 1.0);
        ShapeHandle(Arc::new(mesh))
    }

    #[instrument(level = "debug", skip_all)]
    fn register_shape_with_scale(
        &mut self,
        shape: DistilledShape,
        bitmap_source: &dyn BitmapSource,
        scale: f32,
    ) -> ShapeHandle {
        let mesh = self.register_shape_internal(shape, bitmap_source, scale);
        ShapeHandle(Arc::new(mesh))
    }

    #[instrument(level = "debug", skip_all)]
    fn submit_frame(
        &mut self,
        clear: Color,
        commands: CommandList,
        cache_entries: Vec<BitmapCacheEntry>,
    ) {
        // Each frame starts its own submission budget. Carrying a part-full count across frames
        // would split an ordinary frame early just because the previous one ended mid-count.
        crate::submission_splitter::reset();

        let frame_output = match self.target.get_next_texture() {
            Ok(frame) => frame,
            Err(e) => {
                tracing::warn!("Couldn't begin new render frame: {}", e);
                // Attempt to recreate the swap chain in this case.
                self.target.resize(
                    &self.descriptors.device,
                    self.target.width(),
                    self.target.height(),
                );
                return;
            }
        };

        #[cfg(feature = "aether_metrics")]
        let submit_started = std::time::Instant::now();
        #[cfg(feature = "aether_metrics")]
        let cache_entry_count = cache_entries.len() as u64;
        #[cfg(feature = "aether_metrics")]
        let cache_started = std::time::Instant::now();

        // Every blur-family filter the frame runs, so the grouping an atlas could reach is measured
        // across the whole frame rather than per entry.
        #[cfg(feature = "aether_metrics")]
        let mut filter_signatures: Vec<u64> = Vec::new();

        // How many entries at a time may share one blur. Planned up front so the loop below can
        // take a whole group at once; a plan of all ones is exactly the old behaviour.
        let group_plan = plan_cache_entry_groups(&cache_entries);
        let mut remaining = cache_entries.into_iter();

        for group_len in group_plan {
            if group_len > 1 {
                let group: Vec<BitmapCacheEntry> = remaining.by_ref().take(group_len).collect();
                let group_signatures = self.render_atlased_cache_group(group);
                #[cfg(feature = "aether_metrics")]
                filter_signatures.extend(group_signatures);
                #[cfg(not(feature = "aether_metrics"))]
                drop(group_signatures);
                self.active_frame.maybe_flush(&self.descriptors);
                continue;
            }
            let Some(entry) = remaining.next() else {
                break;
            };
            #[cfg(feature = "aether_metrics")]
            crate::aether_metrics::record_cache_entry(entry.filters.len() as u64);
            #[cfg(feature = "aether_metrics")]
            let entry_is_filtered = !entry.filters.is_empty();
            #[cfg(feature = "aether_metrics")]
            let entry_started = std::time::Instant::now();
            let texture = as_texture(&entry.handle);
            let logical_size = bitmap_cache_filter_source_size(
                (texture.texture.width(), texture.texture.height()),
                (entry.logical_width, entry.logical_height),
            );
            let surface = Surface::new(
                &self.descriptors,
                self.surface.quality(),
                texture.texture.width(),
                texture.texture.height(),
                wgpu::TextureFormat::Rgba8Unorm,
            );
            if entry.filters.is_empty() {
                surface.draw_commands(
                    RenderTargetMode::ExistingWithColor(
                        texture.texture.clone(),
                        wgpu::Color {
                            r: f64::from(entry.clear.r) / 255.0,
                            g: f64::from(entry.clear.g) / 255.0,
                            b: f64::from(entry.clear.b) / 255.0,
                            a: f64::from(entry.clear.a) / 255.0,
                        },
                    ),
                    &self.descriptors,
                    &self.meshes,
                    entry.commands,
                    &mut self.active_frame.staging_belt,
                    &self.dynamic_transforms,
                    &mut self.active_frame.command_encoder,
                    LayerRef::None,
                    &mut self.offscreen_texture_pool,
                );
            } else {
                // We're relying on there being no impotent filters here,
                // so that we can safely start by using the actual CAB texture.
                // It's guaranteed that at least one filter would have used it and moved the target to something else,
                // letting us safely copy back to it later.
                let mut target = surface.draw_commands(
                    RenderTargetMode::ExistingWithColor(
                        texture.texture.clone(),
                        wgpu::Color {
                            r: f64::from(entry.clear.r) / 255.0,
                            g: f64::from(entry.clear.g) / 255.0,
                            b: f64::from(entry.clear.b) / 255.0,
                            a: f64::from(entry.clear.a) / 255.0,
                        },
                    ),
                    &self.descriptors,
                    &self.meshes,
                    entry.commands,
                    &mut self.active_frame.staging_belt,
                    &self.dynamic_transforms,
                    &mut self.active_frame.command_encoder,
                    LayerRef::None,
                    &mut self.offscreen_texture_pool,
                );
                let mut first_filter = true;
                for filter in entry.filters {
                    #[cfg(feature = "aether_metrics")]
                    if let Some(signature) =
                        crate::aether_metrics::atlasable_filter_signature(&filter)
                    {
                        filter_signatures.push(signature);
                    }
                    // The target's own extent, not its texture's. The offscreen pool rounds
                    // requested sizes out to a grid so that near-identical content shares a bucket,
                    // so a pooled texture is routinely larger than the region drawn into it. Taking
                    // the texture's dimensions here would hand the next filter the padding as
                    // though it were content, which for a blur means smearing cleared pixels back
                    // over the edge of the image.
                    let source_size = if first_filter {
                        logical_size
                    } else {
                        (target.width(), target.height())
                    };
                    target = self.descriptors.filters.apply(
                        &self.descriptors,
                        &mut self.active_frame.command_encoder,
                        &mut self.offscreen_texture_pool,
                        &mut self.active_frame.staging_belt,
                        FilterSource {
                            texture: target.color_texture(),
                            view: target.color_view().clone(),
                            point: (0, 0),
                            size: source_size,
                        },
                        filter,
                    );
                    first_filter = false;
                }
                self.active_frame.command_encoder.copy_texture_to_texture(
                    target.color_texture().as_image_copy(),
                    texture.texture.as_image_copy(),
                    wgpu::Extent3d {
                        width: logical_size.0,
                        height: logical_size.1,
                        depth_or_array_layers: 1,
                    },
                );
            }
            #[cfg(feature = "aether_metrics")]
            crate::aether_metrics::record_cache_entry_time(
                entry_is_filtered,
                entry_started.elapsed(),
            );
            // Periodically flush GPU work to prevent OOM when many cache entries
            // accumulate (e.g. when a large container's cacheAsBitmap is skipped
            // but its hundreds of children each have their own bitmap caches).
            self.active_frame.maybe_flush(&self.descriptors);
        }
        #[cfg(feature = "aether_metrics")]
        crate::aether_metrics::record_filter_groups(&mut filter_signatures);
        #[cfg(feature = "aether_metrics")]
        let cache_elapsed = cache_started.elapsed();

        self.surface.draw_commands_and_copy_to(
            frame_output.view(),
            RenderTargetMode::FreshWithColor(wgpu::Color {
                r: f64::from(clear.r) / 255.0,
                g: f64::from(clear.g) / 255.0,
                b: f64::from(clear.b) / 255.0,
                a: f64::from(clear.a) / 255.0,
            }),
            &self.descriptors,
            &mut self.active_frame.staging_belt,
            &self.dynamic_transforms,
            &mut self.active_frame.command_encoder,
            &self.meshes,
            commands,
            LayerRef::None,
            &mut self.texture_pool,
        );
        #[cfg(feature = "aether_metrics")]
        let queue_started = std::time::Instant::now();
        self.active_frame.staging_belt.finish();

        self.active_frame
            .submit_for_target(&self.descriptors, &self.target, frame_output);
        #[cfg(feature = "aether_metrics")]
        let queue_elapsed = queue_started.elapsed();
        let general_maintenance = self.texture_pool.finish_frame();
        let offscreen_maintenance = self.offscreen_texture_pool.finish_frame();
        #[cfg(feature = "aether_metrics")]
        crate::aether_metrics::record_submit_frame(
            submit_started.elapsed(),
            cache_elapsed,
            cache_entry_count,
            queue_elapsed,
        );
        #[cfg(feature = "aether_metrics")]
        crate::aether_metrics::record_pool_maintenance(
            crate::aether_metrics::TexturePoolKind::General,
            general_maintenance,
        );
        #[cfg(feature = "aether_metrics")]
        crate::aether_metrics::record_pool_maintenance(
            crate::aether_metrics::TexturePoolKind::Offscreen,
            offscreen_maintenance,
        );
        // Sampled here rather than read inside the device-lost callback, which must not reach back
        // into the device it is reporting the death of. These are plain atomic loads.
        #[cfg(feature = "aether_metrics")]
        {
            let hal = self.descriptors.device.get_internal_counters().hal;
            crate::aether_metrics::record_gpu_residency(
                hal.texture_memory.read().max(0) as u64,
                hal.textures.read().max(0) as u64,
                hal.memory_allocations.read().max(0) as u64,
            );
        }
        #[cfg(not(feature = "aether_metrics"))]
        let _ = (general_maintenance, offscreen_maintenance);
    }

    #[instrument(level = "debug", skip_all)]
    fn register_bitmap(&mut self, bitmap: Bitmap<'_>) -> Result<BitmapHandle, BitmapError> {
        let mut bitmap = bitmap.to_rgba();

        self.clamp_bitmap(&mut bitmap);

        let extent = wgpu::Extent3d {
            width: bitmap.width(),
            height: bitmap.height(),
            depth_or_array_layers: 1,
        };

        let texture_label = create_debug_label!("Bitmap");
        let texture = self
            .descriptors
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: texture_label.as_deref(),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                view_formats: &[wgpu::TextureFormat::Rgba8Unorm],
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_SRC,
            });
        #[cfg(feature = "aether_metrics")]
        crate::aether_metrics::record_texture_created(
            crate::aether_metrics::TextureOrigin::Bitmap,
            extent.width,
            extent.height,
            1,
            u64::from(extent.width) * u64::from(extent.height) * 4,
        );

        self.descriptors.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: Default::default(),
                aspect: wgpu::TextureAspect::All,
            },
            bitmap.data(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * extent.width),
                rows_per_image: None,
            },
            extent,
        );

        let handle = BitmapHandle(Arc::new(Texture {
            texture,
            bind_linear: Default::default(),
            bind_nearest: Default::default(),
            copy_count: Cell::new(0),
        }));

        Ok(handle)
    }

    #[instrument(level = "debug", skip_all)]
    fn update_texture(
        &mut self,
        handle: &BitmapHandle,
        bitmap: Bitmap<'_>,
        mut region: PixelRegion,
    ) -> Result<(), BitmapError> {
        if region.width() == 0 || region.height() == 0 {
            // Nothing to do. It's important to bail out now, as the
            // write_texture call panics when the source buffer is of zero size.
            return Ok(());
        }

        let texture = as_texture(handle);

        let mut bitmap = bitmap.to_rgba();
        if self.clamp_bitmap(&mut bitmap) {
            // If we're updating a resized texture, just redo the whole thing.
            // We can't trivially map pixel regions as we use a filter to resize.
            region = PixelRegion::for_whole_size(bitmap.width(), bitmap.height());
        }

        let extent = wgpu::Extent3d {
            width: region.width(),
            height: region.height(),
            depth_or_array_layers: 1,
        };

        self.active_frame.submit_direct(&self.descriptors);
        self.descriptors.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: region.x_min,
                    y: region.y_min,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &bitmap.data()[(region.y_min * texture.texture.width() * 4) as usize
                ..(region.y_max * texture.texture.width() * 4) as usize],
            wgpu::TexelCopyBufferLayout {
                offset: (region.x_min * 4) as wgpu::BufferAddress,
                bytes_per_row: Some(4 * texture.texture.width()),
                rows_per_image: None,
            },
            extent,
        );

        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    fn render_offscreen(
        &mut self,
        handle: BitmapHandle,
        commands: CommandList,
        quality: StageQuality,
        bounds: PixelRegion,
    ) -> Option<Box<dyn SyncHandle>> {
        let texture = as_texture(&handle);

        let extent = wgpu::Extent3d {
            width: texture.texture.width(),
            height: texture.texture.height(),
            depth_or_array_layers: 1,
        };

        let mut target = TextureTarget {
            size: extent,
            texture: texture.texture.clone(),
            format: wgpu::TextureFormat::Rgba8Unorm,
            buffer: None,
        };

        let frame_output = target
            .get_next_texture()
            .expect("TextureTargetFrame.get_next_texture is infallible");

        let surface = Surface::new(
            &self.descriptors,
            quality,
            texture.texture.width(),
            texture.texture.height(),
            wgpu::TextureFormat::Rgba8Unorm,
        );
        surface.draw_commands_and_copy_to(
            frame_output.view(),
            RenderTargetMode::FreshWithTexture(target.get_texture()),
            &self.descriptors,
            &mut self.active_frame.staging_belt,
            &self.dynamic_transforms,
            &mut self.active_frame.command_encoder,
            &self.meshes,
            commands,
            LayerRef::Current,
            &mut self.offscreen_texture_pool,
        );

        self.active_frame.maybe_flush(&self.descriptors);
        Some(self.make_queue_sync_handle(target, None, handle, bounds))
    }

    fn is_filter_supported(&self, filter: &Filter) -> bool {
        matches!(
            filter,
            Filter::BlurFilter(_)
                | Filter::GlowFilter(_)
                | Filter::DropShadowFilter(_)
                | Filter::ColorMatrixFilter(_)
                | Filter::ShaderFilter(_)
                | Filter::BevelFilter(_)
                | Filter::DisplacementMapFilter(_)
        )
    }

    fn is_offscreen_supported(&self) -> bool {
        true
    }

    fn apply_filter(
        &mut self,
        source: BitmapHandle,
        source_point: (u32, u32),
        source_size: (u32, u32),
        destination: BitmapHandle,
        dest_point: (i32, i32),
        filter: Filter,
    ) -> Option<Box<dyn SyncHandle>> {
        let source_texture = as_texture(&source);
        let dest_texture = as_texture(&destination);

        let copy_area = PixelRegion::for_whole_size(
            dest_texture.texture.width(),
            dest_texture.texture.height(),
        );

        let target = TextureTarget {
            size: wgpu::Extent3d {
                width: dest_texture.texture.width(),
                height: dest_texture.texture.height(),
                depth_or_array_layers: 1,
            },
            texture: dest_texture.texture.clone(),
            format: wgpu::TextureFormat::Rgba8Unorm,
            buffer: None,
        };

        let applied_filter = self.descriptors.filters.apply(
            &self.descriptors,
            &mut self.active_frame.command_encoder,
            &mut self.offscreen_texture_pool,
            &mut self.active_frame.staging_belt,
            FilterSource {
                texture: &source_texture.texture,
                view: source_texture.texture.create_view(&Default::default()),
                point: source_point,
                size: source_size,
            },
            filter,
        );

        let (dest_x, dest_y) = dest_point;

        let src_offset_x = dest_x.min(0).unsigned_abs();
        let src_offset_y = dest_y.min(0).unsigned_abs();

        let final_dest_x = dest_x.max(0) as u32;
        let final_dest_y = dest_y.max(0) as u32;

        let available_width = applied_filter.width().saturating_sub(src_offset_x);
        let available_height = applied_filter.height().saturating_sub(src_offset_y);
        let dest_available_width = dest_texture.texture.width().saturating_sub(final_dest_x);
        let dest_available_height = dest_texture.texture.height().saturating_sub(final_dest_y);

        let copy_width = available_width.min(dest_available_width);
        let copy_height = available_height.min(dest_available_height);

        if copy_width == 0 || copy_height == 0 {
            return None;
        }

        self.active_frame.command_encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: applied_filter.color_texture(),
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: src_offset_x,
                    y: src_offset_y,
                    z: 0,
                },
                aspect: Default::default(),
            },
            wgpu::TexelCopyTextureInfo {
                texture: &dest_texture.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: final_dest_x,
                    y: final_dest_y,
                    z: 0,
                },
                aspect: Default::default(),
            },
            wgpu::Extent3d {
                width: copy_width,
                height: copy_height,
                depth_or_array_layers: 1,
            },
        );

        self.active_frame.maybe_flush(&self.descriptors);
        Some(self.make_queue_sync_handle(target, None, destination, copy_area))
    }

    fn compile_pixelbender_shader(
        &mut self,
        shader: PixelBenderShader,
    ) -> Result<PixelBenderShaderHandle, BitmapError> {
        self.compile_pixelbender_shader_impl(shader)
    }

    fn run_pixelbender_shader(
        &mut self,
        shader: PixelBenderShaderHandle,
        arguments: &[PixelBenderShaderArgument],
        target: &PixelBenderTarget,
    ) -> Result<PixelBenderOutput, BitmapError> {
        let output_channels = shader
            .0
            .parsed_shader()
            .output_channels()
            .expect("No output parameter");
        let has_padding = output_channels == 3;

        let texture_format =
            crate::pixel_bender::temporary_texture_format_for_channels(output_channels as u32);

        let target_handle = match target {
            PixelBenderTarget::Bitmap(handle) => handle.clone(),
            PixelBenderTarget::Bytes { width, height } => {
                let extent = wgpu::Extent3d {
                    width: *width,
                    height: *height,
                    depth_or_array_layers: 1,
                };
                // FIXME - cache this texture somehow. We might also want to consider using
                // a compute shader
                let texture_label = create_debug_label!("Temporary pixelbender output texture");
                let texture = self
                    .descriptors
                    .device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: texture_label.as_deref(),
                        size: extent,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: texture_format,
                        view_formats: &[texture_format],
                        usage: wgpu::TextureUsages::TEXTURE_BINDING
                            | wgpu::TextureUsages::COPY_DST
                            | wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::COPY_SRC,
                    });
                BitmapHandle(Arc::new(Texture {
                    texture,
                    bind_linear: Default::default(),
                    bind_nearest: Default::default(),
                    copy_count: Cell::new(0),
                }))
            }
        };

        let target_texture = as_texture(&target_handle);

        let extent = wgpu::Extent3d {
            width: target_texture.texture.width(),
            height: target_texture.texture.height(),
            depth_or_array_layers: 1,
        };

        let copy_dimensions = BufferDimensions::new(
            target_texture.texture.width() as usize,
            target_texture.texture.height() as usize,
            target_texture.texture.format(),
        );
        let buffer_info = Some(TextureBufferInfo {
            buffer: MaybeOwnedBuffer::Borrowed(
                self.offscreen_buffer_pool
                    .take(&self.descriptors, copy_dimensions.clone()),
                copy_dimensions,
            ),
            copy_area: PixelRegion::for_whole_size(
                target_texture.texture.width(),
                target_texture.texture.height(),
            ),
        });

        let mut texture_target = TextureTarget {
            size: extent,
            texture: target_texture.texture.clone(),
            format: target_texture.texture.format(),
            buffer: buffer_info,
        };

        let frame_output = texture_target
            .get_next_texture()
            .expect("TextureTargetFrame.get_next_texture is infallible");

        run_pixelbender_shader_impl(
            &self.descriptors,
            shader,
            ShaderMode::ShaderJob,
            arguments,
            &target_texture.texture,
            &mut self.active_frame.command_encoder,
            Some(wgpu::RenderPassColorAttachment {
                view: frame_output.view(),
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            }),
            1,
            // When running a standalone shader, we always process the entire image
            &FilterSource::for_entire_texture(&target_texture.texture),
        )?;

        let index = Some(self.active_frame.submit_for_target(
            &self.descriptors,
            &texture_target,
            frame_output,
        ));

        let sync_handle = self.make_queue_sync_handle(
            texture_target,
            index,
            target_handle,
            PixelRegion::for_whole_size(extent.width, extent.height),
        );

        match target {
            PixelBenderTarget::Bitmap(_) => Ok(PixelBenderOutput::Bitmap(sync_handle)),
            PixelBenderTarget::Bytes { width, .. } => {
                let mut output = None;
                self.resolve_sync_handle(
                    sync_handle,
                    Box::new(|raw_pixels, buffer_width| {
                        let width = *width as usize;

                        if buffer_width as usize
                            != width * output_channels * std::mem::size_of::<f32>()
                        {
                            let mut new_pixels = Vec::new();
                            for row in raw_pixels.chunks(buffer_width as usize) {
                                let actual_row = &row[0..(width * std::mem::size_of::<[f32; 4]>())];

                                for pixel in
                                    actual_row.chunks_exact(std::mem::size_of::<[f32; 4]>())
                                {
                                    if has_padding {
                                        // Take the first three channels
                                        new_pixels.extend_from_slice(
                                            &pixel[0..(3 * std::mem::size_of::<f32>())],
                                        );
                                    } else {
                                        // Copy the pixel as-is
                                        new_pixels.extend_from_slice(pixel);
                                    }
                                }
                            }
                            output = Some(new_pixels);
                        } else {
                            output = Some(raw_pixels.to_vec());
                        };
                    }),
                )?;
                Ok(PixelBenderOutput::Bytes(output.unwrap()))
            }
        }
    }

    fn create_empty_texture(
        &mut self,
        width: NonZeroU32,
        height: NonZeroU32,
    ) -> Result<BitmapHandle, BitmapError> {
        let width = width.get();
        let height = height.get();

        if width > self.descriptors.limits.max_texture_dimension_2d
            || height > self.descriptors.limits.max_texture_dimension_2d
        {
            return Err(BitmapError::TooLarge);
        }

        let extent = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };

        let texture_label = create_debug_label!("Bitmap");
        let texture = self
            .descriptors
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: texture_label.as_deref(),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                view_formats: &[wgpu::TextureFormat::Rgba8Unorm],
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_SRC,
            });
        Ok(BitmapHandle(Arc::new(Texture {
            texture,
            bind_linear: Default::default(),
            bind_nearest: Default::default(),
            copy_count: Cell::new(0),
        })))
    }

    fn resolve_sync_handle(
        &mut self,
        handle: Box<dyn SyncHandle>,
        with_rgba: RgbaBufRead,
    ) -> Result<(), ruffle_render::error::Error> {
        let handle = Box::<dyn Any>::downcast::<QueueSyncHandle>(handle).unwrap();
        handle
            .capture(with_rgba, &mut self.active_frame)
            .ok_or(ruffle_render::error::Error::GpuReadbackFailed)
    }
}

pub async fn request_adapter_and_device(
    backend: wgpu::Backends,
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'static>>,
    power_preference: wgpu::PowerPreference,
) -> Result<(wgpu::Adapter, wgpu::Device, wgpu::Queue), Error> {
    let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference,
        compatible_surface: surface,
        force_fallback_adapter: false,
    }).await
        .map_err(|_e| {
            let names = get_backend_names(backend);
            if names.is_empty() {
                "Ruffle requires hardware acceleration, but no compatible graphics device was found (no backend provided?)".to_string()
            } else if cfg!(target_vendor = "apple") {
                "Ruffle does not support OpenGL on macOS/iOS.".to_string()
            } else {
                format!("Ruffle requires hardware acceleration, but no compatible graphics device was found supporting {}", format_list(&names, "or"))
            }
        })?;

    let (device, queue) = request_device(&adapter).await?;
    Ok((adapter, device, queue))
}

// We try to request the highest limits we can get away with
async fn request_device(
    adapter: &wgpu::Adapter,
) -> Result<(wgpu::Device, wgpu::Queue), wgpu::RequestDeviceError> {
    // We start off with the lowest limits we actually need - basically GL-ES 3.0
    let mut limits = wgpu::Limits::downlevel_webgl2_defaults();
    // Then we increase parts of it to the maximum supported by the adapter, to take advantage of
    // more powerful hardware or capabilities
    limits = limits.using_resolution(adapter.limits());
    limits = limits.using_alignment(adapter.limits());
    limits.max_uniform_buffer_binding_size = adapter.limits().max_uniform_buffer_binding_size;
    limits.max_inter_stage_shader_components = adapter.limits().max_inter_stage_shader_components;
    // This will be a default limit in a future wgpu version (down from 8).
    // It's required for some WebGL devices to be supported.
    limits.max_color_attachments = 4;

    let mut features = Default::default();

    let try_features = [
        wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES,
        wgpu::Features::TEXTURE_COMPRESSION_BC,
        wgpu::Features::FLOAT32_FILTERABLE,
    ];

    for feature in try_features {
        if adapter.features().contains(feature) {
            features |= feature;
        }
    }

    adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: None,
            required_features: features,
            required_limits: limits,
            memory_hints: effective_memory_hints(&adapter.get_info()),
            trace: wgpu::Trace::Off,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
        })
        .await
}

/// Parse an explicit device memory policy.
///
/// The AMD device loss has survived two fixes aimed at how much is allocated, and the remaining
/// suspicion is about the shape of the blocks it is allocated from. `MemoryUsage` was chosen for
/// AMD Vulkan on the reasoning that small blocks avoid speculative allocation, and that reasoning
/// has never actually been tested against the failure. This exists so both can be measured on one
/// machine in one sitting rather than guessed at across another rebuild.
fn parse_memory_policy(value: &str) -> Option<wgpu::MemoryHints> {
    match value.trim().to_ascii_lowercase().as_str() {
        "performance" => Some(wgpu::MemoryHints::Performance),
        "usage" | "memoryusage" | "memory-usage" => Some(wgpu::MemoryHints::MemoryUsage),
        _ => None,
    }
}

/// The memory policy in force: the override if one was given, otherwise the per-adapter default.
///
/// Read from the environment rather than exposed as a setting. It is a diagnostic for one specific
/// investigation and has no business anywhere a player would find it.
fn effective_memory_hints(adapter_info: &wgpu::AdapterInfo) -> wgpu::MemoryHints {
    static OVERRIDE: std::sync::OnceLock<Option<wgpu::MemoryHints>> = std::sync::OnceLock::new();
    let override_hint = OVERRIDE.get_or_init(|| {
        let raw = std::env::var("AETHER_GPU_MEMORY_POLICY").ok()?;
        let parsed = parse_memory_policy(&raw);
        if parsed.is_none() {
            tracing::warn!("Ignoring unrecognised AETHER_GPU_MEMORY_POLICY {raw:?}");
        }
        parsed
    });

    match override_hint {
        Some(hint) => hint.clone(),
        None => memory_hints_for_adapter_info(adapter_info),
    }
}

#[inline]
fn memory_hints_for_adapter_info(_adapter_info: &wgpu::AdapterInfo) -> wgpu::MemoryHints {
    // AMD Vulkan used to be special-cased to `MemoryUsage`, on the reasoning that its 8-64 MiB
    // blocks avoid large speculative allocations on AMD's Windows driver where `Performance` grows
    // blocks from 128 MiB to 512 MiB. That reasoning was never tested against the device loss it
    // was meant to help, and measuring it on the 6800 XT that kept crashing showed it was the cause
    // rather than the cure:
    //
    //             policy   survived   peak texture memory   peak live textures
    //        MemoryUsage       9.3s                795 MB                1,585
    //        MemoryUsage      14.4s                588 MB                2,262
    //        Performance     262.8s              3,025 MB               28,132
    //
    // Same machine, same map, minutes apart. Small blocks meant the allocator had to ask the driver
    // for a new one constantly, and it was that request which failed -- the last sixteen textures
    // before one such fault were all 128x128 to 384x384, so it was not the size of any single
    // request that could not be met. Large blocks ask far less often and the client survives.
    wgpu::MemoryHints::Performance
}

const AMD_VULKAN_MAX_FRAMES_IN_FLIGHT: usize = 2;

#[inline]
fn submission_retirement_limit_for_adapter_info(adapter_info: &wgpu::AdapterInfo) -> Option<usize> {
    is_amd_vulkan(adapter_info).then_some(AMD_VULKAN_MAX_FRAMES_IN_FLIGHT)
}

/// Determines how we choose our frame buffer
#[derive(Clone)]
pub enum RenderTargetMode {
    // Construct a new frame buffer, clearng it with the provided color.
    // This is used when rendering to the actual display,
    // or when applying a filter. In both cases, we have a fixed background color,
    // and don't need to blend with anything else
    FreshWithColor(wgpu::Color),
    // Construct a new frame buffer, cleared with an existing texture.
    // we will blend with the previous contents of the texture.
    // This is used in `render_offscreen`, as we need to blend with the previous
    // contents of our `BitmapData` texture
    FreshWithTexture(wgpu::Texture),
    // Use the provided texture as our frame buffer, and clear it with the given color.
    ExistingWithColor(wgpu::Texture, wgpu::Color),
}

impl RenderTargetMode {
    pub fn color(&self) -> Option<wgpu::Color> {
        match self {
            RenderTargetMode::FreshWithColor(color) => Some(*color),
            RenderTargetMode::FreshWithTexture(_) => None,
            RenderTargetMode::ExistingWithColor(_, color) => Some(*color),
        }
    }
}

pub struct ActiveFrame {
    pub staging_belt: wgpu::util::StagingBelt,
    pub command_encoder: wgpu::CommandEncoder,
    draws_since_flush: u32,
    max_draws_per_flush: u32,
    submission_retirement: SubmissionRetirement<SubmissionIndex>,
}

struct SubmissionRetirement<T> {
    max_in_flight: Option<usize>,
    submissions: VecDeque<T>,
}

impl<T> SubmissionRetirement<T> {
    fn new(max_in_flight: Option<usize>) -> Self {
        Self {
            max_in_flight,
            submissions: VecDeque::new(),
        }
    }

    fn push(&mut self, submission: T) -> Option<T> {
        let max_in_flight = self.max_in_flight?;
        self.submissions.push_back(submission);
        (self.submissions.len() > max_in_flight)
            .then(|| self.submissions.pop_front())
            .flatten()
    }

    fn push_direct(&mut self, submission: T) -> Option<T> {
        self.push(submission)
    }

    fn push_target(&mut self, submission: T) -> Option<T> {
        self.push(submission)
    }
}

/// Whether a viewport report actually differs from the one the renderer is already built for.
///
/// Winit reports a resize per frame of a drag, and on Windows it reports one for a window move as
/// well, where nothing has changed. Answering those rebuilds the swapchain and every render target
/// for a size the renderer already has.
///
/// The scale factor is part of the question because dragging a window between monitors of different
/// DPI changes it while the pixel size stays put, and the renderer does have to hear about that.
fn viewport_change_is_real(
    current: wgpu::Extent3d,
    current_scale_factor: f64,
    width: u32,
    height: u32,
    scale_factor: f64,
) -> bool {
    current.width != width
        || current.height != height
        // Compared exactly, because it is carried through unchanged rather than computed: winit
        // hands back the same f64 for an unchanged monitor, and a bit of drift here is a rebuild
        // that is wanted rather than one that is wasted.
        || current_scale_factor != scale_factor
}

/// Whether a quality change leaves the texture pools holding nothing usable.
///
/// Sample count is part of a pooled texture's identity, so changing it means no entry can answer a
/// request any more. Quality settings that share a sample count, such as High and Best, leave the
/// pool entirely valid, and throwing it away then would be a stall in exchange for nothing.
fn quality_change_strands_pooled_textures(before: StageQuality, after: StageQuality) -> bool {
    before.sample_count() != after.sample_count()
}

#[cfg(test)]
mod quality_change_tests {
    use super::*;

    fn extent(width: u32, height: u32) -> wgpu::Extent3d {
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        }
    }

    /// The one that mattered. Dragging a window by its title bar reports a resize per frame with
    /// the size unchanged, and answering each one rebuilt the swapchain and every render target.
    #[test]
    fn a_resize_report_for_the_size_we_already_have_is_not_a_change() {
        assert!(!viewport_change_is_real(
            extent(1280, 720),
            1.0,
            1280,
            720,
            1.0
        ));
    }

    #[test]
    fn a_real_resize_is_a_change() {
        assert!(viewport_change_is_real(
            extent(1280, 720),
            1.0,
            1281,
            720,
            1.0
        ));
        assert!(viewport_change_is_real(
            extent(1280, 720),
            1.0,
            1280,
            721,
            1.0
        ));
    }

    /// Dragging a window onto a monitor at a different DPI keeps the pixel size and changes the
    /// scale factor, and the renderer does have to hear about that one.
    #[test]
    fn a_scale_factor_change_alone_is_a_change() {
        assert!(viewport_change_is_real(
            extent(1280, 720),
            1.0,
            1280,
            720,
            1.5
        ));
    }

    /// Low and Best differ in sample count, so nothing pooled at one is usable at the other.
    #[test]
    fn a_change_in_sample_count_strands_everything_the_pools_hold() {
        assert!(quality_change_strands_pooled_textures(
            StageQuality::Low,
            StageQuality::Best
        ));
        assert!(quality_change_strands_pooled_textures(
            StageQuality::Best,
            StageQuality::Low
        ));
        assert!(quality_change_strands_pooled_textures(
            StageQuality::Medium,
            StageQuality::High
        ));
    }

    /// High and Best are both 4x, so the pool is still entirely valid and throwing it away would be
    /// a stall for nothing.
    #[test]
    fn a_change_that_keeps_the_sample_count_keeps_the_pool() {
        assert!(!quality_change_strands_pooled_textures(
            StageQuality::High,
            StageQuality::Best
        ));
        assert!(!quality_change_strands_pooled_textures(
            StageQuality::Low,
            StageQuality::Low
        ));
    }
}

#[cfg(test)]
mod bitmap_cache_capacity_tests {
    use super::*;

    fn adapter_info(name: &str, vendor: u32, backend: wgpu::Backend) -> wgpu::AdapterInfo {
        wgpu::AdapterInfo {
            name: name.to_string(),
            vendor,
            device: 0,
            device_type: wgpu::DeviceType::DiscreteGpu,
            driver: String::new(),
            driver_info: String::new(),
            backend,
        }
    }

    #[test]
    fn a_memory_policy_can_be_named_explicitly_and_junk_is_refused() {
        assert!(matches!(
            parse_memory_policy("performance"),
            Some(wgpu::MemoryHints::Performance)
        ));
        assert!(matches!(
            parse_memory_policy("  PerFormance  "),
            Some(wgpu::MemoryHints::Performance)
        ));
        assert!(matches!(
            parse_memory_policy("usage"),
            Some(wgpu::MemoryHints::MemoryUsage)
        ));
        // An unreadable value must fall back to the per-adapter default rather than pick one,
        // otherwise a typo silently changes what is being measured.
        assert!(parse_memory_policy("fastest").is_none());
        assert!(parse_memory_policy("").is_none());
    }

    /// AMD Vulkan was singled out for the memory-conserving policy and it was the one configuration
    /// that kept losing its device. On the reporter's 6800 XT it survived 9 and 14 seconds under
    /// `MemoryUsage` against 263 seconds under `Performance`, so the special case is gone and no
    /// adapter gets small blocks back without a measurement saying it should.
    #[test]
    fn every_adapter_gets_large_suballocation_blocks() {
        for (name, vendor, backend) in [
            ("AMD Radeon RX 6800 XT", 0x1002, wgpu::Backend::Vulkan),
            ("AMD Radeon RX 6800 XT", 0x1002, wgpu::Backend::Dx12),
            ("NVIDIA GeForce RTX 3080", 0x10de, wgpu::Backend::Vulkan),
            ("Intel Arc A770", 0x8086, wgpu::Backend::Vulkan),
        ] {
            assert!(
                matches!(
                    memory_hints_for_adapter_info(&adapter_info(name, vendor, backend)),
                    wgpu::MemoryHints::Performance
                ),
                "{name} on {backend:?} should use large blocks"
            );
        }
    }

    #[test]
    fn filters_process_only_the_logical_cache_region() {
        assert_eq!(
            bitmap_cache_filter_source_size((637, 584), (463, 498)),
            (463, 498)
        );
        assert_eq!(
            bitmap_cache_filter_source_size((637, 584), (900, 498)),
            (637, 498)
        );
    }

    #[test]
    fn submission_retirement_waits_for_the_oldest_frame_at_the_limit() {
        let mut retirement = SubmissionRetirement::new(Some(2));

        assert_eq!(retirement.push(11), None);
        assert_eq!(retirement.push(12), None);
        assert_eq!(retirement.push(13), Some(11));
        assert_eq!(retirement.push(14), Some(12));
    }

    #[test]
    fn intermediate_flushes_share_the_frame_retirement_budget() {
        let mut retirement = SubmissionRetirement::new(Some(2));

        assert_eq!(retirement.push_direct(21), None);
        assert_eq!(retirement.push_target(22), None);
        assert_eq!(retirement.push_direct(23), Some(21));
        assert_eq!(retirement.push_target(24), Some(22));
    }

    #[test]
    fn submission_retirement_is_limited_to_amd_vulkan() {
        let amd_vulkan = adapter_info("AMD Radeon RX 6800 XT", 0x1002, wgpu::Backend::Vulkan);
        assert_eq!(
            submission_retirement_limit_for_adapter_info(&amd_vulkan),
            Some(2)
        );

        let nvidia_vulkan = adapter_info("NVIDIA GeForce RTX 3080", 0x10de, wgpu::Backend::Vulkan);
        assert_eq!(
            submission_retirement_limit_for_adapter_info(&nvidia_vulkan),
            None
        );

        let amd_dx12 = adapter_info("AMD Radeon RX 6800 XT", 0x1002, wgpu::Backend::Dx12);
        assert_eq!(
            submission_retirement_limit_for_adapter_info(&amd_dx12),
            None
        );
    }
}

impl ActiveFrame {
    pub fn new(
        descriptors: &Descriptors,
        max_draws_per_flush: u32,
        max_in_flight_submissions: Option<usize>,
    ) -> Self {
        Self {
            command_encoder: descriptors
                .device
                .create_command_encoder(&Default::default()),
            staging_belt: wgpu::util::StagingBelt::new(65536),
            draws_since_flush: 0,
            max_draws_per_flush: max_draws_per_flush.max(1),
            submission_retirement: SubmissionRetirement::new(max_in_flight_submissions),
        }
    }

    pub fn submit_for_target<T: RenderTarget>(
        &mut self,
        descriptors: &Descriptors,
        target: &T,
        frame: T::Frame,
    ) -> SubmissionIndex {
        self.draws_since_flush = 0;
        self.staging_belt.finish();
        let draw_encoder = std::mem::replace(
            &mut self.command_encoder,
            descriptors
                .device
                .create_command_encoder(&Default::default()),
        );
        let index = target.submit(
            &descriptors.device,
            &descriptors.queue,
            Some(draw_encoder.finish()),
            frame,
        );
        self.staging_belt.recall();
        let oldest_submission = self.submission_retirement.push_target(index.clone());
        self.retire_submission(descriptors, oldest_submission, "frame");
        index
    }

    pub fn submit_direct(&mut self, descriptors: &Descriptors) -> SubmissionIndex {
        self.draws_since_flush = 0;
        self.staging_belt.finish();
        let draw_encoder = std::mem::replace(
            &mut self.command_encoder,
            descriptors
                .device
                .create_command_encoder(&Default::default()),
        );
        let index = descriptors.queue.submit(Some(draw_encoder.finish()));
        self.staging_belt.recall();
        let oldest_submission = self.submission_retirement.push_direct(index.clone());
        self.retire_submission(descriptors, oldest_submission, "intermediate");
        index
    }

    fn retire_submission(
        &self,
        descriptors: &Descriptors,
        oldest_submission: Option<SubmissionIndex>,
        submission_kind: &str,
    ) {
        let Some(oldest_submission) = oldest_submission else {
            return;
        };

        if let Err(error) = descriptors.device.poll(wgpu::PollType::Wait {
            submission_index: Some(oldest_submission),
            timeout: None,
        }) {
            tracing::warn!("Failed to retire an AMD Vulkan {submission_kind} submission: {error}");
        }
    }

    pub fn maybe_flush(&mut self, descriptors: &Descriptors) {
        // [NA] This is kind of a hack.
        // If we do "too much" during one frame, the submission ends up being way too large and goes OutOfMemory.
        // What it is that we're OOMing on is likely buffers and temporary textures and such from render_offscreen
        // Hard to track that though... so let's just flush it out if we do more than X draws per frame
        self.draws_since_flush += 1;

        if self.draws_since_flush >= self.max_draws_per_flush {
            self.submit_direct(descriptors);
        }
    }
}

/// Note the stage's multisampling, when it changes.
///
/// Only the stage matters here, and only on the two paths that replace it -- a resize and a quality
/// change. `Surface::new` looked like the natural place for this and was not: every offscreen render
/// builds a surface of its own, at its own size and so at its own sample count, and logging there
/// alternated between two values several times a frame.
fn report_stage_msaa(surface: &Surface) {
    use std::sync::atomic::{AtomicU8, Ordering};

    static LAST: AtomicU8 = AtomicU8::new(u8::MAX);
    let sample_count = surface.sample_count();
    let reported = u8::try_from(sample_count).unwrap_or(u8::MAX);
    if LAST.swap(reported, Ordering::Relaxed) != reported {
        tracing::info!(
            "Stage quality {} is using {sample_count}x MSAA",
            surface.quality()
        );
    }
}

/// One member of an atlased group, after its own commands have been drawn but before its filter.
struct DrawnCacheEntry {
    /// The `cacheAsBitmap` texture the finished result is copied back into.
    cache_texture: wgpu::Texture,
    /// The extent actually drawn, which is smaller than the texture whenever the pool rounded up.
    logical: (u32, u32),
    filter: ruffle_render::filters::Filter,
    target: CommandTarget,
}

/// How many cache entries may share one blur. Bounds how many targets are alive at once.
///
/// Measured groups average 3.9, so eight gives up almost nothing while keeping the worst case
/// somewhere it can be reasoned about. That bound is not tidiness: the region-limited MSAA
/// experiment held a frame's targets together and took peak GPU texture memory from 1,966 MB to
/// 11,451 MB.
const DEFAULT_MAX_ATLAS_GROUP: usize = 8;

/// The cap on a card that cannot spare the memory.
///
/// Atlasing measured peak GPU texture memory 825 -> 1,286 MB on a 10 GB card, which is comfortable
/// there and is not on the 2 GB cards low VRAM mode exists for. Three still captures most of the
/// grouping, since the average group is 3.9.
const LOW_VRAM_MAX_ATLAS_GROUP: usize = 3;

static FILTER_ATLAS_MAX_GROUP: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(DEFAULT_MAX_ATLAS_GROUP);

/// Hold atlas groups to what a low VRAM card can afford.
pub fn set_filter_atlas_low_vram(low_vram: bool) {
    let cap = if low_vram {
        LOW_VRAM_MAX_ATLAS_GROUP
    } else {
        DEFAULT_MAX_ATLAS_GROUP
    };
    FILTER_ATLAS_MAX_GROUP.store(cap, std::sync::atomic::Ordering::Relaxed);
}

fn filter_atlas_max_group() -> usize {
    FILTER_ATLAS_MAX_GROUP.load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether cache entries sharing a blur kernel may be blurred together in one atlas.
///
/// On by default, on measurement: an A/B over two ~21 minute sessions matched on draw count put the
/// cost of a filtered cache entry at 0.085 ms without it and 0.025 ms with it, a 71% cut, while the
/// filterless half stayed flat. `AETHER_FILTER_ATLAS=0` composites each filter on its own as
/// before, which is the control for measuring it again and the switch to reach for if filtered
/// content ever looks wrong.
fn filter_atlas_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        !matches!(
            std::env::var("AETHER_FILTER_ATLAS").as_deref(),
            Ok("0") | Ok("off") | Ok("false")
        )
    })
}

/// The blur this entry would run, if it is one that can be shared.
///
/// Only entries carrying exactly one filter qualify. A second filter consumes the first one's
/// output, so the blur being shared would no longer be the blur that filter runs -- and measurement
/// says this costs nothing: filters per filtered entry came out at 1.00 across a whole session.
fn cache_entry_blur_key(entry: &BitmapCacheEntry) -> Option<crate::filters::atlas::BlurKey> {
    let [only] = &entry.filters[..] else {
        return None;
    };
    let (blur_x, blur_y, passes) = crate::filters::filter_shares_a_blur(only)?;
    Some(crate::filters::atlas::BlurKey::new(blur_x, blur_y, passes))
}

/// Split the frame's cache entries into runs that may be blurred together.
///
/// The returned lengths sum to `entries.len()`, so the caller consumes them in order and never has
/// to reorder anything -- which matters, because cache entries arrive in dependency order and a
/// parent must still be drawn after the child it draws.
fn plan_cache_entry_groups(entries: &[BitmapCacheEntry]) -> Vec<usize> {
    let mut plan = Vec::new();
    let mut index = 0;
    while index < entries.len() {
        let len = if filter_atlas_enabled() {
            crate::filters::atlas::plan_atlas_group(
                entries.len() - index,
                |offset| cache_entry_blur_key(&entries[index + offset]),
                |drawer, drawn| {
                    crate::filters::atlas::draws_handle(
                        &entries[index + drawer].commands,
                        &entries[index + drawn].handle,
                    )
                },
                filter_atlas_max_group(),
            )
        } else {
            1
        };
        let len = len.max(1);
        plan.push(len);
        index += len;
    }
    plan
}

fn bitmap_cache_filter_source_size(capacity: (u32, u32), logical_size: (u32, u32)) -> (u32, u32) {
    (
        logical_size.0.min(capacity.0),
        logical_size.1.min(capacity.1),
    )
}
