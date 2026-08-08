use crate::avm2::Multiname;
use crate::avm2::activation::Activation;
use crate::avm2::error::{Error, make_error_1063};
use crate::avm2::method::{Method, MethodKind, ParamConfig};
use crate::avm2::object::{ClassObject, FunctionObject};
use crate::avm2::scope::ScopeChain;
use crate::avm2::traits::TraitKind;
use crate::avm2::value::Value;
#[cfg(feature = "aether_diagnostics")]
use crate::string::AvmString;
use crate::string::WString;
use gc_arena::{Collect, Gc};
use std::borrow::Cow;
use std::cell::Cell;
use std::fmt;

/// Represents a bound method.
#[derive(Clone, Collect)]
#[collect(no_drop)]
pub struct BoundMethod<'gc> {
    /// The method code to execute from a given ABC file.
    method: Method<'gc>,

    /// The scope this method was defined in.
    scope: ScopeChain<'gc>,

    /// The receiver that this function is always called with.
    ///
    /// If `None`, then the receiver provided by the caller is used. A
    /// `Some` value indicates a bound executable.
    ///
    /// This should never be `Value::Null` or `Value::Undefined`.
    bound_receiver: Option<Value<'gc>>,

    /// The superclass of the bound class for this method.
    ///
    /// The `bound_superclass` is the superclass of the class that defined
    /// this method. If `None`, then there is no defining class and `super`
    /// operations should be invalid.
    bound_superclass: Option<ClassObject<'gc>>,
}

impl<'gc> BoundMethod<'gc> {
    pub fn from_method(
        method: Method<'gc>,
        scope: ScopeChain<'gc>,
        receiver: Option<Value<'gc>>,
        superclass: Option<ClassObject<'gc>>,
    ) -> Self {
        Self {
            method,
            scope,
            bound_receiver: receiver,
            bound_superclass: superclass,
        }
    }

    pub fn exec(
        &self,
        unbound_receiver: Value<'gc>,
        arguments: FunctionArgs<'_, 'gc>,
        activation: &mut Activation<'_, 'gc>,
        callee: Option<FunctionObject<'gc>>,
    ) -> Result<Value<'gc>, Error<'gc>> {
        let receiver = if let Some(receiver) = self.bound_receiver {
            receiver
        } else if matches!(unbound_receiver, Value::Null | Value::Undefined) {
            self.scope
                .get(0)
                .expect("No global scope for function call")
                .values()
        } else {
            unbound_receiver
        };

        exec(
            self.method,
            self.scope,
            receiver,
            self.bound_superclass,
            arguments,
            activation,
            callee,
        )
    }

    pub fn as_method(&self) -> Method<'gc> {
        self.method
    }

    pub fn debug_full_name(&self) -> WString {
        let mut output = WString::new();
        display_function(&mut output, self.as_method());
        output
    }

    pub fn signature(&self) -> &[ParamConfig<'gc>] {
        self.method.signature()
    }

    pub fn is_variadic(&self) -> bool {
        self.method.is_variadic()
    }

    pub fn return_type(&self) -> Option<Gc<'gc, Multiname<'gc>>> {
        self.method.return_type()
    }
}

#[derive(Clone, Copy)]
pub enum FunctionArgs<'a, 'gc> {
    AsCellArgs(&'a [Cell<Value<'gc>>]),
    AsArgs(&'a [Value<'gc>]),
}

impl<'a, 'gc> FunctionArgs<'a, 'gc> {
    pub fn empty() -> Self {
        FunctionArgs::AsArgs(&[])
    }

