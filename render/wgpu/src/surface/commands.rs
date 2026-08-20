use crate::backend::RenderTargetMode;
use crate::blend::TrivialBlend;
use crate::blend::{BlendType, ComplexBlend};
use crate::blend_region::{BlendRegion, blend_region_grid, plan_blend_region, quantise_region};
use crate::buffer_builder::BufferBuilder;
use crate::buffer_pool::TexturePool;
use crate::dynamic_transforms::DynamicTransforms;
use crate::mesh::{DrawType, Mesh, as_mesh};
use crate::surface::Surface;
use crate::surface::target::CommandTarget;
use crate::{Descriptors, MaskState, Pipelines, Transforms, as_texture};
use ruffle_render::backend::ShapeHandle;
use ruffle_render::bitmap::{BitmapHandle, BitmapSize, PixelSnapping};
use ruffle_render::commands::{Command, CommandHandler, CommandList, RenderBlendMode};
use ruffle_render::lines::{emulate_line, emulate_line_rect};
use ruffle_render::matrix::Matrix;
use ruffle_render::pixel_bender::PixelBenderShaderHandle;
use ruffle_render::quality::StageQuality;
use ruffle_render::transform::Transform;
use std::mem;
use swf::{BlendMode, Color, ColorTransform, Rectangle, Twips};
use wgpu::Backend;

use super::target::PoolOrArcTexture;

pub struct CommandRenderer<'pass, 'frame: 'pass, 'global: 'frame> {
    pipelines: &'frame Pipelines,
    descriptors: &'global Descriptors,
    num_masks: u32,
    mask_state: MaskState,
    render_pass: wgpu::RenderPass<'pass>,
    needs_stencil: bool,
    dynamic_transforms: &'global DynamicTransforms,
}

impl<'pass, 'frame: 'pass, 'global: 'frame> CommandRenderer<'pass, 'frame, 'global> {
    pub fn new(
        pipelines: &'frame Pipelines,
        descriptors: &'global Descriptors,
        dynamic_transforms: &'global DynamicTransforms,
        render_pass: wgpu::RenderPass<'pass>,
        num_masks: u32,
        mask_state: MaskState,
        needs_stencil: bool,
    ) -> Self {
        Self {
            pipelines,
            num_masks,
            mask_state,
            render_pass,
            descriptors,
            needs_stencil,
            dynamic_transforms,
        }
    }

    pub fn execute(&mut self, command: &'frame DrawCommand) {
        if self.needs_stencil {
            match self.mask_state {
                MaskState::NoMask => {}
                MaskState::DrawMaskStencil => {
                    self.render_pass.set_stencil_reference(self.num_masks - 1);
                }
                MaskState::DrawMaskedContent => {
                    self.render_pass.set_stencil_reference(self.num_masks);
                }
                MaskState::ClearMaskStencil => {
                    self.render_pass.set_stencil_reference(self.num_masks);
                }
            }
        }

        match command {
            DrawCommand::RenderBitmap {
                bitmap,
                transform_buffer,
                smoothing,
                blend_mode,
                render_stage3d,
            } => self.render_bitmap(
                bitmap,
                *transform_buffer,
                *smoothing,
                *blend_mode,
                *render_stage3d,
            ),
            DrawCommand::RenderTexture {
                _texture,
                binds,
                transform_buffer,
                blend_mode,
            } => self.render_texture(*transform_buffer, binds, *blend_mode),
            DrawCommand::RenderShape {
                shape,
                transform_buffer,
                blend_mode,
            } => self.render_shape(shape, *transform_buffer, *blend_mode),
            DrawCommand::DrawRect { transform_buffer } => self.draw_rect(*transform_buffer),
            DrawCommand::DrawLine { transform_buffer } => {
                self.draw_lines::<false>(*transform_buffer)
            }
            DrawCommand::DrawLineRect { transform_buffer } => {
                self.draw_lines::<true>(*transform_buffer)
            }
            DrawCommand::PushMask => self.push_mask(),
            DrawCommand::ActivateMask => self.activate_mask(),
            DrawCommand::DeactivateMask => self.deactivate_mask(),
            DrawCommand::PopMask => self.pop_mask(),
            DrawCommand::RenderAlphaMask {
                maskee,
                mask,
                binds,
                transform_buffer,
            } => self.render_alpha_mask(maskee, mask, binds, *transform_buffer),
        }
    }

    pub fn prep_color(&mut self, blend_mode: TrivialBlend) {
        if self.needs_stencil {
            self.render_pass
                .set_pipeline(self.pipelines.color[blend_mode].pipeline_for(self.mask_state));
        } else {
            self.render_pass
                .set_pipeline(self.pipelines.color[blend_mode].stencilless_pipeline());
        }
    }

    pub fn prep_lines(&mut self) {
        if self.needs_stencil {
            self.render_pass
                .set_pipeline(self.pipelines.lines.pipeline_for(self.mask_state));
        } else {
            self.render_pass
                .set_pipeline(self.pipelines.lines.stencilless_pipeline());
        }
    }

    pub fn prep_gradient(
        &mut self,
        bind_group: &'pass wgpu::BindGroup,
        texture_transforms_offset: wgpu::DynamicOffset,
        gradient_offset: wgpu::DynamicOffset,
        blend_mode: TrivialBlend,
    ) {
        if self.needs_stencil {
            self.render_pass
                .set_pipeline(self.pipelines.gradients[blend_mode].pipeline_for(self.mask_state));
        } else {
            self.render_pass
                .set_pipeline(self.pipelines.gradients[blend_mode].stencilless_pipeline());
        }

        self.render_pass.set_bind_group(
            2,
            bind_group,
            &[texture_transforms_offset, gradient_offset],
        );
    }

    pub fn prep_bitmap(
        &mut self,
        bind_group: &'pass wgpu::BindGroup,
        blend_mode: TrivialBlend,
        render_stage3d: bool,
    ) {
        match (self.needs_stencil, render_stage3d) {
            (true, true) => {
                self.render_pass
                    .set_pipeline(&self.pipelines.bitmap_opaque_dummy_stencil);
            }
            (true, false) => {
                self.render_pass
                    .set_pipeline(self.pipelines.bitmap[blend_mode].pipeline_for(self.mask_state));
            }
            (false, true) => {
                self.render_pass.set_pipeline(&self.pipelines.bitmap_opaque);
            }
            (false, false) => {
                self.render_pass
                    .set_pipeline(self.pipelines.bitmap[blend_mode].stencilless_pipeline());
            }
        }

        self.render_pass.set_bind_group(2, bind_group, &[]);
    }

    pub fn prep_alpha_mask(&mut self, bind_group: &'pass wgpu::BindGroup) {
        if self.needs_stencil {
            self.render_pass
                .set_pipeline(self.pipelines.alpha_mask.pipeline_for(self.mask_state));
        } else {
            self.render_pass
                .set_pipeline(self.pipelines.alpha_mask.stencilless_pipeline());
        }

        self.render_pass.set_bind_group(2, bind_group, &[]);
    }

    pub fn draw(
        &mut self,
        vertices: wgpu::BufferSlice<'pass>,
        indices: wgpu::BufferSlice<'pass>,
        num_indices: u32,
    ) {
        self.render_pass.set_vertex_buffer(0, vertices);
        self.render_pass
            .set_index_buffer(indices, wgpu::IndexFormat::Uint32);

        self.render_pass.draw_indexed(0..num_indices, 0, 0..1);
    }

    pub fn render_bitmap(
        &mut self,
        bitmap: &'frame BitmapHandle,
        transform_buffer: wgpu::DynamicOffset,
        smoothing: bool,
        blend_mode: TrivialBlend,
        render_stage3d: bool,
    ) {
        if cfg!(feature = "render_debug_labels") {
            self.render_pass
                .push_debug_group(&format!("render_bitmap {:?}", bitmap.0));
        }
        let texture = as_texture(bitmap);

        let descriptors = self.descriptors;
        let bind = texture.bind_group(
            smoothing,
            &descriptors.device,
            &descriptors.bind_layouts.bitmap,
            &descriptors.quad,
            bitmap.clone(),
            &descriptors.bitmap_samplers,
        );
        self.prep_bitmap(&bind.bind_group, blend_mode, render_stage3d);
        self.render_pass.set_bind_group(
            1,
            &self.dynamic_transforms.bind_group,
            &[transform_buffer],
        );

        self.draw(
            self.descriptors.quad.vertices_pos.slice(..),
            self.descriptors.quad.indices.slice(..),
            6,
        );
        if cfg!(feature = "render_debug_labels") {
            self.render_pass.pop_debug_group();
        }
    }

