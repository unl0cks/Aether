use crate::buffer_pool::BufferDescription;
use crate::descriptors::Descriptors;
use crate::globals::Globals;
use std::borrow::Cow;
use wgpu::util::DeviceExt;
use wgpu::{CommandEncoder, TextureFormat};

macro_rules! create_debug_label {
    ($($arg:tt)*) => (
        if cfg!(feature = "render_debug_labels") {
            Some(format!($($arg)*))
        } else {
            None
        }
    )
}

pub fn remove_srgb(format: wgpu::TextureFormat) -> wgpu::TextureFormat {
    match format {
        wgpu::TextureFormat::Rgba8UnormSrgb => wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Bgra8UnormSrgb => wgpu::TextureFormat::Bgra8Unorm,
        wgpu::TextureFormat::Bc1RgbaUnormSrgb => wgpu::TextureFormat::Bc1RgbaUnorm,
        wgpu::TextureFormat::Bc2RgbaUnormSrgb => wgpu::TextureFormat::Bc2RgbaUnorm,
        wgpu::TextureFormat::Bc3RgbaUnormSrgb => wgpu::TextureFormat::Bc3RgbaUnorm,
        wgpu::TextureFormat::Bc7RgbaUnormSrgb => wgpu::TextureFormat::Bc7RgbaUnorm,
        wgpu::TextureFormat::Etc2Rgb8UnormSrgb => wgpu::TextureFormat::Etc2Rgb8Unorm,
        wgpu::TextureFormat::Etc2Rgb8A1UnormSrgb => wgpu::TextureFormat::Etc2Rgb8A1Unorm,
        wgpu::TextureFormat::Etc2Rgba8UnormSrgb => wgpu::TextureFormat::Etc2Rgba8Unorm,
        wgpu::TextureFormat::Astc {
            block,
            channel: wgpu::AstcChannel::UnormSrgb,
        } => wgpu::TextureFormat::Astc {
            block,
            channel: wgpu::AstcChannel::Unorm,
        },
        _ => format,
    }
}

pub fn format_list<'a>(values: &[&'a str], connector: &'a str) -> Cow<'a, str> {
    match values.len() {
        0 => Cow::Borrowed(""),
        1 => Cow::Borrowed(values[0]),
        _ => Cow::Owned(
            values[0..values.len() - 1].join(", ")
                + " "
                + connector
                + " "
                + values[values.len() - 1],
        ),
    }
}

pub fn get_backend_names(backends: wgpu::Backends) -> Vec<&'static str> {
    let mut names = Vec::new();

    if backends.contains(wgpu::Backends::VULKAN) {
        names.push("Vulkan");
    }
    if backends.contains(wgpu::Backends::DX12) {
        names.push("DirectX 12");
    }
    if backends.contains(wgpu::Backends::METAL) {
        names.push("Metal");
    }
    if backends.contains(wgpu::Backends::GL) {
        names.push("Open GL");
    }
    if backends.contains(wgpu::Backends::BROWSER_WEBGPU) {
        names.push("Web GPU");
    }

    names
}

pub fn create_buffer_with_data(
    device: &wgpu::Device,
    data: &[u8],
    usage: wgpu::BufferUsages,
    label: Option<String>,
) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        usage,
        label: label.as_deref(),
        contents: data,
    })
}

// Based off wgpu example 'capture'
#[derive(Debug, Clone)]
pub struct BufferDimensions {
    pub width: usize,
    pub height: usize,
    pub unpadded_bytes_per_row: usize,
    pub padded_bytes_per_row: u32,
}

impl BufferDimensions {
    pub fn new(width: usize, height: usize, format: TextureFormat) -> Self {
        let bytes_per_pixel = format.block_copy_size(None).unwrap() as usize;
        let unpadded_bytes_per_row = width * bytes_per_pixel;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
        let padded_bytes_per_row_padding = (align - unpadded_bytes_per_row % align) % align;
        let padded_bytes_per_row = (unpadded_bytes_per_row + padded_bytes_per_row_padding) as u32;

        Self {
            width,
            height,
            unpadded_bytes_per_row,
            padded_bytes_per_row,
        }
    }

    pub fn size(&self) -> u64 {
        self.padded_bytes_per_row as u64 * self.height as u64
    }
}

impl BufferDescription for BufferDimensions {
    type Cost = u64;

