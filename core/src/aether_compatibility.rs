//! Narrow compatibility repairs used by Aether without enabling diagnostic instrumentation.

use crate::avm2::{
    Activation as Avm2Activation, FunctionArgs as Avm2FunctionArgs, Multiname as Avm2Multiname,
    TObject as _, Value as Avm2Value,
};
use crate::context::UpdateContext;
use crate::display_object::{
    Avm2LifecycleTraversal, DisplayObject, MovieClip, TDisplayObject, TDisplayObjectContainer,
};
use crate::locale::get_current_date_time;
use crate::string::AvmString;
use crate::timer::TimerCallback;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

static TIMELINE_CHILD_REBIND_ENABLED: AtomicBool = AtomicBool::new(false);

#[inline]
pub fn set_timeline_child_rebind_enabled(enabled: bool) {
    TIMELINE_CHILD_REBIND_ENABLED.store(enabled, Ordering::Relaxed);
}

#[inline]
pub fn timeline_child_rebind_enabled() -> bool {
    TIMELINE_CHILD_REBIND_ENABLED.load(Ordering::Relaxed)
}

fn contains_ascii_case_insensitive(haystack: &str, needle: &str) -> bool {
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

#[inline]
fn is_aqw_aura_mask_segment_parts(
    parent_movie_url: &str,
    parent_class_local_name: &str,
    child_name: Option<&str>,
    is_direct_child: bool,
) -> bool {
    is_direct_child
        && contains_ascii_case_insensitive(
            parent_movie_url,
            "game.aq.com/game/gamefiles/spider.swf",
        )
        && parent_class_local_name == "ActMaskReverse"
        && matches!(child_name, Some("e0" | "e1" | "e2" | "e3"))
}

#[inline]
pub(crate) fn is_aqw_spellcraft_drag_timer_target(
    movie_url: &str,
    method_name: &str,
    bound_class_local_name: Option<&str>,
    bound_class_has_public_namespace: bool,
) -> bool {
    if !contains_ascii_case_insensitive(
        movie_url,
        "game.aq.com/game/gamefiles/maps/tradeskills/spellcraft/game-spellcraftr2.swf",
    ) {
        return false;
    }

    method_name.ends_with("scGame_1/frame6")
        || (method_name == "frame6"
            && bound_class_local_name == Some("scGame_1")
            && bound_class_has_public_namespace)
}

#[inline]
fn aqw_spellcraft_drag_delay_ms() -> f64 {
    1_000.0 / 60.0
}

#[inline]
pub(crate) fn is_aqw_spellcraft_drop_target(
    movie_url: &str,
    method_name: &str,
    bound_class_local_name: Option<&str>,
    bound_class_has_public_namespace: bool,
) -> bool {
    if !contains_ascii_case_insensitive(
        movie_url,
        "game.aq.com/game/gamefiles/maps/tradeskills/spellcraft/game-spellcraftr2.swf",
    ) {
        return false;
    }

    method_name.ends_with("scGame_1/DragStop")
        || (method_name == "DragStop"
            && bound_class_local_name == Some("scGame_1")
            && bound_class_has_public_namespace)
}

#[inline]
fn select_aqw_spellcraft_effect_target(
    hit_targets: [bool; 5],
    pickup_point: Option<&str>,
) -> Option<u8> {
    hit_targets
        .iter()
        .position(|hit| *hit)
        .map(|index| index as u8 + 1)
        .or_else(|| {
            pickup_point
                .and_then(|name| name.strip_prefix("mcTarget"))
                .and_then(|index| index.parse::<u8>().ok())
                .filter(|index| (1..=5).contains(index))
        })
}

#[inline]
pub(crate) fn is_aqw_valiance_track_target(
    movie_url: &str,
    method_name: &str,
    bound_class_local_name: Option<&str>,
    receiver_class_local_name: Option<&str>,
) -> bool {
    let (method_owner, method_member) = method_name
        .rsplit_once('/')
        .map_or((None, method_name), |(owner, member)| (Some(owner), member));
    let method_member = method_member
        .rsplit_once(':')
        .map_or(method_member, |(_, member)| member);
    let method_owner_is_spell_w = method_owner.is_some_and(|owner| {
        owner == "SpellW"
            || owner
                .rsplit_once("::")
                .is_some_and(|(_, name)| name == "SpellW")
    });

    contains_ascii_case_insensitive(
        movie_url,
        "game.aq.com/game/gamefiles/assets/assets_2026.swf",
    ) && method_member == "trackTC"
        && (bound_class_local_name == Some("SpellW") || method_owner_is_spell_w)
        && receiver_class_local_name == Some("sp_qchronoa2")
}

#[inline]
fn aqw_valiance_y_offset() -> f64 {
    // Valiance spans roughly -282 to +108 pixels around AQW's foot-level target origin. Moving
    // the tracked effect down 36 pixels centers its main artwork over the avatar instead of above it.
    36.0
}

pub(crate) fn is_aqw_aura_refresh_target(
    movie_url: &str,
    method_name: &str,
    bound_class_local_name: Option<&str>,
    bound_class_is_public: bool,
) -> bool {
    contains_ascii_case_insensitive(movie_url, "spider.swf")
        && matches!(method_name, "World/updateAuraData" | "updateAuraData")
        && bound_class_local_name == Some("World")
        && bound_class_is_public
}

pub(crate) fn is_aqw_aura_insertion_target(
    movie_url: &str,
    method_name: &str,
    bound_class_local_name: Option<&str>,
    bound_class_is_public: bool,
    bound_class_has_public_namespace: bool,
) -> bool {
    if !contains_ascii_case_insensitive(movie_url, "spider.swf") {
        return false;
    }

    if matches!(method_name, "World/showAuraChange" | "showAuraChange") {
        return bound_class_local_name == Some("World") && bound_class_is_public;
    }

    if method_name.ends_with("playerAuras/handleAura")
        || method_name.ends_with("targetAuras/handleAura")
    {
        return true;
    }

    match bound_class_local_name {
        Some("playerAuras") => {
            aura_insertion_namespace_is_allowed(
                bound_class_is_public,
                bound_class_has_public_namespace,
            ) && method_name == "handleAura"
        }
        Some("targetAuras") => {
            aura_insertion_namespace_is_allowed(
                bound_class_is_public,
                bound_class_has_public_namespace,
            ) && method_name == "handleAura"
        }
        _ => false,
    }
}

pub(crate) fn is_aqw_aura_countdown_target(
    movie_url: &str,
    method_name: &str,
    bound_class_local_name: Option<&str>,
    bound_class_is_public: bool,
    bound_class_has_public_namespace: bool,
) -> bool {
    if !contains_ascii_case_insensitive(movie_url, "spider.swf") {
        return false;
    }

    if method_name.ends_with("playerAuras/countDownAct")
        || method_name.ends_with("targetAuras/countDownAct")
    {
        return true;
    }

    method_name == "countDownAct"
        && matches!(bound_class_local_name, Some("playerAuras" | "targetAuras"))
        && aura_insertion_namespace_is_allowed(
            bound_class_is_public,
            bound_class_has_public_namespace,
        )
}

#[inline]
fn aura_insertion_namespace_is_allowed(
    bound_class_is_public: bool,
    bound_class_has_public_namespace: bool,
) -> bool {
    bound_class_is_public || bound_class_has_public_namespace
}

#[inline]
fn is_plausible_aqw_aura_timestamp(timestamp: f64) -> bool {
    const MIN_PLAUSIBLE_UNIX_TIMESTAMP_MS: f64 = 1_000_000_000_000.0;

    timestamp.is_finite() && timestamp >= MIN_PLAUSIBLE_UNIX_TIMESTAMP_MS
}

#[inline]
fn should_repair_aqw_incoming_aura_timestamp(is_passive: bool, timestamp: f64) -> bool {
    !is_passive && !is_plausible_aqw_aura_timestamp(timestamp)
}

#[inline]
fn aura_refresh_identity_matches(
    name_matches: bool,
    caster_type_matches: bool,
    caster_id_matches: bool,
) -> bool {
    name_matches && caster_type_matches && caster_id_matches
}

#[inline]
fn aura_countdown_child_needs_rebind(
    field_is_display_object: bool,
    field_matches_current_child: bool,
) -> bool {
    !field_is_display_object || !field_matches_current_child
}

#[inline]
fn select_aura_refresh_timestamp(incoming_timestamp: f64, current_timestamp: f64) -> f64 {
    if is_plausible_aqw_aura_timestamp(incoming_timestamp) {
        incoming_timestamp
    } else {
        current_timestamp
    }
}

/// Return whether an explicit goto is advancing one of AQW's four aura countdown mask segments.
///
/// AQW traces show these physical children inheriting unrelated skill frame scripts.
/// Their timeline graphics still need to advance, but those unrelated scripts must not run.
pub(crate) fn is_aqw_aura_mask_segment(clip: MovieClip<'_>) -> bool {
    let Some(parent) = clip.parent() else {
        return false;
    };
    let Some(parent_object) = parent.object2() else {
        return false;
    };
    let child_name = clip
        .has_explicit_name()
        .then(|| clip.name().map(|name| name.to_string()))
        .flatten();
    let parent_class_local_name = parent_object
        .instance_class()
        .name()
        .local_name()
        .as_wstr()
        .to_string();

    is_aqw_aura_mask_segment_parts(
        parent.movie().url(),
        &parent_class_local_name,
        child_name.as_deref(),
        true,
    )
}

pub(crate) fn smooth_aqw_spellcraft_drag_timer<'gc>(
    activation: &mut Avm2Activation<'_, 'gc>,
    receiver: Avm2Value<'gc>,
) -> Result<bool, crate::avm2::Error<'gc>> {
    let drag_timer = receiver.get_public_property(
        AvmString::new_utf8(activation.gc(), "dragTimer"),
        activation,
    )?;
    if matches!(drag_timer, Avm2Value::Null | Avm2Value::Undefined) {
        return Ok(false);
    }

    drag_timer.set_public_property(
        AvmString::new_utf8(activation.gc(), "delay"),
        Avm2Value::Number(aqw_spellcraft_drag_delay_ms()),
        activation,
    )?;
    Ok(true)
}

