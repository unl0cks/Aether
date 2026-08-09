use crate::backend::WgpuRenderBackend;
use crate::target::RenderTarget;
use crate::{
    Descriptors, GradientUniforms, PosColorVertex, PosVertex, TextureTransforms, as_texture,
};
use std::any::Any;
use std::collections::HashMap;
use std::ops::Range;

use crate::buffer_builder::BufferBuilder;
use ruffle_render::backend::{RenderBackend, ShapeHandle, ShapeHandleImpl};
use ruffle_render::bitmap::BitmapSource;
use ruffle_render::tessellator::{Bitmap, Draw as LyonDraw, DrawType as TessDrawType, Gradient};
use swf::{CharacterId, GradientInterpolation};

/// How big to make gradient textures. Larger will keep more detail, but be slower and use more memory.
const GRADIENT_SIZE: usize = 256;
const GRADIENT_ATLAS_ROWS: u32 = 4_096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct GradientAtlasLocation {
    page: usize,
    row: u32,
}

#[derive(Debug)]
struct GradientAtlasPage {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

#[derive(Debug, Default)]
pub(crate) struct GradientAtlas {
    layout: GradientAtlasLayout,
    pages: Vec<GradientAtlasPage>,
}

impl GradientAtlas {
    fn allocate(
        &mut self,
        descriptors: &Descriptors,
        pixels: [u8; GRADIENT_SIZE * 4],
    ) -> (GradientAtlasLocation, wgpu::TextureView) {
        let (location, is_new) = self.layout.allocate_with_status(pixels);

        while self.pages.len() <= location.page {
            let texture = descriptors.device.create_texture(&wgpu::TextureDescriptor {
                label: create_debug_label!("Gradient atlas page {}", self.pages.len()).as_deref(),
                size: wgpu::Extent3d {
                    width: GRADIENT_SIZE as u32,
                    height: GRADIENT_ATLAS_ROWS,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            #[cfg(feature = "aether_metrics")]
            crate::aether_metrics::record_texture_created(
                crate::aether_metrics::TextureOrigin::GradientAtlas,
                GRADIENT_SIZE as u32,
                GRADIENT_ATLAS_ROWS,
                1,
                GRADIENT_SIZE as u64 * GRADIENT_ATLAS_ROWS as u64 * 4,
            );
            let view = texture.create_view(&Default::default());
            self.pages.push(GradientAtlasPage { texture, view });
        }

        let page = &self.pages[location.page];
        if is_new {
            descriptors.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &page.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: location.row,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(GRADIENT_SIZE as u32 * 4),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: GRADIENT_SIZE as u32,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
        }

        (location, page.view.clone())
    }
}

#[derive(Debug, Default)]
struct GradientAtlasLayout {
    locations: HashMap<[u8; GRADIENT_SIZE * 4], GradientAtlasLocation>,
}

impl GradientAtlasLayout {
    #[cfg(test)]
    fn allocate(&mut self, pixels: [u8; GRADIENT_SIZE * 4]) -> GradientAtlasLocation {
        self.allocate_with_status(pixels).0
    }

    fn allocate_with_status(
        &mut self,
        pixels: [u8; GRADIENT_SIZE * 4],
    ) -> (GradientAtlasLocation, bool) {
        if let Some(location) = self.locations.get(&pixels) {
            return (*location, false);
        }

        let index = self.locations.len();
        let location = GradientAtlasLocation {
            page: index / GRADIENT_ATLAS_ROWS as usize,
            row: (index % GRADIENT_ATLAS_ROWS as usize) as u32,
        };
        self.locations.insert(pixels, location);
        (location, true)
    }

    fn sample_y(row: u32) -> f32 {
        debug_assert!(row < GRADIENT_ATLAS_ROWS);
        (row as f32 + 0.5) / GRADIENT_ATLAS_ROWS as f32
    }
}

#[derive(Debug, Default)]
struct GradientBindGroupLayout {
    slots: HashMap<usize, usize>,
}

impl GradientBindGroupLayout {
    fn slot_for_page(&mut self, page: usize) -> usize {
        if let Some(slot) = self.slots.get(&page) {
            return *slot;
        }

        let slot = self.slots.len();
        self.slots.insert(page, slot);
        slot
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.slots.len()
    }
}

#[derive(Debug)]
pub struct Mesh {
    pub draws: Vec<Draw>,
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
}

impl ShapeHandleImpl for Mesh {}

pub fn as_mesh(handle: &ShapeHandle) -> &Mesh {
    <dyn Any>::downcast_ref(&*handle.0).expect("Shape handle must be a WGPU ShapeData")
}

#[derive(Debug)]
pub struct PendingDraw {
    pub draw_type: PendingDrawType,
    pub vertices: Range<wgpu::BufferAddress>,
    pub indices: Range<wgpu::BufferAddress>,
    pub num_indices: u32,
    pub num_mask_indices: u32,
}

impl PendingDraw {
    pub fn finish_all(
        pending: Vec<Self>,
        descriptors: &Descriptors,
        uniform_buffer: &wgpu::Buffer,
        gradients: &[CommonGradient],
    ) -> Vec<Draw> {
        let mut layout = GradientBindGroupLayout::default();
        let mut bind_groups = Vec::new();
        pending
            .into_iter()
            .map(|draw| {
                draw.finish(
                    descriptors,
                    uniform_buffer,
                    gradients,
                    &mut layout,
                    &mut bind_groups,
                )
            })
            .collect()
    }

    fn finish(
        self,
        descriptors: &Descriptors,
        uniform_buffer: &wgpu::Buffer,
        gradients: &[CommonGradient],
        gradient_layout: &mut GradientBindGroupLayout,
        gradient_bind_groups: &mut Vec<wgpu::BindGroup>,
    ) -> Draw {
        Draw {
            draw_type: self.draw_type.finish(
                descriptors,
                uniform_buffer,
                gradients,
                gradient_layout,
                gradient_bind_groups,
            ),
            vertices: self.vertices,
            indices: self.indices,
            num_indices: self.num_indices,
            num_mask_indices: self.num_mask_indices,
        }
    }
}

#[derive(Debug)]
pub struct Draw {
    pub draw_type: DrawType,
    pub vertices: Range<wgpu::BufferAddress>,
    pub indices: Range<wgpu::BufferAddress>,
    pub num_indices: u32,
    pub num_mask_indices: u32,
}

impl PendingDraw {
    #[expect(clippy::too_many_arguments)]
    pub fn new<T: RenderTarget>(
        backend: &mut WgpuRenderBackend<T>,
        source: &dyn BitmapSource,
        draw: LyonDraw,
        shape_id: CharacterId,
        draw_id: usize,
        uniform_buffer: &mut BufferBuilder,
        vertex_buffer: &mut BufferBuilder,
        index_buffer: &mut BufferBuilder,
    ) -> Option<Self> {
        let vertices = if matches!(draw.draw_type, TessDrawType::Color) {
            let vertices: Vec<_> = draw
                .vertices
                .into_iter()
                .map(PosColorVertex::from)
                .collect();
            vertex_buffer
                .add(&vertices)
                .expect("Mesh vertex buffer was too large!")
        } else {
            let vertices: Vec<_> = draw.vertices.into_iter().map(PosVertex::from).collect();
            vertex_buffer
                .add(&vertices)
                .expect("Mesh vertex buffer was too large!")
        };

        let indices = index_buffer
            .add(&draw.indices)
            .expect("Mesh index buffer was too large!");

        let index_count = draw.indices.len() as u32;
        let draw_type = match draw.draw_type {
            TessDrawType::Color => PendingDrawType::color(),
            TessDrawType::Gradient { matrix, gradient } => {
                PendingDrawType::gradient(gradient, matrix, shape_id, draw_id, uniform_buffer)
            }
            TessDrawType::Bitmap(bitmap) => {
                PendingDrawType::bitmap(bitmap, shape_id, draw_id, source, backend, uniform_buffer)?
            }
        };
        Some(PendingDraw {
            draw_type,
            vertices,
            indices,
            num_indices: index_count,
            num_mask_indices: draw.mask_index_count,
        })
    }
}

#[derive(Debug)]
pub enum PendingDrawType {
    Color,
    Gradient {
        texture_transforms_index: wgpu::BufferAddress,
        gradient_index: usize,
        bind_group_label: Option<String>,
    },
    Bitmap {
        texture_transforms_index: wgpu::BufferAddress,
        texture_view: wgpu::TextureView,
        is_repeating: bool,
        is_smoothed: bool,
        bind_group_label: Option<String>,
    },
}

/// Converts an RGBA color from sRGB space to linear color space.
fn srgb_to_linear(color: f32) -> f32 {
    if color <= 0.04045 {
        color / 12.92
    } else {
        f32::powf((color + 0.055) / 1.055, 2.4)
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

impl PendingDrawType {
    pub fn color() -> Self {
        PendingDrawType::Color
    }

    pub fn gradient(
        gradient_index: usize,
        matrix: [[f32; 3]; 3],
        shape_id: CharacterId,
        draw_id: usize,
        uniform_buffers: &mut BufferBuilder,
    ) -> Self {
        let tex_transforms_index = create_texture_transforms(&matrix, uniform_buffers);

        let bind_group_label =
            create_debug_label!("Shape {} (gradient) draw {} bindgroup", shape_id, draw_id);
        PendingDrawType::Gradient {
            texture_transforms_index: tex_transforms_index,
            gradient_index,
            bind_group_label,
        }
    }

    pub fn bitmap(
        bitmap: Bitmap,
        shape_id: CharacterId,
        draw_id: usize,
        source: &dyn BitmapSource,
        backend: &mut dyn RenderBackend,
        uniform_buffers: &mut BufferBuilder,
    ) -> Option<Self> {
        let handle = source.bitmap_handle(bitmap.bitmap_id, backend)?;
        let texture = as_texture(&handle);
        let texture_view = texture.texture.create_view(&Default::default());
        let texture_transforms_index = create_texture_transforms(&bitmap.matrix, uniform_buffers);
        let bind_group_label =
            create_debug_label!("Shape {} (bitmap) draw {} bindgroup", shape_id, draw_id);

        Some(PendingDrawType::Bitmap {
            texture_transforms_index,
            texture_view,
            is_repeating: bitmap.is_repeating,
            is_smoothed: bitmap.is_smoothed,
            bind_group_label,
        })
    }

    fn finish(
        self,
        descriptors: &Descriptors,
        uniform_buffer: &wgpu::Buffer,
        gradients: &[CommonGradient],
        gradient_layout: &mut GradientBindGroupLayout,
        gradient_bind_groups: &mut Vec<wgpu::BindGroup>,
    ) -> DrawType {
        match self {
            PendingDrawType::Color => DrawType::Color,
            PendingDrawType::Gradient {
                texture_transforms_index,
                gradient_index,
                bind_group_label,
            } => {
                let common = &gradients[gradient_index];
                let slot = gradient_layout.slot_for_page(common.atlas_page);
                if slot == gradient_bind_groups.len() {
                    let bind_group =
                        descriptors
                            .device
                            .create_bind_group(&wgpu::BindGroupDescriptor {
                                layout: &descriptors.bind_layouts.gradient,
                                entries: &[
                                    wgpu::BindGroupEntry {
                                        binding: 0,
                                        resource: wgpu::BindingResource::Buffer(
                                            wgpu::BufferBinding {
                                                buffer: uniform_buffer,
                                                offset: 0,
                                                size: wgpu::BufferSize::new(std::mem::size_of::<
                                                    TextureTransforms,
                                                >(
                                                )
                                                    as u64),
                                            },
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 1,
                                        resource: wgpu::BindingResource::Buffer(
                                            wgpu::BufferBinding {
                                                buffer: uniform_buffer,
                                                offset: 0,
                                                size: wgpu::BufferSize::new(std::mem::size_of::<
                                                    GradientUniforms,
                                                >(
                                                )
                                                    as u64),
                                            },
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 2,
                                        resource: wgpu::BindingResource::TextureView(
                                            &common.texture_view,
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 3,
                                        resource: wgpu::BindingResource::Sampler(
                                            descriptors.bitmap_samplers.get_sampler(false, true),
                                        ),
                                    },
                                ],
                                label: bind_group_label.as_deref(),
                            });
                    gradient_bind_groups.push(bind_group);
                }
                DrawType::Gradient {
                    bind_group: gradient_bind_groups[slot].clone(),
                    texture_transforms_offset: texture_transforms_index
                        .try_into()
                        .expect("gradient texture-transform offset must fit in u32"),
                    gradient_offset: common
                        .buffer_offset
                        .try_into()
                        .expect("gradient uniform offset must fit in u32"),
                }
            }
            PendingDrawType::Bitmap {
                texture_transforms_index,
                texture_view,
                is_repeating,
                is_smoothed,
                bind_group_label,
            } => {
                let binds = BitmapBinds::new(
                    &descriptors.device,
                    &descriptors.bind_layouts.bitmap,
                    descriptors
                        .bitmap_samplers
                        .get_sampler(is_repeating, is_smoothed),
                    uniform_buffer,
                    texture_transforms_index,
                    texture_view,
                    bind_group_label,
                );

                DrawType::Bitmap { binds }
            }
        }
    }
}

#[derive(Debug)]
pub enum DrawType {
    Color,
    Gradient {
        bind_group: wgpu::BindGroup,
        texture_transforms_offset: wgpu::DynamicOffset,
        gradient_offset: wgpu::DynamicOffset,
    },
    Bitmap {
        binds: BitmapBinds,
    },
}

#[derive(Debug)]
pub struct CommonGradient {
    atlas_page: usize,
    texture_view: wgpu::TextureView,
    buffer_offset: wgpu::BufferAddress,
}

impl CommonGradient {
    pub fn new(
        descriptors: &Descriptors,
        gradient: Gradient,
        atlas: &mut GradientAtlas,
        uniform_buffers: &mut BufferBuilder,
    ) -> Self {
        let colors = if gradient.records.is_empty() {
            [0; GRADIENT_SIZE * 4]
        } else {
            let mut colors = [0; GRADIENT_SIZE * 4];
            let mut last = 0;
            let mut next;

            let convert = if gradient.interpolation == GradientInterpolation::LinearRgb {
                |c| srgb_to_linear(c / 255.0) * 255.0
            } else {
                |c| c
            };

            for t in 0..GRADIENT_SIZE {
                if last + 1 < gradient.records.len()
                    && t > gradient.records[last + 1].ratio as usize
                {
                    last += 1;
                }
                next = (last + 1).min(gradient.records.len() - 1);

                assert!(last == next || last + 1 == next);

                let last_record = &gradient.records[last];
                let next_record = &gradient.records[next];

                let a = if t <= last_record.ratio as usize || last_record.ratio == next_record.ratio
                {
                    // We are before the first gradient record,
                    // or this record's ratio is equal to the next one,
                    // meaning we need to do a full stop of this color for 1 pixel.
                    0.0
                } else if t > next_record.ratio as usize {
                    // We are after the last record
                    1.0
                } else {
                    (t as f32 - last_record.ratio as f32)
                        / (next_record.ratio as f32 - last_record.ratio as f32)
                };

                colors[t * 4] = lerp(
                    convert(last_record.color.r as f32),
                    convert(next_record.color.r as f32),
                    a,
                ) as u8;
                colors[(t * 4) + 1] = lerp(
                    convert(last_record.color.g as f32),
                    convert(next_record.color.g as f32),
                    a,
                ) as u8;
                colors[(t * 4) + 2] = lerp(
                    convert(last_record.color.b as f32),
                    convert(next_record.color.b as f32),
                    a,
                ) as u8;
                colors[(t * 4) + 3] =
                    lerp(last_record.color.a as f32, next_record.color.a as f32, a) as u8;
            }

            colors
        };
        let (location, texture_view) = atlas.allocate(descriptors, colors);

        let buffer_offset = uniform_buffers
            .add(&[GradientUniforms::new(
                gradient,
                GradientAtlasLayout::sample_y(location.row),
            )])
            .expect("Mesh uniform buffer was too large!")
            .start;

        Self {
            atlas_page: location.page,
            texture_view,
            buffer_offset,
        }
    }
}

#[derive(Debug)]
pub struct BitmapBinds {
    pub bind_group: wgpu::BindGroup,
}

impl BitmapBinds {
    pub fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        uniform_buffer: &wgpu::Buffer,
        texture_transforms: wgpu::BufferAddress,
        texture_view: wgpu::TextureView,
        label: Option<String>,
    ) -> Self {
        let bind_group =
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: uniform_buffer,
                            offset: texture_transforms,
                            size: wgpu::BufferSize::new(
                                std::mem::size_of::<TextureTransforms>() as u64
                            ),
                        }),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
                label: label.as_deref(),
            });
        Self { bind_group }
    }
}

fn create_texture_transforms(
    matrix: &[[f32; 3]; 3],
    buffer: &mut BufferBuilder,
) -> wgpu::BufferAddress {
    let mut texture_transform = [[0.0; 4]; 4];
    texture_transform[0][..3].copy_from_slice(&matrix[0]);
    texture_transform[1][..3].copy_from_slice(&matrix[1]);
    texture_transform[2][..3].copy_from_slice(&matrix[2]);
    buffer
        .add(&[texture_transform])
        .expect("Mesh uniform buffer was too large!")
        .start
}

#[cfg(test)]
mod tests {
    use super::{GRADIENT_ATLAS_ROWS, GRADIENT_SIZE, GradientAtlasLayout, GradientBindGroupLayout};

    #[test]
    fn gradient_atlas_packs_crowded_room_scale_into_seven_textures() {
        let mut layout = GradientAtlasLayout::default();
        let mut last = None;

        for index in 0_u32..28_504 {
            let mut pixels = [0; GRADIENT_SIZE * 4];
            pixels[..4].copy_from_slice(&index.to_le_bytes());
            last = Some(layout.allocate(pixels));
        }

        let last = last.expect("the trace contains gradients");
        assert_eq!(GRADIENT_ATLAS_ROWS, 4_096);
        assert_eq!(last.page, 6);
        assert_eq!(last.row, 3_927);
    }

    #[test]
    fn gradient_atlas_reuses_identical_color_ramps() {
        let mut layout = GradientAtlasLayout::default();
        let pixels = [37; GRADIENT_SIZE * 4];

        let first = layout.allocate(pixels);
        let second = layout.allocate(pixels);

        assert_eq!(first, second);
    }

    #[test]
    fn gradient_atlas_only_uploads_a_color_ramp_once() {
        let mut layout = GradientAtlasLayout::default();
        let pixels = [91; GRADIENT_SIZE * 4];

        let (first, first_is_new) = layout.allocate_with_status(pixels);
        let (second, second_is_new) = layout.allocate_with_status(pixels);

        assert_eq!(first, second);
        assert!(first_is_new);
        assert!(!second_is_new);
    }

    #[test]
    fn gradient_atlas_samples_the_center_of_each_row() {
        let first = GradientAtlasLayout::sample_y(0);
        let last = GradientAtlasLayout::sample_y(GRADIENT_ATLAS_ROWS - 1);

        assert_eq!(first, 0.5 / 4_096.0);
        assert_eq!(last, 4_095.5 / 4_096.0);
    }

    #[test]
    fn gradient_draws_share_one_bind_group_per_mesh_atlas_page() {
        let mut layout = GradientBindGroupLayout::default();

        assert_eq!(layout.slot_for_page(4), 0);
        assert_eq!(layout.slot_for_page(4), 0);
        assert_eq!(layout.slot_for_page(9), 1);
        assert_eq!(layout.slot_for_page(4), 0);
        assert_eq!(layout.len(), 2);
    }
}
