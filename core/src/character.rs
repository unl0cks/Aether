use std::cell::{Cell, RefCell};

use crate::backend::audio::SoundHandle;
use crate::binary_data::BinaryData;
use crate::display_object::{
    Avm1Button, Avm2Button, BitmapClass, EditText, Graphic, MorphShape, MovieClip, Text, Video,
};
use crate::font::Font;
use gc_arena::barrier::unlock;
use gc_arena::lock::Lock;
use gc_arena::{Collect, Gc, Mutation};
use ruffle_render::backend::RenderBackend;
use ruffle_render::bitmap::{Bitmap as RenderBitmap, BitmapHandle, BitmapSize};
use ruffle_render::error::Error as RenderError;
use swf::DefineBitsLossless;

#[derive(Copy, Clone, Collect, Debug)]
#[collect(no_drop)]
pub enum Character<'gc> {
    EditText(EditText<'gc>),
    Graphic(Graphic<'gc>),
    MovieClip(MovieClip<'gc>),
    Bitmap(Gc<'gc, BitmapCharacter<'gc>>),
    Avm1Button(Avm1Button<'gc>),
    Avm2Button(Avm2Button<'gc>),
    Font(Font<'gc>),
    MorphShape(MorphShape<'gc>),
    Text(Text<'gc>),
    Sound(#[collect(require_static)] SoundHandle),
    Video(Video<'gc>),
    BinaryData(Gc<'gc, BinaryData>),
}

#[derive(Collect, Debug)]
#[collect(no_drop)]
pub struct BitmapCharacter<'gc> {
    #[collect(require_static)]
    compressed: CompressedBitmap,
    /// A lazily constructed GPU handle, used when performing fills with this bitmap.
    ///
    /// This is a cache, not ownership: [`Self::release_bitmap_handle`] may drop it at any
    /// time and the next request re-uploads from `compressed`. It has to be releasable
    /// because a movie's library lives as long as any `Arc<SwfMovie>`, and an application
    /// domain retains one for every script it has ever exported.
    #[collect(require_static)]
    handle: RefCell<Option<BitmapHandle>>,
    /// Whether this bitmap has been drawn since the last eviction sweep. Gives a cached
    /// upload one sweep of reprieve before it is dropped, so anything actually on screen
    /// survives while gear for departed players does not.
    #[collect(require_static)]
    drawn_since_sweep: Cell<bool>,
    /// The bitmap class set by `SymbolClass` - this is used when we instantaite
    /// a `Bitmap` displayobject.
    avm2_class: Lock<BitmapClass<'gc>>,
}

impl<'gc> BitmapCharacter<'gc> {
    pub fn new(compressed: CompressedBitmap) -> Self {
        Self {
            compressed,
            handle: RefCell::new(None),
            drawn_since_sweep: Cell::new(false),
            avm2_class: Lock::new(BitmapClass::NoSubclass),
        }
    }

    pub fn compressed(&self) -> &CompressedBitmap {
        &self.compressed
    }

    pub fn avm2_class(&self) -> BitmapClass<'gc> {
        self.avm2_class.get()
    }

    pub fn set_avm2_class(this: Gc<'gc, Self>, bitmap_class: BitmapClass<'gc>, mc: &Mutation<'gc>) {
        unlock!(Gc::write(mc, this), Self, avm2_class).set(bitmap_class);
    }

    pub fn bitmap_handle(
        &self,
        backend: &mut dyn RenderBackend,
    ) -> Result<BitmapHandle, RenderError> {
        self.drawn_since_sweep.set(true);
        if let Some(handle) = self.handle.borrow().as_ref() {
            return Ok(handle.clone());
        }
        let decoded = self.compressed.decode()?;
        let new_handle = backend.register_bitmap(decoded)?;
        *self.handle.borrow_mut() = Some(new_handle.clone());
        Ok(new_handle)
    }

    /// Second-chance eviction: drop this bitmap's cached GPU upload unless it has been drawn
    /// since the previous sweep. Returns whether an upload was released.
    ///
    /// Uploads happen lazily on first draw, so hidden content never uploads -- but nothing
    /// released them when it stopped being drawn again. Measured on AQW: revealing ten
    /// players took live textures from 296 to 2,443, and hiding them again freed none.
    pub fn sweep_idle_gpu_upload(&self) -> bool {
        if self.drawn_since_sweep.replace(false) {
            return false;
        }
        self.release_bitmap_handle()
    }

    /// Drop this bitmap's cached GPU upload, returning whether there was one.
    ///
    /// Safe to call at any time: the compressed source is retained, so the next request
    /// simply re-uploads.
    pub fn release_bitmap_handle(&self) -> bool {
        self.handle.borrow_mut().take().is_some()
    }

    /// Whether this bitmap is currently holding a GPU upload alongside its compressed source.
    pub fn has_bitmap_handle(&self) -> bool {
        self.handle.borrow().is_some()
    }
}

/// Holds a bitmap from an SWF tag, plus the decoded width/height.
/// We avoid decompressing the image until it's actually needed - some pathological SWFS
/// like 'House' have thousands of highly-compressed (mostly empty) bitmaps, which can
/// take over 10GB of ram if we decompress them all during preloading.
#[derive(Clone, Debug)]
pub enum CompressedBitmap {
    Jpeg {
        data: Vec<u8>,
        alpha: Option<Vec<u8>>,
        width: u32,
        height: u32,
    },
    Lossless(DefineBitsLossless<'static>),
}

impl CompressedBitmap {
    /// Bytes this source occupies on the Rust heap for as long as its library is resident.
    ///
    /// The compressed source is deliberately never dropped -- releasing a bitmap frees its GPU
    /// upload and re-decodes from here on the next draw -- so this is the part of a bitmap
    /// character that a session keeps paying for. `characters` alone cannot say whether tens of
    /// thousands of them amount to megabytes or gigabytes, which is the question the census exists
    /// to answer.
    pub fn resident_bytes(&self) -> usize {
        match self {
            CompressedBitmap::Jpeg { data, alpha, .. } => {
                data.len() + alpha.as_ref().map_or(0, Vec::len)
            }
            CompressedBitmap::Lossless(define_bits_lossless) => define_bits_lossless.data.len(),
        }
    }

    pub fn size(&self) -> BitmapSize {
        match self {
            CompressedBitmap::Jpeg { width, height, .. } => BitmapSize {
                width: *width,
                height: *height,
            },
            CompressedBitmap::Lossless(define_bits_lossless) => BitmapSize {
                width: define_bits_lossless.width.into(),
                height: define_bits_lossless.height.into(),
            },
        }
    }
    pub fn decode(&self) -> Result<RenderBitmap<'static>, RenderError> {
        match self {
            CompressedBitmap::Jpeg {
                data,
                alpha,
                width: _,
                height: _,
            } => ruffle_render::utils::decode_define_bits_jpeg(data, alpha.as_deref()),
            CompressedBitmap::Lossless(define_bits_lossless) => {
                ruffle_render::utils::decode_define_bits_lossless(define_bits_lossless)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruffle_render::backend::{ViewportDimensions, null::NullRenderer};
    use std::borrow::Cow;
    use swf::BitmapFormat;

    fn one_pixel_bitmap() -> CompressedBitmap {
        use flate2::{Compression, write::ZlibEncoder};
        use std::io::Write;

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&[0xff, 0x11, 0x22, 0x33]).unwrap();
        CompressedBitmap::Lossless(DefineBitsLossless {
            version: 2,
            id: 1,
            format: BitmapFormat::Rgb32,
            width: 1,
            height: 1,
            data: Cow::Owned(encoder.finish().unwrap()),
        })
    }

    #[test]
    fn an_idle_gpu_upload_is_evicted_but_a_drawn_one_is_spared() {
        // Bitmaps are only released on Loader.unload, and AQW drops Loaders instead of
        // reusing them, so most uploads were never reachable by that path. A second-chance
        // clock evicts anything not drawn since the previous sweep, regardless of whether
        // the owning movie is ever unloaded.
        let mut renderer = NullRenderer::new(ViewportDimensions {
            width: 100,
            height: 100,
            scale_factor: 1.0,
        });
        let character = BitmapCharacter::new(one_pixel_bitmap());
        character.bitmap_handle(&mut renderer).expect("upload");

        // Drawn since the last sweep, so it is spared and its reprieve is consumed.
        assert!(!character.sweep_idle_gpu_upload(), "recently drawn");
        // Still untouched on the next sweep, so it goes.
        assert!(
            character.sweep_idle_gpu_upload(),
            "idle upload must be evicted"
        );
        assert!(!character.sweep_idle_gpu_upload(), "nothing left to evict");

        // Drawing again re-uploads and re-arms the reprieve.
        character.bitmap_handle(&mut renderer).expect("re-upload");
        assert!(!character.sweep_idle_gpu_upload());
        assert!(character.sweep_idle_gpu_upload());
    }

    #[test]
    fn a_bitmap_character_can_release_its_cached_gpu_upload() {
        // Every bitmap in every loaded SWF lazily uploads a GPU texture and caches it here.
        // AQW exports each loaded SWF's scripts into a shared application domain, and those
        // entries are never removed, so the movie -- and this cached upload -- lives for the
        // life of the process. Measured: 4,396 live textures after 100 seconds.
        let mut renderer = NullRenderer::new(ViewportDimensions {
            width: 100,
            height: 100,
            scale_factor: 1.0,
        });
        let character = BitmapCharacter::new(one_pixel_bitmap());
        assert!(!character.release_bitmap_handle(), "nothing cached yet");

        character
            .bitmap_handle(&mut renderer)
            .expect("first upload");
        assert!(
            character.release_bitmap_handle(),
            "a cached upload must be releasable"
        );
        assert!(!character.release_bitmap_handle(), "already released");

        // The compressed source is retained, so anything still drawing re-uploads on demand.
        character.bitmap_handle(&mut renderer).expect("re-upload");
        assert!(character.release_bitmap_handle());
    }
}