    pub fn from_slice(args: &'a [Value<'gc>]) -> Self {
        FunctionArgs::AsArgs(args)
    }

    pub fn from_cell_slice(args: &'a [Cell<Value<'gc>>]) -> Self {
        FunctionArgs::AsCellArgs(args)
    }

    pub fn to_slice(self) -> Cow<'a, [Value<'gc>]> {
        match self {
            FunctionArgs::AsCellArgs(arguments) => {
                Cow::Owned(arguments.iter().map(|o| o.get()).collect::<Vec<_>>())
            }
            FunctionArgs::AsArgs(arguments) => Cow::Borrowed(arguments),
        }
    }

    pub fn get_at(&self, index: usize) -> Value<'gc> {
        match self {
            FunctionArgs::AsCellArgs(arguments) => arguments[index].get(),
            FunctionArgs::AsArgs(arguments) => arguments[index],
        }
    }

    pub fn iter(&'a self) -> FunctionArgsIter<'a, 'gc> {
        FunctionArgsIter {
            args: self,
            next: 0,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            FunctionArgs::AsCellArgs(arguments) => arguments.len(),
            FunctionArgs::AsArgs(arguments) => arguments.len(),
        }
    }
}

pub struct FunctionArgsIter<'a, 'gc> {
    args: &'a FunctionArgs<'a, 'gc>,
    next: usize,
}

impl<'a, 'gc> Iterator for FunctionArgsIter<'a, 'gc> {
    type Item = Value<'gc>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next == self.args.len() {
            None
        } else {
            self.next += 1;
            Some(self.args.get_at(self.next - 1))
        }
    }
}

#[cfg(feature = "aether_diagnostics")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AetherMovementMethod {
    EnterFrameWalk,
    SimulateTo,
    WalkTo,
    StopWalking,
}

/// Classify a qualified ABC method name of the form `AvatarMC/method`.
///
/// AQW publishes private methods with a namespace qualifier on the member half, for example
/// `AvatarMC/private:onEnterFrameWalk`. The qualifier is stripped so both spellings resolve; the
/// class half must still be exactly `AvatarMC`.
#[cfg(feature = "aether_diagnostics")]
fn classify_aether_movement_method(method_name: &str) -> Option<AetherMovementMethod> {
    let (class_name, member) = method_name.split_once('/')?;
    if class_name != "AvatarMC" {
        return None;
    }
    let member = member.rsplit_once(':').map_or(member, |(_, name)| name);
    classify_bare_aether_movement_method(member)
}

/// The simple ABC names AQW uses when it does not publish a qualified `AvatarMC/method` name.
#[cfg(feature = "aether_diagnostics")]
fn classify_bare_aether_movement_method(method_name: &str) -> Option<AetherMovementMethod> {
    match method_name {
        "onEnterFrameWalk" => Some(AetherMovementMethod::EnterFrameWalk),
        "simulateTo" => Some(AetherMovementMethod::SimulateTo),
        "walkTo" => Some(AetherMovementMethod::WalkTo),
        "stopWalking" => Some(AetherMovementMethod::StopWalking),
        _ => None,
    }
}

#[cfg(feature = "aether_diagnostics")]
fn classify_aether_movement_method_for_parts(
    method_name: &str,
    bound_class: Option<(&str, bool)>,
) -> Option<AetherMovementMethod> {
    classify_aether_movement_method(method_name).or_else(|| {
        (bound_class == Some(("AvatarMC", true)))
            .then(|| classify_bare_aether_movement_method(method_name))
            .flatten()
    })
}

#[cfg(feature = "aether_diagnostics")]
fn ascii_contains_ignore_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

