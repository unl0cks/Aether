// Remove this when we decide on how to handle multithreaded rendering (especially on wasm)
#![allow(clippy::arc_with_non_send_sync)]

use crate::backend::ActiveFrame;
use crate::bitmaps::BitmapSamplers;
use crate::buffer_pool::{BufferPool, PoolEntry};
use crate::descriptors::Quad;
use crate::mesh::BitmapBinds;
use crate::pipelines::Pipelines;
use crate::target::{RenderTarget, SwapChainTarget};
use crate::utils::{
    BufferDimensions, capture_image, create_buffer_with_data, format_list, get_backend_names,
};
use bytemuck::{Pod, Zeroable};
use descriptors::Descriptors;
use enum_map::Enum;
use ruffle_render::backend::RawTexture;
use ruffle_render::bitmap::{BitmapHandle, BitmapHandleImpl, PixelRegion, SyncHandle};
use ruffle_render::shape_utils::GradientType;
use ruffle_render::tessellator::{Gradient as TessGradient, Vertex as TessVertex};
use std::any::Any;
use std::cell::{Cell, OnceCell};
use std::sync::Arc;
use swf::GradientSpread;
pub use wgpu;

type Error = Box<dyn std::error::Error>;

#[macro_use]
pub mod utils;

// Deliberately not behind `aether_metrics`. Public builds are made without it, which is the
// reason every AMD device-loss report so far arrived with no resource data in it.
pub mod aether_gpu_timeline;
// Ungated for the same reason: a device loss on a public build is exactly when the shape of the
// allocator's free space is worth knowing, and totals alone have already failed to explain one.
pub mod aether_allocator_report;
// Ungated: this changes how frames are submitted on every build, not just diagnostic ones.
#[cfg(feature = "aether_metrics")]
pub mod aether_metrics;
pub mod submission_splitter;

mod bitmaps;
mod context3d;
mod globals;
mod pipelines;
mod pixel_bender;
pub mod target;
pub mod texture_pool_policy;

pub mod backend;
mod blend;
pub mod blend_region;
mod buffer_builder;
mod buffer_pool;
#[cfg(feature = "clap")]
pub mod clap;
pub mod descriptors;
pub mod device_status;
mod dynamic_transforms;
mod filters;
mod layouts;
mod mesh;
mod shaders;
mod surface;

/// Override backend multisampling for surfaces created after this call.
pub fn set_backend_msaa_override(sample_count: Option<u32>) {
    surface::set_backend_msaa_override(sample_count);
}

impl BitmapHandleImpl for Texture {}

pub fn as_texture(handle: &BitmapHandle) -> &Texture {
    <dyn Any>::downcast_ref(&*handle.0).unwrap()
}

