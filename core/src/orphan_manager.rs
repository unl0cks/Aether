//! Special handling for AVM2 orphan objects

use crate::context::UpdateContext;
use crate::display_object::{
    Avm2LifecycleTraversal, DisplayObject, DisplayObjectWeak, TDisplayObject,
};
use fnv::FnvHashSet;
use gc_arena::{Collect, Mutation};
use std::rc::Rc;

/// The list of 'orphan' objects - these objects have no parent,
/// so we need to manually run their frames in `run_all_phases_avm2` to match
/// Flash's behavior. Clips are added to this list with `add_orphan_movie`.
/// and are removed automatically by `cleanup_dead_orphans`.
///
/// We store `DisplayObjectWeak`, since we don't want to keep these objects
/// alive if they would otherwise be garbage-collected. The movie will
/// stop ticking whenever garbage collection runs if there are no more
/// strong references around (this matches Flash's behavior).
#[derive(Collect)]
#[collect(no_drop)]
pub struct OrphanManager<'gc> {
    orphans: Rc<Vec<DisplayObjectWeak<'gc>>>,

    /// Addresses of everything in `orphans`, so membership is a lookup rather than a walk.
    ///
    /// Adding an orphan used to scan the whole list to see whether it was already there, which is
    /// quadratic in a movie that orphans a lot of objects -- and AQW orphans one for every piece of
    /// every avatar it loads, continuously. The list is still what gets iterated, because order is
    /// observable; this only answers "is it in there already".
    ///
    /// Keyed by address, exactly as the walk it replaces was. That is sound for the same reason:
    /// a `GcWeak` keeps its allocation alive, so no two live entries can share an address.
    #[collect(require_static)]
    present: FnvHashSet<usize>,
}

impl<'gc> OrphanManager<'gc> {
    fn orphans_mut(&mut self) -> &mut Vec<DisplayObjectWeak<'gc>> {
        Rc::make_mut(&mut self.orphans)
    }

    /// Adds a `MovieClip` to the orphan list. In AVM2, movies advance their
    /// frames even when they are not on a display list. Unfortunately,
    /// multiple SWFS rely on this behavior, so we need to match Flash's
    /// behavior. This should not be called manually - `movie_clip` will
    /// call it when necessary.
    pub fn add_orphan_obj(&mut self, dobj: DisplayObject<'gc>) {
        // Removal itself is observable during the remaining lifecycle phases.
        // Schedule one conservative pass even when the subtree was clean while
        // attached; each phase will keep itself dirty only if work remains.
        dobj.mark_avm2_lifecycle_dirty(Avm2LifecycleTraversal::Enter);
        dobj.mark_avm2_lifecycle_dirty(Avm2LifecycleTraversal::Construct);
        dobj.mark_avm2_lifecycle_dirty(Avm2LifecycleTraversal::FrameScripts);

        // Note: comparing pointers is correct because GcWeak keeps its allocation alive,
        // so the pointers can't overlap by accident.
        if self.present.insert(dobj.as_ptr() as usize) {
            self.orphans_mut().push(dobj.downgrade());
        }
    }

    /// Removes a specific object from orphan lifecycle processing.
    ///
    /// This is used by `Loader` teardown: unloaded content must not continue
    /// advancing after its loader has released it. Other objects detached by
    /// ActionScript retain Flash's normal orphan behavior.
    pub fn remove_orphan_obj(&mut self, dobj: DisplayObject<'gc>) -> bool {
        if !self.present.remove(&(dobj.as_ptr() as usize)) {
            return false;
        }
        self.orphans_mut()
            .retain(|orphan| !std::ptr::eq(orphan.as_ptr(), dobj.as_ptr()));
        true
    }

    pub fn each_orphan_obj(
        context: &mut UpdateContext<'gc>,
        mut f: impl FnMut(DisplayObject<'gc>, &mut UpdateContext<'gc>),
    ) {
        // Clone the Rc before iterating over it. Any modifications must go through
        // `Rc::make_mut` in `orphan_objects_mut`, which will leave this `Rc` unmodified.
        // This ensures that any orphan additions/removals done by `f` will not affect
        // the iteration in this method.
        let orphan_objs: Rc<_> = context.orphan_manager.orphans.clone();

        for orphan in orphan_objs.iter() {
            if let Some(dobj) = valid_orphan(*orphan, context.gc()) {
                f(dobj, context);
            }
        }
    }

    /// How many orphans are being kept alive and ticked.
    ///
    /// AQW orphans a piece for every part of every avatar it loads, so this is the count that grows
    /// if they are ever kept past their welcome. Reported by the memory census, where a number that
    /// climbs over hours separates "the orphan list is the leak" from "the orphan list is a
    /// bystander" without having to guess.
    pub fn len(&self) -> usize {
        self.orphans.len()
    }

    /// Called at the end of `run_all_phases_avm2` - removes any movies
    /// that have been garbage collected, or are no longer orphans
    /// (they've since acquired a parent).
    pub fn cleanup_dead_orphans(&mut self, mc: &Mutation<'gc>) {
        let present = &mut self.present;
        Rc::make_mut(&mut self.orphans).retain(|d| {
            let keep = orphan_survives(*d, mc);
            if !keep {
                present.remove(&(d.as_ptr() as usize));
            }
            keep
        });
    }

}

