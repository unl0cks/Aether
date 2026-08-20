//! SWF decoding support

use swf::{CharacterId, TagCode};
use thiserror::Error;

pub use ruffle_common::tag_utils::{SwfMovie, SwfSlice, SwfStream};

#[derive(Error, Debug)]
pub enum Error {
    #[error("Couldn't read SWF: {0}")]
    InvalidSwf(#[from] swf::error::Error),

    #[error("Couldn't register bitmap: {0}")]
    InvalidBitmap(#[from] ruffle_render::error::Error),

    #[error("Couldn't register font: {0}")]
    InvalidFont(#[from] ttf_parser::FaceParsingError),

    #[error("Attempted to preload video frames into non-video character {0}")]
    PreloadVideoIntoInvalidCharacter(CharacterId),

    #[error("IO Error: {0}")]
    IOError(#[from] std::io::Error),

    #[error("Invalid SWF url")]
    InvalidSwfUrl,
}

/// Whether or not to end tag decoding.
pub enum ControlFlow {
    /// Stop decoding after this tag.
    Exit,

    /// Continue decoding the next tag.
    Continue,
}

/// Decode tags from a SWF stream reader.
///
/// The given `tag_callback` will be called for each decoded tag. It will be
/// provided with the stream to read from, the tag code read, and the tag's
/// size. The callback is responsible for (optionally) parsing the contents of
/// the tag; otherwise, it will be skipped.
///
/// Decoding will terminate when the following conditions occur:
///
///  * The `tag_callback` calls for the decoding to finish.
///  * The decoder encounters a tag longer than the underlying SWF slice
///    (indicated by returning false)
///  * The SWF stream is otherwise corrupt or unreadable (indicated as an error
///    result)
///
/// Decoding will also log tags longer than the SWF slice, error messages
/// yielded from the tag callback, and unknown tags. It will *only* return an
/// error message if the SWF tag itself could not be parsed. Other forms of
/// irregular decoding will be signalled by returning false.
pub fn decode_tags<'a, F>(reader: &mut SwfStream<'a>, mut tag_callback: F) -> Result<bool, Error>
where
    F: for<'b> FnMut(&'b mut SwfStream<'a>, TagCode) -> Result<ControlFlow, Error>,
{
    loop {
        let (tag_code, tag_len) = reader.read_tag_code_and_length()?;
        if tag_len > reader.get_ref().len() {
            tracing::error!("Unexpected EOF when reading tag");
            *reader.get_mut() = &reader.get_ref()[reader.get_ref().len()..];
            return Ok(false);
        }

        let tag_slice = &reader.get_ref()[..tag_len];
        let end_slice = &reader.get_ref()[tag_len..];
        if let Some(tag) = TagCode::from_u16(tag_code) {
            *reader.get_mut() = tag_slice;
            let result = tag_callback(reader, tag);

            match result {
                Err(e) => {
                    tracing::error!("Error running definition tag: {:?}, got {}", tag, e)
                }
                Ok(ControlFlow::Exit) => {
                    *reader.get_mut() = end_slice;
                    break;
                }
                Ok(ControlFlow::Continue) => {}
            }
        } else {
            tracing::warn!("Unknown tag code: {:?}", tag_code);
        }

        *reader.get_mut() = end_slice;
    }

    Ok(true)
}

/// Utility method to construct a movie from a file on disk.
#[cfg(any(unix, windows, target_os = "redox"))]
pub fn movie_from_path<P: AsRef<std::path::Path>>(
    path: P,
    loader_url: Option<String>,
) -> Result<SwfMovie, Error> {
    let data = std::fs::read(&path)?;

    let abs_path = path.as_ref().canonicalize()?;
    let url = url::Url::from_file_path(abs_path).map_err(|()| Error::InvalidSwfUrl)?;

    SwfMovie::from_data(&data, url.into(), loader_url).map_err(Error::InvalidSwf)
}

#[cfg(test)]
mod decode_tags_tests {
    use super::*;

    /// A tag header is a u16 of `code << 6 | length`, with 0x3F meaning "a u32 length follows".
    fn tag(code: u16, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        if body.len() < 0x3F {
            out.extend_from_slice(&((code << 6) | body.len() as u16).to_le_bytes());
        } else {
            out.extend_from_slice(&((code << 6) | 0x3F).to_le_bytes());
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        }
        out.extend_from_slice(body);
        out
    }

    /// ShowFrame, a DefineSprite with a body nobody reads, ShowFrame, End.
    fn stream() -> Vec<u8> {
        let mut data = tag(1, &[]);
        data.extend(tag(39, &[0xAA; 10]));
        data.extend(tag(1, &[]));
        data.extend(tag(0, &[]));
        data
    }

    fn visit(reader: &mut SwfStream<'_>, stop_at: Option<TagCode>) -> Vec<TagCode> {
        let mut seen = Vec::new();
        let _ = decode_tags(reader, |_reader, tag| {
            seen.push(tag);
            Ok(if Some(tag) == stop_at {
                ControlFlow::Exit
            } else {
                ControlFlow::Continue
            })
        });
        seen
    }

    /// What makes it safe to skip a definition tag without reading it: the loop restores the reader
    /// to the end of the tag itself, so a callback that consumes none of the body still lands on
    /// the next tag rather than inside this one.
    #[test]
    fn continuing_from_a_callback_skips_the_rest_of_that_tag() {
        let data = stream();
        let mut reader = SwfStream::new(&data, 10);
        assert_eq!(
            visit(&mut reader, None),
            vec![
                TagCode::ShowFrame,
                TagCode::DefineSprite,
                TagCode::ShowFrame,
                TagCode::End
            ],
        );
    }

    /// And what makes `Exit` expensive: it ends the whole pass, leaving the reader just past the
    /// tag that stopped it. `MovieClip::preload` is pumped once per frame, so a pass that stops on
    /// every already-defined sprite advances exactly one sprite per frame -- which is why a
    /// re-shown armour took seconds to appear.
    #[test]
    fn exiting_ends_the_pass_and_a_resumed_one_starts_after_that_tag() {
        let data = stream();
        let mut reader = SwfStream::new(&data, 10);

        let first = visit(&mut reader, Some(TagCode::DefineSprite));
        assert_eq!(first, vec![TagCode::ShowFrame, TagCode::DefineSprite]);

        // Resuming does not repeat the sprite, and does not land inside its body.
        let second = visit(&mut reader, Some(TagCode::DefineSprite));
        assert_eq!(second, vec![TagCode::ShowFrame, TagCode::End]);
    }
}
