use crate::Transforms;
use crate::backend::RenderTargetMode;
use crate::buffer_pool::{AlwaysCompatible, PoolEntry, TexturePool};
use crate::descriptors::Descriptors;
use crate::globals::Globals;
use crate::utils::create_buffer_with_data;
use crate::utils::run_copy_pipeline;
use std::cell::OnceCell;
use std::sync::Arc;

#[derive(Debug)]
pub struct ResolveBuffer {
    texture: PoolOrArcTexture,
}

impl ResolveBuffer {
    pub fn new(
        descriptors: &Descriptors,
        size: wgpu::Extent3d,
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
        pool: &mut TexturePool,
    ) -> Self {
        let texture = pool.get_texture(descriptors, size, usage, format, 1);
        Self {
            texture: PoolOrArcTexture::Pool(texture),
        }
    }

    pub fn new_manual(texture: wgpu::Texture) -> Self {
        Self {
            texture: PoolOrArcTexture::Manual((
                texture.clone(),
                texture.create_view(&Default::default()),
            )),
        }
    }

    pub fn view(&self) -> &wgpu::TextureView {
        match self.texture {
            PoolOrArcTexture::Pool(ref texture) => &texture.1,
            PoolOrArcTexture::Manual(ref texture) => &texture.1,
        }
    }

    pub fn texture(&self) -> &wgpu::Texture {
        match self.texture {
            PoolOrArcTexture::Pool(ref texture) => &texture.0,
            PoolOrArcTexture::Manual(ref texture) => &texture.0,
        }
    }

    pub fn take_texture(self) -> PoolOrArcTexture {
        self.texture
    }
}

#[derive(Debug)]
pub struct FrameBuffer {
    texture: PoolOrArcTexture,
    size: wgpu::Extent3d,
}

#[derive(Debug)]
/// Holds either a `PoolEntry` texture, or an `Arc`-wrapped texture.
/// This is used to select between using a texture pool for our framebuffer/resolve-buffer
/// (when rendering to the main screen), or rendering to a non-pooled `Texture`
/// (when doing an offscreen render to a BitmapData texture)
pub enum PoolOrArcTexture {
    Pool(PoolEntry<(wgpu::Texture, wgpu::TextureView), AlwaysCompatible>),
    Manual((wgpu::Texture, wgpu::TextureView)),
}

impl PoolOrArcTexture {
    pub fn texture(&self) -> &wgpu::Texture {
        match self {
            PoolOrArcTexture::Pool(texture) => &texture.0,
            PoolOrArcTexture::Manual(texture) => &texture.0,
        }
    }
    pub fn view(&self) -> &wgpu::TextureView {
        match self {
            PoolOrArcTexture::Pool(texture) => &texture.1,
            PoolOrArcTexture::Manual(texture) => &texture.1,
        }
    }
}

impl FrameBuffer {
    pub fn new(
        descriptors: &Descriptors,
        sample_count: u32,
        size: wgpu::Extent3d,
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
        pool: &mut TexturePool,
    ) -> Self {
        let texture = pool.get_texture(descriptors, size, usage, format, sample_count);

        Self {
            texture: PoolOrArcTexture::Pool(texture),
            size,
        }
    }

    pub fn new_manual(texture: wgpu::Texture, size: wgpu::Extent3d) -> Self {
        Self {
            texture: PoolOrArcTexture::Manual((
                texture.clone(),
                texture.create_view(&Default::default()),
            )),
            size,
        }
    }

    pub fn view(&self) -> &wgpu::TextureView {
        match self.texture {
            PoolOrArcTexture::Pool(ref texture) => &texture.1,
            PoolOrArcTexture::Manual(ref texture) => &texture.1,
        }
    }

    pub fn texture(&self) -> &wgpu::Texture {
        match self.texture {
            PoolOrArcTexture::Pool(ref texture) => &texture.0,
            PoolOrArcTexture::Manual(ref texture) => &texture.0,
        }
    }

    pub fn take_texture(self) -> PoolOrArcTexture {
        self.texture
    }

    pub fn size(&self) -> wgpu::Extent3d {
        self.size
    }
}

#[derive(Debug)]
pub struct BlendBuffer {
    texture: PoolEntry<(wgpu::Texture, wgpu::TextureView), AlwaysCompatible>,
}

impl BlendBuffer {
    pub fn new(
        descriptors: &Descriptors,
        size: wgpu::Extent3d,
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
        pool: &mut TexturePool,
    ) -> Self {
        let texture = pool.get_texture(descriptors, size, usage, format, 1);

        Self { texture }
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.texture.1
    }

    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture.0
    }
}