    pub fn render_texture(
        &mut self,
        transform_buffer: wgpu::DynamicOffset,
        bind_group: &'frame wgpu::BindGroup,
        blend_mode: TrivialBlend,
    ) {
        if cfg!(feature = "render_debug_labels") {
            self.render_pass.push_debug_group("render_texture");
        }
        self.prep_bitmap(bind_group, blend_mode, false);

        self.render_pass.set_bind_group(
            1,
            &self.dynamic_transforms.bind_group,
            &[transform_buffer],
        );

        self.draw(
            self.descriptors.quad.vertices_pos.slice(..),
            self.descriptors.quad.indices.slice(..),
            6,
        );
        if cfg!(feature = "render_debug_labels") {
            self.render_pass.pop_debug_group();
        }
    }

    pub fn render_shape(
        &mut self,
        shape: &'frame ShapeHandle,
        transform_buffer: wgpu::DynamicOffset,
        blend_mode: TrivialBlend,
    ) {
        if cfg!(feature = "render_debug_labels") {
            self.render_pass.push_debug_group("render_shape");
        }

        let mesh = as_mesh(shape);
        for draw in &mesh.draws {
            let num_indices = if self.mask_state != MaskState::DrawMaskStencil
                && self.mask_state != MaskState::ClearMaskStencil
            {
                draw.num_indices
            } else {
                // Omit strokes when drawing a mask stencil.
                draw.num_mask_indices
            };
            if num_indices == 0 {
                continue;
            }

            match &draw.draw_type {
                DrawType::Color => {
                    self.prep_color(blend_mode);
                }
                DrawType::Gradient {
                    bind_group,
                    texture_transforms_offset,
                    gradient_offset,
                } => {
                    self.prep_gradient(
                        bind_group,
                        *texture_transforms_offset,
                        *gradient_offset,
                        blend_mode,
                    );
                }
                DrawType::Bitmap { binds, .. } => {
                    self.prep_bitmap(&binds.bind_group, blend_mode, false);
                }
            }
            self.render_pass.set_bind_group(
                1,
                &self.dynamic_transforms.bind_group,
                &[transform_buffer],
            );

            self.draw(
                mesh.vertex_buffer.slice(draw.vertices.clone()),
                mesh.index_buffer.slice(draw.indices.clone()),
                num_indices,
            );
        }
        if cfg!(feature = "render_debug_labels") {
            self.render_pass.pop_debug_group();
        }
    }

    pub fn render_alpha_mask(
        &mut self,
        _maskee: &PoolOrArcTexture,
        _mask: &PoolOrArcTexture,
        bind_group: &'frame wgpu::BindGroup,
        transform_buffer: wgpu::DynamicOffset,
    ) {
        if cfg!(feature = "render_debug_labels") {
            self.render_pass.push_debug_group("render_alpha_mask");
        }

        self.prep_alpha_mask(bind_group);

        self.render_pass.set_bind_group(
            1,
            &self.dynamic_transforms.bind_group,
            &[transform_buffer],
        );

        self.draw(
            self.descriptors.quad.vertices_pos.slice(..),
            self.descriptors.quad.indices.slice(..),
            6,
        );

        if cfg!(feature = "render_debug_labels") {
            self.render_pass.pop_debug_group();
        }
    }

    pub fn draw_rect(&mut self, transform_buffer: wgpu::DynamicOffset) {
        if cfg!(feature = "render_debug_labels") {
            self.render_pass.push_debug_group("draw_rect");
        }
        // A rectangle is only ever drawn on its own account, never as the draw a blend collapsed
        // into, so it stays at Normal.
        self.prep_color(TrivialBlend::Normal);

        self.render_pass.set_bind_group(
            1,
            &self.dynamic_transforms.bind_group,
            &[transform_buffer],
        );

        self.draw(
            self.descriptors.quad.vertices_pos_color.slice(..),
            self.descriptors.quad.indices.slice(..),
            6,
        );
        if cfg!(feature = "render_debug_labels") {
            self.render_pass.pop_debug_group();
        }
    }

    pub fn draw_lines<const RECT: bool>(&mut self, transform_buffer: wgpu::DynamicOffset) {
        if cfg!(feature = "render_debug_labels") {
            self.render_pass.push_debug_group("draw_lines");
        }
        self.prep_lines();

        self.render_pass.set_bind_group(
            1,
            &self.dynamic_transforms.bind_group,
            &[transform_buffer],
        );

        self.draw(
            self.descriptors.quad.vertices_pos_color.slice(..),
            if RECT {
                self.descriptors.quad.indices_line_rect.slice(..)
            } else {
                self.descriptors.quad.indices_line.slice(..)
            },
            if RECT { 5 } else { 2 },
        );
        if cfg!(feature = "render_debug_labels") {
            self.render_pass.pop_debug_group();
        }
    }

    pub fn push_mask(&mut self) {
        debug_assert!(
            self.mask_state == MaskState::NoMask || self.mask_state == MaskState::DrawMaskedContent
        );
        self.num_masks += 1;
        self.mask_state = MaskState::DrawMaskStencil;
        self.render_pass.set_stencil_reference(self.num_masks - 1);
    }

    pub fn activate_mask(&mut self) {
        debug_assert!(self.num_masks > 0 && self.mask_state == MaskState::DrawMaskStencil);
        self.mask_state = MaskState::DrawMaskedContent;
        self.render_pass.set_stencil_reference(self.num_masks);
    }

    pub fn deactivate_mask(&mut self) {
        debug_assert!(self.num_masks > 0 && self.mask_state == MaskState::DrawMaskedContent);
        self.mask_state = MaskState::ClearMaskStencil;
        self.render_pass.set_stencil_reference(self.num_masks);
    }

    pub fn pop_mask(&mut self) {
        debug_assert!(self.num_masks > 0 && self.mask_state == MaskState::ClearMaskStencil);
        self.num_masks -= 1;
        self.render_pass.set_stencil_reference(self.num_masks);
        if self.num_masks == 0 {
            self.mask_state = MaskState::NoMask;
        } else {
            self.mask_state = MaskState::DrawMaskedContent;
        };
    }

    pub fn num_masks(&self) -> u32 {
        self.num_masks
    }

    pub fn mask_state(&self) -> MaskState {
        self.mask_state
    }
}

pub enum Chunk {
    Draw {
        chunk: Vec<DrawCommand>,
        needs_stencil: bool,
        transforms: BufferBuilder,
        /// Where this chunk draws, in the space blend regions use. `None` means it holds a command
        /// whose extent is not known, so it must be assumed to cover everything.
        ///
        /// Only used to decide whether a blend may be reordered past this chunk; nothing about how
        /// the chunk itself renders depends on it.
        bounds: Option<DrawRegions>,
    },
    Blend {
        texture: PoolOrArcTexture,
        blend_mode: ChunkBlendMode,
        needs_stencil: bool,
        /// Where this child sits in the target, when it covers less than all of it. `None` is a
        /// full-surface child and composites exactly as it always has.
        region: Option<(u32, u32, u32, u32)>,
    },
}

#[derive(Debug)]
pub enum ChunkBlendMode {
    Complex(ComplexBlend),
    Shader(PixelBenderShaderHandle),
}

#[derive(Debug)]
pub enum DrawCommand {
    RenderBitmap {
        bitmap: BitmapHandle,
        transform_buffer: wgpu::DynamicOffset,
        smoothing: bool,
        blend_mode: TrivialBlend,
        render_stage3d: bool,
    },
    RenderTexture {
        _texture: PoolOrArcTexture,
        binds: wgpu::BindGroup,
        transform_buffer: wgpu::DynamicOffset,
        blend_mode: TrivialBlend,
    },
    RenderAlphaMask {
        maskee: PoolOrArcTexture,
        mask: PoolOrArcTexture,
        binds: wgpu::BindGroup,
        transform_buffer: wgpu::DynamicOffset,
    },
    RenderShape {
        shape: ShapeHandle,
        transform_buffer: wgpu::DynamicOffset,
        blend_mode: TrivialBlend,
    },
    DrawRect {
        transform_buffer: wgpu::DynamicOffset,
    },
    DrawLine {
        transform_buffer: wgpu::DynamicOffset,
    },
    DrawLineRect {
        transform_buffer: wgpu::DynamicOffset,
    },
    PushMask,
    ActivateMask,
    DeactivateMask,
    PopMask,
}

#[derive(Copy, Clone)]
pub enum LayerRef<'a> {
    None,
    Current,
    Parent(&'a CommandTarget),
}

/// Replaces every blend with a RenderBitmap, with the subcommands rendered out to a temporary texture
/// Every complex blend will be its own item, but every other draw will be chunked together
#[expect(clippy::too_many_arguments)]
pub fn chunk_blends<'a>(
    commands: CommandList,
    descriptors: &'a Descriptors,
    staging_belt: &'a mut wgpu::util::StagingBelt,
    dynamic_transforms: &'a DynamicTransforms,
    draw_encoder: &mut wgpu::CommandEncoder,
    meshes: &'a Vec<Mesh>,
    quality: StageQuality,
    origin: (u32, u32),
    width: u32,
    height: u32,
    nearest_layer: LayerRef,
    texture_pool: &mut TexturePool,
) -> Vec<Chunk> {
    WgpuCommandHandler::new(
        descriptors,
        staging_belt,
        dynamic_transforms,
        draw_encoder,
        meshes,
        quality,
        origin,
        width,
        height,
        nearest_layer,
        texture_pool,
    )
    .chunk_blends(commands)
}

