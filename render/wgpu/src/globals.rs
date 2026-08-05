//use super::utils::create_debug_label;
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

#[derive(Debug)]
pub struct Globals {
    bind_group: wgpu::BindGroup,
    _buffer: wgpu::Buffer,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct GlobalsUniform {
    view_matrix: [[f32; 4]; 4],
}

/// The pixels-to-NDC matrix for a render target covering `[offset_x, offset_x + width]` by
/// `[offset_y, offset_y + height]` of the space the draw commands were recorded in.
///
/// The offset is what lets a blend or mask render into a sub-target sized to its contents rather
/// than to the whole stage: the commands keep their original coordinates, and this matrix moves the
/// window instead of moving them. With a zero offset it is exactly the old full-surface matrix.
pub fn region_view_matrix(offset_x: u32, offset_y: u32, width: u32, height: u32) -> [[f32; 4]; 4] {
    let w = (width.max(1) as f32) / 2.0;
    let h = (height.max(1) as f32) / 2.0;
    [
        [1.0 / w, 0.0, 0.0, 0.0],
        [0.0, -1.0 / h, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [
            -1.0 - offset_x as f32 / w,
            1.0 + offset_y as f32 / h,
            0.0,
            1.0,
        ],
    ]
}

impl Globals {
    pub fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        offset_x: u32,
        offset_y: u32,
        viewport_width: u32,
        viewport_height: u32,
    ) -> Self {
        let temp_label = create_debug_label!("Globals buffer");
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: temp_label.as_deref(),
            contents: bytemuck::cast_slice(&[GlobalsUniform {
                view_matrix: region_view_matrix(
                    offset_x,
                    offset_y,
                    viewport_width,
                    viewport_height,
                ),
            }]),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let bind_group_label = create_debug_label!("Globals bind group");
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: bind_group_label.as_deref(),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });

        Self {
            bind_group,
            _buffer: buffer,
        }
    }

    pub fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }
}

#[cfg(test)]
mod tests {
    use super::region_view_matrix;

    /// Apply the (column-major, row-vector) view matrix to a pixel coordinate.
    fn to_ndc(m: &[[f32; 4]; 4], x: f32, y: f32) -> (f32, f32) {
        (x * m[0][0] + y * m[1][0] + m[3][0], x * m[0][1] + y * m[1][1] + m[3][1])
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn a_full_surface_maps_its_corners_to_the_whole_clip_volume() {
        let m = region_view_matrix(0, 0, 2560, 1440);

        let (x, y) = to_ndc(&m, 0.0, 0.0);
        assert!(close(x, -1.0) && close(y, 1.0), "top-left was ({x}, {y})");
        let (x, y) = to_ndc(&m, 2560.0, 1440.0);
        assert!(close(x, 1.0) && close(y, -1.0), "bottom-right was ({x}, {y})");
    }

    #[test]
    fn an_offset_region_maps_its_own_corners_to_the_whole_clip_volume() {
        // The point of the offset: commands keep their stage coordinates, and a target covering
        // only [400..480] x [300..360] still fills itself with exactly that slice.
        let m = region_view_matrix(400, 300, 80, 60);

        let (x, y) = to_ndc(&m, 400.0, 300.0);
        assert!(close(x, -1.0) && close(y, 1.0), "region top-left was ({x}, {y})");
        let (x, y) = to_ndc(&m, 480.0, 360.0);
        assert!(close(x, 1.0) && close(y, -1.0), "region bottom-right was ({x}, {y})");
        // And the region's centre lands in the middle, not off to one side.
        let (x, y) = to_ndc(&m, 440.0, 330.0);
        assert!(close(x, 0.0) && close(y, 0.0), "region centre was ({x}, {y})");
    }

    #[test]
    fn a_zero_offset_region_is_the_old_full_surface_matrix() {
        assert_eq!(region_view_matrix(0, 0, 800, 600), [
            [1.0 / 400.0, 0.0, 0.0, 0.0],
            [0.0, -1.0 / 300.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [-1.0, 1.0, 0.0, 1.0],
        ]);
    }
}