#[derive(Debug)]
pub struct StencilBuffer {
    texture: PoolEntry<(wgpu::Texture, wgpu::TextureView), AlwaysCompatible>,
}

impl StencilBuffer {
    pub fn new(
        descriptors: &Descriptors,
        msaa_sample_count: u32,
        size: wgpu::Extent3d,
        pool: &mut TexturePool,
    ) -> Self {
        let texture = pool.get_texture(
            descriptors,
            size,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
            wgpu::TextureFormat::Stencil8,
            msaa_sample_count,
        );

        Self { texture }
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.texture.1
    }
}

pub struct CommandTarget {
    frame_buffer: FrameBuffer,
    blend_buffer: OnceCell<BlendBuffer>,
    resolve_buffer: Option<ResolveBuffer>,
    depth: OnceCell<StencilBuffer>,
    globals: Arc<Globals>,
    /// Where this target sits in the space its commands were recorded in. Zero for the stage
    /// surface and for filters; a region's corner for a blend or mask sub-target. The globals view
    /// matrix is built around it, so any quad drawn over this target has to agree.
    origin: (u32, u32),
    size: wgpu::Extent3d,
    format: wgpu::TextureFormat,
    sample_count: u32,
    whole_frame_bind_group: OnceCell<(wgpu::Buffer, wgpu::BindGroup)>,
    color_needs_clear: OnceCell<bool>,
    render_target_mode: RenderTargetMode,
}

impl CommandTarget {
    /// A target covering the whole space its commands were recorded in. Filters and the main stage
    /// surface all want this.
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        descriptors: &Descriptors,
        pool: &mut TexturePool,
        size: wgpu::Extent3d,
        format: wgpu::TextureFormat,
        sample_count: u32,
        render_target_mode: RenderTargetMode,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Self {
        Self::new_at(
            (0, 0),
            descriptors,
            pool,
            size,
            format,
            sample_count,
            render_target_mode,
            encoder,
        )
    }

