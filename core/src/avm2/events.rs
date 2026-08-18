//! Core event structure

use crate::avm2::Avm2;
use crate::avm2::activation::Activation;
use crate::avm2::function::FunctionArgs;
#[cfg(feature = "aether_metrics")]
use crate::avm2::function::display_function;
use crate::avm2::globals::slots::flash_events_event_dispatcher as slots;
#[cfg(feature = "aether_metrics")]
use crate::avm2::method::MethodKind;
use crate::avm2::object::{EventObject, FunctionObject, Object, TObject as _};
use crate::display_object::TDisplayObject;
use crate::string::AvmString;
#[cfg(feature = "aether_metrics")]
use crate::string::WString;
use fnv::FnvHashMap;
use gc_arena::{Collect, Mutation};
use smallvec::SmallVec;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
#[cfg(feature = "aether_metrics")]
use std::time::Instant;

/// Which phase of event dispatch is currently occurring.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EventPhase {
    /// The event has yet to be fired on the target and is descending the
    /// ancestors of the event target.
    Capturing = 1,

    /// The event is currently firing on the target.
    AtTarget = 2,

    /// The event has already fired on the target and is ascending the
    /// ancestors of the event target.
    Bubbling = 3,
}

/// How this event is allowed to propagate.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PropagationMode {
    /// Propagate events normally.
    Allow,

    /// Stop capturing or bubbling events.
    Stop,

    /// Stop running event handlers altogether.
    StopImmediate,
}

/// Represents data fields of an event that can be fired on an object that
/// implements `IEventDispatcher`.
#[derive(Clone, Collect)]
#[collect(no_drop)]
pub struct Event<'gc> {
    /// Whether the event "bubbles" - fires on its parents after it
    /// fires on the child.
    bubbles: bool,

    /// Whether the event has a default response that an event handler
    /// can request to not occur.
    cancelable: bool,

    /// Whether the event's default response has been cancelled.
    cancelled: bool,

    /// Whether event propagation has stopped.
    #[collect(require_static)]
    propagation: PropagationMode,

    /// The object currently having its event handlers invoked.
    current_target: Option<Object<'gc>>,

    /// The current event phase.
    #[collect(require_static)]
    event_phase: EventPhase,

    /// The object this event was dispatched on.
    target: Option<Object<'gc>>,

    /// The name of the event being triggered.
    event_type: AvmString<'gc>,
}

impl<'gc> Event<'gc> {
    /// Construct a new event of a given type.
    pub fn new(event_type: AvmString<'gc>) -> Self {
        Event {
            bubbles: false,
            cancelable: false,
            cancelled: false,
            propagation: PropagationMode::Allow,
            current_target: None,
            event_phase: EventPhase::AtTarget,
            target: None,
            event_type,
        }
    }

    pub fn event_type(&self) -> AvmString<'gc> {
        self.event_type
    }

    pub fn set_event_type(&mut self, event_type: AvmString<'gc>) {
        self.event_type = event_type;
    }

    pub fn is_bubbling(&self) -> bool {
        self.bubbles
    }

    pub fn set_bubbles(&mut self, bubbling: bool) {
        self.bubbles = bubbling;
    }

    pub fn is_cancelable(&self) -> bool {
        self.cancelable
    }

    pub fn set_cancelable(&mut self, cancelable: bool) {
        self.cancelable = cancelable;
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    pub fn cancel(&mut self) {
        if self.cancelable {
            self.cancelled = true;
        }
    }

    pub fn is_propagation_stopped(&self) -> bool {
        self.propagation != PropagationMode::Allow
    }

    pub fn stop_propagation(&mut self) {
        if self.propagation != PropagationMode::StopImmediate {
            self.propagation = PropagationMode::Stop;
        }
    }

    pub fn is_propagation_stopped_immediately(&self) -> bool {
        self.propagation == PropagationMode::StopImmediate
    }

    pub fn stop_immediate_propagation(&mut self) {
        self.propagation = PropagationMode::StopImmediate;
    }

    pub fn phase(&self) -> EventPhase {
        self.event_phase
    }

    pub fn set_phase(&mut self, phase: EventPhase) {
        self.event_phase = phase;
    }

    pub fn target(&self) -> Option<Object<'gc>> {
        self.target
    }

    pub fn set_target(&mut self, target: Object<'gc>) {
        self.target = Some(target)
    }

    pub fn current_target(&self) -> Option<Object<'gc>> {
        self.current_target
    }

    pub fn set_current_target(&mut self, current_target: Object<'gc>) {
        self.current_target = Some(current_target)
    }
}