    fn cost_to_use(&self, other: &Self) -> Option<Self::Cost> {
        if self.size() <= other.size() {
            Some(other.size() - self.size())
        } else {
            None
        }
    }
}

/// Whether a buffer that was submitted for readback is actually safe to map.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BufferReadback {
    /// The buffer is mapped and can be read.
    Ready,
    /// Waiting for the copy to finish failed, so the contents are not ready.
    PollFailed,
    /// The map itself failed, which normally means the device was lost.
    MapFailed,
    /// The map callback never fired, which means the device was dropped underneath us.
    MapAbandoned,
}

/// Decide whether a readback buffer may be mapped.
///
/// This has to be decided *before* calling [`wgpu::BufferSlice::get_mapped_range`],
/// because that call routes failures through wgpu's `handle_error_fatal`, which
/// panics unconditionally and cannot be intercepted by an error scope or by an
/// uncaptured-error handler. Reading a buffer whose map failed is therefore an
/// immediate, unrecoverable process abort.
pub fn buffer_readback(
    poll: &Result<wgpu::PollStatus, wgpu::PollError>,
    map: &Result<Result<(), wgpu::BufferAsyncError>, std::sync::mpsc::RecvError>,
) -> BufferReadback {
    if poll.is_err() {
        return BufferReadback::PollFailed;
    }
    match map {
        Ok(Ok(())) => BufferReadback::Ready,
        Ok(Err(_)) => BufferReadback::MapFailed,
        Err(_) => BufferReadback::MapAbandoned,
    }
}

pub fn capture_image<R, F: FnOnce(&[u8], u32) -> R>(
    device: &wgpu::Device,
    buffer: &wgpu::Buffer,
    dimensions: &BufferDimensions,
    index: Option<wgpu::SubmissionIndex>,
    with_rgba: F,
) -> Option<R> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let buffer_slice = buffer.slice(..);
    buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
        // If nothing is waiting any more the readback has been abandoned, which is
        // handled below. Unwrapping here would panic on wgpu's callback thread.
        let _ = sender.send(result);
    });
    let poll = device.poll(wgpu::PollType::Wait {
        submission_index: index,
        timeout: None,
    });
    let map = match &poll {
        // The wait succeeded, so the map callback is guaranteed to have run.
        Ok(_) => receiver.recv(),
        // The wait failed, which usually means the device is gone. The callback may
        // never fire, so take whatever is already there rather than blocking forever.
        Err(_) => receiver.try_recv().map_err(|_| std::sync::mpsc::RecvError),
    };

    let readback = buffer_readback(&poll, &map);
    if readback != BufferReadback::Ready {
        // If the map itself succeeded we still hold the mapping, so release it before
        // the buffer goes back to the pool, or the next user of it will fail to map.
        if matches!(map, Ok(Ok(()))) {
            buffer.unmap();
        }
        tracing::warn!("Skipping GPU readback: {readback:?}");
        return None;
    }

    let map = buffer_slice.get_mapped_range();
    let result = with_rgba(&map, dimensions.padded_bytes_per_row);
    drop(map);
    buffer.unmap();
    Some(result)
}

#[cfg(not(target_family = "wasm"))]
pub fn buffer_to_image(
    device: &wgpu::Device,
    buffer: &wgpu::Buffer,
    dimensions: &BufferDimensions,
    index: Option<wgpu::SubmissionIndex>,
    size: wgpu::Extent3d,
) -> Option<image::RgbaImage> {
    capture_image(device, buffer, dimensions, index, |rgba, _buffer_width| {
        let mut bytes = Vec::with_capacity(dimensions.height * dimensions.unpadded_bytes_per_row);

        for chunk in rgba.chunks(dimensions.padded_bytes_per_row as usize) {
            bytes.extend_from_slice(&chunk[..dimensions.unpadded_bytes_per_row]);
        }

        // The image copied from the GPU uses premultiplied alpha, so
        // convert to straight alpha if requested by the user.
        ruffle_render::utils::unmultiply_alpha_rgba(&mut bytes);

        image::RgbaImage::from_raw(size.width, size.height, bytes)
            .expect("Retrieved texture buffer must be a valid RgbaImage")
    })
}

pub fn supported_sample_count(
    adapter: &wgpu::Adapter,
    mut sample_count: u32,
    format: wgpu::TextureFormat,
) -> u32 {
    let features = adapter.get_texture_format_features(format).flags;

    // Keep halving the sample count until we get one that's supported - or 1 (no multisampling)
    // It's not guaranteed that supporting 4x means supporting 2x, so there's no "max" option
    // And it's probably safer to round down than up, given it's a performance setting.
    while sample_count > 1 && !features.sample_count_supported(sample_count) {
        sample_count /= 2;
    }
    sample_count
}

