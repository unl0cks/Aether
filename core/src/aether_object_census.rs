//! Which ActionScript classes are being constructed, and how many times.
//!
//! The collector can say how many objects are live and how many bytes they hold, but not what they
//! are, and that is the whole question. Nineteen minutes idle in a static room -- no movies loaded,
//! no characters registered, no SWF bytes added -- took the Rust heap from 574 MB to 1,276 MB, with
//! the collector's own floor accounting for most of it. Objects are surviving collection that
//! nothing should still be creating, and a total gives no way to find out which.
//!
//! Counting construction rather than survival is deliberate. A class that leaks has to be built
//! before it can be kept, so runaway construction is the earlier and louder signal, and it needs no
//! cooperation from the collector to observe.
//!
//! Lock-free because this sits on the allocation path of every AVM2 object in the process. A mutex
//! here would change the thing being measured: the client would slow down, fewer frames would run,
//! and fewer objects would be built per minute. The table is fixed-size and open-addressed, with
//! anything past capacity folded into an overflow counter so the report can say it was truncated
//! rather than quietly under-report.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::avm2::Class;

/// How many distinct classes can be tracked.
///
/// AQW's own class count runs to a few thousand across every asset it loads, but the ones being
/// constructed in a room that is not loading anything are far fewer. Overflow is reported.
const CLASS_SLOTS: usize = 4096;

/// How many distinct event types can be tracked.
///
/// `flash.events::Event` was 48% of every object constructed in the process -- 4,750 a second
/// against four broadcasts a frame, so something is building one per display object per frame. The
/// class name cannot say which; the event type string can, and there are few enough of them that a
/// small table covers it.
const EVENT_TYPE_SLOTS: usize = 256;

/// How far to probe before giving up and counting a construction as overflow.
const MAX_PROBE: usize = 8;

struct ClassSlot {
    /// The `Class` pointer this slot belongs to, or 0 while unclaimed.
    key: AtomicUsize,
    constructions: AtomicU64,
    /// Captured when the slot is claimed, while the class is still known to be alive.
    ///
    /// The key is only ever compared, never dereferenced: by the time the report is written the
    /// collector may long since have moved on, and reading a name back out of a stale pointer
    /// would be reporting on freed memory.
    name: OnceLock<String>,
}

impl ClassSlot {
    const fn new() -> Self {
        Self {
            key: AtomicUsize::new(0),
            constructions: AtomicU64::new(0),
            name: OnceLock::new(),
        }
    }
}

static CLASS_CENSUS: [ClassSlot; CLASS_SLOTS] = [const { ClassSlot::new() }; CLASS_SLOTS];

/// Constructions that did not fit the table.
static OVERFLOW_CONSTRUCTIONS: AtomicU64 = AtomicU64::new(0);

static EVENT_TYPE_CENSUS: [ClassSlot; EVENT_TYPE_SLOTS] =
    [const { ClassSlot::new() }; EVENT_TYPE_SLOTS];

/// Event constructions whose type did not fit the table.
static OVERFLOW_EVENT_TYPES: AtomicU64 = AtomicU64::new(0);

/// FNV-1a, so an event type can key a slot without allocating a `String` on the dispatch path.
fn hash_event_type(event_type: &str) -> usize {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in event_type.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    // Zero marks an unclaimed slot, so it cannot also be a valid key.
    (hash as usize) | 1
}

/// Note that one bare `Event` of this type has been constructed.
pub fn record_event_construction(event_type: &str) {
    let key = hash_event_type(event_type);
    let start = key % EVENT_TYPE_SLOTS;

    for probe in 0..MAX_PROBE {
        let slot = &EVENT_TYPE_CENSUS[(start + probe) % EVENT_TYPE_SLOTS];
        let occupant = slot.key.load(Ordering::Relaxed);

        if occupant == key {
            slot.constructions.fetch_add(1, Ordering::Relaxed);
            return;
        }

        if occupant == 0
            && slot
                .key
                .compare_exchange(0, key, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            let _ = slot.name.set(event_type.to_owned());
            slot.constructions.fetch_add(1, Ordering::Relaxed);
            return;
        }
    }

    OVERFLOW_EVENT_TYPES.fetch_add(1, Ordering::Relaxed);
}