fn collect_handler_snapshot<T, I>(handlers: I) -> SmallVec<[T; 2]>
where
    I: IntoIterator<Item = T>,
{
    handlers.into_iter().collect()
}

fn remove_handler_and_empty_priorities<T: PartialEq>(
    priorities: &mut BTreeMap<i32, Vec<T>>,
    handler: &T,
) {
    for handlers in priorities.values_mut() {
        if let Some(position) = handlers.iter().position(|candidate| candidate == handler) {
            handlers.remove(position);
        }
    }
    priorities.retain(|_, handlers| !handlers.is_empty());
}

/// A set of handlers organized by event type, priority, and order added.
#[derive(Clone, Collect)]
#[collect(no_drop)]
pub struct DispatchList<'gc>(FnvHashMap<AvmString<'gc>, BTreeMap<i32, Vec<EventHandler<'gc>>>>);

impl<'gc> DispatchList<'gc> {
    /// Construct a new dispatch list.
    pub fn new() -> Self {
        Self(Default::default())
    }

    /// Get all of the event handlers for a given event type, if such a type
    /// exists.
    fn get_event(&self, event: AvmString<'gc>) -> Option<&BTreeMap<i32, Vec<EventHandler<'gc>>>> {
        self.0.get(&event)
    }

    /// Get a single priority level of event handlers for a given event type,
    /// for mutation.
    fn get_event_priority_mut(
        &mut self,
        event: AvmString<'gc>,
        priority: i32,
    ) -> &mut Vec<EventHandler<'gc>> {
        self.0
            .entry(event)
            .or_default()
            .entry(priority)
            .or_default()
    }

    /// Add an event handler to this dispatch list.
    ///
    /// This enforces the invariant that an `EventHandler` must not appear at
    /// more than one priority (since we can't enforce that with clever-er data
    /// structure selection). If an event handler already exists, it will not
    /// be added again, and this function will silently fail.
    pub fn add_event_listener(
        &mut self,
        event: AvmString<'gc>,
        priority: i32,
        handler: FunctionObject<'gc>,
        use_capture: bool,
    ) {
        let new_handler = EventHandler::new(handler, use_capture);

        if let Some(event_sheaf) = self.get_event(event) {
            for other_set in event_sheaf.values() {
                if other_set.contains(&new_handler) {
                    return;
                }
            }
        }

        self.get_event_priority_mut(event, priority)
            .push(new_handler);
    }

    /// Remove an event handler from this dispatch list.
    ///
    /// Any listener that has the same handler and capture-phase flag will be
    /// removed from any priority in the list.
    pub fn remove_event_listener(
        &mut self,
        event: AvmString<'gc>,
        handler: FunctionObject<'gc>,
        use_capture: bool,
    ) {
        let old_handler = EventHandler::new(handler, use_capture);

        let remove_event = if let Some(event_sheaf) = self.0.get_mut(&event) {
            remove_handler_and_empty_priorities(event_sheaf, &old_handler);
            event_sheaf.is_empty()
        } else {
            false
        };

        if remove_event {
            self.0.remove(&event);
        }
    }

    /// Determine if there are any event listeners in this dispatch list.
    /// Whether any handler for `event` is registered for the given phase.
    ///
    /// Dispatch reaches capture handlers only during the capture phase and plain handlers only at
    /// target and during bubbling, so asking "is anyone listening" without saying which phase
    /// over-counts: a plain handler on an ancestor cannot be reached by an event that does not
    /// bubble.
    pub fn has_event_listener_in_phase(&self, event: AvmString<'gc>, use_capture: bool) -> bool {
        self.get_event(event)
            .into_iter()
            .flat_map(|event_sheaf| event_sheaf.values())
            .flat_map(|set| set.iter())
            .any(|handler| handler.use_capture == use_capture)
    }

    pub fn has_event_listener(&self, event: AvmString<'gc>) -> bool {
        if let Some(event_sheaf) = self.get_event(event) {
            for set in event_sheaf.values() {
                if !set.is_empty() {
                    return true;
                }
            }
        }

        false
    }