struct WgpuCommandHandler<'a> {
    descriptors: &'a Descriptors,
    quality: StageQuality,
    /// Where this handler's target sits in the space its commands were recorded in. Zero for the
    /// stage surface; the region's corner for a blend rendering into a sub-target.
    origin: (u32, u32),
    width: u32,
    height: u32,
    nearest_layer: LayerRef<'a>,
    meshes: &'a Vec<Mesh>,
    staging_belt: &'a mut wgpu::util::StagingBelt,
    dynamic_transforms: &'a DynamicTransforms,
    draw_encoder: &'a mut wgpu::CommandEncoder,
    texture_pool: &'a mut TexturePool,
    emulate_lines: bool,

    result: Vec<Chunk>,
    current: Vec<DrawCommand>,
    transforms: BufferBuilder,
    needs_stencil: bool,
    num_masks: i32,
    /// Where everything in `current` draws, in the space the commands were recorded in.
    ///
    /// `None` means at least one command in the chunk could not be bounded, so the chunk has to be
    /// treated as covering everything. See [`Chunk::Draw::bounds`].
    current_bounds: ChunkBounds,
}

/// How many disjoint boxes a draw chunk's extent is described by.
///
/// One union per chunk was the first attempt and it does not work: a crowded AQW frame packs
/// twenty avatars into a chunk, so the union is most of the screen and no blend can ever be shown
/// to clear it. Measured with a single box, 246 blend passes stayed 246 against an ideal of 63.
/// Keeping a handful of boxes describes "two avatars, opposite corners" for what it is. Eight is
/// enough to separate the clusters that matter without making the overlap test expensive, since it
/// runs against every candidate blend.
const CHUNK_BOUND_BOXES: usize = 8;

/// The screen extent of a draw chunk, or the admission that it is not known.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChunkBounds {
    /// Nothing has been drawn into this chunk yet.
    Empty,
    /// Everything so far is inside these boxes, in the recording space.
    Known([Rectangle<Twips>; CHUNK_BOUND_BOXES], u8),
    /// Something in this chunk cannot be bounded. Treat it as covering the whole target.
    Unknown,
}

const EMPTY_BOX: Rectangle<Twips> = Rectangle {
    x_min: Twips::ZERO,
    x_max: Twips::ZERO,
    y_min: Twips::ZERO,
    y_max: Twips::ZERO,
};

/// How much area merging two boxes would add over keeping them apart. Chooses the cheapest pair
/// to collapse once the table is full, so distant clusters stay distinct for as long as possible.
fn union_cost(a: &Rectangle<Twips>, b: &Rectangle<Twips>) -> i64 {
    let area = |r: &Rectangle<Twips>| {
        let w = (r.x_max.get() as i64 - r.x_min.get() as i64).max(0);
        let h = (r.y_max.get() as i64 - r.y_min.get() as i64).max(0);
        w.saturating_mul(h)
    };
    area(&a.union(b))
        .saturating_sub(area(a))
        .saturating_sub(area(b))
}

impl ChunkBounds {
    fn add(&mut self, bounds: Option<Rectangle<Twips>>) {
        let Some(bounds) = bounds else {
            *self = ChunkBounds::Unknown;
            return;
        };
        if !bounds.is_valid() {
            return;
        }

        match self {
            ChunkBounds::Unknown => {}
            ChunkBounds::Empty => {
                let mut boxes = [EMPTY_BOX; CHUNK_BOUND_BOXES];
                boxes[0] = bounds;
                *self = ChunkBounds::Known(boxes, 1);
            }
            ChunkBounds::Known(boxes, len) => {
                let used = *len as usize;
                if used < CHUNK_BOUND_BOXES {
                    boxes[used] = bounds;
                    *len += 1;
                    return;
                }
                // Full: fold the new box into whichever existing one it costs least to widen.
                let mut best = 0;
                let mut best_cost = i64::MAX;
                for (index, held) in boxes.iter().enumerate() {
                    let cost = union_cost(held, &bounds);
                    if cost < best_cost {
                        best_cost = cost;
                        best = index;
                    }
                }
                boxes[best] = boxes[best].union(&bounds);
            }
        }
    }

    fn take(&mut self) -> Self {
        mem::replace(self, ChunkBounds::Empty)
    }

    /// As pixel rectangles in the space blend regions use, or `None` if unbounded.
    ///
    /// An empty chunk draws nothing, so it obstructs nothing and reports no boxes at all.
    fn to_regions(self) -> Option<DrawRegions> {
        match self {
            ChunkBounds::Unknown => None,
            ChunkBounds::Empty => Some(DrawRegions::default()),
            ChunkBounds::Known(boxes, len) => {
                let mut regions = DrawRegions::default();
                for held in boxes.iter().take(len as usize) {
                    if !held.is_valid() {
                        continue;
                    }
                    let x = held.x_min.to_pixels().floor().max(0.0) as u32;
                    let y = held.y_min.to_pixels().floor().max(0.0) as u32;
                    let right = held.x_max.to_pixels().ceil().max(0.0) as u32;
                    let bottom = held.y_max.to_pixels().ceil().max(0.0) as u32;
                    regions.push((x, y, right.saturating_sub(x), bottom.saturating_sub(y)));
                }
                Some(regions)
            }
        }
    }
}

/// The boxes a draw chunk occupies, in the space blend regions use.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DrawRegions {
    boxes: [(u32, u32, u32, u32); CHUNK_BOUND_BOXES],
    len: u8,
}

impl DrawRegions {
    fn push(&mut self, region: (u32, u32, u32, u32)) {
        if (self.len as usize) < CHUNK_BOUND_BOXES {
            self.boxes[self.len as usize] = region;
            self.len += 1;
        }
    }

    /// Whether every box is spoken for, so further drawing widens one instead of adding another.
    pub fn is_at_capacity(&self) -> bool {
        self.len as usize >= CHUNK_BOUND_BOXES
    }

    /// Whether a blend covering `region` touches any of this chunk's drawing.
    pub fn overlaps(&self, region: Option<(u32, u32, u32, u32)>) -> bool {
        self.boxes[..self.len as usize]
            .iter()
            .any(|held| blend_regions_overlap(Some(*held), region))
    }
}

impl<'a> WgpuCommandHandler<'a> {
    #[expect(clippy::too_many_arguments)]
    fn new(
        descriptors: &'a Descriptors,
        staging_belt: &'a mut wgpu::util::StagingBelt,
        dynamic_transforms: &'a DynamicTransforms,
        draw_encoder: &'a mut wgpu::CommandEncoder,
        meshes: &'a Vec<Mesh>,
        quality: StageQuality,
        origin: (u32, u32),
        width: u32,
        height: u32,
        nearest_layer: LayerRef<'a>,
        texture_pool: &'a mut TexturePool,
    ) -> Self {
        let transforms = Self::new_transforms(descriptors, dynamic_transforms);

        // DirectX does support drawing lines, but it's very inconsistent.
        // With MSAA, lines have 1.4px thickness, which makes them too thick.
        // Without MSAA, lines have 1px thickness, but their placement is sometimes off.
        let emulate_lines = descriptors.backend == Backend::Dx12;

        Self {
            descriptors,
            quality,
            origin,
            width,
            height,
            nearest_layer,
            meshes,
            staging_belt,
            dynamic_transforms,
            draw_encoder,
            texture_pool,
            emulate_lines,

            result: vec![],
            current: vec![],
            current_bounds: ChunkBounds::Empty,
            transforms,
            needs_stencil: false,
            num_masks: 0,
        }
    }

    fn new_transforms(
        descriptors: &'a Descriptors,
        dynamic_transforms: &'a DynamicTransforms,
    ) -> BufferBuilder {
        let mut transforms = BufferBuilder::new_for_uniform(&descriptors.limits);
        transforms.set_buffer_limit(dynamic_transforms.buffer.size());
        transforms
    }

    /// Replaces every blend with a RenderBitmap, with the subcommands rendered out to a temporary texture
    /// Every complex blend will be its own item, but every other draw will be chunked together
    fn chunk_blends(&mut self, commands: CommandList) -> Vec<Chunk> {
        commands.execute(self);

        let current = mem::take(&mut self.current);
        let mut result = mem::take(&mut self.result);
        let needs_stencil = mem::take(&mut self.needs_stencil);
        let transforms = mem::replace(
            &mut self.transforms,
            Self::new_transforms(self.descriptors, self.dynamic_transforms),
        );

        if !current.is_empty() {
            result.push(Chunk::Draw {
                chunk: current,
                needs_stencil,
                transforms,
                bounds: self.current_bounds.take().to_regions(),
            });
        }

        reorder_blends_for_batching(&mut result);

        result
    }

