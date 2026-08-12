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

    /// The region a render target actually drew into, which is not always the whole texture.
    ///
    /// The offscreen pool rounds requested sizes out to a grid so near-identical content shares a
    /// bucket, so a filter's output sits in the top-left corner of a texture that may be larger.
    /// Reading it back with [`Self::for_whole_texture`] would treat the padding as content and
    /// sample a glow or bevel across it.
    pub fn for_target(texture: &wgpu::Texture, size: (u32, u32)) -> Self {
        Self {
            texture_size: (texture.width(), texture.height()),
            point: (0, 0),
            size,
        }
    }

    /// A quad covering this region, for a pass whose output is the same shape as its input.
    ///
    /// `position` spans the whole render target, because the caller confines the target to the
    /// region with a viewport. `uv` stops at the content, because the texture behind it may be
    /// larger than the content is.
    pub fn vertices(&self) -> [FilterVertex; 4] {
        let corners = self.corners();
        let uv = |corner: (f32, f32)| self.uv(corner, (0.0, 0.0));
        [
            FilterVertex {
                position: [0.0, 0.0],
                uv: uv(corners[0]),
            },
            FilterVertex {
                position: [1.0, 0.0],
                uv: uv(corners[1]),
            },
            FilterVertex {
                position: [1.0, 1.0],
                uv: uv(corners[2]),
            },
            FilterVertex {
                position: [0.0, 1.0],
                uv: uv(corners[3]),
            },
        ]
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

/// Approximate a gradient glow with the supported single-color glow pipeline.
///
/// Prefer a visible chromatic stop so authored state colors survive decorative transparent and
/// neutral stops. This is intentionally renderer-generic: the selected color always comes from
/// the SWF rather than from knowledge of a particular movie.
fn gradient_glow_fallback(filter: &swf::GradientFilter) -> (swf::GlowFilter, (f32, f32)) {
    let color = filter
        .colors
        .iter()
        .max_by_key(|record| {
            let max = record.color.r.max(record.color.g).max(record.color.b);
            let min = record.color.r.min(record.color.g).min(record.color.b);
            u32::from(record.color.a) * (u32::from(max - min) + 1)
        })
        .map(|record| record.color)
        .unwrap_or(swf::Color::TRANSPARENT);

    let mut flags = swf::GlowFilterFlags::from_passes(filter.num_passes());
    flags.set(swf::GlowFilterFlags::INNER_GLOW, filter.is_inner());
    flags.set(swf::GlowFilterFlags::KNOCKOUT, filter.is_knockout());
    flags.set(
        swf::GlowFilterFlags::COMPOSITE_SOURCE,
        filter
            .flags
            .contains(swf::GradientFilterFlags::COMPOSITE_SOURCE),
    );

    let distance = filter.distance.to_f32();
    let angle = filter.angle.to_f32();
    let blur_offset = (-angle.cos() * distance, -angle.sin() * distance);

    (
        swf::GlowFilter {
            color,
            blur_x: filter.blur_x,
            blur_y: filter.blur_y,
            strength: filter.strength,
            flags,
        },
        blur_offset,
    )
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
        self.region().vertices()
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
                &[],
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
            Filter::GradientGlowFilter(filter) => {
                let (fallback, blur_offset) = gradient_glow_fallback(&filter);
                Some(descriptors.filters.glow.apply(
                    descriptors,
                    texture_pool,
                    draw_encoder,
                    staging_belt,
                    &source,
                    &fallback,
                    &self.blur,
                    blur_offset,
                    &filter.colors,
                ))
            }
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
                    Filter::GradientBevelFilter(_) => "GradientBevelFilter",
                    Filter::ConvolutionFilter(_) => "ConvolutionFilter",
                    Filter::ColorMatrixFilter(_)
                    | Filter::BlurFilter(_)
                    | Filter::GlowFilter(_)
                    | Filter::DropShadowFilter(_)
                    | Filter::BevelFilter(_)
                    | Filter::GradientGlowFilter(_)
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
mod region_tests {
    use super::FilterRegion;

    /// A filter's output occupies the corner of a texture the pool may have rounded up, so the UVs
    /// that read it back have to stop at the content. Taking the whole texture would drag the
    /// cleared padding into the glow.
    #[test]
    fn a_target_region_stops_at_the_content_not_the_texture_edge() {
        let region = FilterRegion {
            texture_size: (64, 32),
            point: (0, 0),
            size: (37, 25),
        };

        let corners = region.corners();
        assert_eq!(corners[2], (37.0, 25.0), "extent comes from the region");

        // Bottom-right UV normalises the content extent against the texture, not against itself.
        let uv = region.uv((37.0, 25.0), (0.0, 0.0));
        assert!((uv[0] - 37.0 / 64.0).abs() < 1e-6, "{uv:?}");
        assert!((uv[1] - 25.0 / 32.0).abs() < 1e-6, "{uv:?}");
    }

    /// When the texture is exactly the region -- every pool texture before quantisation, and every
    /// unpooled one after -- the UVs must still run right to the edge.
    #[test]
    fn an_exact_texture_still_maps_corner_to_corner() {
        let region = FilterRegion {
            texture_size: (40, 32),
            point: (0, 0),
            size: (40, 32),
        };

        let uv = region.uv((40.0, 32.0), (0.0, 0.0));
        assert!((uv[0] - 1.0).abs() < 1e-6, "{uv:?}");
        assert!((uv[1] - 1.0).abs() < 1e-6, "{uv:?}");
    }
}

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

    fn gradient_filter(
        colors: Vec<swf::GradientRecord>,
        flags: swf::GradientFilterFlags,
    ) -> swf::GradientFilter {
        swf::GradientFilter {
            colors,
            blur_x: swf::Fixed16::from_f32(8.0),
            blur_y: swf::Fixed16::from_f32(6.0),
            angle: swf::Fixed16::from_f32(std::f32::consts::FRAC_PI_2),
            distance: swf::Fixed16::from_f32(4.0),
            strength: swf::Fixed8::from_f32(1.5),
            flags,
        }
    }

    #[test]
    fn gradient_glow_fallback_preserves_a_visible_red_hue() {
        let filter = gradient_filter(
            vec![
                swf::GradientRecord {
                    ratio: 0,
                    color: swf::Color::from_rgb(0xffffff, 0),
                },
                swf::GradientRecord {
                    ratio: 128,
                    color: swf::Color::from_rgb(0xf04438, 192),
                },
                swf::GradientRecord {
                    ratio: 255,
                    color: swf::Color::from_rgb(0x666666, 255),
                },
            ],
            swf::GradientFilterFlags::COMPOSITE_SOURCE,
        );

        let (fallback, _) = gradient_glow_fallback(&filter);

        assert_eq!(fallback.color, swf::Color::from_rgb(0xf04438, 192));
    }

    #[test]
    fn gradient_glow_fallback_preserves_a_visible_teal_hue() {
        let filter = gradient_filter(
            vec![
                swf::GradientRecord {
                    ratio: 0,
                    color: swf::Color::from_rgb(0xffffff, 0),
                },
                swf::GradientRecord {
                    ratio: 160,
                    color: swf::Color::from_rgb(0x39c8b0, 180),
                },
                swf::GradientRecord {
                    ratio: 255,
                    color: swf::Color::from_rgb(0x777777, 255),
                },
            ],
            swf::GradientFilterFlags::COMPOSITE_SOURCE,
        );

        let (fallback, _) = gradient_glow_fallback(&filter);

        assert_eq!(fallback.color, swf::Color::from_rgb(0x39c8b0, 180));
    }

    #[test]
    fn gradient_glow_fallback_uses_the_most_visible_neutral_stop() {
        let filter = gradient_filter(
            vec![
                swf::GradientRecord {
                    ratio: 0,
                    color: swf::Color::from_rgb(0xffffff, 0),
                },
                swf::GradientRecord {
                    ratio: 255,
                    color: swf::Color::from_rgb(0x666666, 255),
                },
            ],
            swf::GradientFilterFlags::COMPOSITE_SOURCE,
        );

        let (fallback, _) = gradient_glow_fallback(&filter);

        assert_eq!(fallback.color, swf::Color::from_rgb(0x666666, 255));
    }

    #[test]
    fn gradient_glow_fallback_preserves_filter_behavior_and_offset() {
        let flags = swf::GradientFilterFlags::INNER_SHADOW
            | swf::GradientFilterFlags::KNOCKOUT
            | swf::GradientFilterFlags::COMPOSITE_SOURCE
            | swf::GradientFilterFlags::from_passes(3);
        let filter = gradient_filter(
            vec![swf::GradientRecord {
                ratio: 255,
                color: swf::Color::RED,
            }],
            flags,
        );

        let (fallback, offset) = gradient_glow_fallback(&filter);

        assert_eq!(fallback.blur_x, filter.blur_x);
        assert_eq!(fallback.blur_y, filter.blur_y);
        assert_eq!(fallback.strength, filter.strength);
        assert!(fallback.is_inner());
        assert!(fallback.is_knockout());
        assert!(fallback.composite_source());
        assert_eq!(fallback.num_passes(), 3);
        assert!(offset.0.abs() < 0.0001);
        assert!((offset.1 + 4.0).abs() < 0.0001);
    }
}