    /// Yield the event handlers on this dispatch list for a given event.
    ///
    /// Event handlers will be yielded in the order they are intended to be
    /// executed.
    ///
    /// `use_capture` indicates if you want handlers that execute during the
    /// capture phase, or handlers that execute during the bubble and target
    /// phases.
    pub fn iter_event_handlers<'a>(
        &'a self,
        event: AvmString<'gc>,
        use_capture: bool,
    ) -> impl 'a + Iterator<Item = FunctionObject<'gc>> {
        self.get_event(event)
            .into_iter()
            .flat_map(|event_sheaf| event_sheaf.iter().rev())
            .flat_map(|(_p, v)| v.iter())
            .filter(move |eh| eh.use_capture == use_capture)
            .map(|eh| eh.handler)
    }
}

impl Default for DispatchList<'_> {
    fn default() -> Self {
        Self::new()
    }
}

/// A single instance of an event handler.
#[derive(Clone, Collect)]
#[collect(no_drop)]
struct EventHandler<'gc> {
    /// The event handler to call.
    handler: FunctionObject<'gc>,

    /// Indicates if this handler should only be called for capturing events
    /// (when `true`), or if it should only be called for bubbling and
    /// at-target events (when `false`).
    use_capture: bool,
}

impl<'gc> EventHandler<'gc> {
    fn new(handler: FunctionObject<'gc>, use_capture: bool) -> Self {
        Self {
            handler,
            use_capture,
        }
    }
}

impl PartialEq for EventHandler<'_> {
    fn eq(&self, rhs: &Self) -> bool {
        self.use_capture == rhs.use_capture
            && std::ptr::eq(self.handler.as_ptr(), rhs.handler.as_ptr())
    }
}

impl Eq for EventHandler<'_> {}

impl Hash for EventHandler<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.use_capture.hash(state);
        self.handler.as_ptr().hash(state);
    }
}

/// Retrieve the parent of a given `EventDispatcher`.
///
/// `EventDispatcher` does not provide a generic way for it's subclasses to
/// indicate ancestry. Instead, only specific event targets provide a hierarchy
/// to traverse. If no hierarchy is available, this returns `None`, as if the
/// target had no parent.
pub fn parent_of(target: Object<'_>) -> Option<Object<'_>> {
    if let Some(dobj) = target.as_display_object()
        && let Some(dparent) = dobj.parent()
        && let Some(parent) = dparent.object2()
    {
        return Some(parent.into());
    }

    None
}

/// Call all of the event handlers on a given target.
///
/// The `target` is the current target of the `event`. `event` must be a valid
/// `EventObject`, or this function will panic. You must have already set the
/// event's phase to match what targets you are dispatching to, or you will
/// call the wrong handlers.
#[derive(Clone, Copy, Debug, Default)]
struct DispatchOutcome {
    had_handlers: bool,
    #[cfg_attr(not(feature = "aether_metrics"), allow(dead_code))]
    handler_snapshot_spilled: bool,
}

fn dispatch_event_to_target<'gc>(
    activation: &mut Activation<'_, 'gc>,
    dispatcher: Object<'gc>,
    real_target: Object<'gc>,
    current_target: Object<'gc>,
    event: EventObject<'gc>,
    simulate_dispatch: bool,
    profile_enter_frame_handlers: bool,
) -> DispatchOutcome {
    #[cfg(not(feature = "aether_metrics"))]
    let _ = profile_enter_frame_handlers;

    avm_debug!(
        activation.context.avm2,
        "Event dispatch: {} to {current_target:?}",
        event.event().event_type(),
    );

    let dispatch_list = dispatcher.get_slot(slots::DISPATCH_LIST).as_object();

    if dispatch_list.is_none() {
        // Objects with no dispatch list act as if they had an empty one
        return DispatchOutcome::default();
    }

    let dispatch_list = dispatch_list.unwrap();

    let mut evtmut = event.event_mut(activation.gc());
    let name = evtmut.event_type();
    let use_capture = evtmut.phase() == EventPhase::Capturing;

    let handlers = collect_handler_snapshot(
        dispatch_list
            .as_dispatch_mut(activation.gc())
            .expect("Internal dispatch list is missing during dispatch!")
            .iter_event_handlers(name, use_capture),
    );
    let had_handlers = !handlers.is_empty();

    let outcome = DispatchOutcome {
        had_handlers,
        handler_snapshot_spilled: handlers.spilled(),
    };

    if had_handlers {
        evtmut.set_target(real_target);
        evtmut.set_current_target(current_target);
    }

    drop(evtmut);

    if simulate_dispatch {
        return outcome;
    }

    if had_handlers {
        let global = activation.context.avm2.toplevel_global_object().unwrap();
        let args = [event.into()];

        for handler in &handlers {
            if event.event().is_propagation_stopped_immediately() {
                break;
            }

            #[cfg(feature = "aether_metrics")]
            let attributed_method =
                profile_enter_frame_handlers.then(|| handler.executable().as_method());
            #[cfg(feature = "aether_metrics")]
            let handler_started = attributed_method.map(|_| Instant::now());

            let result = handler.call(activation, global.into(), FunctionArgs::from_slice(&args));

            #[cfg(feature = "aether_metrics")]
            if let (Some(method), Some(handler_started)) = (attributed_method, handler_started) {
                crate::aether_diagnostics::record_enter_frame_handler(
                    method.diagnostic_identity(),
                    handler_started.elapsed(),
                    || {
                        let mut qualified_name = WString::new();
                        display_function(&mut qualified_name, method);
                        crate::aether_diagnostics::EnterFrameHandlerDescriptor {
                            qualified_name: qualified_name.to_utf8_lossy().into_owned(),
                            movie_url: method.owner_movie().url().to_string(),
                            abc_method_index: method.abc_method_index(),
                            kind: match method.method_kind() {
                                MethodKind::Native { .. } => "native",
                                MethodKind::Bytecode { .. } => "bytecode",
                            },
                        }
                    },
                );
            }

            if let Err(err) = result {
                let event_name = event.event().event_type();

                Avm2::uncaught_error(
                    activation,
                    None, // TODO we need to set this, but how?
                    err,
                    &format!("Error dispatching event \"{}\"", event_name),
                );
            }
        }
    }

    outcome
}

