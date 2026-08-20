use crate::avm2::object::{Object, WeakObject};
use crate::string::AvmString;
use fnv::FnvBuildHasher;
use gc_arena::Collect;
use hashbrown::HashTable;
use hashbrown::hash_table::Entry;
use std::cell::Cell;
use std::hash::{BuildHasher, Hash};

#[derive(Debug, Collect, Copy, Clone)]
#[collect(no_drop)]
pub struct DynamicProperty<V> {
    pub value: V,
    pub enumerable: bool,
}

#[derive(Eq, PartialEq, Hash, Copy, Clone, Collect)]
#[collect(no_drop)]
pub enum DynamicKey<'gc> {
    String(AvmString<'gc>),
    // When the name parses as a non-negative integer, we use that integer as the key
    // See `ScriptObject::get_property_local` and `ScriptObject::set_property_local`.
    // This is observable when iterating over the object keys, as the key
    // can be `number`
    Uint(u32),
    Object(Object<'gc>),
    /// An object key that does not keep its object alive. Only `flash.utils.Dictionary`
    /// constructed with `weakKeys` produces these.
    ///
    /// It is a separate variant rather than a flag on `Object` because the two hash and compare
    /// differently the moment the target dies, and because the derived `Collect` has to trace this
    /// one weakly -- which is the entire point. A dictionary that uses weak keys uses them for
    /// every entry, so the two variants never have to match each other.
    WeakObject(WeakObject<'gc>),
}

impl<'gc> DynamicKey<'gc> {
    /// Whether this key refers to an object that has since been collected.
    ///
    /// Such an entry is not deleted the instant its key dies -- nothing is watching -- so every
    /// read has to treat it as absent until a sweep physically removes it.
    pub fn is_dead(&self) -> bool {
        match self {
            DynamicKey::WeakObject(weak) => weak.is_dropped(),
            _ => false,
        }
    }
}

/// A HashMap designed for dynamic properties on an object.
///
/// Uses `HashTable` directly to expose stable bucket indices, which are
/// needed for correct iteration when entries are added or removed mid-iteration.
#[derive(Debug, Clone, Collect)]
#[collect(no_drop)]
pub struct DynamicMap<K, V> {
    table: HashTable<(K, DynamicProperty<V>)>,
    #[collect(require_static)]
    hasher: FnvBuildHasher,
    // The last index that was given back to flash
    #[collect(require_static)]
    public_index: Cell<usize>,
    // The actual bucket index that represents where an item is in the table
    #[collect(require_static)]
    real_index: Cell<usize>,
}

impl<K: Eq + Hash, V> Default for DynamicMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Eq + Hash, V> DynamicMap<K, V> {
    fn hash_key(&self, key: &K) -> u64 {
        self.hasher.hash_one(key)
    }

    pub fn new() -> Self {
        Self {
            table: HashTable::new(),
            hasher: FnvBuildHasher::default(),
            public_index: Cell::new(0),
            real_index: Cell::new(0),
        }
    }

    pub fn get(&self, key: &K) -> Option<&DynamicProperty<V>> {
        let hash = self.hash_key(key);

        self.table.find(hash, |(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn contains_key(&self, key: &K) -> bool {
        let hash = self.hash_key(key);

        self.table.find(hash, |(k, _)| k == key).is_some()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &DynamicProperty<V>)> {
        self.table.iter().map(|(k, v)| (k, v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.table.iter().map(|(k, _)| k)
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    /// Gets the bucket index from the current public index.
    /// Returns `None` if the index is out of bounds.
    fn public_to_real_index(&self, index: usize) -> Option<usize> {
        let mut count = 0;
        let num_buckets = self.table.num_buckets();

        for i in 0..num_buckets {
            if let Some((_, v)) = self.table.get_bucket(i)
                && v.enumerable
            {
                count += 1;

                if count >= index {
                    return Some(i);
                }
            }
        }

        None
    }

    pub fn insert(&mut self, key: K, new_value: V) {
        let hash = self.hash_key(&key);

        match self
            .table
            .entry(hash, |(k, _)| *k == key, |(k, _)| self.hasher.hash_one(k))
        {
            Entry::Occupied(mut occupied) => {
                // NOTE: When inserting a new value into an already-occupied entry,
                // the value of the `enumerable` field isn't reset to `true`
                occupied.get_mut().1.value = new_value;
            }
            Entry::Vacant(vacant) => {
                vacant.insert((
                    key,
                    DynamicProperty {
                        value: new_value,
                        enumerable: true,
                    },
                ));
            }
        }
    }

    pub fn remove(&mut self, key: &K) -> Option<DynamicProperty<V>> {
        let hash = self.hash_key(key);

        match self.table.find_entry(hash, |(k, _)| k == key) {
            Ok(occupied) => Some(occupied.remove().0.1),
            Err(_) => None,
        }
    }

    // NOTE: Per the docs on `HashTable::find_bucket_index`, bucket indices are
    // "only meaningful as long as the table is not resized and no entries are
    // added or removed". The current implementation uses tombstones and never
    // moves elements, so indices of other entries are preserved on removal in
    // practice, but this is not a guaranteed API contract and should not be
    // relied on long-term.
    // https://docs.rs/hashbrown/0.16.1/hashbrown/struct.HashTable.html#method.find_bucket_index
    pub fn next(&self, index: usize) -> Option<usize> {
        // Start iteration from the beginning
        if index == 0 {
            if let Some(real) = self.public_to_real_index(1) {
                self.real_index.set(real);
                self.public_index.set(1);
                return Some(1);
            } else {
                self.public_index.set(0);
                self.real_index.set(0);
                return None;
            }
        }
        // If the public index is zero than this is the first time this is being enumerated,
        // if index != self.public_index, then we are enumerating twice OR we mutated while enumerating.
        //
        // Regardless of the reason, we just need to sync the supplied index to the real index.
        if self.public_index.get() == 0 || index != self.public_index.get() {
            if let Some(real) = self.public_to_real_index(index) {
                // Pick up where we left off in the iteration
                // See https://github.com/adobe/avmplus/blob/858d034a3bd3a54d9b70909386435cf4aec81d21/core/avmplusHashtable.cpp#L395
                self.real_index.set(real);
                self.public_index.set(index);
            } else {
                self.public_index.set(0);
                self.real_index.set(0);
                return None;
            }
        }

        let real = self.real_index.get() + 1;
        let num_buckets = self.table.num_buckets();

        if !self.table.is_empty() && real < num_buckets {
            for i in real..num_buckets {
                if let Some((_, v)) = self.table.get_bucket(i)
                    && v.enumerable
                {
                    self.real_index.set(i);
                    self.public_index.set(self.public_index.get() + 1);
                    return Some(self.public_index.get());
                }
            }
        }

        None
    }

    pub fn pair_at(&self, index: usize) -> Option<&(K, DynamicProperty<V>)> {
        let real_index = if self.public_index.get() == 0 || self.public_index.get() != index {
            self.public_to_real_index(index)?
        } else {
            self.real_index.get()
        };

        self.table.get_bucket(real_index)
    }

    pub fn key_at(&self, index: usize) -> Option<&K> {
        self.pair_at(index).map(|(k, _)| k)
    }

    pub fn value_at(&self, index: usize) -> Option<&V> {
        self.pair_at(index).map(|(_, p)| &p.value)
    }

    pub fn set_enumerable(&mut self, key: &K, enumerable: bool) {
        let hash = self.hash_key(key);

        if let Some((_, prop)) = self.table.find_mut(hash, |(k, _)| k == key) {
            prop.enumerable = enumerable;
        }
    }

    /// Drop every entry whose key `should_remove` accepts, returning how many went.
    ///
    /// Only weak-keyed dictionaries need this: an entry whose key has been collected is already
    /// invisible to reads, but it still holds its *value* alive, so something has to physically
    /// take it out.
    pub fn remove_keys(&mut self, mut should_remove: impl FnMut(&K) -> bool) -> usize {
        let before = self.table.len();
        self.table.retain(|(k, _)| !should_remove(k));
        let removed = before - self.table.len();
        if removed > 0 {
            // Bucket indices are what enumeration walks, and retaining moves entries. Anything
            // part-way through an enumeration has to start over rather than read a stale cursor.
            self.public_index.set(0);
            self.real_index.set(0);
        }
        removed
    }
}

#[cfg(test)]
mod tests {

    use super::DynamicMap;

    #[test]
    fn test_dynamic_map() {
        let mut map: DynamicMap<&'static str, i32> = DynamicMap::new();
        map.insert("a", 0);
        map.insert("b", 0);
        map.insert("c", 0);
        map.insert("d", 0);
        let mut current = 0;
        while let Some(next) = map.next(current) {
            if current == 2 {
                map.insert("e", 0);
                map.insert("f", 0);
            }
            println!("{}", map.key_at(current).unwrap());
            current = next;
        }
        println!("next");
        current = 0;
        while let Some(next) = map.next(current) {
            println!("{}", map.key_at(current).unwrap());
            current = next;
        }
    }
}

#[cfg(test)]
mod weak_key_tests {
    use super::*;
    use gc_arena::{Arena, Gc, Rootable};

    /// The root for the arena test below. The weak handle has to live *inside* the arena: `'gc` is
    /// branded and cannot escape a `mutate` call, which is the safety property that makes any of
    /// this sound in the first place.
    #[derive(Collect)]
    #[collect(no_drop)]
    struct WeakRoot<'gc> {
        strong: Option<Gc<'gc, u32>>,
        weak: Option<gc_arena::GcWeak<'gc, u32>>,
        #[collect(require_static)]
        address: Cell<usize>,
    }

    /// Everything the weak-key design assumes about `gc_arena`, checked against a real collector
    /// rather than taken on faith.
    ///
    /// If any of this were untrue, a `Dictionary` with `weakKeys` would silently keep behaving like
    /// the strong one it used to be -- which is exactly the failure that went unnoticed for years.
    #[test]
    fn a_weak_handle_outlives_its_object_and_reports_the_death() {
        type Root = Rootable![WeakRoot<'_>];
        let mut arena: Arena<Root> = Arena::new(|mc| {
            let strong = Gc::new(mc, 7u32);
            WeakRoot {
                strong: Some(strong),
                weak: Some(Gc::downgrade(strong)),
                address: Cell::new(Gc::as_ptr(strong) as usize),
            }
        });

        // While alive: not dropped, upgrades, and the address matches the strong pointer.
        arena.mutate(|mc, root| {
            let weak = root.weak.expect("root starts with a weak handle");
            assert!(!weak.is_dropped(), "a live object must not read as dropped");
            assert!(weak.upgrade(mc).is_some(), "a live object must upgrade");
            assert_eq!(weak.as_ptr() as usize, root.address.get());
        });

        // Drop the only strong reference and collect to completion. Two cycles, because the first
        // finishes whatever was already in progress.
        arena.mutate_root(|_mc, root| root.strong = None);
        arena.finish_cycle();
        arena.finish_cycle();

        arena.mutate(|mc, root| {
            let weak = root.weak.expect("the handle itself survives");
            // The key is gone. This is what lets a sweep find dead entries, and what makes the
            // whole feature work rather than quietly do nothing.
            assert!(weak.is_dropped(), "a collected object must read as dropped");
            assert!(
                weak.upgrade(mc).is_none(),
                "a collected object must not upgrade"
            );
            // And the address still answers, unchanged, so the entry stays findable and removable
            // after its key has died. Hashing a weak key by address depends on this.
            assert_eq!(weak.as_ptr() as usize, root.address.get());
        });
    }

    /// The sweep removes exactly the entries it is told to and leaves the rest reachable.
    #[test]
    fn removing_keys_drops_only_those_it_matches() {
        let mut map: DynamicMap<u32, i32> = DynamicMap::new();
        for key in 0..10u32 {
            map.insert(key, key as i32 * 10);
        }
        assert_eq!(map.len(), 10);

        let removed = map.remove_keys(|key| key % 2 == 0);
        assert_eq!(removed, 5);
        assert_eq!(map.len(), 5);
        for key in 0..10u32 {
            assert_eq!(map.get(&key).is_some(), key % 2 == 1, "key {key}");
        }
    }

    /// Removing nothing must not disturb an enumeration that is part-way through, and removing
    /// something must reset it: bucket indices are what enumeration walks, and `retain` moves them.
    #[test]
    fn a_sweep_that_removes_nothing_leaves_enumeration_alone() {
        let mut map: DynamicMap<u32, i32> = DynamicMap::new();
        for key in 0..6u32 {
            map.insert(key, 0);
        }

        let first = map.next(0).expect("a non-empty map enumerates");
        let second = map.next(first).expect("and keeps going");
        assert_eq!(map.remove_keys(|_| false), 0);
        // Cursor untouched, so the walk continues from where it was.
        assert_eq!(map.next(second), Some(second + 1));

        // A sweep that removes something restarts the walk instead of trusting a stale cursor.
        assert!(map.remove_keys(|key| *key == 0) > 0);
        assert_eq!(map.next(0), Some(1));
    }

    /// Non-object keys are never "dead", so ordinary objects pay nothing for this.
    #[test]
    fn only_weak_object_keys_can_be_dead() {
        assert!(!DynamicKey::<'_>::Uint(3).is_dead());
    }
}