pub(crate) fn capture_aqw_spellcraft_effect_target<'gc>(
    activation: &mut Avm2Activation<'_, 'gc>,
    receiver: Avm2Value<'gc>,
) -> Result<Option<u8>, crate::avm2::Error<'gc>> {
    fn property<'gc>(
        activation: &mut Avm2Activation<'_, 'gc>,
        receiver: Avm2Value<'gc>,
        name: &str,
    ) -> Result<Avm2Value<'gc>, crate::avm2::Error<'gc>> {
        receiver.get_public_property(AvmString::new_utf8(activation.gc(), name), activation)
    }

    let word_list = property(activation, receiver, "mcWordList")?;
    let pickup_point = property(activation, receiver, "strPickupPoint")?
        .coerce_to_string(activation)?
        .to_utf8_lossy()
        .into_owned();
    let mut hit_targets = [false; 5];
    for (index, hit) in hit_targets.iter_mut().enumerate() {
        let target = property(activation, receiver, &format!("mcTarget{}", index + 1))?;
        *hit = word_list
            .call_public_property(
                AvmString::new_utf8(activation.gc(), "hitTestObject"),
                Avm2FunctionArgs::from_slice(&[target]),
                activation,
            )?
            .coerce_to_boolean();
    }

    Ok(select_aqw_spellcraft_effect_target(
        hit_targets,
        Some(&pickup_point),
    ))
}

pub(crate) fn focus_aqw_spellcraft_effect<'gc>(
    activation: &mut Avm2Activation<'_, 'gc>,
    receiver: Avm2Value<'gc>,
    target_index: u8,
) -> Result<(), crate::avm2::Error<'gc>> {
    // AQW restarts mcGlow on every recipe-compatible occupied slot after each drop. Keep the
    // feedback on the slot that actually changed so stale recipe glows do not appear elsewhere.
    for index in 1..=5 {
        let target = receiver.get_public_property(
            AvmString::new_utf8(activation.gc(), format!("mcTarget{index}")),
            activation,
        )?;
        let glow = target
            .get_public_property(AvmString::new_utf8(activation.gc(), "mcGlow"), activation)?;
        let (method, frame) = if index == target_index {
            ("gotoAndPlay", "Play")
        } else {
            ("gotoAndStop", "Init")
        };
        let frame = Avm2Value::from(AvmString::new_utf8(activation.gc(), frame));
        glow.call_public_property(
            AvmString::new_utf8(activation.gc(), method),
            Avm2FunctionArgs::from_slice(&[frame]),
            activation,
        )?;
    }
    Ok(())
}

pub(crate) fn offset_aqw_valiance_effect<'gc>(
    activation: &mut Avm2Activation<'_, 'gc>,
    receiver: Avm2Value<'gc>,
) -> Result<bool, crate::avm2::Error<'gc>> {
    let y_name = AvmString::new_utf8(activation.gc(), "y");
    let y = receiver
        .get_public_property(y_name, activation)?
        .coerce_to_number(activation)?;
    receiver.set_public_property(
        y_name,
        Avm2Value::Number(y + aqw_valiance_y_offset()),
        activation,
    )?;
    Ok(true)
}

/// Give each newly received timed aura a valid application timestamp before Spider creates its
/// countdown entry. Spider normally writes this field only when the optional `t` field is present;
/// without it, the countdown treats the aura as already expired.
pub(crate) fn repair_aqw_incoming_aura_timestamps<'gc>(
    activation: &mut Avm2Activation<'_, 'gc>,
    response: Avm2Value<'gc>,
) -> Result<usize, crate::avm2::Error<'gc>> {
    fn property<'gc>(
        activation: &mut Avm2Activation<'_, 'gc>,
        object: Avm2Value<'gc>,
        name: &'static str,
    ) -> Result<Avm2Value<'gc>, crate::avm2::Error<'gc>> {
        object.get_public_property(AvmString::new_utf8(activation.gc(), name), activation)
    }

    let timestamp = Avm2Value::Number(get_current_date_time().timestamp_millis() as f64);
    let mut repaired = 0_usize;

    fn repair_aura<'gc>(
        activation: &mut Avm2Activation<'_, 'gc>,
        aura: Avm2Value<'gc>,
        command: AvmString<'gc>,
        timestamp: Avm2Value<'gc>,
    ) -> Result<bool, crate::avm2::Error<'gc>> {
        let is_passive = command.as_wstr() == b"aura+p";
        let current_timestamp = property(activation, aura, "ts")?.coerce_to_number(activation)?;
        if should_repair_aqw_incoming_aura_timestamp(is_passive, current_timestamp) {
            aura.set_public_property(
                AvmString::new_utf8(activation.gc(), "ts"),
                timestamp,
                activation,
            )?;
            return Ok(true);
        }

        Ok(false)
    }

    let auras = property(activation, response, "auras")?;
    if let Some(auras) = auras.as_object() {
        let mut index = auras.get_next_enumerant(0, activation)?;
        while index != 0 {
            let aura = auras.get_enumerant_value(index, activation)?;
            let command = property(activation, aura, "cmd")?.coerce_to_string(activation)?;
            repaired += usize::from(repair_aura(activation, aura, command, timestamp)?);
            index = auras.get_next_enumerant(index, activation)?;
        }
    }

    // The optional aura UI also consumes the raw `resObj.a` packet. Repair both packet layouts
    // before `playerAuras` or `targetAuras` copies `ts` into its countdown state.
    let actions = property(activation, response, "a")?;
    if let Some(actions) = actions.as_object() {
        let mut action_index = actions.get_next_enumerant(0, activation)?;
        while action_index != 0 {
            let action = actions.get_enumerant_value(action_index, activation)?;
            let command = property(activation, action, "cmd")?.coerce_to_string(activation)?;
            let action_auras = property(activation, action, "auras")?;
            if let Some(action_auras) = action_auras.as_object() {
                let mut aura_index = action_auras.get_next_enumerant(0, activation)?;
                while aura_index != 0 {
                    let aura = action_auras.get_enumerant_value(aura_index, activation)?;
                    repaired += usize::from(repair_aura(activation, aura, command, timestamp)?);
                    aura_index = action_auras.get_next_enumerant(aura_index, activation)?;
                }
            } else {
                let aura = property(activation, action, "aura")?;
                if aura.as_object().is_some() {
                    repaired += usize::from(repair_aura(activation, aura, command, timestamp)?);
                }
            }
            action_index = actions.get_next_enumerant(action_index, activation)?;
        }
    }

    Ok(repaired)
}