#[cfg(feature = "aether_diagnostics")]
fn classify_aether_movement_method_for(method: Method<'_>) -> Option<AetherMovementMethod> {
    let method_name = method.method_name();
    let bound_class = if classify_bare_aether_movement_method(method_name.as_ref()).is_some() {
        method.bound_class().and_then(|class| {
            let class_name = class.name();
            (class_name.local_name().as_wstr() == b"AvatarMC")
                .then_some(("AvatarMC", class_name.namespace().is_public()))
        })
    } else {
        None
    };
    let movement_method =
        classify_aether_movement_method_for_parts(method_name.as_ref(), bound_class)?;
    let owner_movie = method.owner_movie();
    let owner_url = owner_movie.url();
    if ascii_contains_ignore_case(owner_url, "spider.swf") {
        return Some(movement_method);
    }

    // A recognised AvatarMC movement method that fails the Spider URL gate would disable both the
    // movement trace and the stop guard with no other evidence. Report the owning movie once so a
    // capture can distinguish "AQW moved this class" from "the hook never matched at all".
    static REPORTED_FOREIGN_OWNER: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);
    if !REPORTED_FOREIGN_OWNER.swap(true, std::sync::atomic::Ordering::Relaxed) {
        tracing::warn!(
            method = %method_name,
            owner_url = %owner_url,
            "AQW movement method rejected: owning movie is not spider.swf"
        );
    }
    None
}