    /// The unit square, which is the geometry every pipeline but `RenderShape` draws.
    ///
    /// Rects, lines and bitmaps all reach here with the size already folded into the matrix, so
    /// their extent is the matrix applied to this.
    const UNIT_QUAD: Rectangle<Twips> = Rectangle {
        x_min: Twips::ZERO,
        x_max: Twips::ONE_PX,
        y_min: Twips::ZERO,
        y_max: Twips::ONE_PX,
    };

    fn add_to_current(
        &mut self,
        matrix: Matrix,
        color_transform: ColorTransform,
        local_bounds: Option<Rectangle<Twips>>,
        command_builder: impl FnOnce(wgpu::DynamicOffset) -> DrawCommand,
    ) {
        self.add_to_current_with_bitmap_uv_scale(
            matrix,
            color_transform,
            [1.0, 1.0],
            local_bounds,
            command_builder,
        );
    }

    /// Queue a shape draw carrying `blend_mode` on the pipelines that draw it.
    fn draw_shape(&mut self, shape: ShapeHandle, transform: Transform, blend_mode: TrivialBlend) {
        // The one command whose extent is not implied by its matrix; `Mesh` keeps it for this.
        let local_bounds = Some(as_mesh(&shape).shape_bounds);
        self.add_to_current(
            transform.matrix,
            transform.color_transform,
            local_bounds,
            |transform_buffer| DrawCommand::RenderShape {
                shape,
                transform_buffer,
                blend_mode,
            },
        );
    }

    /// Queue a bitmap draw carrying `blend_mode` on the pipeline that draws it.
    ///
    /// Shared by an ordinary bitmap, which carries `Normal`, and by a trivial blend that has been
    /// collapsed into the single draw it wrapped, which carries its own. Both need the same
    /// geometry and the same UV scale, and having one of them work them out differently is the sort
    /// of difference that shows up as a bitmap drawn a pixel out.
    fn draw_bitmap(
        &mut self,
        bitmap: BitmapHandle,
        transform: Transform,
        smoothing: bool,
        pixel_snapping: PixelSnapping,
        source_size: Option<BitmapSize>,
        blend_mode: TrivialBlend,
    ) {
        let mut matrix = transform.matrix;
        let texture = as_texture(&bitmap);
        pixel_snapping.apply(&mut matrix);
        let (geometry_size, uv_scale) = bitmap_region_geometry_and_uv_scale(
            (texture.texture.width(), texture.texture.height()),
            source_size.map(|size| (size.width, size.height)),
        );
        matrix *= Matrix::scale(geometry_size.0, geometry_size.1);
        self.add_to_current_with_bitmap_uv_scale(
            matrix,
            transform.color_transform,
            uv_scale,
            Some(Self::UNIT_QUAD),
            |transform_buffer| DrawCommand::RenderBitmap {
                bitmap,
                transform_buffer,
                smoothing,
                blend_mode,
                render_stage3d: false,
            },
        );
    }

    fn add_to_current_with_bitmap_uv_scale(
        &mut self,
        matrix: Matrix,
        color_transform: ColorTransform,
        bitmap_uv_scale: [f32; 2],
        local_bounds: Option<Rectangle<Twips>>,
        command_builder: impl FnOnce(wgpu::DynamicOffset) -> DrawCommand,
    ) {
        let transform = Transforms {
            world_matrix: [
                [matrix.a, matrix.b, 0.0, 0.0],
                [matrix.c, matrix.d, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [
                    matrix.tx.to_pixels() as f32,
                    matrix.ty.to_pixels() as f32,
                    0.0,
                    1.0,
                ],
            ],
            mult_color: color_transform.mult_rgba_normalized(),
            add_color: color_transform.add_rgba_normalized(),
            bitmap_uv_scale: [bitmap_uv_scale[0], bitmap_uv_scale[1], 0.0, 0.0],
        };
        if let Ok(transform_range) = self.transforms.add(&[transform]) {
            self.current.push(command_builder(
                transform_range.start as wgpu::DynamicOffset,
            ));
        } else {
            // The chunk being closed here holds everything bounded so far; this command starts the
            // next one, which is why its own bounds are added below rather than above.
            let bounds = self.current_bounds.take().to_regions();
            self.result.push(Chunk::Draw {
                chunk: mem::take(&mut self.current),
                needs_stencil: self.needs_stencil,
                transforms: mem::replace(
                    &mut self.transforms,
                    BufferBuilder::new_for_uniform(&self.descriptors.limits),
                ),
                bounds,
            });
            self.transforms
                .set_buffer_limit(self.dynamic_transforms.buffer.size());
            let transform_range = self
                .transforms
                .add(&[transform])
                .expect("Buffer must be able to fit a new thing, it was just emptied");
            self.current.push(command_builder(
                transform_range.start as wgpu::DynamicOffset,
            ));
        }

        self.current_bounds
            .add(local_bounds.map(|bounds| matrix * bounds));
    }
}

/// Whether COMPLEX blends may shrink too, on top of `SHRINK_BLEND_TARGETS`.
///
/// Separate because the risk is different. A trivial blend is composited as a plain textured quad,
/// so a region is just a smaller quad. A complex blend samples the parent's blend buffer alongside
/// its own child texture, and the shaders derive one UV from clip position for both -- correct for
/// the parent, wrong for a child that no longer covers the whole target. `region_frame_bind_group`
/// supplies the remap. If an aura or a Multiply/Overlay/Alpha/Erase effect ever appears in the
/// wrong place or at the wrong scale, this is the switch to try first.
const SHRINK_COMPLEX_BLEND_TARGETS: bool = true;

/// Whether runs of compatible blends may share a render pass.
///
/// On by default. Set `AETHER_BLEND_BATCH=0` to composite one blend per pass as before, which is
/// the control for measuring what batching actually bought and the switch to reach for if blended
/// content ever looks wrong.
pub(crate) fn blend_batching_enabled() -> bool {
    crate::aether_switches::BLEND_BATCHING.enabled()
}

/// How far ahead a blend looks for company, in chunks.
///
/// The scan is linear per blend, so leaving it unbounded makes a frame that never batches
/// quadratic. A crowded AQW frame encodes ~500 chunks and wants batches of about four, so this is
/// far more reach than the measured case needs while keeping the worst case bounded.
const BLEND_REORDER_SCAN_LIMIT: usize = 64;

/// Whether blends may be moved next to each other before batching. Opt in with
/// `AETHER_BLEND_REORDER=1`.
///
/// **Off by default, on measurement.** It works and it is not worth it: in a crowded Yulgar it
/// relocated 11 blends a frame out of 296, worth about 1.6% of render passes, and describing draw
/// chunks with eight boxes instead of one changed nothing.
///
/// The reason is worth keeping, because the counter that motivated this reads as though there is
/// far more to gain. `blend_ideal_batches` first-fits blends while ignoring draw chunks entirely,
/// so it reports 58 passes where 283 are spent -- but a blend belongs to an avatar, and in a
/// crowded room the drawing between it and the next compatible blend is other avatars overlapping
/// it on screen. No correct reordering crosses that. The reachable headroom is roughly 283 to 270,
/// not 283 to 58. Do not re-plan this from `ideal`.
///
/// Kept behind a switch rather than deleted so the measurement can be repeated on other scenes,
/// where the geometry may be kinder than a packed hub.
pub(crate) fn blend_reordering_enabled() -> bool {
    crate::aether_switches::BLEND_REORDERING.enabled()
}

/// The facts a batch cares about, for a blend that is allowed to join one.
type BlendKey = (
    std::mem::Discriminant<ComplexBlend>,
    bool,
    Option<(u32, u32, u32, u32)>,
);

fn complex_blend_key(chunk: &Chunk) -> Option<BlendKey> {
    match chunk {
        Chunk::Blend {
            blend_mode: ChunkBlendMode::Complex(mode),
            needs_stencil,
            region,
            ..
            // Alpha and Erase resolve their parent through the nearest layer and may be skipped
            // outright, which is not something a reordering can reason about.
        } if !matches!(mode, ComplexBlend::Alpha | ComplexBlend::Erase) => {
            Some((std::mem::discriminant(mode), *needs_stencil, *region))
        }
        _ => None,
    }
}

/// Move compatible blends next to each other, where doing so cannot change a pixel.
///
/// The batcher in `surface.rs` can only merge blends that are *already* adjacent, and AQW almost
/// never produces those. Measured on a crowded map: of 114 blends in a frame, 97 were followed by
/// ordinary drawing and only 4 by an incompatible mode, while a batcher free to reorder would have
/// needed 29 passes instead of 114. The passes are not lost to blends being incompatible, they are
/// lost to blends being separated. This separates the fixing of that from the batching itself.
///
/// A blend may be moved earlier only when it overlaps nothing it jumps over -- neither the blends
/// already gathered nor the draws in between. Disjoint operations commute, so the result is the
/// order the display list asked for. A draw chunk whose extent is unknown (`bounds: None`) stops
/// the scan rather than being guessed at.
///
/// Stencil work is excluded on both sides. A blend's stencil reference comes from the mask state
/// the chunks before it left behind, so moving one past anything that touches the stencil would
/// composite it against a different mask. Stencilless blends ignore mask state entirely, which is
/// what makes them safe to move at all.
fn reorder_blends_for_batching(chunks: &mut [Chunk]) {
    if !blend_batching_enabled() || !blend_reordering_enabled() {
        return;
    }

    // Planned against a description of the chunks rather than the chunks themselves, so the rule
    // can be tested without a GPU to build real blend targets on.
    let mut plan: Vec<ReorderChunk<std::mem::Discriminant<ComplexBlend>>> = chunks
        .iter()
        .map(|chunk| match chunk {
            Chunk::Draw {
                needs_stencil: true,
                ..
            } => ReorderChunk::Barrier,
            Chunk::Draw { bounds, .. } => match bounds {
                Some(bounds) => ReorderChunk::Draw(*bounds),
                None => ReorderChunk::Barrier,
            },
            chunk => match complex_blend_key(chunk) {
                Some((mode, false, region)) => ReorderChunk::Blend { mode, region },
                _ => ReorderChunk::Barrier,
            },
        })
        .collect();

    let moves = plan_blend_reorder(&mut plan);
    #[cfg(feature = "aether_metrics")]
    {
        let at_capacity = chunks
            .iter()
            .filter(|chunk| {
                matches!(chunk, Chunk::Draw { bounds: Some(bounds), .. }
                    if bounds.is_at_capacity())
            })
            .count() as u64;
        crate::aether_metrics::record_blend_reorder(moves.len() as u64, at_capacity);
    }
    for (insert_at, scan) in moves {
        chunks[insert_at..=scan].rotate_right(1);
    }
}

/// What a chunk looks like to the reorderer.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ReorderChunk<M> {
    /// A stencilless draw covering a known set of boxes.
    Draw(DrawRegions),
    /// Something the scan must stop at: stencil work, an unknown extent, a shader blend, or an
    /// Alpha/Erase whose parent is resolved elsewhere.
    Barrier,
    /// A stencilless complex blend that may join a batch.
    Blend {
        mode: M,
        region: Option<(u32, u32, u32, u32)>,
    },
}