/// Restore the four named timeline children used by AQW's optional aura countdown mask.
///
/// Some AQW sessions resolve an `ActMaskReverse.eN` field to an unrelated avatar or skill clip.
/// The countdown then drives that clip with `gotoAndStop`, running arbitrary frame scripts and
/// aborting the aura update. Rebind only this mask class and only to its currently-present direct
/// children immediately before Spider's countdown handler executes.
pub(crate) fn repair_aqw_aura_countdown_mask<'gc>(
    activation: &mut Avm2Activation<'_, 'gc>,
    event: Avm2Value<'gc>,
) -> Result<usize, crate::avm2::Error<'gc>> {
    let Some(event_object) = event.as_object() else {
        return Ok(0);
    };
    let target = event_object.as_event().and_then(|event| event.target());
    let Some(target) = target else {
        return Ok(0);
    };

    let icon2 = Avm2Value::from(target)
        .get_public_property(AvmString::new_utf8(activation.gc(), "icon2"), activation)?;
    let Some(icon2) = icon2
        .as_object()
        .and_then(|object| object.as_display_object())
    else {
        return Ok(0);
    };
    let Some(mask) = icon2.masker() else {
        return Ok(0);
    };
    let Some(mask_object) = mask.object2() else {
        return Ok(0);
    };
    if mask_object.instance_class().name().local_name().as_wstr() != b"ActMaskReverse" {
        return Ok(0);
    }
    let Some(mask_container) = mask.as_container() else {
        return Ok(0);
    };

    let mask_value = Avm2Value::from(mask_object);
    let mut repaired = 0_usize;
    for child_name in ["e0", "e1", "e2", "e3"] {
        let name = AvmString::new_utf8(activation.gc(), child_name);
        let Some(actual_child) = mask_container.child_by_name(name.as_wstr(), true) else {
            continue;
        };
        let Some(actual_child_object) = actual_child.object2() else {
            continue;
        };

        let multiname = Avm2Multiname::new(activation.avm2().find_public_namespace(), name);
        let current = mask_value.get_property(&multiname, activation)?;
        let current_display = current
            .as_object()
            .and_then(|object| object.as_display_object());
        let field_is_display_object = current_display.is_some();
        let field_matches_current_child =
            current_display.is_some_and(|current| current.id() == actual_child.id());
        if aura_countdown_child_needs_rebind(field_is_display_object, field_matches_current_child) {
            mask_value.set_property(
                &multiname,
                Avm2Value::from(actual_child_object),
                activation,
            )?;
            repaired = repaired.saturating_add(1);
        }
    }

    Ok(repaired)
}

/// Copy Spider's refresh timestamp into the existing aura entry.
///
/// `World/updateAuraData` updates `dur` and `val`, but not `ts`. The UI therefore measures a
/// refreshed aura from its original application time and can remove it immediately. Prefer the
/// timestamp Spider supplied, and use the same clock as `Date` when that field was omitted. This
/// repairs only the entry selected by Spider's own `(nam, casterType, casterId)` identity.
pub(crate) fn repair_aqw_aura_refresh_timestamp<'gc>(
    activation: &mut Avm2Activation<'_, 'gc>,
    incoming_aura: Avm2Value<'gc>,
    aura_collection_owner: Avm2Value<'gc>,
) -> Result<usize, crate::avm2::Error<'gc>> {
    fn property<'gc>(
        activation: &mut Avm2Activation<'_, 'gc>,
        object: Avm2Value<'gc>,
        name: &'static str,
    ) -> Result<Avm2Value<'gc>, crate::avm2::Error<'gc>> {
        object.get_public_property(AvmString::new_utf8(activation.gc(), name), activation)
    }

    let incoming_timestamp =
        property(activation, incoming_aura, "ts")?.coerce_to_number(activation)?;
    let refresh_timestamp = Avm2Value::Number(select_aura_refresh_timestamp(
        incoming_timestamp,
        get_current_date_time().timestamp_millis() as f64,
    ));

    let incoming_name = property(activation, incoming_aura, "nam")?;
    let incoming_caster_type = property(activation, incoming_aura, "casterType")?;
    let incoming_caster_id = property(activation, incoming_aura, "casterId")?;
    let auras = property(activation, aura_collection_owner, "auras")?;
    let Some(auras) = auras.as_object() else {
        return Ok(0);
    };

    let mut repaired = 0_usize;
    let mut index = auras.get_next_enumerant(0, activation)?;
    while index != 0 {
        let existing = auras.get_enumerant_value(index, activation)?;
        let existing_name = property(activation, existing, "nam")?;
        let existing_caster_type = property(activation, existing, "casterType")?;
        let existing_caster_id = property(activation, existing, "casterId")?;

        let name_matches = existing_name.abstract_eq(&incoming_name, activation)?;
        let caster_type_matches =
            existing_caster_type.abstract_eq(&incoming_caster_type, activation)?;
        let caster_id_matches = existing_caster_id.abstract_eq(&incoming_caster_id, activation)?;
        if aura_refresh_identity_matches(name_matches, caster_type_matches, caster_id_matches) {
            existing.set_public_property(
                AvmString::new_utf8(activation.gc(), "ts"),
                refresh_timestamp,
                activation,
            )?;
            repaired = repaired.saturating_add(1);
        }

        index = auras.get_next_enumerant(index, activation)?;
    }

    Ok(repaired)
}

const AQW_EQUIPMENT_RECOVERY_TIMEOUT_MS: i32 = 30_000;

pub(crate) fn is_aqw_equipment_initialization_target(
    movie_url: &str,
    method_name: &str,
    bound_class_local_name: Option<&str>,
    bound_class_is_public: bool,
) -> bool {
    contains_ascii_case_insensitive(movie_url, "game.aq.com/game/gamefiles/spider.swf")
        && method_name == "Avatar/initAvatar"
        && bound_class_local_name == Some("Avatar")
        && bound_class_is_public
}

#[inline]
fn should_schedule_aqw_equipment_recovery(
    is_my_avatar: bool,
    first_load: bool,
    is_initializing_equipment: bool,
    pending_equipment_count: usize,
) -> bool {
    is_my_avatar && first_load && !is_initializing_equipment && pending_equipment_count > 0
}

/// Schedule Spider's own equipment error-recovery method after a first load stops progressing.
///
/// Spider removes pending equipment slots from successful loader callbacks. Its I/O error handlers
/// call `markAnyEquipmentLoaded` to clear the remaining slots, but a loader that never reaches a
/// terminal event leaves the local avatar's loading animation running forever. The bound callback
/// is harmless after a normal load: `firstLoad` is already false and the pending dictionary is
/// empty, so Spider performs no completion work.
pub(crate) fn schedule_aqw_equipment_recovery<'gc>(
    activation: &mut Avm2Activation<'_, 'gc>,
    receiver: Avm2Value<'gc>,
) -> Result<bool, crate::avm2::Error<'gc>> {
    fn property<'gc>(
        activation: &mut Avm2Activation<'_, 'gc>,
        receiver: Avm2Value<'gc>,
        name: &'static str,
    ) -> Result<Avm2Value<'gc>, crate::avm2::Error<'gc>> {
        receiver.get_public_property(AvmString::new_utf8(activation.gc(), name), activation)
    }

    let is_my_avatar = property(activation, receiver, "isMyAvatar")?.coerce_to_boolean();
    let first_load = property(activation, receiver, "firstLoad")?.coerce_to_boolean();
    let is_initializing_equipment =
        property(activation, receiver, "isInitializingEquipment")?.coerce_to_boolean();
    let pending_equipment = property(activation, receiver, "pendingEquipment")?;
    let pending_equipment_count = if let Some(pending_equipment) = pending_equipment.as_object() {
        let mut count = 0_usize;
        let mut index = pending_equipment.get_next_enumerant(0, activation)?;
        while index != 0 {
            count = count.saturating_add(1);
            index = pending_equipment.get_next_enumerant(index, activation)?;
        }
        count
    } else {
        0
    };

    if !should_schedule_aqw_equipment_recovery(
        is_my_avatar,
        first_load,
        is_initializing_equipment,
        pending_equipment_count,
    ) {
        return Ok(false);
    }

    let recovery = property(activation, receiver, "markAnyEquipmentLoaded")?;
    let Some(recovery) = recovery
        .as_object()
        .and_then(|object| object.as_function_object())
    else {
        return Ok(false);
    };

    activation.context.timers.add_timer(
        TimerCallback::Avm2Callback {
            closure: Some(recovery),
            params: Vec::new(),
        },
        AQW_EQUIPMENT_RECOVERY_TIMEOUT_MS,
        true,
    );
    Ok(true)
}

fn is_timeline_child_rebind_target(
    movie_url: &str,
    class_local_name: &str,
    has_nonempty_class_namespace: bool,
) -> bool {
    contains_ascii_case_insensitive(movie_url, "spider.swf")
        && class_local_name == "mcOption"
        && !has_nonempty_class_namespace
}

fn is_aqw_crafting_frame_target(
    movie_url: &str,
    class_namespace_uri: Option<&str>,
    class_local_name: &str,
) -> bool {
    let is_aqw_asset = contains_ascii_case_insensitive(movie_url, "game.aq.com/game/gamefiles/");
    is_aqw_asset
        && ((contains_ascii_case_insensitive(movie_url, "spellcraft")
            && class_namespace_uri == Some("game_fla")
            && class_local_name == "mcInfoOverlay_233")
            || (contains_ascii_case_insensitive(movie_url, "alchemy")
                && class_namespace_uri == Some("alchemyGame_v4_fla")
                && class_local_name == "mcInfoOverlay_388"))
}

#[inline]
fn should_force_aqw_crafting_child(
    target_clip: bool,
    has_explicit_name: bool,
    has_avm2_object: bool,
) -> bool {
    target_clip && has_explicit_name && !has_avm2_object
}