/// Spread pointers across the table. They are allocator addresses, so the low bits carry alignment
/// padding and would otherwise pile every class into the same few slots.
fn slot_for(key: usize) -> usize {
    let mut hash = key as u64;
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(0xff51_afd7_ed55_8ccd);
    hash ^= hash >> 33;
    (hash as usize) % CLASS_SLOTS
}

/// Note that one instance of `class` has been constructed.
pub fn record_construction(class: Class<'_>) {
    let key = class.as_ptr() as usize;
    if key == 0 {
        return;
    }

    let start = slot_for(key);
    for probe in 0..MAX_PROBE {
        let slot = &CLASS_CENSUS[(start + probe) % CLASS_SLOTS];
        let occupant = slot.key.load(Ordering::Relaxed);

        if occupant == key {
            slot.constructions.fetch_add(1, Ordering::Relaxed);
            return;
        }

        if occupant == 0
            && slot
                .key
                .compare_exchange(0, key, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            // Claimed. The class is alive right now, which is the only moment its name can safely
            // be read, so capture it here rather than at report time.
            let _ = slot.name.set(class.name().to_qualified_name_no_mc().to_string());
            slot.constructions.fetch_add(1, Ordering::Relaxed);
            return;
        }
    }

    OVERFLOW_CONSTRUCTIONS.fetch_add(1, Ordering::Relaxed);
}

/// The busiest classes, most constructed first.
///
/// Returns one line per class plus a header, ready to log.
pub fn object_census_report(limit: usize) -> Vec<String> {
    let mut rows: Vec<(u64, &str)> = CLASS_CENSUS
        .iter()
        .filter(|slot| slot.key.load(Ordering::Relaxed) != 0)
        .filter_map(|slot| {
            let constructions = slot.constructions.load(Ordering::Relaxed);
            let name = slot.name.get()?;
            Some((constructions, name.as_str()))
        })
        .collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0));

    let total: u64 = rows.iter().map(|row| row.0).sum();
    let overflow = OVERFLOW_CONSTRUCTIONS.load(Ordering::Relaxed);
    let tracked = rows.len();

    let mut out = vec![format!(
        "object census: {} constructions across {}/{} classes{}",
        total.saturating_add(overflow),
        tracked,
        CLASS_SLOTS,
        if overflow > 0 {
            format!(", {overflow} untracked")
        } else {
            String::new()
        },
    )];

    for (constructions, name) in rows.into_iter().take(limit) {
        out.push(format!("  {constructions:>12}  {name}"));
    }
    out
}

/// The busiest bare `Event` types, most constructed first.
pub fn event_type_census_report(limit: usize) -> Vec<String> {
    let mut rows: Vec<(u64, &str)> = EVENT_TYPE_CENSUS
        .iter()
        .filter(|slot| slot.key.load(Ordering::Relaxed) != 0)
        .filter_map(|slot| {
            Some((
                slot.constructions.load(Ordering::Relaxed),
                slot.name.get()?.as_str(),
            ))
        })
        .collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0));

    let total: u64 = rows.iter().map(|row| row.0).sum();
    let overflow = OVERFLOW_EVENT_TYPES.load(Ordering::Relaxed);

    let mut out = vec![format!(
        "event census: {} bare events across {} types{}",
        total.saturating_add(overflow),
        rows.len(),
        if overflow > 0 {
            format!(", {overflow} untracked")
        } else {
            String::new()
        },
    )];

    for (constructions, name) in rows.into_iter().take(limit) {
        out.push(format!("  {constructions:>12}  {name}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_pointers_do_not_all_land_in_one_slot() {
        // Class pointers are allocator addresses whose low bits are alignment padding. Using them
        // unmixed puts every class within a probe window of the same slot, so the table overflows
        // almost immediately and the report reads as "nothing much is being constructed".
        let slots: std::collections::HashSet<usize> = (0..64)
            .map(|index| slot_for(0x7f00_0000_0000 + index * 64))
            .collect();
        assert!(
            slots.len() > 32,
            "64 realistically spaced pointers landed in only {} slots",
            slots.len()
        );
    }

    #[test]
    fn a_report_with_nothing_recorded_still_names_the_capacity() {
        let report = object_census_report(10);
        assert!(report[0].starts_with("object census:"));
        assert!(report[0].contains(&CLASS_SLOTS.to_string()));
    }
}