/// Plan the moves that bring compatible blends together, and apply them to `chunks`.
///
/// Returns each move as the `(insert_at, scan)` range that was rotated right by one, which is
/// exactly what the caller applies to the real chunk list to match.
fn plan_blend_reorder<M: Copy + PartialEq>(chunks: &mut [ReorderChunk<M>]) -> Vec<(usize, usize)> {
    let mut moves = Vec::new();
    let mut index = 0;
    while index < chunks.len() {
        let ReorderChunk::Blend { mode, region } = chunks[index] else {
            index += 1;
            continue;
        };

        // Regions already in the batch, and regions of everything being jumped over. A candidate
        // has to clear both: the first so the batch can share one snapshot of the parent, the
        // second so moving it earlier cannot be observed.
        let mut batch = vec![region];
        let mut passed: Vec<Option<(u32, u32, u32, u32)>> = Vec::new();
        let mut passed_draws: Vec<DrawRegions> = Vec::new();
        let mut insert_at = index + 1;
        let mut scan = index + 1;
        let limit = (index + 1 + BLEND_REORDER_SCAN_LIMIT).min(chunks.len());

        while scan < limit {
            match chunks[scan] {
                ReorderChunk::Barrier => break,
                ReorderChunk::Draw(bounds) => {
                    passed_draws.push(bounds);
                    scan += 1;
                }
                ReorderChunk::Blend {
                    mode: next_mode,
                    region: next_region,
                } => {
                    let joins = next_mode == mode
                        && !batch
                            .iter()
                            .chain(passed.iter())
                            .any(|held| blend_regions_overlap(*held, next_region))
                        && !passed_draws.iter().any(|drawn| drawn.overlaps(next_region));

                    if joins {
                        // Rotating the span right by one lifts the blend to `insert_at` and pushes
                        // everything it jumped over one place later, so the next unexamined chunk
                        // is still at `scan + 1`.
                        chunks[insert_at..=scan].rotate_right(1);
                        moves.push((insert_at, scan));
                        batch.push(next_region);
                        insert_at += 1;
                    } else {
                        passed.push(next_region);
                    }
                    scan += 1;
                }
            }
        }

        index = insert_at;
    }

    moves
}

/// Whether two blend sub-targets cover any of the same pixels.
///
/// This decides whether a run of blends can share one composite pass. Each complex blend reads the
/// parent back through a snapshot and then writes over it, so a later blend that overlaps an earlier
/// one has to see what the earlier one wrote. Disjoint blends do not, which is what makes them
/// batchable -- and getting this wrong composites against a stale backdrop, silently.
///
/// `None` means the child covers the whole surface, so it overlaps everything including another
/// `None`.
pub(crate) fn blend_regions_overlap(
    a: Option<(u32, u32, u32, u32)>,
    b: Option<(u32, u32, u32, u32)>,
) -> bool {
    let (Some((ax, ay, aw, ah)), Some((bx, by, bw, bh))) = (a, b) else {
        return true;
    };
    // Touching edges do not overlap: a region owns [x, x + width).
    ax < bx.saturating_add(bw)
        && bx < ax.saturating_add(aw)
        && ay < by.saturating_add(bh)
        && by < ay.saturating_add(ah)
}

/// Whether blends and alpha masks render into targets sized to their contents rather than to the
/// whole surface.
///
/// ON, on measurement. It was off for one release because the pool's cumulative counters seemed to
/// show only ~1 stage-sized allocation per frame -- but that reading came from a twelve-second run
/// that never left the login screen. A census of seventy seconds of real gameplay says otherwise:
///
/// ```text
/// texture census: 98171 allocations, 1496.5 GB created, 512 distinct sizes
///   pool  2560x1365  x1    83094 allocs  1161454.7 MB
///   pool  2560x1365  x4     7899 allocs   331227.2 MB
/// ```
///
/// About forty stage-sized targets per frame, 1.16 TB of texture creation. They cannot reuse one
/// another either: `chunk_blends` collects every `Chunk` for the frame before executing any of
/// them, and `Chunk::Blend` and `DrawCommand::RenderAlphaMask` hold their textures, so a frame's
/// targets are all alive at once. Alpha masks take two each and were the larger half.
const SHRINK_BLEND_TARGETS: bool = true;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BlendTargetKind {
    Trivial,
    Complex,
    Shader,
}

fn blend_target_may_shrink(kind: BlendTargetKind) -> bool {
    match kind {
        BlendTargetKind::Trivial => SHRINK_BLEND_TARGETS,
        BlendTargetKind::Complex => SHRINK_BLEND_TARGETS && SHRINK_COMPLEX_BLEND_TARGETS,
        // PixelBender receives no region transform when the blend executes. Its foreground and
        // background inputs therefore have to retain the same full-surface coordinate space.
        BlendTargetKind::Shader => false,
    }
}

/// The single bitmap draw a command list consists of, if that is all it is.
///
/// Measured over roughly two million blends in a session: 99% wrap exactly one draw, and 38.6% of
/// those wrap a bitmap. A blend around one draw is a target, a render pass and a composite spent to
/// arrive at the same pixels the draw would have produced on its own.
fn sole_bitmap_draw(commands: &CommandList) -> Option<&Command> {
    match &commands.commands[..] {
        [only @ Command::RenderBitmap { .. }] => Some(only),
        _ => None,
    }
}

/// The single shape draw a command list consists of, if that is all it is and if that shape is a
/// single mesh draw.
///
/// The second half is not a detail. One `RenderShape` can be several fills, and a blend applied to
/// each of them separately is not the blend applied to the group: where two fills overlap, drawing
/// both with Add adds twice, while the long way round composes them in the target first and adds
/// once. So this only answers for a mesh with exactly one drawable piece, which is what a weapon
/// highlight or a glow layer is.
fn sole_shape_draw(commands: &CommandList) -> Option<&Command> {
    let [only @ Command::RenderShape { shape, .. }] = &commands.commands[..] else {
        return None;
    };
    let drawable = as_mesh(shape)
        .draws
        .iter()
        .filter(|draw| draw.num_indices > 0)
        .count();
    (drawable == 1).then_some(only)
}

/// Whether a blend around a single draw can be replaced by that draw carrying the blend itself.
///
/// Only the trivial blends, and that is the whole of the argument. A trivial blend is a blend state
/// on the pipeline: drawing the bitmap with it set produces `blend(bitmap, destination)`, and the
/// long way round produces `blend(target, destination)` where the target holds nothing but that
/// same bitmap over transparent. The two are the same expression.
///
/// A complex blend cannot do this. It reads the destination in a shader and needs the source
/// gathered somewhere first, which is what the target is for.
///
/// Masks are unaffected either way. The long way round applies the mask to the composite rather
/// than to the draw inside the target, and with one draw in the target those are the same pixels.
fn blend_can_be_carried_by_its_draw(blend_type: &BlendType) -> bool {
    matches!(blend_type, BlendType::Trivial(_))
}