/// Whether an event of this type dispatched at `target` could reach any handler at all.
///
/// Constructing an event object is not free, and the display list dispatches four of them for
/// every child added to or removed from a parent. A room of animating avatars swaps its timeline
/// children every frame, which measured 3,540 bare events a second -- 97% of them `added`,
/// `removed`, `addedToStage` and `removedFromStage` -- against a game that listens for none of
/// them. Each one was allocated, walked through the hierarchy, found nobody, and was collected.
///
/// An event with no handler anywhere in its path is unobservable: dispatching runs no user code
/// and has no other effect, so skipping it is invisible to content.
///
/// Ancestors are checked for *every* event type, not just the bubbling ones, because the capture
/// phase visits them regardless: a `useCapture` listener upstream receives `addedToStage` even
/// though it never bubbles back up. But only capture handlers are reachable that way. A plain
/// handler on an ancestor is reached solely by the bubble phase, which a non-bubbling event never
/// runs, so counting those dispatches an event nothing can receive -- which is what a first
/// attempt at this did, leaving `addedToStage` and `removedFromStage` flowing at 912 a second
/// while `added` and `removed` went to zero.
pub fn has_listener_in_hierarchy<'gc>(
    target: Object<'gc>,
    event_type: AvmString<'gc>,
    bubbles: bool,
) -> bool {
    // Deliberately the looser test on the target itself: dispatch currently reaches only plain
    // handlers at target, but that is a detail of how the phase flag is derived rather than
    // something this guard should depend on. It is one lookup, and being wrong here would mean
    // silently dropping an event that a script did register for.
    if object_has_any_listener(target, event_type) {
        return true;
    }

    // Deliberately the same walk `dispatch_event` performs: up the *display object* parents,
    // considering only those whose AVM2 side has been constructed. Anything this misses would
    // also be missed by the dispatch it is guarding.
    let mut parent = target.as_display_object().and_then(|dobj| dobj.parent());
    while let Some(parent_dobj) = parent {
        if let Some(parent_obj) = parent_dobj.object2() {
            let ancestor: Object<'gc> = parent_obj.into();
            if object_has_listener_in_phase(ancestor, event_type, true)
                || (bubbles && object_has_listener_in_phase(ancestor, event_type, false))
            {
                return true;
            }
        }
        parent = parent_dobj.parent();
    }

    false
}

/// Whether this one object has a handler for `event_type` in either phase.
fn object_has_any_listener<'gc>(object: Object<'gc>, event_type: AvmString<'gc>) -> bool {
    object
        .get_slot(slots::DISPATCH_LIST)
        .as_object()
        .and_then(|list| list.as_dispatch())
        .is_some_and(|list| list.has_event_listener(event_type))
}

/// Whether this one object has a handler for `event_type` reachable in the given phase.
fn object_has_listener_in_phase<'gc>(
    object: Object<'gc>,
    event_type: AvmString<'gc>,
    use_capture: bool,
) -> bool {
    object
        .get_slot(slots::DISPATCH_LIST)
        .as_object()
        .and_then(|list| list.as_dispatch())
        .is_some_and(|list| list.has_event_listener_in_phase(event_type, use_capture))
}