pub fn raw_texture_as_texture(handle: &dyn RawTexture) -> &wgpu::Texture {
    <dyn Any>::downcast_ref(handle).unwrap()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum MaskState {
    NoMask,
    DrawMaskStencil,
    DrawMaskedContent,
    ClearMaskStencil,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Transforms {
    world_matrix: [[f32; 4]; 4],
    mult_color: [f32; 4],
    add_color: [f32; 4],
    bitmap_uv_scale: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct TextureTransforms {
    u_matrix: [[f32; 4]; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct PosVertex {
    position: [f32; 2],
}

impl From<TessVertex> for PosVertex {
    fn from(vertex: TessVertex) -> Self {
        Self {
            position: [vertex.x, vertex.y],
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct PosColorVertex {
    position: [f32; 2],
    color: [f32; 4],
}

impl From<TessVertex> for PosColorVertex {
    fn from(vertex: TessVertex) -> Self {
        Self {
            position: [vertex.x, vertex.y],
            color: [
                f32::from(vertex.color.r) / 255.0,
                f32::from(vertex.color.g) / 255.0,
                f32::from(vertex.color.b) / 255.0,
                f32::from(vertex.color.a) / 255.0,
            ],
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct GradientUniforms {
    focal_point: f32,
    interpolation: i32,
    shape: i32,
    repeat: i32,
    atlas: [f32; 4],
}

impl GradientUniforms {
    fn new(gradient: TessGradient, atlas_y: f32) -> Self {
        Self {
            focal_point: gradient.focal_point.to_f32().clamp(-0.98, 0.98),
            interpolation: (gradient.interpolation == swf::GradientInterpolation::LinearRgb) as i32,
            shape: match gradient.gradient_type {
                GradientType::Linear => 1,
                GradientType::Radial => 2,
                GradientType::Focal => 3,
            },
            repeat: match gradient.repeat_mode {
                GradientSpread::Pad => 1,
                GradientSpread::Reflect => 2,
                GradientSpread::Repeat => 3,
            },
            atlas: [atlas_y, 0.0, 0.0, 0.0],
        }
    }
}

#[derive(Debug)]
pub enum QueueSyncHandle {
    AlreadyCopied {
        index: Option<wgpu::SubmissionIndex>,
        buffer: PoolEntry<wgpu::Buffer, BufferDimensions>,
        copy_dimensions: BufferDimensions,
        descriptors: Arc<Descriptors>,
    },
    NotCopied {
        handle: BitmapHandle,
        copy_area: PixelRegion,
        descriptors: Arc<Descriptors>,
        pool: Arc<BufferPool<wgpu::Buffer, BufferDimensions>>,
    },
}

impl SyncHandle for QueueSyncHandle {}

impl QueueSyncHandle {
    pub fn capture<R, F: FnOnce(&[u8], u32) -> R>(
        self,
        with_rgba: F,
        frame: &mut ActiveFrame,
    ) -> Option<R> {
        match self {
            QueueSyncHandle::AlreadyCopied {
                index,
                buffer,
                copy_dimensions,
                descriptors,
            } => capture_image(
                &descriptors.device,
                &buffer,
                &copy_dimensions,
                index,
                with_rgba,
            ),
            QueueSyncHandle::NotCopied {
                handle,
                copy_area,
                descriptors,
                pool,
            } => {
                let texture = as_texture(&handle);

                let buffer_dimensions = BufferDimensions::new(
                    copy_area.width() as usize,
                    copy_area.height() as usize,
                    texture.texture.format(),
                );

                let buffer = pool.take(&descriptors, buffer_dimensions.clone());
                frame.command_encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d {
                            x: copy_area.x_min,
                            y: copy_area.y_min,
                            z: 0,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(buffer_dimensions.padded_bytes_per_row),
                            rows_per_image: None,
                        },
                    },
                    wgpu::Extent3d {
                        width: copy_area.width(),
                        height: copy_area.height(),
                        depth_or_array_layers: 1,
                    },
                );
                let index = frame.submit_direct(&descriptors);

                let image = capture_image(
                    &descriptors.device,
                    &buffer,
                    &buffer_dimensions,
                    Some(index),
                    with_rgba,
                );

                // After we've read pixels from a texture enough times, we'll store this buffer so that
                // future reads will be faster (it'll copy as part of the draw process instead)
                texture
                    .copy_count
                    .set(texture.copy_count.get().saturating_add(1));

                image
            }
        }
    }
}

#[derive(Debug)]
pub struct Texture {
    pub(crate) texture: wgpu::Texture,
    bind_linear: OnceCell<BitmapBinds>,
    bind_nearest: OnceCell<BitmapBinds>,
    copy_count: Cell<u8>,
}

impl Texture {
    pub fn bind_group(
        &self,
        smoothed: bool,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        quad: &Quad,
        handle: BitmapHandle,
        samplers: &BitmapSamplers,
    ) -> &BitmapBinds {
        let bind = match smoothed {
            true => &self.bind_linear,
            false => &self.bind_nearest,
        };
        bind.get_or_init(|| {
            BitmapBinds::new(
                device,
                layout,
                samplers.get_sampler(false, smoothed),
                &quad.texture_transforms,
                0 as wgpu::BufferAddress,
                self.texture.create_view(&Default::default()),
                create_debug_label!("Bitmap {:?} bind group (smoothed: {})", handle.0, smoothed),
            )
        })
    }
}

/// How many GPU objects were alive, for a crash report.
///
/// Deliberately not behind the metrics feature. The AMD device-loss crashes are reported against
/// normal release builds, and every report so far has arrived with no resource counts at all, so
/// three separate fixes shipped without anyone being able to see whether they moved the numbers on
/// the affected machine.
///
/// `memory_allocations` is the one to read first: it counts real driver allocations, and Vulkan
/// caps those per device. A driver can answer `VK_ERROR_OUT_OF_DEVICE_MEMORY` on that limit while
/// most of the card's memory is still free, which is exactly the shape these crashes have.
pub fn device_resource_report(device: &wgpu::Device) -> String {
    format_hal_counters(&device.get_internal_counters().hal)
}

fn format_hal_counters(hal: &wgpu::HalCounters) -> String {
    let counts = [
        ("textures", &hal.textures),
        ("texture views", &hal.texture_views),
        ("buffers", &hal.buffers),
        ("bind groups", &hal.bind_groups),
        ("bind group layouts", &hal.bind_group_layouts),
        ("samplers", &hal.samplers),
        ("render pipelines", &hal.render_pipelines),
        ("compute pipelines", &hal.compute_pipelines),
        ("pipeline layouts", &hal.pipeline_layouts),
        ("shader modules", &hal.shader_modules),
        ("command encoders", &hal.command_encoders),
        ("query sets", &hal.query_sets),
        ("fences", &hal.fences),
        ("memory allocations", &hal.memory_allocations),
    ];

    let mut report = String::from(
        "live GPU objects:
",
    );
    for (name, counter) in counts {
        report.push_str(&format!(
            "  {name:<20} {}
",
            counter.read()
        ));
    }

    report.push_str(&format!(
        "
GPU memory attributed by wgpu-hal:
  {:<20} {:.1} MB
  {:<20} {:.1} MB
",
        "textures",
        hal.texture_memory.read() as f64 / 1_048_576.0,
        "buffers",
        hal.buffer_memory.read() as f64 / 1_048_576.0,
    ));
    report
}

#[cfg(test)]
mod device_resource_report_tests {
    use super::format_hal_counters;

    /// Pins what a crash reader depends on. A `Device` cannot be built without a GPU, so this
    /// drives the formatter on the same type wgpu hands back.
    #[test]
    fn the_report_names_every_resource_class_and_its_memory() {
        let mut hal = wgpu::HalCounters::default();
        hal.textures.add(28_504);
        hal.bind_groups.add(29_240);
        hal.buffers.add(21_410);
        hal.memory_allocations.add(4_096);
        hal.texture_memory.add(7 * 1_048_576);

        let report = format_hal_counters(&hal);

        // The four numbers the AMD investigation actually turns on.
        assert!(report.contains("textures             28504"), "{report}");
        assert!(report.contains("bind groups          29240"), "{report}");
        assert!(report.contains("buffers              21410"), "{report}");
        assert!(
            report.contains("memory allocations   4096"),
            "driver allocation count is the suspected cap: {report}"
        );
        assert!(report.contains("7.0 MB"), "{report}");
    }

    #[test]
    fn a_healthy_device_reports_zeros_rather_than_nothing() {
        let report = format_hal_counters(&wgpu::HalCounters::default());

        assert!(report.contains("textures             0"), "{report}");
        assert!(report.contains("memory allocations   0"), "{report}");
    }
}