impl CommandHandler for WgpuCommandHandler<'_> {
    fn blend(
        &mut self,
        commands: CommandList,
        blend_mode: RenderBlendMode,
        bounds: Option<Rectangle<Twips>>,
    ) {
        let target_layer = if let RenderBlendMode::Builtin(BlendMode::Layer) = &blend_mode {
            LayerRef::Current
        } else {
            self.nearest_layer
        };
        let blend_type = BlendType::from(blend_mode);

        // Where this blend's sub-target actually has to be. Complex blends carry an explicit region
        // transform when composited. PixelBender blends do not, so they retain the full surface.
        let may_shrink = blend_target_may_shrink(match &blend_type {
            BlendType::Trivial(_) => BlendTargetKind::Trivial,
            BlendType::Complex(_) => BlendTargetKind::Complex,
            BlendType::Shader(_) => BlendTargetKind::Shader,
        });
        let region = match (may_shrink, bounds.filter(|_| may_shrink)) {
            (true, Some(bounds)) => {
                // Core has already grown these bounds for the object's filters, so the region is
                // clamped to the surface and nothing else.
                match plan_blend_region(
                    bounds,
                    &[],
                    (1.0, 1.0),
                    self.origin,
                    self.width,
                    self.height,
                ) {
                    Some(region) => quantise_region(
                        region,
                        self.origin,
                        self.width,
                        self.height,
                        blend_region_grid(region.width, region.height),
                    ),
                    // None of it lands on the surface, so none of it can be seen. Drawing it would
                    // cost a target, a render pass and a composite to produce nothing.
                    None => return,
                }
            }
            _ => BlendRegion::at(self.origin, self.width, self.height),
        };

        // Hand the blend to the draw and skip the target entirely, where that is the same thing.
        if let BlendType::Trivial(trivial) = blend_type
            && blend_can_be_carried_by_its_draw(&blend_type)
            && let Some(Command::RenderBitmap {
                bitmap,
                transform,
                smoothing,
                pixel_snapping,
                source_size,
            }) = sole_bitmap_draw(&commands)
        {
            self.draw_bitmap(
                bitmap.clone(),
                transform.clone(),
                *smoothing,
                *pixel_snapping,
                *source_size,
                trivial,
            );
            return;
        }

        if let BlendType::Trivial(trivial) = blend_type
            && blend_can_be_carried_by_its_draw(&blend_type)
            && let Some(Command::RenderShape { shape, transform }) = sole_shape_draw(&commands)
        {
            self.draw_shape(shape.clone(), transform.clone(), trivial);
            return;
        }

        let surface = Surface::new(
            self.descriptors,
            self.quality,
            region.width,
            region.height,
            wgpu::TextureFormat::Rgba8Unorm,
        );
        let clear_color = blend_type.default_color();
        let target = surface.draw_commands_at(
            (region.x, region.y),
            RenderTargetMode::FreshWithColor(clear_color),
            self.descriptors,
            self.meshes,
            commands,
            self.staging_belt,
            self.dynamic_transforms,
            self.draw_encoder,
            target_layer,
            self.texture_pool,
        );
        target.ensure_cleared(self.draw_encoder);

        // We currently do not support shader blends in masks. In order not to
        // break other parts of the scene, we just fall back to a normal blend.
        //
        // TODO Add support for shader blends in masks.
        let is_shader_blend_in_mask =
            self.num_masks > 0 && matches!(blend_type, BlendType::Shader(_));
        let blend_type = if is_shader_blend_in_mask {
            BlendType::Trivial(TrivialBlend::Normal)
        } else {
            blend_type
        };

        match blend_type {
            BlendType::Trivial(blend_mode) => {
                // The sub-target is a unit quad scaled to its own size and moved to where it sits
                // on the surface. With a full-surface region the translation is zero and this is
                // the matrix it always was.
                let transform = Transform {
                    matrix: Matrix::translate(
                        Twips::from_pixels(region.x as f64),
                        Twips::from_pixels(region.y as f64),
                    ) * Matrix::scale(region.width as f32, region.height as f32),
                    color_transform: Default::default(),
                    perspective_projection: None,
                };
                let texture = target.take_color_texture();
                let bind_group =
                    self.descriptors
                        .device
                        .create_bind_group(&wgpu::BindGroupDescriptor {
                            layout: &self.descriptors.bind_layouts.bitmap,
                            entries: &[
                                wgpu::BindGroupEntry {
                                    binding: 0,
                                    resource: self
                                        .descriptors
                                        .quad
                                        .texture_transforms
                                        .as_entire_binding(),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 1,
                                    resource: wgpu::BindingResource::TextureView(texture.view()),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 2,
                                    resource: wgpu::BindingResource::Sampler(
                                        self.descriptors.bitmap_samplers.get_sampler(false, false),
                                    ),
                                },
                            ],
                            label: None,
                        });
                self.add_to_current(
                    transform.matrix,
                    transform.color_transform,
                    None,
                    |transform_buffer| DrawCommand::RenderTexture {
                        _texture: texture,
                        binds: bind_group,
                        transform_buffer,
                        blend_mode,
                    },
                );
            }
            blend_type => {
                if !self.current.is_empty() {
                    let bounds = self.current_bounds.take().to_regions();
                    self.result.push(Chunk::Draw {
                        chunk: mem::take(&mut self.current),
                        needs_stencil: self.needs_stencil,
                        transforms: mem::replace(
                            &mut self.transforms,
                            BufferBuilder::new_for_uniform(&self.descriptors.limits),
                        ),
                        bounds,
                    });
                }
                self.transforms
                    .set_buffer_limit(self.dynamic_transforms.buffer.size());
                let chunk_blend_mode = match blend_type {
                    BlendType::Complex(complex) => ChunkBlendMode::Complex(complex),
                    BlendType::Shader(shader) => ChunkBlendMode::Shader(shader),
                    _ => unreachable!(),
                };
                self.result.push(Chunk::Blend {
                    texture: target.take_color_texture(),
                    blend_mode: chunk_blend_mode,
                    needs_stencil: self.num_masks > 0,
                    region: (!region.is_full(self.origin, self.width, self.height)).then_some((
                        region.x,
                        region.y,
                        region.width,
                        region.height,
                    )),
                });
                self.needs_stencil = self.num_masks > 0;
            }
        }
    }

    fn render_bitmap(
        &mut self,
        bitmap: BitmapHandle,
        transform: Transform,
        smoothing: bool,
        pixel_snapping: PixelSnapping,
        source_size: Option<BitmapSize>,
    ) {
        self.draw_bitmap(
            bitmap,
            transform,
            smoothing,
            pixel_snapping,
            source_size,
            TrivialBlend::Normal,
        );
    }
    fn render_stage3d(&mut self, bitmap: BitmapHandle, transform: Transform) {
        let mut matrix = transform.matrix;
        {
            let texture = as_texture(&bitmap);
            matrix *= Matrix::scale(
                texture.texture.width() as f32,
                texture.texture.height() as f32,
            );
        }
        self.add_to_current(
            matrix,
            transform.color_transform,
            Some(Self::UNIT_QUAD),
            |transform_buffer| DrawCommand::RenderBitmap {
                bitmap,
                transform_buffer,
                smoothing: false,
                blend_mode: TrivialBlend::Normal,
                render_stage3d: true,
            },
        );
    }

    fn render_shape(&mut self, shape: ShapeHandle, transform: Transform) {
        self.draw_shape(shape, transform, TrivialBlend::Normal);
    }

    fn draw_rect(&mut self, color: Color, matrix: Matrix) {
        self.add_to_current(
            matrix,
            ColorTransform::multiply_from(color),
            Some(Self::UNIT_QUAD),
            |transform_buffer| DrawCommand::DrawRect { transform_buffer },
        );
    }

    fn draw_line(&mut self, color: Color, mut matrix: Matrix) {
        if self.emulate_lines {
            let mut cl = CommandList::new();
            emulate_line(&mut cl, color, matrix);
            cl.execute(self);
        } else {
            matrix.tx += Twips::HALF_PX;
            matrix.ty += Twips::HALF_PX;
            self.add_to_current(
                matrix,
                ColorTransform::multiply_from(color),
                None,
                |transform_buffer| DrawCommand::DrawLine { transform_buffer },
            );
        }
    }

    fn draw_line_rect(&mut self, color: Color, mut matrix: Matrix) {
        if self.emulate_lines {
            let mut cl = CommandList::new();
            emulate_line_rect(&mut cl, color, matrix);
            cl.execute(self);
        } else {
            matrix.tx += Twips::HALF_PX;
            matrix.ty += Twips::HALF_PX;
            self.add_to_current(
                matrix,
                ColorTransform::multiply_from(color),
                None,
                |transform_buffer| DrawCommand::DrawLineRect { transform_buffer },
            );
        }
    }

    fn push_mask(&mut self) {
        self.needs_stencil = true;
        self.num_masks += 1;
        self.current.push(DrawCommand::PushMask);
    }

    fn activate_mask(&mut self) {
        self.needs_stencil = true;
        self.current.push(DrawCommand::ActivateMask);
    }

    fn deactivate_mask(&mut self) {
        self.needs_stencil = true;
        self.current.push(DrawCommand::DeactivateMask);
    }

    fn pop_mask(&mut self) {
        self.needs_stencil = true;
        self.num_masks -= 1;
        self.current.push(DrawCommand::PopMask);
    }

    fn render_alpha_mask(
        &mut self,
        maskee_commands: CommandList,
        mask_commands: CommandList,
        bounds: Option<Rectangle<Twips>>,
    ) {
        // Two stage-sized targets per alpha mask was the single biggest source of texture churn in
        // the renderer: a census of real gameplay recorded 83,094 allocations of the 2560x1365
        // surface in seventy seconds, about forty per frame, because masks and blends each take
        // their own and none of them can be reused while the frame's chunks are still being built.
        // Sizing them to the maskee cuts that by the ratio of object area to screen area.
        //
        // Both targets must share ONE region: the shader samples maskee and mask as an aligned
        // pair, so they have to be the same size and cover the same part of the surface.
        let region = match bounds.filter(|_| SHRINK_BLEND_TARGETS) {
            Some(bounds) => match plan_blend_region(
                bounds,
                &[],
                (1.0, 1.0),
                self.origin,
                self.width,
                self.height,
            ) {
                Some(region) => quantise_region(
                    region,
                    self.origin,
                    self.width,
                    self.height,
                    blend_region_grid(region.width, region.height),
                ),
                // Nothing of the maskee lands on the surface, so nothing of the masked result can.
                None => return,
            },
            None => BlendRegion::at(self.origin, self.width, self.height),
        };

        let surface = Surface::new(
            self.descriptors,
            self.quality,
            region.width,
            region.height,
            wgpu::TextureFormat::Rgba8Unorm,
        );

        let maskee = surface.draw_commands_at(
            (region.x, region.y),
            RenderTargetMode::FreshWithColor(wgpu::Color::TRANSPARENT),
            self.descriptors,
            self.meshes,
            maskee_commands,
            self.staging_belt,
            self.dynamic_transforms,
            self.draw_encoder,
            LayerRef::None,
            self.texture_pool,
        );
        maskee.ensure_cleared(self.draw_encoder);
        let matrix = Matrix::translate(
            Twips::from_pixels(region.x as f64),
            Twips::from_pixels(region.y as f64),
        ) * Matrix::scale(region.width as f32, region.height as f32);
        let maskee = maskee.take_color_texture();

        let mask = surface.draw_commands_at(
            (region.x, region.y),
            RenderTargetMode::FreshWithColor(wgpu::Color::TRANSPARENT),
            self.descriptors,
            self.meshes,
            mask_commands,
            self.staging_belt,
            self.dynamic_transforms,
            self.draw_encoder,
            LayerRef::None,
            self.texture_pool,
        );
        mask.ensure_cleared(self.draw_encoder);
        let mask = mask.take_color_texture();

        let binds = self
            .descriptors
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &self.descriptors.bind_layouts.alpha_mask,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(maskee.view()),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(mask.view()),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(
                            self.descriptors.bitmap_samplers.get_sampler(false, false),
                        ),
                    },
                ],
                label: None,
            });

        self.add_to_current(matrix, Default::default(), None, |transform_buffer| {
            DrawCommand::RenderAlphaMask {
                maskee,
                mask,
                binds,
                transform_buffer,
            }
        });
    }
}