/// Execute a method.
///
/// The function will either be called directly if it is a Rust builtin, or
/// executed on the same AVM2 instance as the activation passed in here.
/// The value returned in either case will be provided here.
///
/// It is a panicking logic error to attempt to execute user code while any
/// reachable object is currently under a write lock.
///
/// Passed-in arguments will be conformed to the set of method parameters
/// declared on the function.
///
/// It is the caller's responsibility to ensure that the `receiver` passed
/// to this method is not Value::Null or Value::Undefined.
pub fn exec<'gc>(
    method: Method<'gc>,
    scope: ScopeChain<'gc>,
    receiver: Value<'gc>,
    bound_superclass: Option<ClassObject<'gc>>,
    arguments: FunctionArgs<'_, 'gc>,
    activation: &mut Activation<'_, 'gc>,
    callee: Option<FunctionObject<'gc>>,
) -> Result<Value<'gc>, Error<'gc>> {
    let mc = activation.gc();

    #[cfg(feature = "aether_compatibility")]
    let aether_aura_refresh_arguments = {
        let bound_class = method.bound_class();
        let bound_class_local_name = bound_class.map(|class| {
            class
                .name()
                .local_name()
                .as_wstr()
                .to_utf8_lossy()
                .into_owned()
        });
        let bound_class_is_public =
            bound_class.is_some_and(|class| class.name().namespace().is_public());
        let applies = crate::aether_compatibility::is_aqw_aura_refresh_target(
            method.owner_movie().url(),
            method.method_name().as_ref(),
            bound_class_local_name.as_deref(),
            bound_class_is_public,
        );
        (applies && arguments.len() >= 3).then(|| (arguments.get_at(1), arguments.get_at(2)))
    };

    #[cfg(feature = "aether_compatibility")]
    let aether_aura_insertion_argument = {
        let bound_class = method.bound_class();
        let bound_class_local_name = bound_class.map(|class| {
            class
                .name()
                .local_name()
                .as_wstr()
                .to_utf8_lossy()
                .into_owned()
        });
        let bound_class_is_public =
            bound_class.is_some_and(|class| class.name().namespace().is_public());
        let applies = crate::aether_compatibility::is_aqw_aura_insertion_target(
            method.owner_movie().url(),
            method.method_name().as_ref(),
            bound_class_local_name.as_deref(),
            bound_class_is_public,
        );
        (applies && arguments.len() >= 1).then(|| arguments.get_at(0))
    };

    #[cfg(feature = "aether_compatibility")]
    if let Some(response) = aether_aura_insertion_argument
        && let Err(error) =
            crate::aether_compatibility::repair_aqw_incoming_aura_timestamps(activation, response)
    {
        tracing::warn!(?error, "AQW incoming aura timestamp repair failed");
    }

    #[cfg(feature = "aether_compatibility")]
    let aether_equipment_initialization = {
        let bound_class = method.bound_class();
        let bound_class_local_name = bound_class.map(|class| {
            class
                .name()
                .local_name()
                .as_wstr()
                .to_utf8_lossy()
                .into_owned()
        });
        let bound_class_is_public =
            bound_class.is_some_and(|class| class.name().namespace().is_public());
        crate::aether_compatibility::is_aqw_equipment_initialization_target(
            method.owner_movie().url(),
            method.method_name().as_ref(),
            bound_class_local_name.as_deref(),
            bound_class_is_public,
        )
    };

    #[cfg(feature = "aether_diagnostics")]
    let aether_movement_method = crate::aether_diagnostics::movement_tracking_enabled()
        .then(|| classify_aether_movement_method_for(method))
        .flatten();

    #[cfg(feature = "aether_diagnostics")]
    let _aether_movement_frame_guard =
        if aether_movement_method == Some(AetherMovementMethod::EnterFrameWalk) {
            Some(crate::aether_diagnostics::enter_movement_frame(receiver))
        } else {
            None
        };

    #[cfg(feature = "aether_diagnostics")]
    let aether_movement_simulation = {
        fn numeric_argument(value: Value<'_>) -> Option<f64> {
            match value {
                Value::Number(value) => Some(value),
                Value::Integer(value) => Some(f64::from(value)),
                _ => None,
            }
        }

        match aether_movement_method {
            Some(AetherMovementMethod::SimulateTo) if arguments.len() >= 3 => {
                let requested_x = numeric_argument(arguments.get_at(0));
                let requested_y = numeric_argument(arguments.get_at(1));
                let speed = numeric_argument(arguments.get_at(2));
                match (requested_x, requested_y, speed) {
                    (Some(requested_x), Some(requested_y), Some(speed)) => {
                        Some(crate::aether_diagnostics::record_movement_simulate_start(
                            receiver,
                            requested_x,
                            requested_y,
                            speed,
                        ))
                    }
                    _ => None,
                }
            }
            Some(AetherMovementMethod::WalkTo) if arguments.len() >= 3 => {
                let target_x = numeric_argument(arguments.get_at(0));
                let target_y = numeric_argument(arguments.get_at(1));
                let speed = numeric_argument(arguments.get_at(2));
                if let (Some(target_x), Some(target_y), Some(speed)) = (target_x, target_y, speed) {
                    crate::aether_diagnostics::record_movement_walk_to(
                        receiver, target_x, target_y, speed,
                    );
                }
                None
            }
            Some(AetherMovementMethod::StopWalking) => {
                fn mc_char<'gc>(
                    receiver: Value<'gc>,
                    activation: &mut Activation<'_, 'gc>,
                ) -> Option<Value<'gc>> {
                    let mc_char_name = AvmString::new_utf8(activation.gc(), "mcChar");
                    let mc_char = receiver
                        .get_public_property(mc_char_name, activation)
                        .ok()?;
                    if matches!(mc_char, Value::Null | Value::Undefined) {
                        return None;
                    }
                    Some(mc_char)
                }

                fn mc_char_on_move<'gc>(
                    mc_char: Value<'gc>,
                    activation: &mut Activation<'_, 'gc>,
                ) -> Option<bool> {
                    let on_move_name = AvmString::new_utf8(activation.gc(), "onMove");
                    let on_move = mc_char.get_public_property(on_move_name, activation).ok()?;
                    Some(on_move.coerce_to_boolean())
                }

                fn resume_mc_char<'gc>(
                    mc_char: Value<'gc>,
                    activation: &mut Activation<'_, 'gc>,
                ) -> bool {
                    let on_move_name = AvmString::new_utf8(activation.gc(), "onMove");
                    mc_char
                        .set_public_property(on_move_name, Value::Bool(true), activation)
                        .is_ok()
                }

                let mc_char = mc_char(receiver, activation);
                let on_move = mc_char.and_then(|mc_char| mc_char_on_move(mc_char, activation));
                // The diagnostics runtime already requires an active, owner-matched
                // onEnterFrameWalk guard. Do not additionally require stopWalking's immediate
                // caller to be onEnterFrameWalk: AQW routes this branch through helper methods in
                // some equipment/map states, especially while a delayed frame is catching up.
                match crate::aether_diagnostics::movement_stop_guard_action_for_receiver(
                    receiver, on_move,
                ) {
                    crate::aether_diagnostics::MovementStopGuardAction::Allow => {}
                    crate::aether_diagnostics::MovementStopGuardAction::Suppress => {
                        return Ok(Value::Undefined);
                    }
                    crate::aether_diagnostics::MovementStopGuardAction::SuppressAndResume => {
                        if mc_char.is_some_and(|mc_char| resume_mc_char(mc_char, activation)) {
                            return Ok(Value::Undefined);
                        }
                    }
                }
                crate::aether_diagnostics::record_movement_stop(receiver);
                None
            }
            _ => None,
        }
    };

    let caller_dxns = activation.default_xml_namespace();

    let ret = match method.method_kind() {
        MethodKind::Native { native_method, .. } => {
            let caller_domain = activation.caller_domain();
            let caller_movie = activation.caller_movie();
            let mut activation = Activation::from_builtin(
                activation.context,
                bound_superclass,
                scope,
                caller_domain,
                caller_movie,
                caller_dxns,
            );

            method.resolve_info(&mut activation)?;

            let signature = method.resolved_param_config();

            // Check for too many arguments
            if arguments.len() > signature.len() && !method.is_variadic() && !method.is_unchecked()
            {
                return Err(make_error_1063(&mut activation, method, arguments.len()));
            }

            let arguments = activation.resolve_parameters(method, arguments, signature)?;

            #[cfg(feature = "tracy_avm")]
            let _span = {
                let mut name = WString::new();
                display_function(&mut name, method);
                let span = tracy_client::Client::running()
                    .expect("tracy_client should be running")
                    .span_alloc(None, &name.to_utf8_lossy(), "rust", 0, 0);
                span.emit_color(0x2c4980);
                span
            };

            activation.context.avm2.push_call(mc, method);

            native_method(&mut activation, receiver, &arguments)
        }
        MethodKind::Bytecode { .. } => {
            // We must initialize the stack frame here so the lifetime works out
            let stack = activation.context.avm2.stack;
            let stack_frame = stack.get_stack_frame(method);

            // This used to be a one step called Activation::from_method,
            // but avoiding moving an Activation around helps perf
            let mut activation = Activation::from_nothing(activation.context);
            if let Err(e) = activation.init_from_method(
                method,
                scope,
                receiver,
                arguments,
                stack_frame,
                bound_superclass,
                callee,
                caller_dxns,
            ) {
                // If an error is thrown during verification or argument coercion,
                // we still need to call cleanup to dispose of the stack frame
                activation.cleanup();
                return Err(e);
            }

            #[cfg(feature = "tracy_avm")]
            let _span = {
                let mut name = WString::new();
                display_function(&mut name, method);
                let option = tracy_client::Client::running();
                let span = option.expect("tracy_client should be running").span_alloc(
                    None,
                    &name.to_utf8_lossy(),
                    method.owner_movie().url(),
                    line!(),
                    0,
                );
                span.emit_color(0x425fa1);
                span
            };

            activation.context.avm2.push_call(mc, method);

            let result = activation.run_actions(method);

            activation.cleanup();

            result
        }
    };

    #[cfg(feature = "aether_compatibility")]
    if ret.is_ok()
        && let Some((incoming_aura, aura_collection_owner)) = aether_aura_refresh_arguments
        && let Err(error) = crate::aether_compatibility::repair_aqw_aura_refresh_timestamp(
            activation,
            incoming_aura,
            aura_collection_owner,
        )
    {
        tracing::warn!(?error, "AQW aura refresh timestamp repair failed");
    }

    #[cfg(feature = "aether_compatibility")]
    if ret.is_ok()
        && aether_equipment_initialization
        && let Err(error) =
            crate::aether_compatibility::schedule_aqw_equipment_recovery(activation, receiver)
    {
        tracing::warn!(?error, "AQW equipment recovery scheduling failed");
    }

    #[cfg(feature = "aether_diagnostics")]
    if let Some(sequence_id) = aether_movement_simulation {
        let result = match &ret {
            Ok(Value::Null) => "null",
            Ok(Value::Undefined) => "undefined",
            Ok(Value::Object(_)) => "object",
            Ok(_) => "value",
            Err(_) => "error",
        };
        crate::aether_diagnostics::record_movement_simulate_end(sequence_id, result);
    }

    activation.context.avm2.pop_call(mc);
    ret
}

