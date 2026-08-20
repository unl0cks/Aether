//! `flash.utils.Dictionary` native methods

use crate::avm2::Error;
use crate::avm2::activation::Activation;
use crate::avm2::value::Value;

pub use crate::avm2::object::dictionary_allocator;

/// Implements `Dictionary.initWeakKeys`, called by the constructor for `new Dictionary(true)`.
///
/// This used to be a stub that warned and changed nothing, so `weakKeys` was a promise the runtime
/// did not keep. See `DictionaryObject::set_weak_keys` for what that cost.
pub fn init_weak_keys<'gc>(
    _activation: &mut Activation<'_, 'gc>,
    this: Value<'gc>,
    _args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    if let Some(dictionary) = this
        .as_object()
        .and_then(|object| object.as_dictionary_object())
    {
        dictionary.set_weak_keys();
    }

    Ok(Value::Undefined)
}