#[cfg(test)]
mod blend_target_policy_tests {
    use super::{BlendTargetKind, blend_target_may_shrink};

    #[test]
    fn pixel_bender_blends_keep_full_surface_coordinates() {
        assert!(!blend_target_may_shrink(BlendTargetKind::Shader));
    }

    #[test]
    fn ordinary_blends_retain_region_sizing() {
        assert!(blend_target_may_shrink(BlendTargetKind::Trivial));
        assert!(blend_target_may_shrink(BlendTargetKind::Complex));
    }
}

fn bitmap_region_geometry_and_uv_scale(
    texture_size: (u32, u32),
    source_size: Option<(u32, u32)>,
) -> ((f32, f32), [f32; 2]) {
    let source_size = source_size.unwrap_or(texture_size);
    let width = source_size.0.min(texture_size.0);
    let height = source_size.1.min(texture_size.1);
    let uv_x = if texture_size.0 == 0 {
        0.0
    } else {
        width as f32 / texture_size.0 as f32
    };
    let uv_y = if texture_size.1 == 0 {
        0.0
    } else {
        height as f32 / texture_size.1 as f32
    };
    ((width as f32, height as f32), [uv_x, uv_y])
}

#[cfg(test)]
mod blend_batch_tests {
    use super::blend_regions_overlap;

    #[test]
    fn separated_regions_do_not_overlap() {
        assert!(!blend_regions_overlap(
            Some((0, 0, 100, 100)),
            Some((200, 200, 50, 50))
        ));
    }

    /// A region owns [x, x + width), so meeting edges are not an overlap. Treating them as one
    /// would break up runs of avatars standing side by side, which is the common case.
    #[test]
    fn regions_that_only_touch_do_not_overlap() {
        assert!(!blend_regions_overlap(
            Some((0, 0, 100, 100)),
            Some((100, 0, 100, 100))
        ));
        assert!(!blend_regions_overlap(
            Some((0, 0, 100, 100)),
            Some((0, 100, 100, 100))
        ));
    }

    #[test]
    fn a_single_shared_pixel_is_an_overlap() {
        assert!(blend_regions_overlap(
            Some((0, 0, 100, 100)),
            Some((99, 99, 50, 50))
        ));
    }

    #[test]
    fn containment_counts_either_way_round() {
        assert!(blend_regions_overlap(
            Some((0, 0, 200, 200)),
            Some((50, 50, 10, 10))
        ));
        assert!(blend_regions_overlap(
            Some((50, 50, 10, 10)),
            Some((0, 0, 200, 200))
        ));
    }

    /// A full-surface child covers every pixel, so it can never share a pass with anything.
    #[test]
    fn a_full_surface_child_overlaps_everything() {
        assert!(blend_regions_overlap(None, None));
        assert!(blend_regions_overlap(None, Some((10, 10, 5, 5))));
        assert!(blend_regions_overlap(Some((10, 10, 5, 5)), None));
    }

    /// Overlap has to be symmetric or a batch's validity would depend on iteration order.
    #[test]
    fn overlap_is_symmetric() {
        let cases = [
            (Some((0, 0, 10, 10)), Some((5, 5, 10, 10))),
            (Some((0, 0, 10, 10)), Some((20, 0, 10, 10))),
            (Some((0, 0, 10, 10)), None),
        ];
        for (a, b) in cases {
            assert_eq!(blend_regions_overlap(a, b), blend_regions_overlap(b, a));
        }
    }
}

#[cfg(test)]
mod bitmap_region_tests {
    use super::bitmap_region_geometry_and_uv_scale;

    #[test]
    fn oversized_cache_draw_uses_only_its_logical_region() {
        assert_eq!(
            bitmap_region_geometry_and_uv_scale((637, 584), Some((463, 498))),
            ((463.0, 498.0), [463.0 / 637.0, 498.0 / 584.0])
        );
    }

    #[test]
    fn normal_bitmap_draw_uses_the_full_texture() {
        assert_eq!(
            bitmap_region_geometry_and_uv_scale((637, 584), None),
            ((637.0, 584.0), [1.0, 1.0])
        );
    }

    #[test]
    fn logical_region_is_clamped_to_texture_capacity() {
        assert_eq!(
            bitmap_region_geometry_and_uv_scale((637, 584), Some((900, 498))),
            ((637.0, 498.0), [1.0, 498.0 / 584.0])
        );
    }
}

#[cfg(test)]
mod blend_bypass_tests {
    use super::{
        DrawRegions, ReorderChunk, blend_can_be_carried_by_its_draw, plan_blend_reorder,
        sole_bitmap_draw,
    };

    /// A blend of `mode` covering a 10x10 box at `x`, which is disjoint from any other `x` here.
    fn blend(mode: u8, x: u32) -> ReorderChunk<u8> {
        ReorderChunk::Blend {
            mode,
            region: Some((x * 100, 0, 10, 10)),
        }
    }

    fn draw_at(x: u32) -> ReorderChunk<u8> {
        let mut regions = DrawRegions::default();
        regions.push((x * 100, 0, 10, 10));
        ReorderChunk::Draw(regions)
    }