impl<'gc> Default for OrphanManager<'gc> {
    fn default() -> Self {
        Self {
            orphans: Rc::new(Vec::new()),
            present: FnvHashSet::default(),
        }
    }
}

/// Whether an orphan should stay on the list for another frame.
///
/// All clips that become orphaned (have their parent removed, or start out with no parent) get
/// added to the orphan list. However, there's a distinction between clips that are removed from a
/// RemoveObject tag, and clips that are removed from ActionScript.
///
/// Clips removed from a RemoveObject tag only stay on the orphan list until the end of the frame -
/// this lets them run a framescript (with 'this.parent == null') before they're removed. After
/// that, they're removed from the orphan list, and will not be run in any way.
///
/// Clips removed from ActionScript stay on the orphan list, and will be run indefinitely (if there
/// are no remaining strong references, they will eventually be garbage collected).
///
/// To detect this, we check 'placed_by_avm2_script'. This flag gets set to 'true' for objects
/// constructed from ActionScript, and for objects moved around in the timeline (add/remove child,
/// swap depths) by ActionScript. A RemoveObject tag will only affect objects instantiated by the
/// timeline, which have not been moved in the displaylist by ActionScript. Therefore, any orphan we
/// see that has 'placed_by_avm2_script()' should stay on the orphan list, because it was not
/// removed by a RemoveObject tag.
fn orphan_survives<'gc>(dobj: DisplayObjectWeak<'gc>, mc: &Mutation<'gc>) -> bool {
    valid_orphan(dobj, mc).is_some_and(|dobj| dobj.placed_by_avm2_script())
}

/// If the provided `DisplayObjectWeak` should have frames run, returns
/// Some(clip) with an upgraded `MovieClip`.
/// If this returns `None`, the entry should be removed from the orphan list.
fn valid_orphan<'gc>(
    dobj: DisplayObjectWeak<'gc>,
    mc: &Mutation<'gc>,
) -> Option<DisplayObject<'gc>> {
    dobj.upgrade(mc).filter(|dobj| dobj.parent().is_none())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display_object::MovieClip;
    use crate::tag_utils::SwfMovie;
    use gc_arena::arena::rootless_mutate;
    use std::sync::Arc;

    #[test]
    fn explicitly_removed_loader_content_stops_orphan_processing() {
        rootless_mutate(|mc| {
            let movie = Arc::new(SwfMovie::empty(10, None));
            let content: DisplayObject<'_> = MovieClip::new(movie, mc).into();
            let mut manager = OrphanManager::default();

            manager.add_orphan_obj(content);
            assert_eq!(manager.orphans.len(), 1);

            assert!(manager.remove_orphan_obj(content));
            assert!(manager.orphans.is_empty());
            assert!(!manager.remove_orphan_obj(content));
        });
    }

    /// Adding the same object twice still adds it once, now that a set answers the question
    /// instead of a walk over everything already listed.
    #[test]
    fn an_object_is_listed_once_however_often_it_is_offered() {
        rootless_mutate(|mc| {
            let movie = Arc::new(SwfMovie::empty(10, None));
            let content: DisplayObject<'_> = MovieClip::new(movie.clone(), mc).into();
            let other: DisplayObject<'_> = MovieClip::new(movie, mc).into();
            let mut manager = OrphanManager::default();

            manager.add_orphan_obj(content);
            manager.add_orphan_obj(content);
            manager.add_orphan_obj(content);
            assert_eq!(manager.orphans.len(), 1);
            assert_eq!(manager.present.len(), 1);

            manager.add_orphan_obj(other);
            assert_eq!(manager.orphans.len(), 2);

            // Removing one leaves the other, and the set agrees with the list.
            assert!(manager.remove_orphan_obj(content));
            assert_eq!(manager.orphans.len(), 1);
            assert_eq!(manager.present.len(), 1);

            // And it can be added back, which a stale set entry would prevent.
            manager.add_orphan_obj(content);
            assert_eq!(manager.orphans.len(), 2);
        });
    }
}