pub fn dispatch_event<'gc>(
    activation: &mut Activation<'_, 'gc>,
    this: Object<'gc>,
    event: EventObject<'gc>,
    simulate_dispatch: bool,
) -> bool {
    let target = this.get_slot(slots::TARGET).as_object().unwrap_or(this);

    let mut ancestor_list = Vec::new();
    // Edge case - during button construction, we fire bubbling events for objects
    // that are in the hierarchy (and have `DisplayObject.stage` return the actual stage),
    // but do not yet have their *parent* object constructed. As a result, we walk through
    // the parent DisplayObject hierarchy, only adding ancestors that have objects constructed.
    let mut parent = target.as_display_object().and_then(|dobj| dobj.parent());
    while let Some(parent_dobj) = parent {
        if let Some(parent_obj) = parent_dobj.object2() {
            ancestor_list.push(parent_obj.into());
        }
        parent = parent_dobj.parent();
    }

    event
        .event_mut(activation.gc())
        .set_phase(EventPhase::Capturing);

    for ancestor in ancestor_list.iter().rev() {
        if event.event().is_propagation_stopped() {
            break;
        }

        dispatch_event_to_target(
            activation,
            *ancestor,
            target,
            *ancestor,
            event,
            simulate_dispatch,
            false,
        );
    }

    event
        .event_mut(activation.gc())
        .set_phase(EventPhase::AtTarget);

    if !event.event().is_propagation_stopped() {
        dispatch_event_to_target(
            activation,
            this,
            target,
            target,
            event,
            simulate_dispatch,
            false,
        );
    }

    event
        .event_mut(activation.context.gc_context)
        .set_phase(EventPhase::Bubbling);

    if event.event().is_bubbling() {
        for ancestor in ancestor_list.iter() {
            if event.event().is_propagation_stopped() {
                break;
            }

            dispatch_event_to_target(
                activation,
                *ancestor,
                target,
                *ancestor,
                event,
                simulate_dispatch,
                false,
            );
        }
    }

    // If the target is set, the event was handled
    event.event().target.is_some()
}

/// Like `dispatch_event`, but does not run the Capturing and Bubbling phases,
/// and dispatches the event regardless of whether propagation has been stopped.
/// This matches FP's event broadcasting logic.
pub fn broadcast_event<'gc>(
    activation: &mut Activation<'_, 'gc>,
    this: Object<'gc>,
    event: EventObject<'gc>,
    profile_enter_frame_handlers: bool,
) -> bool {
    let target = this.get_slot(slots::TARGET).as_object().unwrap_or(this);

    event
        .event_mut(activation.gc())
        .set_phase(EventPhase::AtTarget);

    let outcome = dispatch_event_to_target(
        activation,
        this,
        target,
        target,
        event,
        false,
        profile_enter_frame_handlers,
    );

    #[cfg(feature = "aether_metrics")]
    if outcome.handler_snapshot_spilled {
        crate::aether_metrics::avm2_broadcast_handler_snapshot_spilled();
    }

    outcome.had_handlers
}

pub fn has_event_listener<'gc>(
    mc: &Mutation<'gc>,
    dispatcher: Object<'gc>,
    event: AvmString<'gc>,
) -> bool {
    let Some(dispatch_list) = dispatcher.get_slot(slots::DISPATCH_LIST).as_object() else {
        return false;
    };
    dispatch_list
        .as_dispatch_mut(mc)
        .expect("Internal dispatch list is missing during dispatch!")
        .has_event_listener(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_snapshot_preserves_order_and_stays_inline_for_two_handlers() {
        let snapshot = collect_handler_snapshot([10_u8, 20]);
        assert_eq!(snapshot.as_slice(), &[10, 20]);
        assert!(!snapshot.spilled());
    }

    #[test]
    fn handler_snapshot_spills_without_losing_order_after_inline_capacity() {
        let snapshot = collect_handler_snapshot([10_u8, 20, 30]);
        assert_eq!(snapshot.as_slice(), &[10, 20, 30]);
        assert!(snapshot.spilled());
    }

    #[test]
    fn removing_handlers_prunes_empty_priority_vectors() {
        let mut priorities = BTreeMap::from([(10, vec![1_u8]), (0, vec![2, 3])]);
        remove_handler_and_empty_priorities(&mut priorities, &1);
        assert!(!priorities.contains_key(&10));
        assert_eq!(priorities.get(&0).unwrap(), &vec![2, 3]);
    }
}
