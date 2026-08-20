//! `flash.system.System` native methods

use crate::avm2::Error;
use crate::avm2::activation::Activation;
use crate::avm2::parameters::ParametersExt;
use crate::avm2::value::Value;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

/// Whether content has asked for a full collection since the last frame ended.
///
/// A collection needs `&mut Arena`, which does not exist part-way through running a script, so the
/// request is recorded here and honoured by the player at the next frame boundary. That is also
/// what Flash does: `System.gc()` is a request, not a stop-the-world call at the point of use.
static COLLECTION_REQUESTED: AtomicBool = AtomicBool::new(false);

/// How many requests have been honoured, and what they reclaimed.
///
/// Reported by the memory census rather than logged. The first attempt logged each collection at
/// `debug`, which meant a whole measured session could not say whether the path had run even once
/// -- an ordinary release logs at `info`, so nothing was recorded at all.
static COLLECTIONS_RUN: AtomicUsize = AtomicUsize::new(0);
static COLLECTION_RECLAIMED_BYTES: AtomicU64 = AtomicU64::new(0);
/// The worst single collection, in microseconds. A collection happens inside a frame, so this is
/// the frame it cost -- the number that says whether the policy is affordable.
static COLLECTION_WORST_MICROS: AtomicU64 = AtomicU64::new(0);

/// Whether a full collection was asked for, clearing the request.
pub fn take_collection_request() -> bool {
    COLLECTION_REQUESTED.swap(false, Ordering::Relaxed)
}

/// Note that a requested collection ran, and how many bytes it gave back.
pub fn note_collection_ran(reclaimed_bytes: u64, elapsed: std::time::Duration) {
    COLLECTIONS_RUN.fetch_add(1, Ordering::Relaxed);
    COLLECTION_RECLAIMED_BYTES.fetch_add(reclaimed_bytes, Ordering::Relaxed);
    let micros = elapsed.as_micros().min(u64::MAX as u128) as u64;
    COLLECTION_WORST_MICROS.fetch_max(micros, Ordering::Relaxed);
}

/// How many requested collections have run, and the total they reclaimed.
pub fn collection_totals() -> (usize, u64, u64) {
    (
        COLLECTIONS_RUN.load(Ordering::Relaxed),
        COLLECTION_RECLAIMED_BYTES.load(Ordering::Relaxed),
        COLLECTION_WORST_MICROS.load(Ordering::Relaxed),
    )
}

/// Implements `flash.system.System.gc`.
///
/// This was an empty function, and AQW is built around it doing something. `World.clearLoaders`
/// and `World.cleanupMap` drop the player domains, build a fresh child `ApplicationDomain` for
/// subsequent loads, and then call this to have the old one reclaimed -- so with it inert, the
/// game's own memory management ran and freed nothing. Both callers are map-change routines rather
/// than per-frame ones, which is why honouring them is affordable.
pub fn gc<'gc>(
    _activation: &mut Activation<'_, 'gc>,
    _this: Value<'gc>,
    _args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    COLLECTION_REQUESTED.store(true, Ordering::Relaxed);
    Ok(Value::Undefined)
}

/// Implements `flash.system.System.totalMemoryNumber`.
///
/// Reports what the collector is actually holding. It used to answer a fixed 90 MB, and AQW asks
/// `if (System.totalMemory > 200 * 1024 * 1024)` before deciding how hard to collect -- so the
/// branch could never be taken however much memory the process had really accumulated.
///
/// The arena total is the right analogue: Flash counts memory the player allocated for content,
/// which is what this arena is, rather than the whole process.
pub fn get_total_memory_number<'gc>(
    activation: &mut Activation<'_, 'gc>,
    _this: Value<'gc>,
    _args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    Ok((activation.gc().metrics().total_gc_allocation() as f64).into())
}

/// Implements `flash.system.System.setClipboard` method
pub fn set_clipboard<'gc>(
    activation: &mut Activation<'_, 'gc>,
    _this: Value<'gc>,
    args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    // The following restrictions only apply to the plugin.
    // TODO: Check the type of event that triggered the function call.
    #[cfg(target_family = "wasm")]
    if false {
        return Err(crate::avm2::error::make_error_2176(activation));
    }

    let new_content = args.get_string_non_null(activation, 0, "text")?;
    activation
        .context
        .ui
        .set_clipboard_content(new_content.to_string());

    Ok(Value::Undefined)
}
