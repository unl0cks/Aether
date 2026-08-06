mod bevel;
mod blur;
mod color_matrix;
mod displacement_map;
mod drop_shadow;
mod glow;
mod shader;

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

use crate::buffer_pool::TexturePool;
use crate::descriptors::Descriptors;
use crate::filters::bevel::BevelFilter;
use crate::filters::blur::BlurFilter;
use crate::filters::color_matrix::ColorMatrixFilter;
use crate::filters::displacement_map::DisplacementMapFilter;
use crate::filters::drop_shadow::DropShadowFilter;
use crate::filters::glow::GlowFilter;
use crate::filters::shader::ShaderFilter;
use crate::surface::target::CommandTarget;
use bytemuck::{Pod, Zeroable};
use ruffle_render::filters::Filter;
use wgpu::util::StagingBelt;
use wgpu::vertex_attr_array;

#[derive(Debug)]
pub struct FilterSource<'a> {
    pub texture: &'a wgpu::Texture,
    pub view: wgpu::TextureView,
    pub point: (u32, u32),
    pub size: (u32, u32),
}

/// Where a filter input actually lives: the rectangle being filtered, and the dimensions of the
/// texture holding it.
///
/// The two are not always equal. A `cacheAsBitmap` surface may be allocated larger than its
/// contents, so its UVs have to be normalised against the texture while its extent comes from the
/// region. Filters that sample a second texture alongside the source need one of these per input,
/// because a blurred layer is its own texture and shares neither number with the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilterRegion {
    pub texture_size: (u32, u32),
    pub point: (u32, u32),
    pub size: (u32, u32),
}

impl FilterRegion {
    pub fn for_whole_texture(texture: &wgpu::Texture) -> Self {
        Self {
            texture_size: (texture.width(), texture.height()),
            point: (0, 0),
            size: (texture.width(), texture.height()),
        }
    }

    /// UV of a point `offset` pixels from this region's corner, normalised against its texture.
    fn uv(&self, corner: (f32, f32), offset: (f32, f32)) -> [f32; 2] {
        [
            (self.point.0 as f32 + corner.0 + offset.0) / self.texture_size.0.max(1) as f32,
            (self.point.1 as f32 + corner.1 + offset.1) / self.texture_size.1.max(1) as f32,
        ]
    }

    /// The four corners of the region, in pixels relative to its own top left.
    fn corners(&self) -> [(f32, f32); 4] {
        let (width, height) = (self.size.0 as f32, self.size.1 as f32);
        [(0.0, 0.0), (width, 0.0), (width, height), (0.0, height)]
    }
}

/// Vertices for a filter that samples the source and one blurred layer.
///
/// The blurred layer is a separate texture the size of the filtered region, so its UVs must be
/// normalised against `blur`, not against the source. Those agree only while the source texture is
/// exactly its own region, which stopped being true once cache textures could be allocated larger
/// than their contents.
fn filter_vertices_with_blur(
    source: FilterRegion,
    blur: FilterRegion,
    blur_offset: (f32, f32),
) -> [FilterVertexWithBlur; 4] {
    let positions = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let corners = source.corners();
    std::array::from_fn(|i| FilterVertexWithBlur {
        position: positions[i],
        source_uv: source.uv(corners[i], (0.0, 0.0)),
        blur_uv: blur.uv(corners[i], blur_offset),
    })
}

/// As [`filter_vertices_with_blur`], for filters that sample the blurred layer twice at opposing
/// offsets.
fn filter_vertices_with_double_blur(
    source: FilterRegion,
    blur: FilterRegion,
    blur_offset: (f32, f32),
) -> [FilterVertexWithDoubleBlur; 4] {
    let positions = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let corners = source.corners();
    std::array::from_fn(|i| FilterVertexWithDoubleBlur {
        position: positions[i],
        source_uv: source.uv(corners[i], (0.0, 0.0)),
        blur_uv_left: blur.uv(corners[i], blur_offset),
        blur_uv_right: blur.uv(corners[i], (-blur_offset.0, -blur_offset.1)),
    })
}

impl<'a> FilterSource<'a> {
    pub fn for_entire_texture(texture: &'a wgpu::Texture) -> Self {
        Self {
            texture,
            view: texture.create_view(&Default::default()),
            point: (0, 0),
            size: (texture.width(), texture.height()),
        }
    }

    pub fn region(&self) -> FilterRegion {
        FilterRegion {
            texture_size: (self.texture.width(), self.texture.height()),
            point: self.point,
            size: self.size,
        }
    }

    pub fn vertices(&self) -> [FilterVertex; 4] {
        let source_width = self.texture.width() as f32;
        let source_height = self.texture.height() as f32;
        let left = self.point.0;
        let top = self.point.1;
        let right = left + self.size.0;
        let bottom = top + self.size.1;
        [
            FilterVertex {
                position: [0.0, 0.0],
                uv: [left as f32 / source_width, top as f32 / source_height],
            },
            FilterVertex {
                position: [1.0, 0.0],
                uv: [right as f32 / source_width, top as f32 / source_height],
            },
            FilterVertex {
                position: [1.0, 1.0],
                uv: [right as f32 / source_width, bottom as f32 / source_height],
            },
            FilterVertex {
                position: [0.0, 1.0],
                uv: [left as f32 / source_width, bottom as f32 / source_height],
            },
        ]
    }