    /// A target covering only `size` pixels starting at `origin` within that space. Used by blends,
    /// which otherwise pay for a stage-sized texture per node no matter how small the blended object
    /// is. `origin` reaches the shader through the globals view matrix, so the commands keep their
    /// original coordinates.
    #[expect(clippy::too_many_arguments)]
    pub fn new_at(
        origin: (u32, u32),
        descriptors: &Descriptors,
        pool: &mut TexturePool,
        size: wgpu::Extent3d,
        format: wgpu::TextureFormat,
        sample_count: u32,
        render_target_mode: RenderTargetMode,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Self {
        let globals = pool.get_globals(descriptors, origin.0, origin.1, size.width, size.height);

        let mut make_pooled_frame_buffer = || {
            FrameBuffer::new(
                descriptors,
                sample_count,
                size,
                format,
                if sample_count > 1 {
                    wgpu::TextureUsages::RENDER_ATTACHMENT
                } else {
                    wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::COPY_SRC
                        | wgpu::TextureUsages::COPY_DST
                        | wgpu::TextureUsages::TEXTURE_BINDING
                },
                pool,
            )
        };

        let whole_frame_bind_group = OnceCell::new();

        let (frame_buffer, resolve_buffer) =
            if let RenderTargetMode::ExistingWithColor(texture, _) = &render_target_mode {
                if sample_count > 1 {
                    (
                        make_pooled_frame_buffer(),
                        Some(ResolveBuffer::new_manual(texture.clone())),
                    )
                } else {
                    (
                        FrameBuffer::new_manual(texture.clone(), texture.size()),
                        None,
                    )
                }
            } else if sample_count > 1 {
                (
                    make_pooled_frame_buffer(),
                    Some(ResolveBuffer::new(
                        descriptors,
                        size,
                        format,
                        wgpu::TextureUsages::COPY_SRC
                            | wgpu::TextureUsages::COPY_DST
                            | wgpu::TextureUsages::TEXTURE_BINDING
                            | wgpu::TextureUsages::RENDER_ATTACHMENT,
                        pool,
                    )),
                )
            } else {
                (make_pooled_frame_buffer(), None)
            };

        if let RenderTargetMode::FreshWithTexture(texture) = &render_target_mode {
            if let Some(resolve_buffer) = &resolve_buffer {
                encoder.copy_texture_to_texture(
                    texture.as_image_copy(),
                    resolve_buffer.texture().as_image_copy(),
                    size,
                );
            }

            if sample_count > 1 {
                // Both our frame buffer and resolve buffer need to start out
                // in the same state, so copy our existing texture to the freshly
                // allocated frame buffer. We cannot use `copy_texture_to_texture`,
                // since the sample counts are different.
                run_copy_pipeline(
                    descriptors,
                    format,
                    frame_buffer.texture.view(),
                    &texture.create_view(&Default::default()),
                    get_whole_frame_bind_group(&whole_frame_bind_group, descriptors, origin, size),
                    &globals,
                    sample_count,
                    encoder,
                );
            } else {
                encoder.copy_texture_to_texture(
                    texture.as_image_copy(),
                    frame_buffer.texture().as_image_copy(),
                    size,
                );
            }
        }

        Self {
            frame_buffer,
            blend_buffer: OnceCell::new(),
            resolve_buffer,
            depth: OnceCell::new(),
            globals,
            origin,
            size,
            format,
            sample_count,
            whole_frame_bind_group,
            color_needs_clear: OnceCell::new(),
            render_target_mode,
        }
    }

    pub fn width(&self) -> u32 {
        self.size.width
    }

    pub fn height(&self) -> u32 {
        self.size.height
    }

    pub fn ensure_cleared(&self, encoder: &mut wgpu::CommandEncoder) {
        if self.color_needs_clear.get().is_some() {
            return;
        }
        // If we aren't clearing with a color (eg a texture instead)
        // the there's no point in creating a new render pass that does nothing.
        if self.render_target_mode.color().is_some() {
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: create_debug_label!("Clearing command target").as_deref(),
                color_attachments: &[self.color_attachments()],
                ..Default::default()
            });
        }
    }

    pub fn take_color_texture(self) -> PoolOrArcTexture {
        self.resolve_buffer
            .map(|b| b.take_texture())
            .unwrap_or_else(|| self.frame_buffer.take_texture())
    }

    pub fn globals(&self) -> &Globals {
        &self.globals
    }

    pub fn whole_frame_bind_group(&self, descriptors: &Descriptors) -> &wgpu::BindGroup {
        get_whole_frame_bind_group(
            &self.whole_frame_bind_group,
            descriptors,
            self.origin,
            self.size,
        )
    }

    /// Transforms for compositing a child that covers only `region` of this target.
    ///
    /// Two things have to be right at once. The quad must land on the right part of the target, so
    /// `world_matrix` places it there. And the blend shaders derive their UV from clip position --
    /// which is the position within the TARGET, correct for sampling the parent -- so the child,
    /// being a smaller texture, needs that UV remapped onto its own [0, 1]. That remap rides in
    /// `bitmap_uv_scale` as `(scale, offset)`, which every other draw type leaves at the identity
    /// `(1, 1, 0, 0)`.
    ///
    /// The child's own texture size is asked for rather than assumed to be the region. A pooled
    /// texture is routinely larger than what was drawn into it: the pool rounds requested sizes out
    /// to a grid so that near-identical content shares a bucket, and `CommandTarget` therefore keeps
    /// its logical size separately from its texture's. Mapping the region onto the whole of that
    /// texture squashes the child into a corner of itself and samples cleared padding for the rest,
    /// which is a shrunken image with a rectangle of wrong pixels around it. `FilterRegion` already
    /// normalises against the texture for exactly this reason; this did not.
    ///
    /// Not cached: unlike the whole-frame group there is a different region per blend.
    pub fn region_frame_bind_group(
        &self,
        descriptors: &Descriptors,
        region: (u32, u32, u32, u32),
        child_texture: wgpu::Extent3d,
    ) -> wgpu::BindGroup {
        #[cfg(feature = "aether_metrics")]
        crate::aether_metrics::record_bind_group_created();
        let (rx, ry, rw, rh) = region;
        let (local_x, local_y) = (
            rx.saturating_sub(self.origin.0) as f32,
            ry.saturating_sub(self.origin.1) as f32,
        );
        let (rw, rh) = (rw.max(1) as f32, rh.max(1) as f32);
        let (tw, th) = (
            self.size.width.max(1) as f32,
            self.size.height.max(1) as f32,
        );
        // What the child's [0, 1] actually spans. Equal to the region whenever the pool handed back
        // an exact texture, and larger whenever it did not.
        let (cw, ch) = (
            child_texture.width.max(1) as f32,
            child_texture.height.max(1) as f32,
        );

        let transform = Transforms {
            world_matrix: [
                [rw, 0.0, 0.0, 0.0],
                [0.0, rh, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [rx as f32, ry as f32, 0.0, 1.0],
            ],
            mult_color: [1.0, 1.0, 1.0, 1.0],
            add_color: [0.0, 0.0, 0.0, 0.0],
            // Divided by the child's texture rather than by the region, so the region's left edge
            // lands on 0 and its right edge on `rw / cw` -- where the drawn content actually ends --
            // instead of on 1, which is where the padding ends.
            bitmap_uv_scale: [tw / cw, th / ch, -local_x / cw, -local_y / ch],
        };
        let buffer = create_buffer_with_data(
            &descriptors.device,
            bytemuck::cast_slice(&[transform]),
            wgpu::BufferUsages::UNIFORM,
            create_debug_label!("Region-frame transforms buffer"),
        );
        descriptors
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &descriptors.bind_layouts.transforms,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
                label: create_debug_label!("Region-frame transforms bind group").as_deref(),
            })
    }

    pub fn color_attachments(&self) -> Option<wgpu::RenderPassColorAttachment<'_>> {
        let mut load = wgpu::LoadOp::Load;
        if self.color_needs_clear.set(false).is_ok()
            && let Some(clear_color) = self.render_target_mode.color()
        {
            load = wgpu::LoadOp::Clear(clear_color);
        }
        Some(wgpu::RenderPassColorAttachment {
            view: self.frame_buffer.view(),
            resolve_target: self.resolve_buffer.as_ref().map(|b| b.view()),
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
            depth_slice: None,
        })
    }

    pub fn sample_count(&self) -> u32 {
        self.sample_count
    }

    pub fn stencil_attachment(
        &self,
        descriptors: &Descriptors,
        pool: &mut TexturePool,
    ) -> Option<wgpu::RenderPassDepthStencilAttachment<'_>> {
        let new_buffer = self.depth.get().is_none();
        let stencil = self
            .depth
            .get_or_init(|| StencilBuffer::new(descriptors, self.sample_count, self.size, pool));
        Some(wgpu::RenderPassDepthStencilAttachment {
            view: stencil.view(),
            depth_ops: None,
            stencil_ops: Some(wgpu::Operations {
                load: if new_buffer {
                    wgpu::LoadOp::Clear(0)
                } else {
                    wgpu::LoadOp::Load
                },
                store: wgpu::StoreOp::Store,
            }),
        })
    }

    /// The parent's pixels a blend will composite against, ready to sample.
    ///
    /// `region` is the part of this target the blend covers, in the space its commands were
    /// recorded in, or `None` for a blend spanning the whole surface. Only that part is refreshed:
    /// see [`blend_buffer_copy_region`].
    pub fn update_blend_buffer(
        &self,
        descriptors: &Descriptors,
        pool: &mut TexturePool,
        encoder: &mut wgpu::CommandEncoder,
        region: Option<(u32, u32, u32, u32)>,
    ) -> &BlendBuffer {
        let blend_buffer = self.blend_buffer.get_or_init(|| {
            BlendBuffer::new(
                descriptors,
                self.size,
                self.format,
                wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC,
                pool,
            )
        });
        self.ensure_cleared(encoder);
        if let Some((origin, extent)) =
            blend_buffer_copy_region(self.origin, self.frame_buffer.size(), region)
        {
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: self
                        .resolve_buffer
                        .as_ref()
                        .map(|b| b.texture())
                        .unwrap_or_else(|| self.frame_buffer.texture()),
                    mip_level: 0,
                    origin,
                    aspect: Default::default(),
                },
                wgpu::TexelCopyTextureInfo {
                    texture: blend_buffer.texture(),
                    mip_level: 0,
                    origin,
                    aspect: Default::default(),
                },
                extent,
            );
        }
        blend_buffer
    }

    pub fn color_view(&self) -> &wgpu::TextureView {
        self.resolve_buffer
            .as_ref()
            .map(|b| b.view())
            .unwrap_or_else(|| self.frame_buffer.view())
    }

    pub fn color_texture(&self) -> &wgpu::Texture {
        self.resolve_buffer
            .as_ref()
            .map(|b| b.texture())
            .unwrap_or_else(|| self.frame_buffer.texture())
    }
}

