//! Object representation for `flash.utils.Dictionary`

use crate::avm2::Error;
use crate::avm2::activation::Activation;
use crate::avm2::dynamic_map::DynamicKey;
use crate::avm2::object::script_object::ScriptObjectData;
use crate::avm2::object::{ClassObject, Object, TObject};
use crate::avm2::value::Value;
use crate::string::AvmString;
use core::fmt;
use gc_arena::{Collect, Gc, GcWeak, Mutation};
use ruffle_common::utils::HasPrefixField;
use std::cell::Cell;

/// A class instance allocator that allocates Dictionary objects.
pub fn dictionary_allocator<'gc>(
    class: ClassObject<'gc>,
    activation: &mut Activation<'_, 'gc>,
) -> Result<Object<'gc>, Error<'gc>> {
    let base = ScriptObjectData::new(class);

    Ok(DictionaryObject(Gc::new(
        activation.gc(),
        DictionaryObjectData {
            base,
            weak_keys: Cell::new(false),
            has_weak_object_keys: Cell::new(false),
            entries_at_last_prune: Cell::new(0),
        },
    ))
    .into())
}

/// An object that allows associations between objects and values.
///
/// This is implemented by way of "object space", parallel to the property
/// space that ordinary properties live in. This space has no namespaces, and
/// keys are objects instead of strings.
#[derive(Clone, Collect, Copy)]
#[collect(no_drop)]
pub struct DictionaryObject<'gc>(pub Gc<'gc, DictionaryObjectData<'gc>>);

#[derive(Clone, Collect, Copy, Debug)]
#[collect(no_drop)]
pub struct DictionaryObjectWeak<'gc>(pub GcWeak<'gc, DictionaryObjectData<'gc>>);

impl fmt::Debug for DictionaryObject<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DictionaryObject")
            .field("ptr", &Gc::as_ptr(self.0))
            .finish()
    }
}

#[derive(Clone, Collect, HasPrefixField)]
#[collect(no_drop)]
#[repr(C, align(8))]
pub struct DictionaryObjectData<'gc> {
    /// Base script object
    base: ScriptObjectData<'gc>,

    /// Whether object keys are held weakly, which is what `new Dictionary(true)` asks for.
    ///
    /// Set by the constructor rather than the allocator, because the allocator runs before the
    /// argument exists. A dictionary that has already been written to cannot change its mind, and
    /// nothing tries: the flag is only ever set once, immediately after allocation.
    #[collect(require_static)]
    weak_keys: Cell<bool>,

    /// Whether an object key has ever been stored, which is what a sweep would have to find.
    ///
    /// `weakKeys` weakens *object* keys only; a primitive key is an ordinary property name held
    /// strongly, exactly as Flash does it. AQW leans on that harder than the flag suggests --
    /// `World.avatars`, `uoTree`, `invTree` and `waveTree` are all `new Dictionary(true)` yet keyed
    /// by uid, username and item id, and `avatars` alone is enumerated from about fifteen places,
    /// several of them per frame. Without this, each of those enumerations paid a write barrier and
    /// a full table walk to remove nothing, and would have gone on doing so for the whole session.
    #[collect(require_static)]
    has_weak_object_keys: Cell<bool>,

    /// How many entries there were when dead keys were last swept out.
    ///
    /// A weak key is not removed the moment its object is collected -- nothing is watching -- so
    /// the entries accumulate until something sweeps. Sweeping on every write would be quadratic
    /// over a session, so instead it happens when the map has grown appreciably since the last
    /// sweep, which is amortised constant and bounds the dead entries to a fraction of the live
    /// ones.
    #[collect(require_static)]
    entries_at_last_prune: Cell<usize>,
}

/// Growth since the last sweep that justifies another one, as a fraction of the entries then held.
const PRUNE_GROWTH_NUMERATOR: usize = 1;
const PRUNE_GROWTH_DENOMINATOR: usize = 2;

/// Growth that justifies a sweep regardless, so a small dictionary still gets swept.
const PRUNE_MINIMUM_GROWTH: usize = 8;

impl<'gc> DictionaryObject<'gc> {
    /// Hold object keys weakly from now on.
    ///
    /// Called by the constructor when `weakKeys` is true. Ruffle used to answer this with a stub
    /// warning and then store the keys strongly anyway, which is the opposite of what the argument
    /// asks for and is a leak with no upper bound: AQW keeps two *static* weak dictionaries in
    /// `fl.core.UIComponent` keyed on the component itself, so every UI object ever registered
    /// stayed reachable, and through its class and application domain so did the entire movie it
    /// came from. A session measured 51 GB of resident memory with movies and characters that only
    /// ever rose.
    pub fn set_weak_keys(self) {
        if !self.0.weak_keys.replace(true) {
            #[cfg(feature = "aether_performance")]
            crate::aether_performance::note_weak_dictionary_created();
        }
    }

    pub fn has_weak_keys(self) -> bool {
        self.0.weak_keys.get()
    }