    /// Vertices for a filter sampling this source plus `blur`, a separate blurred layer.
    pub fn vertices_with_blur_offset(
        &self,
        blur: FilterRegion,
        blur_offset: (f32, f32),
    ) -> [FilterVertexWithBlur; 4] {
        filter_vertices_with_blur(self.region(), blur, blur_offset)
    }

    /// As [`Self::vertices_with_blur_offset`], for filters sampling `blur` at opposing offsets.
    pub fn vertices_with_highlight_and_shadow(
        &self,
        blur: FilterRegion,
        blur_offset: (f32, f32),
    ) -> [FilterVertexWithDoubleBlur; 4] {
        filter_vertices_with_double_blur(self.region(), blur, blur_offset)
    }
}

pub struct Filters {
    pub blur: BlurFilter,
    pub color_matrix: ColorMatrixFilter,
    pub shader: ShaderFilter,
    pub glow: GlowFilter,
    pub bevel: BevelFilter,
    pub displacement_map: DisplacementMapFilter,
}

impl Filters {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            blur: BlurFilter::new(device),
            color_matrix: ColorMatrixFilter::new(device),
            shader: ShaderFilter::new(),
            glow: GlowFilter::new(device),
            bevel: BevelFilter::new(device),
            displacement_map: DisplacementMapFilter::new(device),
        }
    }

    pub fn apply(
        &self,
        descriptors: &Descriptors,
        draw_encoder: &mut wgpu::CommandEncoder,
        texture_pool: &mut TexturePool,
        staging_belt: &mut StagingBelt,
        source: FilterSource,
        filter: Filter,
    ) -> CommandTarget {
        let target = match filter {
            Filter::ColorMatrixFilter(filter) => Some(descriptors.filters.color_matrix.apply(
                descriptors,
                texture_pool,
                draw_encoder,
                staging_belt,
                &source,
                &filter,
            )),
            Filter::BlurFilter(filter) => descriptors.filters.blur.apply(
                descriptors,
                texture_pool,
                draw_encoder,
                staging_belt,
                &source,
                &filter,
            ),
            Filter::ShaderFilter(shader) => Some(descriptors.filters.shader.apply(
                descriptors,
                texture_pool,
                draw_encoder,
                &source,
                shader,
            )),
            Filter::GlowFilter(filter) => Some(descriptors.filters.glow.apply(
                descriptors,
                texture_pool,
                draw_encoder,
                staging_belt,
                &source,
                &filter,
                &self.blur,
                (0.0, 0.0),
            )),
            Filter::DropShadowFilter(filter) => Some(DropShadowFilter::apply(
                descriptors,
                texture_pool,
                draw_encoder,
                staging_belt,
                &source,
                &filter,
                &self.blur,
                &self.glow,
            )),
            Filter::BevelFilter(filter) => Some(descriptors.filters.bevel.apply(
                descriptors,
                texture_pool,
                draw_encoder,
                staging_belt,
                &source,
                &filter,
                &self.blur,
            )),
            Filter::DisplacementMapFilter(filter) => descriptors.filters.displacement_map.apply(
                descriptors,
                texture_pool,
                draw_encoder,
                staging_belt,
                &source,
                &filter,
            ),
            filter => {
                static WARNED_FILTERS: LazyLock<Mutex<HashSet<&'static str>>> =
                    LazyLock::new(Default::default);

                let name = match filter {
                    Filter::GradientGlowFilter(_) => "GradientGlowFilter",
                    Filter::GradientBevelFilter(_) => "GradientBevelFilter",
                    Filter::ConvolutionFilter(_) => "ConvolutionFilter",
                    Filter::ColorMatrixFilter(_)
                    | Filter::BlurFilter(_)
                    | Filter::GlowFilter(_)
                    | Filter::DropShadowFilter(_)
                    | Filter::BevelFilter(_)
                    | Filter::DisplacementMapFilter(_)
                    | Filter::ShaderFilter(_) => unreachable!(),
                };
                // Only warn once per filter type
                if WARNED_FILTERS.lock().unwrap().insert(name) {
                    tracing::warn!("Unsupported filter {filter:?}");
                }
                None
            }
        };

        let target = target.unwrap_or_else(|| {
            // Apply a default color matrix - it's essentially a blit
            // TODO: Not need to do this.
            descriptors.filters.color_matrix.apply(
                descriptors,
                texture_pool,
                draw_encoder,
                staging_belt,
                &source,
                &Default::default(),
            )
        });

        // We're about to perform a copy, so make sure that we've applied
        // a clear (in case no other draw commands were issued, we still need
        // the background clear color applied)
        target.ensure_cleared(draw_encoder);
        target
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct FilterVertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
}