impl fmt::Debug for BoundMethod<'_> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("BoundMethod")
            .field("method", &self.method)
            .field("scope", &self.scope)
            .field("receiver", &self.bound_receiver)
            .finish()
    }
}

pub fn display_function<'gc>(output: &mut WString, method: Method<'gc>) {
    let bound_class = method.bound_class();

    if let Some(bound_class) = bound_class {
        let name = bound_class.name().to_qualified_name_no_mc();
        output.push_str(&name);
    }

    // NOTE: The name of a bytecode method refers to the name of the trait that contains the method,
    // rather than the name of the method itself.
    if let Some(bound_class) = bound_class {
        if bound_class.instance_init() == Some(method) {
            if bound_class.is_c_class() {
                // If the associated class is a c_class, its initializer
                // method is a class initializer.
                output.push_utf8("cinit");
            }
            // We purposely do nothing for instance initializers
        } else {
            let mut method_trait = None;

            for t in bound_class.traits() {
                if t.as_method().is_some_and(|tm| tm == method) {
                    method_trait = Some(t);
                    break;
                }
            }

            if let Some(method_trait) = method_trait {
                output.push_char('/');
                match method_trait.kind() {
                    TraitKind::Setter { .. } => output.push_utf8("set "),
                    TraitKind::Getter { .. } => output.push_utf8("get "),
                    _ => (),
                }
                if method_trait.name().namespace().is_namespace() {
                    output.push_str(&method_trait.name().to_qualified_name_no_mc());
                } else {
                    output.push_str(&method_trait.name().local_name());
                }
            } else if !method.method_name().is_empty() {
                // Last resort if we can't find a name anywhere else.
                // SWF's with debug information will provide a method name attached
                // to the method definition, so we can use that.
                output.push_char('/');
                output.push_utf8(&method.method_name());
            }
            // TODO: What happens if we can't find the trait?
        }
    } else if method.is_function() && !method.method_name().is_empty() {
        output.push_utf8("Function/");
        output.push_utf8(&method.method_name());
    } else {
        output.push_utf8("MethodInfo-");
        output.push_utf8(&method.abc_method_index().to_string());
    }

    output.push_utf8("()");
}