#[expect(clippy::too_many_arguments)]
pub fn run_copy_pipeline(
    descriptors: &Descriptors,
    format: wgpu::TextureFormat,
    frame_view: &wgpu::TextureView,
    input: &wgpu::TextureView,
    whole_frame_bind_group: &wgpu::BindGroup,
    globals: &Globals,
    sample_count: u32,
    encoder: &mut CommandEncoder,
) {
    let copy_bind_group = descriptors
        .device
        .create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &descriptors.bind_layouts.bitmap,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: descriptors.quad.texture_transforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(input),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(
                        descriptors.bitmap_samplers.get_sampler(false, false),
                    ),
                },
            ],
            label: create_debug_label!("Copy bind group").as_deref(),
        });

    let pipeline = descriptors.copy_pipeline(format, sample_count);

    // We overwrite the pixels in the target texture (no blending at all),
    // so this doesn't matter.
    let load = wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT);

    let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: create_debug_label!("Copy back to render target").as_deref(),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: frame_view,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
            resolve_target: None,
            depth_slice: None,
        })],
        ..Default::default()
    });

    render_pass.set_pipeline(&pipeline);
    render_pass.set_bind_group(0, globals.bind_group(), &[]);

    render_pass.set_bind_group(1, whole_frame_bind_group, &[0]);
    render_pass.set_bind_group(2, &copy_bind_group, &[]);

    render_pass.set_vertex_buffer(0, descriptors.quad.vertices_pos.slice(..));
    render_pass.set_index_buffer(
        descriptors.quad.indices.slice(..),
        wgpu::IndexFormat::Uint32,
    );

    render_pass.draw_indexed(0..6, 0, 0..1);
    drop(render_pass);
}

#[derive(Debug)]
pub struct SampleCountMap<T> {
    one: T,
    two: T,
    four: T,
    eight: T,
    sixteen: T,
}

impl<T: Default> Default for SampleCountMap<T> {
    fn default() -> Self {
        SampleCountMap {
            one: Default::default(),
            two: Default::default(),
            four: Default::default(),
            eight: Default::default(),
            sixteen: Default::default(),
        }
    }
}

impl<T> SampleCountMap<T> {
    pub fn get(&self, sample_count: u32) -> &T {
        match sample_count {
            1 => &self.one,
            2 => &self.two,
            4 => &self.four,
            8 => &self.eight,
            16 => &self.sixteen,
            _ => unreachable!("Sample counts must be powers of two between 1..=16"),
        }
    }
}

impl<T> SampleCountMap<std::sync::OnceLock<T>> {
    pub fn get_or_init<F>(&self, sample_count: u32, init: F) -> &T
    where
        F: FnOnce() -> T,
    {
        match sample_count {
            1 => self.one.get_or_init(init),
            2 => self.two.get_or_init(init),
            4 => self.four.get_or_init(init),
            8 => self.eight.get_or_init(init),
            16 => self.sixteen.get_or_init(init),
            _ => unreachable!("Sample counts must be powers of two between 1..=16"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BufferReadback, buffer_readback};
    use std::sync::mpsc::RecvError;

    #[test]
    fn successful_map_is_ready() {
        assert_eq!(
            buffer_readback(&Ok(wgpu::PollStatus::QueueEmpty), &Ok(Ok(()))),
            BufferReadback::Ready
        );
    }

    #[test]
    fn failed_map_is_not_readable() {
        assert_eq!(
            buffer_readback(
                &Ok(wgpu::PollStatus::QueueEmpty),
                &Ok(Err(wgpu::BufferAsyncError))
            ),
            BufferReadback::MapFailed
        );
    }

    #[test]
    fn abandoned_callback_is_not_readable() {
        assert_eq!(
            buffer_readback(&Ok(wgpu::PollStatus::QueueEmpty), &Err(RecvError)),
            BufferReadback::MapAbandoned
        );
    }

    #[test]
    fn poll_failure_takes_priority_over_the_map_result() {
        assert_eq!(
            buffer_readback(&Err(wgpu::PollError::Timeout), &Ok(Ok(()))),
            BufferReadback::PollFailed
        );
    }
}