pub fn crafting_frame_construction_applies(clip: MovieClip<'_>) -> bool {
    let Some(object) = clip.object2() else {
        return false;
    };
    let class_name = object.instance_class().name();
    let class_namespace_uri = class_name
        .namespace()
        .as_uri_opt()
        .map(|uri| uri.to_string());
    let class_local_name = class_name.local_name().as_wstr().to_string();
    is_aqw_crafting_frame_target(
        clip.movie().url(),
        class_namespace_uri.as_deref(),
        &class_local_name,
    )
}

/// Construct and bind the named direct children used by the two AQW crafting overlays.
///
/// Their generated `__setProp__` frame methods immediately dereference timeline fields such as
/// `_id6.strAction` and `_id3.strFrame`. A re-entrant construction pass can leave those fields null
/// even though the child is present in the display list. Repair before the frame script starts so
/// no partially executed script is retried.
pub fn prepare_aqw_crafting_frame_children<'gc>(
    context: &mut UpdateContext<'gc>,
    clip: MovieClip<'gc>,
) -> TimelineChildRebindSummary {
    let mut summary = TimelineChildRebindSummary::default();
    if !crafting_frame_construction_applies(clip) {
        return summary;
    }

    let Some(parent_object) = clip.object2() else {
        summary
            .errors
            .push("crafting overlay AVM2 object is unavailable".to_owned());
        return summary;
    };
    let Some(domain) = context
        .library
        .library_for_movie(clip.movie())
        .map(|library| library.avm2_domain())
    else {
        summary
            .errors
            .push("crafting overlay AVM2 domain is unavailable".to_owned());
        return summary;
    };

    let mut activation = Avm2Activation::from_domain(context, domain);
    let parent = Avm2Value::from(parent_object);
    let children: Vec<DisplayObject<'gc>> = clip.iter_render_list().collect();
    summary.scanned_containers = 1;
    summary.scanned_direct_children = children.len();

    for child in children {
        let Some(name_string) = child
            .has_explicit_name()
            .then(|| child.name().map(|name| name.to_string()))
            .flatten()
        else {
            continue;
        };
        summary.named_children = summary.named_children.saturating_add(1);

        if should_force_aqw_crafting_child(true, true, child.object2().is_some()) {
            summary.forced_construct_attempts = summary.forced_construct_attempts.saturating_add(1);
            child.mark_avm2_lifecycle_dirty(Avm2LifecycleTraversal::Construct);
            child.construct_frame(activation.context);
            if child.object2().is_some() {
                summary.forced_constructed_fields.push(name_string.clone());
            }
        }

        let Some(child_object) = child.object2() else {
            summary
                .unavailable_fields
                .push(format!("{name_string} (child AVM2 object unavailable)"));
            continue;
        };
        summary.constructed_named_children = summary.constructed_named_children.saturating_add(1);

        let name = AvmString::new_utf8(activation.gc(), name_string.clone());
        let multiname = Avm2Multiname::new(activation.avm2().find_public_namespace(), name);
        match parent.get_property(&multiname, &mut activation) {
            Ok(Avm2Value::Null | Avm2Value::Undefined) => {
                summary.null_or_undefined_fields =
                    summary.null_or_undefined_fields.saturating_add(1);
                match parent.init_property(
                    &multiname,
                    Avm2Value::from(child_object),
                    &mut activation,
                ) {
                    Ok(()) => summary.rebound_fields.push(name_string),
                    Err(error) => summary.errors.push(format!("{name_string}: {error:?}")),
                }
            }
            Ok(_) => summary.occupied_fields.push(name_string),
            Err(error) => summary
                .errors
                .push(format!("{name_string} lookup: {error:?}")),
        }
    }

    summary
}

fn is_aqw_parent_timeline_label_fallback(
    movie_url: &str,
    label: &str,
    receiver_has_label: bool,
    ancestor_has_label: bool,
) -> bool {
    contains_ascii_case_insensitive(movie_url, "spider.swf")
        && matches!(label, "Init" | "Login" | "Game" | "Account" | "Select")
        && !receiver_has_label
        && ancestor_has_label
}

fn is_aqw_avatar_timeline_label_fallback(
    movie_url: &str,
    label: &str,
    receiver_has_label: bool,
    ancestor_name: Option<&str>,
    ancestor_class_local_name: Option<&str>,
    ancestor_has_label: bool,
) -> bool {
    contains_ascii_case_insensitive(movie_url, "spider.swf")
        && label == "Idle"
        && !receiver_has_label
        && ancestor_name == Some("mcChar")
        && ancestor_class_local_name == Some("mcSkel")
        && ancestor_has_label
}

/// AQW's `ServerList.onBackClick` calls `MovieClip(parent).gotoAndPlay("Login")`.
/// Some Loader layouts expose one additional parent wrapper, while the requested top-level label
/// remains on the Loader_Spider ancestor. Redirect only those five known root labels, and only
/// when the receiver genuinely lacks the label and an AQW loader ancestor genuinely owns it.
pub fn resolve_aqw_parent_timeline_label<'gc>(
    clip: MovieClip<'gc>,
    label: &crate::string::WStr,
    context: &UpdateContext<'gc>,
) -> MovieClip<'gc> {
    if !timeline_child_rebind_enabled() || clip.frame_label_to_number(label, context).is_some() {
        return clip;
    }

    let label_string = label.to_string();
    let mut ancestor = clip.parent();
    while let Some(display) = ancestor {
        if let Some(movie_clip) = display.as_movie_clip() {
            let ancestor_has_label = movie_clip.frame_label_to_number(label, context).is_some();
            if is_aqw_parent_timeline_label_fallback(
                movie_clip.movie().url(),
                &label_string,
                false,
                ancestor_has_label,
            ) {
                return movie_clip;
            }

            let ancestor_name = movie_clip.name().map(|name| name.to_string());
            let ancestor_class_local_name = movie_clip.object2().map(|object| {
                object
                    .instance_class()
                    .name()
                    .local_name()
                    .as_wstr()
                    .to_string()
            });
            if is_aqw_avatar_timeline_label_fallback(
                movie_clip.movie().url(),
                &label_string,
                false,
                ancestor_name.as_deref(),
                ancestor_class_local_name.as_deref(),
                ancestor_has_label,
            ) {
                return movie_clip;
            }
        }
        ancestor = display.parent();
    }

    clip
}

pub fn timeline_child_rebind_applies(clip: MovieClip<'_>) -> bool {
    let Some(object) = clip.object2() else {
        return false;
    };

    // QName local names are WStr-backed. Compare the target directly so this hot-path check never
    // constructs a qualified String or allocates a UTF-8 conversion.
    let class_name = object.instance_class().name();
    if class_name.local_name().as_wstr() != b"mcOption" {
        return false;
    }

    let has_nonempty_class_namespace =
        matches!(class_name.namespace().as_uri_opt(), Some(uri) if !uri.is_empty());
    is_timeline_child_rebind_target(clip.movie().url(), "mcOption", has_nonempty_class_namespace)
}

#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "aether_diagnostics", derive(serde::Serialize))]
pub struct TimelineChildRebindSummary {
    pub scanned_direct_children: usize,
    pub scanned_containers: usize,
    pub deepest_container_depth: usize,
    pub named_children: usize,
    pub constructed_named_children: usize,
    pub forced_construct_attempts: usize,
    pub forced_constructed_fields: Vec<String>,
    pub null_or_undefined_fields: usize,
    pub rebound_fields: Vec<String>,
    pub occupied_fields: Vec<String>,
    pub unavailable_fields: Vec<String>,
    pub errors: Vec<String>,
}

