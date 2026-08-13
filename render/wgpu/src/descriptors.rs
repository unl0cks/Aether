use crate::device_status::{DeviceStatus, fault_from_device_lost, fault_from_uncaptured_error};
use crate::filters::{FilterVertex, Filters};
use crate::layouts::BindLayouts;
use crate::pipelines::VERTEX_BUFFERS_DESCRIPTION_POS;
use crate::shaders::Shaders;
use crate::{
    BitmapSamplers, Pipelines, PosColorVertex, PosVertex, TextureTransforms, Transforms,
    create_buffer_with_data,
};
use fnv::FnvHashMap;
use std::fmt::Debug;
use std::sync::{Arc, Mutex};
use wgpu::Backend;

pub struct Descriptors {
    pub wgpu_instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub limits: wgpu::Limits,
    pub backend: Backend,
    pub queue: wgpu::Queue,
    pub bitmap_samplers: BitmapSamplers,
    pub bind_layouts: BindLayouts,
    pub quad: Quad,
    copy_pipeline: Mutex<FnvHashMap<(u32, wgpu::TextureFormat), wgpu::RenderPipeline>>,
    pub shaders: Shaders,
    pipelines: Mutex<FnvHashMap<(u32, wgpu::TextureFormat), Arc<Pipelines>>>,
    /// Transform bind groups for a blend that covers part of its target, kept by the four numbers
    /// that decide their contents.
    ///
    /// Each one used to be a fresh uniform buffer *and* a fresh bind group, built again on every
    /// frame for every blend -- measured at 448 blends in one frame of a crowded room, so 448 of
    /// each, per frame. Both are among the more expensive things wgpu is asked to do, and neither
    /// depends on anything but the region and the texture behind it. Avatars repeat a handful of
    /// sizes between them, so the same few groups serve nearly all of them.
    region_frame_bind_groups: Mutex<FnvHashMap<[u32; 8], Arc<wgpu::BindGroup>>>,
    pub filters: Filters,
    /// Records the first fatal GPU fault so callers can shut down cleanly.
    pub device_status: DeviceStatus,
}

/// Route wgpu's device-loss and uncaptured-error channels into `status`.
///
/// Without these, wgpu discards device-loss notifications entirely and turns every
/// uncaptured error into a panic from inside its own default handler.
fn install_device_fault_handlers(device: &wgpu::Device, status: &DeviceStatus) {
    let lost_status = status.clone();
    device.set_device_lost_callback(move |reason, message| {
        if let Some(fault) = fault_from_device_lost(reason, &message)
            && lost_status.record(fault)
        {
            tracing::error!("Graphics device lost: {message}");
            #[cfg(feature = "aether_metrics")]
            for line in crate::aether_metrics::texture_census_report(14) {
                tracing::error!("{line}");
            }
        }
    });

    let error_status = status.clone();
    device.on_uncaptured_error(Arc::new(move |error| {
        let fault = fault_from_uncaptured_error(&error);
        if error_status.record(fault) {
            tracing::error!("Unrecoverable graphics error: {error}");
            #[cfg(feature = "aether_metrics")]
            for line in crate::aether_metrics::texture_census_report(14) {
                tracing::error!("{line}");
            }
        }
    }));
}

impl Debug for Descriptors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Descriptors")
    }
}

impl Descriptors {
    pub fn new(
        instance: wgpu::Instance,
        adapter: wgpu::Adapter,
        device: wgpu::Device,
        queue: wgpu::Queue,
    ) -> Self {
        let device_status = DeviceStatus::new();
        install_device_fault_handlers(&device, &device_status);

        let limits = device.limits();
        let bind_layouts = BindLayouts::new(&device);
        let bitmap_samplers = BitmapSamplers::new(&device);
        let shaders = Shaders::new(&device);
        let quad = Quad::new(&device);
        let filters = Filters::new(&device);
        let backend = adapter.get_info().backend;

        Self {
            wgpu_instance: instance,
            adapter,
            device,
            limits,
            backend,
            queue,
            bitmap_samplers,
            bind_layouts,
            quad,
            copy_pipeline: Default::default(),
            shaders,
            pipelines: Default::default(),
            region_frame_bind_groups: Default::default(),
            filters,
            device_status,
        }
    }