/// The part of a parent target that a blend actually reads back.
///
/// A complex blend composites its child against the parent's blend buffer, sampling it at the
/// fragment's position within the parent. The child's quad covers only its own region, so those
/// are the only texels the shader can ever read. The copy that fills the buffer was handing over
/// the whole parent regardless, which on a crowded stage is one full surface copy per blended
/// object per frame.
///
/// The buffer itself stays full size so the shader can keep indexing it in target space. Only the
/// copy shrinks. `None` means the region does not land on this target at all, so there is nothing
/// to copy and a zero-sized copy would be an error rather than a saving.
fn blend_buffer_copy_region(
    target_origin: (u32, u32),
    target_size: wgpu::Extent3d,
    region: Option<(u32, u32, u32, u32)>,
) -> Option<(wgpu::Origin3d, wgpu::Extent3d)> {
    let Some((x, y, width, height)) = region else {
        return Some((wgpu::Origin3d::ZERO, target_size));
    };

    // The region is in the space the commands were recorded in; the target may start part way
    // into it.
    let local_x = x.saturating_sub(target_origin.0).min(target_size.width);
    let local_y = y.saturating_sub(target_origin.1).min(target_size.height);
    let width = width.min(target_size.width - local_x);
    let height = height.min(target_size.height - local_y);
    if width == 0 || height == 0 {
        return None;
    }

    Some((
        wgpu::Origin3d {
            x: local_x,
            y: local_y,
            z: 0,
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    ))
}

#[cfg(test)]
mod blend_buffer_copy_tests {
    use super::blend_buffer_copy_region;

    const STAGE: wgpu::Extent3d = wgpu::Extent3d {
        width: 2560,
        height: 1365,
        depth_or_array_layers: 1,
    };

    #[test]
    fn a_blend_copies_back_only_the_region_it_composites_over() {
        // One armour layer on one avatar, against a full stage.
        let (origin, extent) =
            blend_buffer_copy_region((0, 0), STAGE, Some((820, 410, 320, 256))).unwrap();

        assert_eq!((origin.x, origin.y), (820, 410));
        assert_eq!((extent.width, extent.height), (320, 256));
    }

    #[test]
    fn a_full_surface_blend_still_copies_the_whole_parent() {
        let (origin, extent) = blend_buffer_copy_region((0, 0), STAGE, None).unwrap();

        assert_eq!((origin.x, origin.y), (0, 0));
        assert_eq!((extent.width, extent.height), (STAGE.width, STAGE.height));
    }

    #[test]
    fn a_region_is_measured_from_the_targets_own_origin() {
        // Alpha and Erase composite against the nearest layer, which is its own sub-target sitting
        // somewhere else on the surface.
        let target = wgpu::Extent3d {
            width: 512,
            height: 512,
            depth_or_array_layers: 1,
        };
        let (origin, extent) =
            blend_buffer_copy_region((800, 400), target, Some((820, 410, 320, 256))).unwrap();

        assert_eq!((origin.x, origin.y), (20, 10));
        assert_eq!((extent.width, extent.height), (320, 256));
    }

    #[test]
    fn a_region_running_past_the_edge_is_clamped_to_what_exists() {
        let (origin, extent) =
            blend_buffer_copy_region((0, 0), STAGE, Some((2400, 1300, 320, 256))).unwrap();

        assert_eq!((origin.x, origin.y), (2400, 1300));
        assert_eq!((extent.width, extent.height), (160, 65));
    }

    #[test]
    fn a_region_entirely_off_the_target_copies_nothing() {
        assert_eq!(
            blend_buffer_copy_region((0, 0), STAGE, Some((2560, 1365, 320, 256))),
            None
        );
    }
}

fn get_whole_frame_bind_group<'a>(
    once_cell: &'a OnceCell<(wgpu::Buffer, wgpu::BindGroup)>,
    descriptors: &Descriptors,
    origin: (u32, u32),
    size: wgpu::Extent3d,
) -> &'a wgpu::BindGroup {
    &once_cell
        .get_or_init(|| {
            // The quad has to land on the target in the SAME space the globals view matrix was
            // built for, so it starts at the target's origin rather than at zero. That is always
            // zero for the stage surface, which is why this went unnoticed until sub-targets
            // acquired origins of their own.
            let transform = Transforms {
                world_matrix: [
                    [size.width as f32, 0.0, 0.0, 0.0],
                    [0.0, size.height as f32, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                    [origin.0 as f32, origin.1 as f32, 0.0, 1.0],
                ],
                mult_color: [1.0, 1.0, 1.0, 1.0],
                add_color: [0.0, 0.0, 0.0, 0.0],
                bitmap_uv_scale: [1.0, 1.0, 0.0, 0.0],
            };
            let transforms_buffer = create_buffer_with_data(
                &descriptors.device,
                bytemuck::cast_slice(&[transform]),
                wgpu::BufferUsages::UNIFORM,
                create_debug_label!("Whole-frame transforms buffer"),
            );
            let whole_frame_bind_group =
                descriptors
                    .device
                    .create_bind_group(&wgpu::BindGroupDescriptor {
                        layout: &descriptors.bind_layouts.transforms,
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: transforms_buffer.as_entire_binding(),
                        }],
                        label: create_debug_label!("Whole-frame transforms bind group").as_deref(),
                    });
            (transforms_buffer, whole_frame_bind_group)
        })
        .1
}