#[cfg(all(test, feature = "aether_diagnostics"))]
mod aether_movement_tests {
    use super::*;

    #[test]
    fn movement_hook_classifies_only_exact_avatar_methods() {
        assert_eq!(
            classify_aether_movement_method("AvatarMC/onEnterFrameWalk"),
            Some(AetherMovementMethod::EnterFrameWalk)
        );
        assert_eq!(
            classify_aether_movement_method("AvatarMC/simulateTo"),
            Some(AetherMovementMethod::SimulateTo)
        );
        assert_eq!(
            classify_aether_movement_method("AvatarMC/walkTo"),
            Some(AetherMovementMethod::WalkTo)
        );
        assert_eq!(
            classify_aether_movement_method("AvatarMC/stopWalking"),
            Some(AetherMovementMethod::StopWalking)
        );
        assert_eq!(
            classify_aether_movement_method("PetMC/onEnterFrameWalk"),
            None
        );
        assert_eq!(
            classify_aether_movement_method("MonsterMC/onEnterFrameWalk"),
            None
        );
        assert_eq!(classify_aether_movement_method("onEnterFrameWalk"), None);
    }

    #[test]
    fn movement_hook_accepts_private_enter_frame_only_for_avatar_mc() {
        assert_eq!(
            classify_aether_movement_method_for_parts("onEnterFrameWalk", Some(("AvatarMC", true)),),
            Some(AetherMovementMethod::EnterFrameWalk)
        );
        assert_eq!(
            classify_aether_movement_method_for_parts("onEnterFrameWalk", Some(("PetMC", true)),),
            None
        );
        assert_eq!(
            classify_aether_movement_method_for_parts(
                "onEnterFrameWalk",
                Some(("MonsterMC", true)),
            ),
            None
        );
        assert_eq!(
            classify_aether_movement_method_for_parts(
                "onEnterFrameWalk",
                Some(("AvatarMC", false)),
            ),
            None
        );
        assert_eq!(
            classify_aether_movement_method_for_parts("onEnterFrameWalk", None),
            None
        );
    }