    /// A transform bind group for a partial-region blend, built once per distinct shape.
    ///
    /// Keyed on every number that reaches the buffer -- the region's size and corner, and the four
    /// UV terms -- because all of them vary per blend. Keyed on the bits rather than the floats
    /// because floats are not `Hash`; each one is derived from an integer pixel count, so equal
    /// regions always produce an identical key.
    ///
    /// Blend regions are snapped to a grid, so an avatar that moves a few pixels keeps the same
    /// region for several frames and keeps hitting this.
    pub fn region_frame_bind_group(
        &self,
        world_size: [f32; 2],
        world_corner: [f32; 2],
        uv_scale: [f32; 4],
    ) -> Arc<wgpu::BindGroup> {
        let key = [
            world_size[0].to_bits(),
            world_size[1].to_bits(),
            world_corner[0].to_bits(),
            world_corner[1].to_bits(),
            uv_scale[0].to_bits(),
            uv_scale[1].to_bits(),
            uv_scale[2].to_bits(),
            uv_scale[3].to_bits(),
        ];
        if let Some(group) = self.region_frame_bind_groups.lock().unwrap().get(&key) {
            return group.clone();
        }

        #[cfg(feature = "aether_metrics")]
        crate::aether_metrics::record_bind_group_created();

        let transform = Transforms {
            world_matrix: [
                [world_size[0], 0.0, 0.0, 0.0],
                [0.0, world_size[1], 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [world_corner[0], world_corner[1], 0.0, 1.0],
            ],
            mult_color: [1.0, 1.0, 1.0, 1.0],
            add_color: [0.0, 0.0, 0.0, 0.0],
            bitmap_uv_scale: uv_scale,
        };

        let buffer = create_buffer_with_data(
            &self.device,
            bytemuck::cast_slice(&[transform]),
            wgpu::BufferUsages::UNIFORM,
            create_debug_label!("Region-frame transforms buffer"),
        );
        let group = Arc::new(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &self.bind_layouts.transforms,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
            label: create_debug_label!("Region-frame transforms bind group").as_deref(),
        }));

        // A crowded room settles on a few hundred distinct regions. This only bounds a
        // pathological case; starting over is cheaper than tracking use counts for something this
        // small, and the groups rebuild in a frame.
        const MAX_CACHED: usize = 4096;
        let mut cache = self.region_frame_bind_groups.lock().unwrap();
        if cache.len() >= MAX_CACHED {
            cache.clear();
        }
        cache.insert(key, group.clone());
        group
    }

    pub fn copy_pipeline(
        &self,
        format: wgpu::TextureFormat,
        msaa_sample_count: u32,
    ) -> wgpu::RenderPipeline {
        let mut pipelines = self
            .copy_pipeline
            .lock()
            .expect("Pipelines should not be already locked");
        pipelines
            .entry((msaa_sample_count, format))
            .or_insert_with(|| {
                let copy_texture_pipeline_layout =
                    &self
                        .device
                        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                            label: create_debug_label!("Copy pipeline layout").as_deref(),
                            bind_group_layouts: &[
                                &self.bind_layouts.globals,
                                &self.bind_layouts.transforms,
                                &self.bind_layouts.bitmap,
                            ],
                            push_constant_ranges: &[],
                        });
                self.device
                    .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: create_debug_label!("Copy pipeline").as_deref(),
                        layout: Some(copy_texture_pipeline_layout),
                        vertex: wgpu::VertexState {
                            module: &self.shaders.copy_shader,
                            entry_point: Some("main_vertex"),
                            buffers: &VERTEX_BUFFERS_DESCRIPTION_POS,
                            compilation_options: Default::default(),
                        },
                        fragment: Some(wgpu::FragmentState {
                            module: &self.shaders.copy_shader,
                            entry_point: Some("main_fragment"),
                            targets: &[Some(wgpu::ColorTargetState {
                                format,
                                // All of our blending has been done by now, so we want
                                // to overwrite the target pixels without any blending
                                blend: Some(wgpu::BlendState::REPLACE),
                                write_mask: Default::default(),
                            })],
                            compilation_options: Default::default(),
                        }),
                        primitive: wgpu::PrimitiveState {
                            topology: wgpu::PrimitiveTopology::TriangleList,
                            strip_index_format: None,
                            front_face: wgpu::FrontFace::Ccw,
                            cull_mode: None,
                            polygon_mode: wgpu::PolygonMode::default(),
                            unclipped_depth: false,
                            conservative: false,
                        },
                        depth_stencil: None,
                        multisample: wgpu::MultisampleState {
                            count: msaa_sample_count,
                            mask: !0,
                            alpha_to_coverage_enabled: false,
                        },
                        multiview: None,
                        cache: None,
                    })
            })
            .clone()
    }

    pub fn pipelines(&self, msaa_sample_count: u32, format: wgpu::TextureFormat) -> Arc<Pipelines> {
        let mut pipelines = self
            .pipelines
            .lock()
            .expect("Pipelines should not be already locked");
        pipelines
            .entry((msaa_sample_count, format))
            .or_insert_with(|| {
                Arc::new(Pipelines::new(
                    &self.device,
                    &self.shaders,
                    format,
                    msaa_sample_count,
                    &self.bind_layouts,
                ))
            })
            .clone()
    }
}