pub const VERTEX_BUFFERS_DESCRIPTION_FILTERS: [wgpu::VertexBufferLayout; 1] =
    [wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<FilterVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &vertex_attr_array![
            0 => Float32x2,
            1 => Float32x2,
        ],
    }];

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct FilterVertexWithBlur {
    pub position: [f32; 2],
    pub source_uv: [f32; 2],
    pub blur_uv: [f32; 2],
}

pub const VERTEX_BUFFERS_DESCRIPTION_FILTERS_WITH_BLUR: [wgpu::VertexBufferLayout; 1] =
    [wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<FilterVertexWithBlur>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &vertex_attr_array![
            0 => Float32x2,
            1 => Float32x2,
            2 => Float32x2,
        ],
    }];

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct FilterVertexWithDoubleBlur {
    pub position: [f32; 2],
    pub source_uv: [f32; 2],
    pub blur_uv_left: [f32; 2],
    pub blur_uv_right: [f32; 2],
}

pub const VERTEX_BUFFERS_DESCRIPTION_FILTERS_WITH_DOUBLE_BLUR: [wgpu::VertexBufferLayout; 1] =
    [wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<FilterVertexWithDoubleBlur>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &vertex_attr_array![
            0 => Float32x2,
            1 => Float32x2,
            2 => Float32x2,
            3 => Float32x2,
        ],
    }];

#[cfg(test)]
mod blur_layer_uv_tests {
    use super::*;

    /// A cache texture rounded up to a 64px grid, holding a 247x226 region.
    const OVERSIZED_SOURCE: FilterRegion = FilterRegion {
        texture_size: (256, 256),
        point: (0, 0),
        size: (247, 226),
    };

    /// The blur pass allocates its target at exactly the filtered region's size.
    const BLUR_LAYER: FilterRegion = FilterRegion {
        texture_size: (247, 226),
        point: (0, 0),
        size: (247, 226),
    };

    #[test]
    fn a_blur_layer_is_sampled_across_the_whole_of_its_own_texture() {
        let vertices = filter_vertices_with_blur(OVERSIZED_SOURCE, BLUR_LAYER, (0.0, 0.0));

        assert_eq!(vertices[0].blur_uv, [0.0, 0.0]);
        assert_eq!(
            vertices[2].blur_uv,
            [1.0, 1.0],
            "the blurred layer fills its own texture, so its far corner is 1,1 however large the \
             source texture is"
        );
    }

    #[test]
    fn the_source_keeps_its_own_share_of_an_oversized_texture() {
        let vertices = filter_vertices_with_blur(OVERSIZED_SOURCE, BLUR_LAYER, (0.0, 0.0));

        assert_eq!(vertices[0].source_uv, [0.0, 0.0]);
        assert_eq!(vertices[2].source_uv, [247.0 / 256.0, 226.0 / 256.0]);
    }

    #[test]
    fn a_blur_offset_is_scaled_by_the_layer_it_offsets_into() {
        let vertices = filter_vertices_with_blur(OVERSIZED_SOURCE, BLUR_LAYER, (4.0, -8.0));

        assert_eq!(vertices[0].blur_uv, [4.0 / 247.0, -8.0 / 226.0]);
    }

    #[test]
    fn an_exactly_sized_source_is_unchanged() {
        // The overwhelmingly common case, and the one that hid this: when the texture is exactly
        // its region, normalising against either gives the same numbers.
        let exact = FilterRegion {
            texture_size: (463, 498),
            point: (0, 0),
            size: (463, 498),
        };
        let vertices = filter_vertices_with_blur(exact, exact, (0.0, 0.0));

        assert_eq!(vertices[0].source_uv, [0.0, 0.0]);
        assert_eq!(vertices[0].blur_uv, [0.0, 0.0]);
        assert_eq!(vertices[2].source_uv, [1.0, 1.0]);
        assert_eq!(vertices[2].blur_uv, [1.0, 1.0]);
    }

    #[test]
    fn a_source_offset_within_a_larger_texture_is_preserved() {
        // `BitmapData.applyFilter` filters a sub-rect of a bigger texture, so the source origin is
        // not always zero.
        let sub_rect = FilterRegion {
            texture_size: (200, 100),
            point: (50, 25),
            size: (100, 50),
        };
        let vertices = filter_vertices_with_blur(sub_rect, sub_rect, (0.0, 0.0));

        assert_eq!(vertices[0].source_uv, [50.0 / 200.0, 25.0 / 100.0]);
        assert_eq!(vertices[2].source_uv, [150.0 / 200.0, 75.0 / 100.0]);
    }

    #[test]
    fn a_double_blur_offsets_in_both_directions_within_its_own_layer() {
        let vertices = filter_vertices_with_double_blur(OVERSIZED_SOURCE, BLUR_LAYER, (4.0, 8.0));

        assert_eq!(vertices[0].blur_uv_left, [4.0 / 247.0, 8.0 / 226.0]);
        assert_eq!(vertices[0].blur_uv_right, [-4.0 / 247.0, -8.0 / 226.0]);
        assert_eq!(
            vertices[2].blur_uv_left,
            [(247.0 + 4.0) / 247.0, (226.0 + 8.0) / 226.0]
        );
    }
}