    #[test]
    fn movement_hook_accepts_namespace_qualified_avatar_method_names() {
        // Ground truth from the live spider.swf ABC constant pool: AQW publishes
        // `AvatarMC/private:onEnterFrameWalk`, not `AvatarMC/onEnterFrameWalk`.
        assert_eq!(
            classify_aether_movement_method("AvatarMC/private:onEnterFrameWalk"),
            Some(AetherMovementMethod::EnterFrameWalk)
        );
        assert_eq!(
            classify_aether_movement_method("AvatarMC/private:stopWalking"),
            Some(AetherMovementMethod::StopWalking)
        );
        assert_eq!(
            classify_aether_movement_method("PetMC/private:onEnterFrameWalk"),
            None,
            "a qualifier must not let a non-avatar class through"
        );
        assert_eq!(
            classify_aether_movement_method("AvatarMC/private:somethingElse"),
            None,
            "only the four movement methods are tracked"
        );
    }

    #[test]
    fn movement_hook_accepts_bare_avatar_method_names() {
        // AQW does not always publish the qualified `AvatarMC/method` ABC name. When only the
        // simple name is present, the public `AvatarMC` bound class is the identifying evidence,
        // exactly as it already is for `onEnterFrameWalk`.
        for (method_name, expected) in [
            ("simulateTo", AetherMovementMethod::SimulateTo),
            ("walkTo", AetherMovementMethod::WalkTo),
            ("stopWalking", AetherMovementMethod::StopWalking),
        ] {
            assert_eq!(
                classify_aether_movement_method_for_parts(method_name, Some(("AvatarMC", true))),
                Some(expected),
                "bare {method_name} on a public AvatarMC must be tracked"
            );
            assert_eq!(
                classify_aether_movement_method_for_parts(method_name, Some(("PetMC", true))),
                None,
                "bare {method_name} must not be claimed for a non-avatar class"
            );
            assert_eq!(
                classify_aether_movement_method_for_parts(method_name, Some(("AvatarMC", false))),
                None,
                "bare {method_name} must require the public AvatarMC namespace"
            );
            assert_eq!(
                classify_aether_movement_method_for_parts(method_name, None),
                None,
                "bare {method_name} without a bound class must not be tracked"
            );
        }
    }

    #[test]
    fn movement_hook_matches_spider_url_without_lowercase_allocation() {
        assert!(ascii_contains_ignore_case(
            "https://game.aq.com/game/gamefiles/Loader_Spider.swf",
            "spider.swf"
        ));
        assert!(ascii_contains_ignore_case(
            "file:///SPIDER.SWF",
            "spider.swf"
        ));
        assert!(!ascii_contains_ignore_case(
            "https://game.aq.com/game/gamefiles/other.swf",
            "spider.swf"
        ));
    }
}