pub struct Quad {
    pub vertices_pos: wgpu::Buffer,
    pub vertices_pos_color: wgpu::Buffer,
    pub filter_vertices: wgpu::Buffer,
    pub indices: wgpu::Buffer,
    pub indices_line: wgpu::Buffer,
    pub indices_line_rect: wgpu::Buffer,
    pub texture_transforms: wgpu::Buffer,
}

impl Quad {
    pub fn new(device: &wgpu::Device) -> Self {
        let vertices_pos = [
            PosVertex {
                position: [0.0, 0.0],
            },
            PosVertex {
                position: [1.0, 0.0],
            },
            PosVertex {
                position: [1.0, 1.0],
            },
            PosVertex {
                position: [0.0, 1.0],
            },
        ];
        let vertices_pos_color = [
            PosColorVertex {
                position: [0.0, 0.0],
                color: [1.0, 1.0, 1.0, 1.0],
            },
            PosColorVertex {
                position: [1.0, 0.0],
                color: [1.0, 1.0, 1.0, 1.0],
            },
            PosColorVertex {
                position: [1.0, 1.0],
                color: [1.0, 1.0, 1.0, 1.0],
            },
            PosColorVertex {
                position: [0.0, 1.0],
                color: [1.0, 1.0, 1.0, 1.0],
            },
        ];
        let filter_vertices = [
            FilterVertex {
                position: [0.0, 0.0],
                uv: [0.0, 0.0],
            },
            FilterVertex {
                position: [1.0, 0.0],
                uv: [1.0, 0.0],
            },
            FilterVertex {
                position: [1.0, 1.0],
                uv: [1.0, 1.0],
            },
            FilterVertex {
                position: [0.0, 1.0],
                uv: [0.0, 1.0],
            },
        ];
        let indices: [u32; 6] = [0, 1, 2, 0, 2, 3];
        let indices_line: [u32; 2] = [0, 1];
        let indices_line_rect: [u32; 5] = [0, 1, 2, 3, 0];

        let vbo_pos = create_buffer_with_data(
            device,
            bytemuck::cast_slice(&vertices_pos),
            wgpu::BufferUsages::VERTEX,
            create_debug_label!("Quad vbo (pos)"),
        );

        let vbo_pos_color = create_buffer_with_data(
            device,
            bytemuck::cast_slice(&vertices_pos_color),
            wgpu::BufferUsages::VERTEX,
            create_debug_label!("Quad vbo (pos & color)"),
        );

        let vbo_filter = create_buffer_with_data(
            device,
            bytemuck::cast_slice(&filter_vertices),
            wgpu::BufferUsages::VERTEX,
            create_debug_label!("Quad vbo (filter)"),
        );

        let ibo = create_buffer_with_data(
            device,
            bytemuck::cast_slice(&indices),
            wgpu::BufferUsages::INDEX,
            create_debug_label!("Quad ibo"),
        );
        let ibo_line = create_buffer_with_data(
            device,
            bytemuck::cast_slice(&indices_line),
            wgpu::BufferUsages::INDEX,
            create_debug_label!("Line ibo"),
        );
        let ibo_line_rect = create_buffer_with_data(
            device,
            bytemuck::cast_slice(&indices_line_rect),
            wgpu::BufferUsages::INDEX,
            create_debug_label!("Line rect ibo"),
        );

        let tex_transforms = create_buffer_with_data(
            device,
            bytemuck::cast_slice(&[TextureTransforms {
                u_matrix: [
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                    [0.0, 0.0, 0.0, 1.0],
                ],
            }]),
            wgpu::BufferUsages::UNIFORM,
            create_debug_label!("Quad tex transforms"),
        );

        Self {
            vertices_pos: vbo_pos,
            vertices_pos_color: vbo_pos_color,
            filter_vertices: vbo_filter,
            indices: ibo,
            indices_line: ibo_line,
            indices_line_rect: ibo_line_rect,
            texture_transforms: tex_transforms,
        }
    }
}