    /// The key this dictionary would file `name` under.
    fn key_for(self, name: Object<'gc>) -> DynamicKey<'gc> {
        if self.0.weak_keys.get() {
            DynamicKey::WeakObject(name.downgrade())
        } else {
            DynamicKey::Object(name)
        }
    }

    /// Retrieve a value in the dictionary's object space.
    pub fn get_property_by_object(self, name: Object<'gc>) -> Value<'gc> {
        // No liveness check: `name` is a live reference, so an entry found under it cannot be one
        // whose key has been collected.
        self.base()
            .values()
            .get(&self.key_for(name))
            .map(|v| v.value)
            .unwrap_or(Value::Undefined)
    }

    /// Set a value in the dictionary's object space.
    pub fn set_property_by_object(self, name: Object<'gc>, value: Value<'gc>, mc: &Mutation<'gc>) {
        let key = self.key_for(name);
        if matches!(key, DynamicKey::WeakObject(_)) && !self.0.has_weak_object_keys.replace(true) {
            #[cfg(feature = "aether_performance")]
            crate::aether_performance::note_weak_dictionary_took_an_object_key();
        }
        self.base().values_mut(mc).insert(key, value);
        self.prune_dead_keys_if_grown(mc);
    }

    /// Whether a sweep could possibly find anything to remove.
    ///
    /// Only a weak *object* key can ever die, so a weak-keyed dictionary that has only been given
    /// primitive keys never needs sweeping and must not be charged for one.
    fn may_have_dead_keys(self) -> bool {
        self.0.weak_keys.get() && self.0.has_weak_object_keys.get()
    }

    /// Delete a value from the dictionary's object space.
    pub fn delete_property_by_object(self, name: Object<'gc>, mc: &Mutation<'gc>) {
        let key = self.key_for(name);
        self.base().values_mut(mc).remove(&key);
    }

    pub fn has_property_by_object(self, name: Object<'gc>) -> bool {
        self.base().values().contains_key(&self.key_for(name))
    }

    /// Drop every entry whose weak key has been collected, returning how many went.
    ///
    /// Reads already treat those entries as absent, so this is about the *values*: until the entry
    /// itself goes, the value it holds stays reachable.
    pub fn prune_dead_keys(self, mc: &Mutation<'gc>) -> usize {
        if !self.may_have_dead_keys() {
            return 0;
        }

        let removed = self.base().values_mut(mc).remove_keys(|key| key.is_dead());
        self.0.entries_at_last_prune.set(self.base().values().len());

        #[cfg(feature = "aether_performance")]
        crate::aether_performance::note_dictionary_keys_pruned(removed);

        removed
    }

    /// Sweep, but only once the map has grown enough since the last sweep to be worth it.
    fn prune_dead_keys_if_grown(self, mc: &Mutation<'gc>) {
        if !self.may_have_dead_keys() {
            return;
        }

        let held = self.base().values().len();
        let last = self.0.entries_at_last_prune.get();
        let growth = held.saturating_sub(last);
        let threshold =
            (last * PRUNE_GROWTH_NUMERATOR / PRUNE_GROWTH_DENOMINATOR).max(PRUNE_MINIMUM_GROWTH);

        if growth >= threshold {
            self.prune_dead_keys(mc);
        }
    }
}

impl<'gc> TObject<'gc> for DictionaryObject<'gc> {
    fn gc_base(&self) -> Gc<'gc, ScriptObjectData<'gc>> {
        HasPrefixField::as_prefix_gc(self.0)
    }

    // Calling `setPropertyIsEnumerable` on a `Dictionary` has no effect -
    // stringified properties are always enumerable.
    fn set_local_property_is_enumerable(
        &self,
        _mc: &Mutation<'gc>,
        _name: AvmString<'gc>,
        _is_enumerable: bool,
    ) {
    }

    /// Step to the next enumerable entry, skipping any whose weak key has been collected.
    ///
    /// A dead entry is logically absent, so `for each` must not stop on one or hand its index to
    /// `get_enumerant_name`. Starting a fresh enumeration is also the natural moment to sweep: no
    /// index has been given out yet, so moving entries cannot disturb a walk in progress.
    fn get_next_enumerant(
        self,
        last_index: u32,
        activation: &mut Activation<'_, 'gc>,
    ) -> Result<u32, Error<'gc>> {
        let base = self.base();

        // The overwhelmingly common case, including every dictionary AQW enumerates in a hot loop:
        // weak-flagged but keyed by uid or name, so no entry can ever be dead and the plain walk is
        // the correct one.
        if !self.may_have_dead_keys() {
            return Ok(base.get_next_enumerant(last_index));
        }

        if last_index == 0 {
            self.prune_dead_keys(activation.gc());
        }

        let mut index = last_index;
        loop {
            index = base.get_next_enumerant(index);
            if index == 0 {
                return Ok(0);
            }
            let dead = base
                .values()
                .key_at(index as usize)
                .is_some_and(|key| key.is_dead());
            if !dead {
                return Ok(index);
            }
        }
    }

    /// The key at `index`, which for a weak dictionary means turning the handle back into an
    /// object. `get_next_enumerant` has already skipped the dead ones, so `Null` here would mean a
    /// key that died between the two calls.
    fn get_enumerant_name(
        self,
        index: u32,
        activation: &mut Activation<'_, 'gc>,
    ) -> Result<Value<'gc>, Error<'gc>> {
        let base = self.base();
        // Copied out so the borrow of the values map ends before `upgrade` needs the arena.
        let key = base.values().key_at(index as usize).copied();
        let Some(key) = key else {
            return Ok(Value::Null);
        };

        Ok(match &key {
            DynamicKey::String(name) => Value::String(*name),
            DynamicKey::Object(object) => Value::Object(*object),
            DynamicKey::Uint(value) => Value::Number(*value as f64),
            DynamicKey::WeakObject(weak) => weak
                .upgrade(activation.gc())
                .map_or(Value::Null, Value::Object),
        })
    }

    fn get_enumerant_value(
        self,
        index: u32,
        _activation: &mut Activation<'_, 'gc>,
    ) -> Result<Value<'gc>, Error<'gc>> {
        Ok(*self
            .base()
            .values()
            .value_at(index as usize)
            .unwrap_or(&Value::Undefined))
    }
}