/// Restore null/undefined public fields for currently-present named timeline children.
///
/// The direct-field probe fixed Gameplay and Social. The nested probe then restored
/// `mcOption.mcVis.btnLeftQual`, but the v5 trace proved that `mcVis.btnRightQual` was still
/// physically present with no AVM2 object. For named children directly under `mcVis`, this version
/// therefore performs one targeted `construct_frame` call before attempting the field repair.
/// It still refuses to overwrite non-null fields, and it remains restricted to `mcOption` in
/// AQW's `spider.swf` by the caller.
pub fn rebind_named_timeline_children<'gc>(
    context: &mut UpdateContext<'gc>,
    clip: MovieClip<'gc>,
) -> TimelineChildRebindSummary {
    const MAX_PARENT_DEPTH: usize = 1;
    const MAX_SCANNED_CONTAINERS: usize = 256;

    let mut summary = TimelineChildRebindSummary::default();

    let domain = context
        .library
        .library_for_movie(clip.movie())
        .map(|library| library.avm2_domain());
    let Some(domain) = domain else {
        summary
            .errors
            .push("movie AVM2 domain is unavailable".to_owned());
        return summary;
    };

    let mut activation = Avm2Activation::from_domain(context, domain);
    let mut parents: Vec<(DisplayObject<'gc>, usize, String)> =
        vec![(clip.into(), 0, String::new())];

    while let Some((parent_display, depth, parent_path)) = parents.pop() {
        if summary.scanned_containers >= MAX_SCANNED_CONTAINERS {
            summary
                .errors
                .push("timeline-child rebind container limit reached".to_owned());
            break;
        }

        let Some(parent_container) = parent_display.as_container() else {
            continue;
        };
        let Some(parent_object) = parent_display.object2() else {
            let path = if parent_path.is_empty() {
                "mcOption".to_owned()
            } else {
                parent_path
            };
            summary
                .errors
                .push(format!("{path}: parent AVM2 object unavailable"));
            continue;
        };

        summary.scanned_containers = summary.scanned_containers.saturating_add(1);
        summary.deepest_container_depth = summary.deepest_container_depth.max(depth);

        let children: Vec<DisplayObject<'gc>> = parent_container.iter_render_list().collect();
        if depth == 0 {
            summary.scanned_direct_children = children.len();
        }

        let parent = Avm2Value::from(parent_object);

        for child in children {
            let explicit_name = if child.has_explicit_name() {
                child.name().map(|name| name.to_string())
            } else {
                None
            };
            let child_path = explicit_name.as_ref().map_or_else(
                || {
                    if parent_path.is_empty() {
                        format!("{}#{}", object_type_name(child), child.depth())
                    } else {
                        format!(
                            "{parent_path}.{}#{}",
                            object_type_name(child),
                            child.depth()
                        )
                    }
                },
                |name| {
                    if parent_path.is_empty() {
                        name.clone()
                    } else {
                        format!("{parent_path}.{name}")
                    }
                },
            );

            let recurse_into_child = depth < MAX_PARENT_DEPTH
                && child.as_container().is_some()
                && (depth > 0 || explicit_name.as_deref() == Some("mcVis"));

            if let Some(name_string) = explicit_name {
                summary.named_children = summary.named_children.saturating_add(1);

                let child_object = match child.object2() {
                    Some(child_object) => child_object,
                    None if depth == 1 && parent_path == "mcVis" => {
                        summary.forced_construct_attempts =
                            summary.forced_construct_attempts.saturating_add(1);

                        // `mcVis.btnRightQual` is placed on the General frame but can be skipped
                        // by the normal parent construction pass while that pass is re-entrant.
                        // Calling the specific child directly avoids the parent clip's
                        // RUNNING_CONSTRUCT_FRAME skip without broadening this experiment.
                        child.construct_frame(activation.context);

                        let Some(child_object) = child.object2() else {
                            summary.unavailable_fields.push(format!(
                                "{child_path} (still unavailable after targeted construct_frame)"
                            ));
                            if recurse_into_child {
                                parents.push((child, depth + 1, child_path));
                            }
                            continue;
                        };

                        summary.forced_constructed_fields.push(child_path.clone());
                        child_object
                    }
                    None => {
                        summary
                            .unavailable_fields
                            .push(format!("{child_path} (child AVM2 object unavailable)"));
                        if recurse_into_child {
                            parents.push((child, depth + 1, child_path));
                        }
                        continue;
                    }
                };

                summary.constructed_named_children =
                    summary.constructed_named_children.saturating_add(1);

                let name = AvmString::new_utf8(activation.gc(), name_string.clone());
                let multiname = Avm2Multiname::new(activation.avm2().find_public_namespace(), name);
                match parent.get_property(&multiname, &mut activation) {
                    Ok(Avm2Value::Null | Avm2Value::Undefined) => {
                        summary.null_or_undefined_fields =
                            summary.null_or_undefined_fields.saturating_add(1);
                        match parent.init_property(
                            &multiname,
                            Avm2Value::from(child_object),
                            &mut activation,
                        ) {
                            Ok(()) => summary.rebound_fields.push(child_path.clone()),
                            Err(error) => summary.errors.push(format!("{child_path}: {error:?}")),
                        }
                    }
                    Ok(_) => {
                        // Keep the old direct-field visibility without filling the trace with
                        // every occupied nested field.
                        if depth == 0 {
                            summary.occupied_fields.push(child_path.clone());
                        }
                    }
                    Err(error) => summary
                        .errors
                        .push(format!("{child_path} lookup: {error:?}")),
                }
            }

            if recurse_into_child {
                parents.push((child, depth + 1, child_path));
            }
        }
    }

    summary
}

fn object_type_name(object: DisplayObject<'_>) -> &'static str {
    match object {
        DisplayObject::Stage(_) => "stage",
        DisplayObject::Bitmap(_) => "bitmap",
        DisplayObject::Avm1Button(_) => "avm1_button",
        DisplayObject::Avm2Button(_) => "avm2_button",
        DisplayObject::EditText(_) => "edit_text",
        DisplayObject::TextLine(_) => "text_line",
        DisplayObject::Graphic(_) => "graphic",
        DisplayObject::MorphShape(_) => "morph_shape",
        DisplayObject::MovieClip(_) => "movie_clip",
        DisplayObject::Text(_) => "text",
        DisplayObject::Video(_) => "video",
        DisplayObject::LoaderDisplay(_) => "loader_display",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_child_rebind_switch_round_trips() {
        set_timeline_child_rebind_enabled(false);
        assert!(!timeline_child_rebind_enabled());
        set_timeline_child_rebind_enabled(true);
        assert!(timeline_child_rebind_enabled());
        set_timeline_child_rebind_enabled(false);
    }

    #[test]
    fn settings_rebind_target_is_narrow() {
        assert!(is_timeline_child_rebind_target(
            "https://game.aq.com/game/gamefiles/spider.swf?ver=1",
            "mcOption",
            false,
        ));
        assert!(is_timeline_child_rebind_target(
            "https://game.aq.com/game/gamefiles/SPIDER.SWF?ver=1",
            "mcOption",
            false,
        ));
        assert!(!is_timeline_child_rebind_target(
            "https://game.aq.com/game/gamefiles/spider.swf?ver=1",
            "OtherClass",
            false,
        ));
        assert!(!is_timeline_child_rebind_target(
            "https://example.invalid/other.swf",
            "mcOption",
            false,
        ));
    }

    #[test]
    fn settings_rebind_target_rejects_nonempty_class_namespace() {
        assert!(!is_timeline_child_rebind_target(
            "https://game.aq.com/game/gamefiles/spider.swf?ver=1",
            "mcOption",
            true,
        ));
    }

    #[test]
    fn aqw_parent_timeline_fallback_is_limited_to_missing_loader_root_labels() {
        assert!(is_aqw_parent_timeline_label_fallback(
            "https://game.aq.com/game/gamefiles/Loader_Spider.swf?ver=1",
            "Login",
            false,
            true,
        ));
        assert!(!is_aqw_parent_timeline_label_fallback(
            "https://game.aq.com/game/gamefiles/Loader_Spider.swf?ver=1",
            "Login",
            true,
            true,
        ));
        assert!(!is_aqw_parent_timeline_label_fallback(
            "https://game.aq.com/game/gamefiles/Loader_Spider.swf?ver=1",
            "Unknown",
            false,
            true,
        ));
        assert!(!is_aqw_parent_timeline_label_fallback(
            "https://example.invalid/other.swf",
            "Login",
            false,
            true,
        ));
    }

    #[test]
    fn aqw_avatar_timeline_fallback_is_limited_to_idle_on_mcchar_skeleton() {
        assert!(is_aqw_avatar_timeline_label_fallback(
            "https://game.aq.com/game/gamefiles/spider.swf?ver=1",
            "Idle",
            false,
            Some("mcChar"),
            Some("mcSkel"),
            true,
        ));
        assert!(!is_aqw_avatar_timeline_label_fallback(
            "https://game.aq.com/game/gamefiles/spider.swf?ver=1",
            "Attack",
            false,
            Some("mcChar"),
            Some("mcSkel"),
            true,
        ));
        assert!(!is_aqw_avatar_timeline_label_fallback(
            "https://game.aq.com/game/gamefiles/spider.swf?ver=1",
            "Idle",
            false,
            Some("previewMCB"),
            Some("mcSkel"),
            true,
        ));
        assert!(!is_aqw_avatar_timeline_label_fallback(
            "https://example.invalid/other.swf",
            "Idle",
            false,
            Some("mcChar"),
            Some("mcSkel"),
            true,
        ));
    }

    #[test]
    fn aura_refresh_target_is_limited_to_spider_world_update_aura_data() {
        assert!(is_aqw_aura_refresh_target(
            "https://game.aq.com/game/gamefiles/spider.swf?ver=1",
            "World/updateAuraData",
            Some("World"),
            true,
        ));
        assert!(is_aqw_aura_refresh_target(
            "https://game.aq.com/game/gamefiles/spider.swf",
            "updateAuraData",
            Some("World"),
            true,
        ));
        assert!(!is_aqw_aura_refresh_target(
            "https://example.invalid/other.swf",
            "World/updateAuraData",
            Some("World"),
            true,
        ));
        assert!(!is_aqw_aura_refresh_target(
            "https://game.aq.com/game/gamefiles/spider.swf",
            "World/removeAura",
            Some("World"),
            true,
        ));
        assert!(!is_aqw_aura_refresh_target(
            "https://game.aq.com/game/gamefiles/spider.swf",
            "World/updateAuraData",
            Some("Avatar"),
            true,
        ));
        assert!(!is_aqw_aura_refresh_target(
            "https://game.aq.com/game/gamefiles/spider.swf",
            "World/updateAuraData",
            Some("World"),
            false,
        ));
    }

    #[test]
    fn aura_insertion_target_includes_spider_world_and_optional_ui_handlers() {
        assert!(is_aqw_aura_insertion_target(
            "https://game.aq.com/game/gamefiles/spider.swf?ver=1",
            "World/showAuraChange",
            Some("World"),
            true,
            true,
        ));
        assert!(is_aqw_aura_insertion_target(
            "https://game.aq.com/game/gamefiles/spider.swf",
            "showAuraChange",
            Some("World"),
            true,
            true,
        ));
        assert!(is_aqw_aura_insertion_target(
            "https://game.aq.com/game/gamefiles/spider.swf?ver=1",
            "playerAuras/handleAura",
            Some("playerAuras"),
            false,
            true,
        ));
        assert!(is_aqw_aura_insertion_target(
            "https://game.aq.com/game/gamefiles/spider.swf?ver=1",
            "playerAuras/handleAura",
            None,
            false,
            false,
        ));
        assert!(is_aqw_aura_insertion_target(
            "https://game.aq.com/game/gamefiles/spider.swf?ver=1",
            "targetAuras/handleAura",
            Some("targetAuras"),
            false,
            true,
        ));
        assert!(!is_aqw_aura_insertion_target(
            "https://example.invalid/other.swf",
            "World/showAuraChange",
            Some("World"),
            true,
            true,
        ));
        assert!(!is_aqw_aura_insertion_target(
            "https://game.aq.com/game/gamefiles/spider.swf",
            "World/updateAuraData",
            Some("World"),
            true,
            true,
        ));
        assert!(!is_aqw_aura_insertion_target(
            "https://game.aq.com/game/gamefiles/spider.swf",
            "World/showAuraChange",
            Some("Avatar"),
            true,
            true,
        ));
        assert!(!is_aqw_aura_insertion_target(
            "https://game.aq.com/game/gamefiles/spider.swf",
            "World/showAuraChange",
            Some("World"),
            false,
            false,
        ));
    }

    #[test]
    fn aura_insertion_allows_public_package_classes_but_not_internal_classes() {
        assert!(aura_insertion_namespace_is_allowed(true, true));
        assert!(aura_insertion_namespace_is_allowed(false, true));
        assert!(!aura_insertion_namespace_is_allowed(false, false));
    }

    #[test]
    fn aura_countdown_mask_repair_is_limited_to_spider_aura_handlers() {
        assert!(is_aqw_aura_countdown_target(
            "https://game.aq.com/game/gamefiles/spider.swf?ver=1",
            "playerAuras/countDownAct",
            Some("playerAuras"),
            false,
            true,
        ));
        assert!(is_aqw_aura_countdown_target(
            "https://game.aq.com/game/gamefiles/spider.swf?ver=1",
            "targetAuras/countDownAct",
            Some("targetAuras"),
            false,
            true,
        ));
        assert!(is_aqw_aura_countdown_target(
            "https://game.aq.com/game/gamefiles/spider.swf?ver=1",
            "playerAuras/countDownAct",
            None,
            false,
            false,
        ));
        assert!(!is_aqw_aura_countdown_target(
            "https://example.invalid/other.swf",
            "playerAuras/countDownAct",
            Some("playerAuras"),
            false,
            true,
        ));
        assert!(!is_aqw_aura_countdown_target(
            "https://game.aq.com/game/gamefiles/spider.swf",
            "World/countDownAct",
            Some("World"),
            true,
            true,
        ));
    }

    #[test]
    fn aura_countdown_rebinds_missing_or_wrong_occupied_child_fields() {
        assert!(aura_countdown_child_needs_rebind(false, false));
        assert!(aura_countdown_child_needs_rebind(true, false));
        assert!(!aura_countdown_child_needs_rebind(true, true));
    }

    #[test]
    fn aura_mask_script_suppression_is_limited_to_direct_named_segments() {
        assert!(is_aqw_aura_mask_segment_parts(
            "https://game.aq.com/game/gamefiles/spider.swf?ver=1",
            "ActMaskReverse",
            Some("e0"),
            true,
        ));
        assert!(is_aqw_aura_mask_segment_parts(
            "https://game.aq.com/game/gamefiles/SPIDER.SWF",
            "ActMaskReverse",
            Some("e3"),
            true,
        ));
        assert!(!is_aqw_aura_mask_segment_parts(
            "https://game.aq.com/game/gamefiles/spider.swf",
            "ActMaskReverse",
            Some("e4"),
            true,
        ));
        assert!(!is_aqw_aura_mask_segment_parts(
            "https://game.aq.com/game/gamefiles/spider.swf",
            "OtherMask",
            Some("e0"),
            true,
        ));
        assert!(!is_aqw_aura_mask_segment_parts(
            "https://example.invalid/spider.swf",
            "ActMaskReverse",
            Some("e0"),
            true,
        ));
        assert!(!is_aqw_aura_mask_segment_parts(
            "https://game.aq.com/game/gamefiles/spider.swf",
            "ActMaskReverse",
            Some("e0"),
            false,
        ));
    }

    #[test]
    fn spellcraft_drag_timer_override_is_limited_to_live_map_frame_six() {
        let movie =
            "https://game.aq.com/game/gamefiles/maps/tradeskills/spellcraft/game-spellcraftr2.swf";
        assert!(is_aqw_spellcraft_drag_timer_target(
            movie,
            "scGame_1/frame6",
            Some("scGame_1"),
            true,
        ));
        assert!(is_aqw_spellcraft_drag_timer_target(
            movie,
            "scGame_1/frame6",
            None,
            false,
        ));
        assert!(!is_aqw_spellcraft_drag_timer_target(
            movie,
            "scGame_1/frame7",
            Some("scGame_1"),
            true,
        ));
        assert!(!is_aqw_spellcraft_drag_timer_target(
            "https://example.invalid/game-spellcraftr2.swf",
            "scGame_1/frame6",
            Some("scGame_1"),
            true,
        ));
        assert!((aqw_spellcraft_drag_delay_ms() - (1_000.0 / 60.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn spellcraft_drop_feedback_follows_the_last_changed_target() {
        let movie =
            "https://game.aq.com/game/gamefiles/maps/tradeskills/spellcraft/game-spellcraftr2.swf";
        assert!(is_aqw_spellcraft_drop_target(
            movie,
            "scGame_1/DragStop",
            Some("scGame_1"),
            true,
        ));
        assert!(!is_aqw_spellcraft_drop_target(
            movie,
            "scGame_1/DragStart",
            Some("scGame_1"),
            true,
        ));
        assert!(!is_aqw_spellcraft_drop_target(
            "https://example.invalid/game-spellcraftr2.swf",
            "scGame_1/DragStop",
            Some("scGame_1"),
            true,
        ));

        assert_eq!(
            select_aqw_spellcraft_effect_target(
                [false, true, false, false, false],
                Some("mcWordSlot3"),
            ),
            Some(2),
        );
        assert_eq!(
            select_aqw_spellcraft_effect_target(
                [false, false, false, false, false],
                Some("mcTarget4"),
            ),
            Some(4),
        );
        assert_eq!(
            select_aqw_spellcraft_effect_target(
                [false, false, false, false, false],
                Some("mcWordSlot3"),
            ),
            None,
        );
    }

    #[test]
    fn valiance_offset_is_limited_to_its_spellw_tracking_callback() {
        let movie = "https://game.aq.com/game/gamefiles/assets/assets_2026.swf";
        assert!(is_aqw_valiance_track_target(
            movie,
            "SpellW/private:trackTC",
            Some("SpellW"),
            Some("sp_qchronoa2"),
        ));
        assert!(is_aqw_valiance_track_target(
            movie,
            "SpellW/private:trackTC",
            None,
            Some("sp_qchronoa2"),
        ));
        assert!(!is_aqw_valiance_track_target(
            movie,
            "SpellW/private:trackTC",
            Some("SpellW"),
            Some("sp_apal4"),
        ));
        assert!(!is_aqw_valiance_track_target(
            movie,
            "SpellW/init",
            Some("SpellW"),
            Some("sp_qchronoa2"),
        ));
        assert_eq!(aqw_valiance_y_offset(), 36.0);
    }

    #[test]
    fn invalid_timed_aura_timestamps_are_repaired() {
        assert!(should_repair_aqw_incoming_aura_timestamp(false, f64::NAN));
        assert!(should_repair_aqw_incoming_aura_timestamp(false, 0.0));
        assert!(should_repair_aqw_incoming_aura_timestamp(false, 1.0));
        assert!(!should_repair_aqw_incoming_aura_timestamp(
            false,
            1_780_000_000_000.0,
        ));
        assert!(!should_repair_aqw_incoming_aura_timestamp(true, f64::NAN));
        assert!(!should_repair_aqw_incoming_aura_timestamp(true, 0.0));
    }

    #[test]
    fn aura_refresh_requires_all_three_identity_fields_to_match() {
        assert!(aura_refresh_identity_matches(true, true, true));
        assert!(!aura_refresh_identity_matches(false, true, true));
        assert!(!aura_refresh_identity_matches(true, false, true));
        assert!(!aura_refresh_identity_matches(true, true, false));
    }

    #[test]
    fn aura_refresh_uses_current_time_when_spider_omits_timestamp() {
        assert_eq!(select_aura_refresh_timestamp(f64::NAN, 42_000.0), 42_000.0);
        assert_eq!(select_aura_refresh_timestamp(0.0, 42_000.0), 42_000.0);
    }

    #[test]
    fn aura_refresh_replaces_relative_or_seconds_timestamps() {
        assert_eq!(select_aura_refresh_timestamp(41_999.0, 42_000.0), 42_000.0);
    }

    #[test]
    fn aura_refresh_preserves_spiders_valid_epoch_milliseconds() {
        assert_eq!(
            select_aura_refresh_timestamp(1_780_000_000_000.0, 1_780_000_001_000.0),
            1_780_000_000_000.0,
        );
    }

    #[test]
    fn crafting_frame_target_is_limited_to_the_two_trace_proven_overlays() {
        assert!(is_aqw_crafting_frame_target(
            "https://game.aq.com/game/gamefiles/maps/spellcraft.swf?ver=1",
            Some("game_fla"),
            "mcInfoOverlay_233",
        ));
        assert!(is_aqw_crafting_frame_target(
            "https://game.aq.com/game/gamefiles/alchemy/alchemyGame_v4.swf",
            Some("alchemyGame_v4_fla"),
            "mcInfoOverlay_388",
        ));
        assert!(!is_aqw_crafting_frame_target(
            "https://game.aq.com/game/gamefiles/maps/battleon.swf",
            Some("game_fla"),
            "mcInfoOverlay_233",
        ));
        assert!(!is_aqw_crafting_frame_target(
            "https://game.aq.com/game/gamefiles/maps/spellcraft.swf",
            Some("game_fla"),
            "OtherOverlay",
        ));
        assert!(!is_aqw_crafting_frame_target(
            "https://example.invalid/spellcraft.swf",
            Some("game_fla"),
            "mcInfoOverlay_233",
        ));
    }

    #[test]
    fn crafting_repair_only_forces_named_unconstructed_children() {
        assert!(should_force_aqw_crafting_child(true, true, false));
        assert!(!should_force_aqw_crafting_child(false, true, false));
        assert!(!should_force_aqw_crafting_child(true, false, false));
        assert!(!should_force_aqw_crafting_child(true, true, true));
    }

    #[test]
    fn equipment_recovery_target_is_limited_to_spider_avatar_initialization() {
        assert!(is_aqw_equipment_initialization_target(
            "https://game.aq.com/game/gamefiles/spider.swf?ver=1",
            "Avatar/initAvatar",
            Some("Avatar"),
            true,
        ));
        assert!(!is_aqw_equipment_initialization_target(
            "https://example.invalid/spider.swf",
            "Avatar/initAvatar",
            Some("Avatar"),
            true,
        ));
        assert!(!is_aqw_equipment_initialization_target(
            "https://game.aq.com/game/gamefiles/spider.swf",
            "Avatar/updateItemAnimation",
            Some("Avatar"),
            true,
        ));
        assert!(!is_aqw_equipment_initialization_target(
            "https://game.aq.com/game/gamefiles/spider.swf",
            "Avatar/initAvatar",
            Some("AvatarMC"),
            true,
        ));
        assert!(!is_aqw_equipment_initialization_target(
            "https://game.aq.com/game/gamefiles/spider.swf",
            "Avatar/initAvatar",
            Some("Avatar"),
            false,
        ));
    }

    #[test]
    fn equipment_recovery_requires_a_finished_stuck_own_first_load() {
        assert!(should_schedule_aqw_equipment_recovery(true, true, false, 1,));
        assert!(!should_schedule_aqw_equipment_recovery(
            false, true, false, 1,
        ));
        assert!(!should_schedule_aqw_equipment_recovery(
            true, false, false, 1,
        ));
        assert!(!should_schedule_aqw_equipment_recovery(true, true, true, 1,));
        assert!(!should_schedule_aqw_equipment_recovery(
            true, true, false, 0,
        ));
    }
}

/// Every AQW class lookup that failed, not just the distinct ones the log reports.
///
/// The warning in `avm2::domain` is deduplicated per (domain, name), so the log cannot
/// distinguish a few hundred misses at load time from a per-frame storm. This can.
static AQW_DEFINITION_LOOKUP_MISSES: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

pub(crate) fn record_definition_lookup_miss() {
    AQW_DEFINITION_LOOKUP_MISSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Total AQW class-resolution failures since the last call.
pub fn take_aqw_definition_lookup_miss_count() -> u64 {
    AQW_DEFINITION_LOOKUP_MISSES.swap(0, std::sync::atomic::Ordering::Relaxed)
}

/// How AQW's portrait health numbers are grouped, if at all.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HpSeparator {
    /// Leave the number exactly as AQW wrote it.
    None,
    /// `1,250,000`.
    Comma,
    /// `1 250 000`. Some readers find a comma easy to mistake for a decimal point, and much of
    /// the world writes decimals that way, so a space is the less ambiguous choice for them.
    Space,
}

impl HpSeparator {
    fn character(self) -> Option<char> {
        match self {
            HpSeparator::None => None,
            HpSeparator::Comma => Some(','),
            // A plain ASCII space, not a thin or non-breaking one. AQW's embedded fonts cannot be
            // relied on to carry the typographically correct glyphs.
            HpSeparator::Space => Some(' '),
        }
    }
}

static HP_SEPARATOR: AtomicU8 = AtomicU8::new(0);

/// Group AQW's portrait health numbers for readability.
#[inline]
pub fn set_hp_separator(separator: HpSeparator) {
    HP_SEPARATOR.store(
        match separator {
            HpSeparator::None => 0,
            HpSeparator::Comma => 1,
            HpSeparator::Space => 2,
        },
        Ordering::Relaxed,
    );
}

#[inline]
pub fn hp_separator() -> HpSeparator {
    match HP_SEPARATOR.load(Ordering::Relaxed) {
        1 => HpSeparator::Comma,
        2 => HpSeparator::Space,
        _ => HpSeparator::None,
    }
}

/// The AQW portrait TextField that receives a character's current hit points.
///
/// `World/updatePortrait` assigns it directly: `strIntHP.text = String(intHP)`, with no separators
/// and no formatting of any kind. The same portrait code runs for the local player, other players
/// and monsters, so matching this one field name covers every health readout in the game.
const AQW_HP_TEXT_FIELD: &str = "strIntHP";

/// AQW text fields that carry quantities worth grouping, beyond the portrait health readout.
///
/// A name that AQW does not actually use costs nothing: the field never appears, so the rule never
/// fires. That makes a slightly generous list safe, which matters because these are quest and
/// detail panels whose exact field names are not all confirmed.
const AQW_QUANTITY_TEXT_FIELDS: &[&str] =
    &["tDesc", "tbDesc", "strDesc", "txtDetail", "tDetailTemplate"];

/// Smallest digit run worth grouping.
static SEPARATOR_MIN_DIGITS: AtomicU8 = AtomicU8::new(4);

/// Group from a thousand (`4`) or from ten thousand (`5`).
///
/// Four-digit numbers are common in ordinary play and some readers find `2,750` busier than
/// `2750`, so where grouping starts is a preference rather than a fixed rule.
#[inline]
pub fn set_separator_min_digits(min_digits: u8) {
    SEPARATOR_MIN_DIGITS.store(min_digits.clamp(4, 9), Ordering::Relaxed);
}

#[inline]
pub fn separator_min_digits() -> u8 {
    SEPARATOR_MIN_DIGITS.load(Ordering::Relaxed)
}

/// Whether this text assignment is one AQW fills with quantities.
pub fn is_aqw_quantity_text_field(field_name: Option<&str>) -> bool {
    field_name.is_some_and(|name| AQW_QUANTITY_TEXT_FIELDS.contains(&name))
}

/// Group every standalone number inside a line of text, as `Bones 152197/1000000` to
/// `Bones 152,197/1,000,000`.
///
/// A digit run is only grouped when it stands alone as a quantity. A run touching a letter, a dot
/// or a hyphen is left alone, which keeps identifiers, version numbers, decimals and negatives
/// intact. `citadelruins-99922` is the case that motivated the hyphen rule.
pub fn group_number_runs(text: &str, separator: HpSeparator, min_digits: usize) -> Option<String> {
    if separator.character().is_none() {
        return None;
    }

    let bytes = text.as_bytes();
    let mut grouped = String::with_capacity(text.len() + text.len() / 3);
    let mut index = 0;
    let mut changed = false;

    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() {
            grouped.push(bytes[index] as char);
            index += 1;
            continue;
        }

        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }

        let joined_before = start
            .checked_sub(1)
            .is_some_and(|before| is_number_glue(bytes[before]));
        let joined_after = bytes.get(index).copied().is_some_and(is_number_glue);
        let run = &text[start..index];

        match group_digits(run, separator, min_digits) {
            Some(separated) if !joined_before && !joined_after => {
                grouped.push_str(&separated);
                changed = true;
            }
            _ => grouped.push_str(run),
        }
    }

    changed.then_some(grouped)
}

/// Whether a byte next to a digit run means the run is part of a larger token rather than a
/// quantity of its own.
fn is_number_glue(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'-' || byte == b'.' || byte == b'_'
}

/// Group a plain run of digits for readability, as `1250000` to `1,250,000`.
///
/// Requested by a dyslexic player who could read the digits but not the magnitude: with boss health
/// in the millions there is nothing to anchor where the millions place sits, and the number changes
/// several times a second during a fight.
///
/// Only an unbroken run of digits is touched. Anything AQW puts in this field that is not a bare
/// number, such as the literal `X` it writes for a dead target, is passed through unchanged.
pub fn group_digits(text: &str, separator: HpSeparator, min_digits: usize) -> Option<String> {
    let separator = separator.character()?;
    // Four digits is where grouping starts helping and where Flash's own number formatting would
    // have begun. Below that it only adds noise.
    if text.len() < min_digits || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }

    let mut grouped = String::with_capacity(text.len() + text.len() / 3);
    for (position, digit) in text.chars().enumerate() {
        if position > 0 && (text.len() - position) % 3 == 0 {
            grouped.push(separator);
        }
        grouped.push(digit);
    }
    Some(grouped)
}

/// Whether this text assignment is AQW writing a health value into a portrait.
pub fn is_aqw_hp_text_field(field_name: Option<&str>) -> bool {
    field_name == Some(AQW_HP_TEXT_FIELD)
}

/// Rewrite an AQW portrait health value with thousands separators, if that is what this is.
///
/// Returns `None` for every other text assignment in the game, so chat, item names, coin totals
/// and everything else reach the field untouched.
pub fn aqw_grouped_hp_text(field_name: Option<&str>, text: &str) -> Option<String> {
    let min_digits = separator_min_digits() as usize;
    if is_aqw_hp_text_field(field_name) {
        return group_digits(text, hp_separator(), min_digits);
    }
    if is_aqw_quantity_text_field(field_name) {
        // These carry sentences, so group the numbers inside rather than the whole string.
        return group_number_runs(text, hp_separator(), min_digits);
    }
    None
}

#[cfg(test)]
mod hp_separator_tests {
    use super::{HpSeparator, group_digits, group_number_runs, is_aqw_hp_text_field};

    #[test]
    fn boss_health_gets_separators_at_every_thousand() {
        assert_eq!(
            group_digits("1250000", HpSeparator::Comma, 4).as_deref(),
            Some("1,250,000")
        );
        assert_eq!(
            group_digits("999999999", HpSeparator::Comma, 4).as_deref(),
            Some("999,999,999")
        );
        assert_eq!(
            group_digits("1000", HpSeparator::Comma, 4).as_deref(),
            Some("1,000")
        );
        assert_eq!(
            group_digits("12345", HpSeparator::Comma, 4).as_deref(),
            Some("12,345")
        );
        assert_eq!(
            group_digits("123456", HpSeparator::Comma, 4).as_deref(),
            Some("123,456")
        );
    }

    #[test]
    fn spaces_group_the_same_places_as_commas() {
        assert_eq!(
            group_digits("1250000", HpSeparator::Space, 4).as_deref(),
            Some("1 250 000")
        );
        assert_eq!(
            group_digits("12345", HpSeparator::Space, 4).as_deref(),
            Some("12 345")
        );
        // Off means untouched, whatever the number.
        assert_eq!(group_digits("1250000", HpSeparator::None, 4), None);
    }

    #[test]
    fn quest_progress_lines_group_both_sides_of_the_slash() {
        assert_eq!(
            group_number_runs(
                "Unleash Doom: Inquisitor Bones 152197/1000000",
                HpSeparator::Comma,
                4
            )
            .as_deref(),
            Some("Unleash Doom: Inquisitor Bones 152,197/1,000,000")
        );
        assert_eq!(
            group_number_runs("Refined Metal 0/1000000", HpSeparator::Comma, 4).as_deref(),
            Some("Refined Metal 0/1,000,000")
        );
    }

    #[test]
    fn room_numbers_and_other_identifiers_are_never_grouped() {
        // The case from a real screenshot. A room name is not a quantity.
        assert_eq!(
            group_number_runs("1 player in citadelruins-99922", HpSeparator::Comma, 4),
            None
        );
        // Anything welded to letters, dots or underscores is part of a token, not a count.
        assert_eq!(group_number_runs("v1000000", HpSeparator::Comma, 4), None);
        assert_eq!(group_number_runs("item_10000", HpSeparator::Comma, 4), None);
        assert_eq!(group_number_runs("1.50000", HpSeparator::Comma, 4), None);
        assert_eq!(group_number_runs("-25000", HpSeparator::Comma, 4), None);
        assert_eq!(
            group_number_runs("spider.swf?ver=0.60398", HpSeparator::Comma, 4),
            None
        );
    }

    #[test]
    fn grouping_can_start_at_ten_thousand_instead() {
        // 2750 reads fine unseparated; 12750 is where it starts to help.
        assert_eq!(group_digits("2750", HpSeparator::Comma, 5), None);
        assert_eq!(
            group_digits("12750", HpSeparator::Comma, 5).as_deref(),
            Some("12,750")
        );
        assert_eq!(
            group_number_runs("Bones 2750/1000000", HpSeparator::Comma, 5).as_deref(),
            Some("Bones 2750/1,000,000")
        );
    }

    #[test]
    fn short_numbers_are_left_alone() {
        // Grouping these would add noise without making the magnitude any clearer.
        assert_eq!(group_digits("0", HpSeparator::Comma, 4), None);
        assert_eq!(group_digits("42", HpSeparator::Comma, 4), None);
        assert_eq!(group_digits("999", HpSeparator::Comma, 4), None);
    }

    #[test]
    fn anything_that_is_not_a_bare_number_is_untouched() {
        // AQW writes a literal X into this field for a dead target.
        assert_eq!(group_digits("X", HpSeparator::Comma, 4), None);
        assert_eq!(group_digits("1,250,000", HpSeparator::Comma, 4), None);
        assert_eq!(group_digits("12000/45000", HpSeparator::Comma, 4), None);
        assert_eq!(group_digits("-5000", HpSeparator::Comma, 4), None);
        assert_eq!(group_digits("", HpSeparator::Comma, 4), None);
        assert_eq!(group_digits("1234 HP", HpSeparator::Comma, 4), None);
    }

    #[test]
    fn the_feature_is_inert_until_it_is_switched_on() {
        use super::{aqw_grouped_hp_text, set_hp_separator};

        set_hp_separator(HpSeparator::None);
        assert_eq!(aqw_grouped_hp_text(Some("strIntHP"), "1250000"), None);

        set_hp_separator(HpSeparator::Comma);
        assert_eq!(
            aqw_grouped_hp_text(Some("strIntHP"), "1250000").as_deref(),
            Some("1,250,000")
        );
        // Every other field in the game is left alone even while the feature is on.
        assert_eq!(aqw_grouped_hp_text(Some("txtChat"), "1250000"), None);
        assert_eq!(aqw_grouped_hp_text(None, "1250000"), None);

        set_hp_separator(HpSeparator::Space);
        assert_eq!(
            aqw_grouped_hp_text(Some("strIntHP"), "1250000").as_deref(),
            Some("1 250 000")
        );
        set_hp_separator(HpSeparator::None);
    }

    #[test]
    fn only_the_portrait_health_field_is_rewritten() {
        assert!(is_aqw_hp_text_field(Some("strIntHP")));
        assert!(!is_aqw_hp_text_field(Some("strIntHPMax")));
        assert!(!is_aqw_hp_text_field(Some("xstrIntHP")));
        assert!(!is_aqw_hp_text_field(Some("txtChat")));
        assert!(!is_aqw_hp_text_field(None));
    }
}