    /// Two draws far apart, described separately rather than as the span between them.
    fn draw_at_both(a: u32, b: u32) -> ReorderChunk<u8> {
        let mut regions = DrawRegions::default();
        regions.push((a * 100, 0, 10, 10));
        regions.push((b * 100, 0, 10, 10));
        ReorderChunk::Draw(regions)
    }

    /// The case the whole thing exists for: blends separated by drawing they do not touch.
    ///
    /// Measured on a crowded AQW map, 97 of 114 blends were in exactly this position, which is why
    /// the run-length batcher downstream never fired.
    #[test]
    fn blends_separated_by_unrelated_drawing_are_brought_together() {
        let mut chunks = vec![
            blend(1, 0),
            draw_at(5),
            blend(1, 1),
            draw_at(6),
            blend(1, 2),
        ];
        plan_blend_reorder(&mut chunks);
        assert_eq!(
            chunks,
            vec![
                blend(1, 0),
                blend(1, 1),
                blend(1, 2),
                draw_at(5),
                draw_at(6)
            ],
            "disjoint blends must gather so the batcher downstream can share one pass"
        );
    }

    /// The safety property. A blend that would land on top of drawing it used to follow has to
    /// stay put, because moving it there composites it against a backdrop that has not been drawn.
    #[test]
    fn a_blend_never_moves_past_drawing_it_overlaps() {
        let mut chunks = vec![blend(1, 0), draw_at(1), blend(1, 1)];
        let before = chunks.clone();
        plan_blend_reorder(&mut chunks);
        assert_eq!(
            chunks, before,
            "the second blend overlaps the draw between them and must not be reordered"
        );
    }

    /// Two blends that cover the same pixels have to composite in the order they were issued,
    /// since the later one reads what the earlier one wrote.
    #[test]
    fn overlapping_blends_are_left_in_their_original_order() {
        let mut chunks = vec![blend(1, 0), draw_at(9), blend(1, 0)];
        let before = chunks.clone();
        plan_blend_reorder(&mut chunks);
        assert_eq!(chunks, before);
    }

    /// Modes cannot mix in one pass: each carries its own pipeline.
    #[test]
    fn a_blend_of_another_mode_is_passed_over_rather_than_gathered() {
        let mut chunks = vec![blend(1, 0), blend(2, 1), blend(1, 2)];
        plan_blend_reorder(&mut chunks);
        assert_eq!(
            chunks,
            vec![blend(1, 0), blend(1, 2), blend(2, 1)],
            "the mismatched mode should be stepped over, not treated as the end of the run"
        );
    }

    /// Stencil work, unknown extents and shader blends all arrive as a barrier, and nothing may
    /// cross one -- a blend's stencil reference depends on the mask state left behind before it.
    #[test]
    fn nothing_is_reordered_across_a_barrier() {
        let mut chunks = vec![blend(1, 0), ReorderChunk::Barrier, blend(1, 1)];
        let before = chunks.clone();
        plan_blend_reorder(&mut chunks);
        assert_eq!(chunks, before);
    }

    /// A draw with an unknown extent reaches the planner as a barrier, so it is equally opaque.
    /// The reason a chunk keeps several boxes instead of one union. A chunk that draws at the far
    /// left and the far right does not draw in the middle, and a blend sitting in the gap has to be
    /// allowed to cross it -- with a single union it never could, which is what left 246 blend
    /// passes standing against an ideal of 63.
    #[test]
    fn a_blend_may_cross_a_draw_chunk_through_a_gap_between_its_boxes() {
        let mut chunks = vec![blend(1, 0), draw_at_both(8, 9), blend(1, 4)];
        plan_blend_reorder(&mut chunks);
        assert_eq!(
            chunks,
            vec![blend(1, 0), blend(1, 4), draw_at_both(8, 9)],
            "a blend in the gap between a chunk's boxes touches none of its drawing"
        );
    }

    /// The same chunk still blocks a blend that lands on one of its boxes.
    #[test]
    fn a_blend_landing_on_one_box_of_a_chunk_still_cannot_cross_it() {
        let mut chunks = vec![blend(1, 0), draw_at_both(8, 9), blend(1, 9)];
        let before = chunks.clone();
        plan_blend_reorder(&mut chunks);
        assert_eq!(chunks, before);
    }

    #[test]
    fn an_unbounded_draw_stops_the_scan() {
        let mut chunks = vec![blend(1, 0), ReorderChunk::Barrier, blend(1, 1)];
        plan_blend_reorder(&mut chunks);
        assert_eq!(
            chunks[0],
            blend(1, 0),
            "a draw whose extent is unknown covers everything, so nothing may cross it"
        );
        assert_eq!(chunks[2], blend(1, 1));
    }

    /// The moves handed back have to describe the same permutation the planner applied, since the
    /// caller replays them against the real chunk list and the two must not drift.
    #[test]
    fn the_reported_moves_reproduce_the_planned_order() {
        let mut planned = vec![
            blend(1, 0),
            draw_at(5),
            blend(1, 1),
            draw_at(6),
            blend(1, 2),
        ];
        let mut replayed = planned.clone();
        for (insert_at, scan) in plan_blend_reorder(&mut planned) {
            replayed[insert_at..=scan].rotate_right(1);
        }
        assert_eq!(planned, replayed);
    }

    /// Every chunk must survive a reorder: this moves work, it never drops or duplicates it.
    #[test]
    fn reordering_preserves_every_chunk() {
        let mut chunks = vec![
            blend(1, 0),
            draw_at(5),
            blend(2, 1),
            blend(1, 1),
            draw_at(6),
            blend(1, 2),
            ReorderChunk::Barrier,
            blend(1, 3),
        ];
        let mut before = chunks.clone();
        plan_blend_reorder(&mut chunks);
        before.sort_by_key(|c| format!("{c:?}"));
        let mut after = chunks.clone();
        after.sort_by_key(|c| format!("{c:?}"));
        assert_eq!(before, after);
    }
    use crate::blend::{BlendType, ComplexBlend, TrivialBlend};
    use ruffle_render::bitmap::{BitmapHandle, BitmapHandleImpl, PixelSnapping};
    use ruffle_render::commands::{Command, CommandList};
    use ruffle_render::matrix::Matrix;
    use ruffle_render::transform::Transform;
    use std::sync::Arc;
    use swf::Color;

    #[derive(Debug)]
    struct NotARealTexture;
    impl BitmapHandleImpl for NotARealTexture {}

    fn bitmap_draw() -> Command {
        Command::RenderBitmap {
            bitmap: BitmapHandle(Arc::new(NotARealTexture)),
            transform: Transform::default(),
            smoothing: false,
            pixel_snapping: PixelSnapping::Never,
            source_size: None,
        }
    }

    fn rect_draw() -> Command {
        Command::DrawRect {
            color: Color::WHITE,
            matrix: Matrix::IDENTITY,
        }
    }

    fn list(commands: Vec<Command>) -> CommandList {
        let mut list = CommandList::new();
        list.commands = commands;
        list
    }

    #[test]
    fn one_bitmap_and_nothing_else_is_a_sole_bitmap_draw() {
        assert!(sole_bitmap_draw(&list(vec![bitmap_draw()])).is_some());
    }

    /// The saving comes from not needing a target to gather several draws into, so anything with a
    /// second draw in it still needs the target. A rectangle behind the bitmap is the ordinary way
    /// that happens: an opaque background drawn under a cached object.
    #[test]
    fn anything_beside_the_bitmap_is_not() {
        assert!(sole_bitmap_draw(&list(vec![])).is_none());
        assert!(sole_bitmap_draw(&list(vec![rect_draw()])).is_none());
        assert!(sole_bitmap_draw(&list(vec![rect_draw(), bitmap_draw()])).is_none());
        assert!(sole_bitmap_draw(&list(vec![bitmap_draw(), bitmap_draw()])).is_none());
        assert!(sole_bitmap_draw(&list(vec![Command::PushMask])).is_none());
    }

    /// A trivial blend is a blend state on the pipeline, so the draw can carry it.
    #[test]
    fn a_trivial_blend_can_be_carried_by_the_draw() {
        for trivial in [
            TrivialBlend::Normal,
            TrivialBlend::Add,
            TrivialBlend::Subtract,
            TrivialBlend::Screen,
        ] {
            assert!(blend_can_be_carried_by_its_draw(&BlendType::Trivial(
                trivial
            )));
        }
    }

    /// A complex blend reads the destination in a shader and needs its source gathered somewhere
    /// first, which is exactly what the target it is being asked to skip is for.
    #[test]
    fn a_complex_blend_cannot_be() {
        for complex in [
            ComplexBlend::Multiply,
            ComplexBlend::Lighten,
            ComplexBlend::Darken,
            ComplexBlend::Difference,
            ComplexBlend::Invert,
            ComplexBlend::Alpha,
            ComplexBlend::Erase,
            ComplexBlend::Overlay,
            ComplexBlend::HardLight,
        ] {
            assert!(!blend_can_be_carried_by_its_draw(&BlendType::Complex(
                complex
            )));
        }
    }
}
