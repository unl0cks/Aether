//! Narrow compatibility repairs used by Aether without enabling diagnostic instrumentation.

use crate::aether_movie::{is_aqw_game_movie, is_aqw_loader_movie, is_hosted_aqw_game_movie};
use crate::avm2::{
    Activation as Avm2Activation, FunctionArgs as Avm2FunctionArgs, Multiname as Avm2Multiname,
    TObject as _, Value as Avm2Value,
};
use crate::context::UpdateContext;
use crate::display_object::{
    Avm2LifecycleTraversal, DisplayObject, MovieClip, TDisplayObject, TDisplayObjectContainer,
};
use crate::locale::get_current_date_time;
use crate::string::{AvmString, WStr};
use crate::timer::TimerCallback;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use swf::Twips;

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
    find_ascii_case_insensitive(haystack, needle).is_some()
}

fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

#[inline]
fn is_aqw_aura_mask_segment_parts(
    parent_movie_url: &str,
    parent_class_local_name: &str,
    child_name: Option<&str>,
    is_direct_child: bool,
) -> bool {
    is_direct_child
        && is_hosted_aqw_game_movie(parent_movie_url)
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
    is_aqw_game_movie(movie_url)
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
    if !is_aqw_game_movie(movie_url) {
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
    if !is_aqw_game_movie(movie_url) {
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

/// Tint the Focus aura icon red so a taunt can be timed at a glance.
///
/// Taunt applies Focus and Reckless together and AQW draws both as the same white skull, so the
/// only thing separating them is that Focus runs six seconds and Reckless ten. Loop taunting in an
/// ultra means reading which of two identical icons is the shorter one, mid-fight, every cycle.
///
/// Matched on the aura's name rather than the class that cast it, which is what makes one rule
/// cover Chaos Avenger, King's Echo, Paladin Slayer, DeathKnight Lord, the Naval Commander and
/// ShadowStalker/ShadowWeaver of Time and Chrono variants, Blood Titan, Dragon Slayer General,
/// Defender, Legion Paladin, Frostval Barbarian, Legendary Hero, Royal Battlemage and the rest:
/// they all name the taunt `Focus`.
///
/// The name is not in the movie -- auras are server-defined and arrive by name -- so this reads
/// `auraName` off AQW's own icon holder, measured as class `ib3` on the live build.
///
/// Runs off the countdown event, so it re-applies for as long as the aura is up and needs no undo:
/// the icon is destroyed with the aura.
pub(crate) fn recolour_aqw_focus_aura_icon<'gc>(
    activation: &mut Avm2Activation<'_, 'gc>,
    event: Avm2Value<'gc>,
) -> Result<bool, crate::avm2::Error<'gc>> {
    if !focus_aura_recoloured() {
        return Ok(false);
    }
    let Some(event_object) = event.as_object() else {
        return Ok(false);
    };
    let Some(target) = event_object.as_event().and_then(|event| event.target()) else {
        return Ok(false);
    };

    let target_value = Avm2Value::from(target);
    let aura_name = aqw_aura_icon_name(activation, target_value)?;
    report_focus_icon_shape(activation, target_value, aura_name);
    let Some(aura_name) = aura_name else {
        return Ok(false);
    };
    if !aura_name
        .as_wstr()
        .eq_ignore_case(WStr::from_units(b"Focus"))
    {
        return Ok(false);
    }

    // Tint the icon holder, not a layer inside it.
    //
    // Tinting `icon2` -- the only art layer this build exposes -- left the icon its original colour
    // on screen, so whatever `icon2` is, it is not what gets drawn. The holder is what visibly
    // works. It takes the square's border and the stack count with it, which is not ideal, and is
    // far better than an icon that does not mark itself at all.
    let Some(holder) = target.as_display_object() else {
        return Ok(false);
    };
    holder.set_color_transform(focus_tint());

    Ok(true)
}

/// The aura's name, from whichever field this build of AQW put it in.
///
/// `auraName` is the name the icon carries directly; `aura` is the aura object, whose own name is
/// `nam`, the field Spider sends. None of the three is a declared trait, so all three are tried
/// rather than assumed.
fn aqw_aura_icon_name<'gc>(
    activation: &mut Avm2Activation<'_, 'gc>,
    icon: Avm2Value<'gc>,
) -> Result<Option<AvmString<'gc>>, crate::avm2::Error<'gc>> {
    for direct in ["auraName", "nam"] {
        let value =
            icon.get_public_property(AvmString::new_utf8(activation.gc(), direct), activation)?;
        if let Avm2Value::String(name) = value
            && !name.is_empty()
        {
            return Ok(Some(name));
        }
    }

    let aura =
        icon.get_public_property(AvmString::new_utf8(activation.gc(), "aura"), activation)?;
    if aura.as_object().is_some() {
        let value =
            aura.get_public_property(AvmString::new_utf8(activation.gc(), "nam"), activation)?;
        if let Avm2Value::String(name) = value
            && !name.is_empty()
        {
            return Ok(Some(name));
        }
    }

    Ok(None)
}

/// Note that AQW's aura countdown was intercepted, and with how many arguments.
///
/// The hook only takes the countdown event when there is an argument to take, so a countdown
/// invoked directly rather than as a listener matches and then yields nothing -- indistinguishable,
/// from the outside, from not matching at all. This separates the two, which is the difference
/// between the method match being wrong and the hook reading the wrong thing.
pub(crate) fn note_aqw_aura_countdown_call(applies: bool, argument_count: usize) {
    if !applies || !focus_aura_recoloured() {
        return;
    }
    const REPORTS: usize = 4;
    static REPORTED: AtomicUsize = AtomicUsize::new(0);
    if REPORTED.fetch_add(1, Ordering::Relaxed) >= REPORTS {
        return;
    }
    tracing::info!(
        "AQW Focus aura recolour: aura countdown intercepted with {argument_count} argument(s)"
    );
}

/// Say what the first few countdown targets actually looked like.
///
/// Reports whatever happened, including success. An earlier version logged only failures, which
/// made "the hook never fired" and "the hook fired and found nothing" indistinguishable -- both
/// were silence, and silence is the one answer that cannot be acted on. The fields being read are
/// set on a dynamic clip at runtime, so they cannot be confirmed from the movie's class table.
fn report_focus_icon_shape<'gc>(
    activation: &mut Avm2Activation<'_, 'gc>,
    icon: Avm2Value<'gc>,
    aura_name: Option<AvmString<'gc>>,
) {
    /// Enough to see several different auras go past, few enough to not be noise.
    const REPORTS: usize = 8;
    static REPORTED: AtomicUsize = AtomicUsize::new(0);
    if REPORTED.fetch_add(1, Ordering::Relaxed) >= REPORTS {
        return;
    }

    let class = icon
        .as_object()
        .map(|object| object.instance_class().name().local_name().to_string())
        .unwrap_or_else(|| "not an object".to_string());
    let present = ["auraName", "nam", "aura", "icon1", "icon2", "iMask"]
        .into_iter()
        .filter(|field| {
            let name = AvmString::new_utf8(activation.gc(), *field);
            icon.get_public_property(name, activation)
                .is_ok_and(|value| !matches!(value, Avm2Value::Undefined | Avm2Value::Null))
        })
        .collect::<Vec<_>>()
        .join(", ");
    let named = match aura_name {
        Some(name) => name.to_string(),
        None => "<no name found>".to_string(),
    };
    tracing::info!(
        "AQW Focus aura recolour: countdown target `{class}` carrying [{present}], aura name {named}"
    );
}

/// Flip AQW's own FPS counter, exactly as the game's options row does.
///
/// `optionHandler.as` ("Display FPS") and `World.toggleFPS()` both just flip
/// `rootClass.ui.mcFPS.visible`; the counter's arithmetic runs either way. Calling the game's own
/// public `World.toggleFPS()` keeps the hotkey behaviourally identical to the menu row. Quietly a
/// no-op outside AQW, and before the world exists (login, server select, loading).
pub fn toggle_aqw_fps_display(context: &mut UpdateContext<'_>) -> bool {
    let Some(root) = context.stage.iter_render_list().next() else {
        return false;
    };
    if !is_aqw_game_movie(root.movie().url()) {
        return false;
    }
    let Some(root_object) = root.object2() else {
        return false;
    };

    let mut activation = Avm2Activation::from_nothing(context);
    let toggled = (|| -> Result<bool, crate::avm2::Error<'_>> {
        let world = Avm2Value::from(root_object).get_public_property(
            AvmString::new_utf8(activation.gc(), "world"),
            &mut activation,
        )?;
        if matches!(world, Avm2Value::Null | Avm2Value::Undefined) {
            return Ok(false);
        }
        world.call_public_property(
            AvmString::new_utf8(activation.gc(), "toggleFPS"),
            Avm2FunctionArgs::empty(),
            &mut activation,
        )?;
        Ok(true)
    })();

    match toggled {
        Ok(toggled) => toggled,
        Err(error) => {
            tracing::warn!(?error, "AQW FPS toggle failed");
            false
        }
    }
}

/// Where the FPS counter is pinned along the top of AQW's 960-unit-wide stage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FpsCounterAnchor {
    Left,
    Center,
    Right,
}

/// Place and scale AQW's own FPS counter.
///
/// The clip is the game's `ui.mcFPS`; scaling and moving it are ordinary display-object property
/// writes, the same ones its own scripts could make. Coordinates are in AQW's fixed 960x550
/// design space, which the stage scale mode maps to the window. Quietly a no-op outside AQW and
/// before the UI exists.
///
/// The clip's registration point is NOT its top-left corner: the artwork extends left of and
/// above the origin. Anchoring `x` directly therefore hung half the counter off screen ("only
/// the decimals showed" on Top left), and scaling grew the art up past the stage top. So the
/// anchor maths runs on the clip's VISIBLE rectangle -- `getBounds(ui)`, measured after
/// scaling, in the same space `x`/`y` move the clip in -- and the move applied is the
/// difference between where that rectangle is and where it should sit. Applying it twice is a
/// no-op, which matters because it runs on every save and every toggle.
pub fn style_aqw_fps_display(
    context: &mut UpdateContext<'_>,
    anchor: FpsCounterAnchor,
    scale: f64,
) -> bool {
    const STAGE_WIDTH: f64 = 960.0;
    const EDGE_MARGIN: f64 = 8.0;

    let Some(root) = context.stage.iter_render_list().next() else {
        return false;
    };
    if !is_aqw_game_movie(root.movie().url()) {
        return false;
    }
    let Some(root_object) = root.object2() else {
        return false;
    };

    let mut activation = Avm2Activation::from_nothing(context);
    let styled = (|| -> Result<bool, crate::avm2::Error<'_>> {
        let ui = Avm2Value::from(root_object)
            .get_public_property(AvmString::new_utf8(activation.gc(), "ui"), &mut activation)?;
        if matches!(ui, Avm2Value::Null | Avm2Value::Undefined) {
            return Ok(false);
        }
        let fps = ui.get_public_property(
            AvmString::new_utf8(activation.gc(), "mcFPS"),
            &mut activation,
        )?;
        if matches!(fps, Avm2Value::Null | Avm2Value::Undefined) {
            return Ok(false);
        }

        fps.set_public_property(
            AvmString::new_utf8(activation.gc(), "scaleX"),
            Avm2Value::Number(scale),
            &mut activation,
        )?;
        fps.set_public_property(
            AvmString::new_utf8(activation.gc(), "scaleY"),
            Avm2Value::Number(scale),
            &mut activation,
        )?;

        let bounds = fps.call_public_property(
            AvmString::new_utf8(activation.gc(), "getBounds"),
            Avm2FunctionArgs::from_slice(&[ui]),
            &mut activation,
        )?;
        let bounds_x = bounds
            .get_public_property(AvmString::new_utf8(activation.gc(), "x"), &mut activation)?
            .coerce_to_number(&mut activation)?;
        let bounds_y = bounds
            .get_public_property(AvmString::new_utf8(activation.gc(), "y"), &mut activation)?
            .coerce_to_number(&mut activation)?;
        let bounds_width = bounds
            .get_public_property(
                AvmString::new_utf8(activation.gc(), "width"),
                &mut activation,
            )?
            .coerce_to_number(&mut activation)?;
        let x_now = fps
            .get_public_property(AvmString::new_utf8(activation.gc(), "x"), &mut activation)?
            .coerce_to_number(&mut activation)?;
        let y_now = fps
            .get_public_property(AvmString::new_utf8(activation.gc(), "y"), &mut activation)?
            .coerce_to_number(&mut activation)?;
        if !(bounds_x.is_finite()
            && bounds_y.is_finite()
            && bounds_width.is_finite()
            && x_now.is_finite()
            && y_now.is_finite())
        {
            // A clip with no drawable content yet reports nothing useful; writing NaN into
            // its position would stick. Leave it where the game put it.
            return Ok(false);
        }

        let target_left = match anchor {
            FpsCounterAnchor::Left => EDGE_MARGIN,
            FpsCounterAnchor::Center => (STAGE_WIDTH - bounds_width) / 2.0,
            FpsCounterAnchor::Right => STAGE_WIDTH - bounds_width - EDGE_MARGIN,
        };
        fps.set_public_property(
            AvmString::new_utf8(activation.gc(), "x"),
            Avm2Value::Number(x_now + (target_left - bounds_x)),
            &mut activation,
        )?;
        // Flush with the stage top at every size, which is where the game's own art sits.
        fps.set_public_property(
            AvmString::new_utf8(activation.gc(), "y"),
            Avm2Value::Number(y_now - bounds_y),
            &mut activation,
        )?;
        Ok(true)
    })();

    match styled {
        Ok(styled) => styled,
        Err(error) => {
            tracing::warn!(?error, "AQW FPS counter styling failed");
            false
        }
    }
}

/// A colour the Focus aura icon can be marked with.
///
/// Multiply-only, because that is what a colour transform on existing artwork can do without
/// flattening it: every channel is either kept or taken down, never added to. So each colour here
/// is really "which channels survive", and the skull's own light and shade survive with them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FocusAuraColour {
    #[default]
    Red,
    Orange,
    Yellow,
    Green,
    Cyan,
    Blue,
    Indigo,
    Pink,
    Magenta,
}

impl FocusAuraColour {
    /// Every colour, in the order they are offered.
    pub const ALL: [Self; 9] = [
        Self::Red,
        Self::Orange,
        Self::Yellow,
        Self::Green,
        Self::Cyan,
        Self::Blue,
        Self::Indigo,
        Self::Pink,
        Self::Magenta,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Red => "red",
            Self::Orange => "orange",
            Self::Yellow => "yellow",
            Self::Green => "green",
            Self::Cyan => "cyan",
            Self::Blue => "blue",
            Self::Indigo => "indigo",
            Self::Pink => "pink",
            Self::Magenta => "magenta",
        }
    }

    /// How far each channel is kept, as a multiplier.
    ///
    /// `DIM` is the measured setting: far enough down to be unmistakable beside the untinted skull
    /// next to it, not so far that the artwork turns into a flat silhouette. `MID` is for the
    /// colours that are a mix rather than a corner of the cube -- orange is red with some green
    /// left in, indigo is blue with some red.
    fn multipliers(self) -> (f64, f64, f64) {
        const DIM: f64 = 0.33;
        const MID: f64 = 0.66;
        match self {
            Self::Red => (1.0, DIM, DIM),
            Self::Orange => (1.0, MID, DIM),
            Self::Yellow => (1.0, 1.0, DIM),
            Self::Green => (DIM, 1.0, DIM),
            Self::Cyan => (DIM, 1.0, 1.0),
            Self::Blue => (DIM, DIM, 1.0),
            Self::Indigo => (MID, DIM, 1.0),
            Self::Pink => (1.0, MID, 0.85),
            Self::Magenta => (1.0, DIM, 1.0),
        }
    }
}

impl std::fmt::Display for FocusAuraColour {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl std::str::FromStr for FocusAuraColour {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|colour| colour.name().eq_ignore_ascii_case(text.trim()))
            .ok_or(())
    }
}

static FOCUS_AURA_COLOUR: AtomicU8 = AtomicU8::new(0);

pub fn set_focus_aura_colour(colour: FocusAuraColour) {
    let index = FocusAuraColour::ALL
        .iter()
        .position(|candidate| *candidate == colour)
        .unwrap_or(0);
    FOCUS_AURA_COLOUR.store(index as u8, Ordering::Relaxed);
}

pub fn focus_aura_colour() -> FocusAuraColour {
    let index = FOCUS_AURA_COLOUR.load(Ordering::Relaxed) as usize;
    FocusAuraColour::ALL.get(index).copied().unwrap_or_default()
}

/// The chosen colour, as a transform to hang on the icon.
fn focus_tint() -> swf::ColorTransform {
    let (r, g, b) = focus_aura_colour().multipliers();
    swf::ColorTransform {
        r_multiply: swf::Fixed8::from_f64(r),
        g_multiply: swf::Fixed8::from_f64(g),
        b_multiply: swf::Fixed8::from_f64(b),
        a_multiply: swf::Fixed8::ONE,
        r_add: 0,
        g_add: 0,
        b_add: 0,
        a_add: 0,
    }
}

#[cfg(test)]
mod focus_aura_colour_tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn every_colour_survives_being_written_and_read_back() {
        // The saved file stores the name, so a colour that does not round-trip is a setting that
        // silently resets to red on restart.
        for colour in FocusAuraColour::ALL {
            assert_eq!(FocusAuraColour::from_str(colour.name()), Ok(colour));
            assert_eq!(FocusAuraColour::from_str(&colour.to_string()), Ok(colour));
        }
    }

    #[test]
    fn a_name_is_read_whatever_its_case_or_spacing() {
        assert_eq!(
            FocusAuraColour::from_str("  MAGENTA "),
            Ok(FocusAuraColour::Magenta)
        );
        assert_eq!(
            FocusAuraColour::from_str("Indigo"),
            Ok(FocusAuraColour::Indigo)
        );
    }

    #[test]
    fn an_unknown_name_is_refused_rather_than_guessed() {
        assert!(FocusAuraColour::from_str("puce").is_err());
        assert!(FocusAuraColour::from_str("").is_err());
    }

    #[test]
    fn no_two_colours_tint_the_same_way() {
        // Nine entries that produced the same transform would be nine identical menu items.
        let mut seen = Vec::new();
        for colour in FocusAuraColour::ALL {
            let multipliers = colour.multipliers();
            assert!(
                !seen.contains(&multipliers),
                "{colour} duplicates another colour"
            );
            seen.push(multipliers);
        }
    }

    #[test]
    fn a_colour_set_is_the_colour_read_back() {
        for colour in FocusAuraColour::ALL {
            set_focus_aura_colour(colour);
            assert_eq!(focus_aura_colour(), colour);
        }
        set_focus_aura_colour(FocusAuraColour::default());
    }
}

static FOCUS_AURA_RECOLOURED: AtomicBool = AtomicBool::new(false);

pub fn set_focus_aura_recoloured(recoloured: bool) {
    // Logged because the alternative failure -- the setting never reaching the core -- looks
    // exactly like the feature not working, and the two need different fixes.
    if FOCUS_AURA_RECOLOURED.swap(recoloured, Ordering::Relaxed) != recoloured {
        tracing::info!(
            "AQW Focus aura recolour is {}",
            if recoloured { "on" } else { "off" }
        );
    }
}

pub fn focus_aura_recoloured() -> bool {
    FOCUS_AURA_RECOLOURED.load(Ordering::Relaxed)
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
    is_hosted_aqw_game_movie(movie_url)
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
    is_aqw_game_movie(movie_url) && class_local_name == "mcOption" && !has_nonempty_class_namespace
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
    // Byte-compare the local name before anything that allocates. This runs for EVERY frame
    // script in the game, and the two overlay classes it exists for appear a handful of times a
    // session; the previous version paid two String conversions per script for that handful.
    // Same treatment as the mcOption check in `timeline_child_rebind_applies`.
    let class_name = object.instance_class().name();
    let local = class_name.local_name().as_wstr();
    if local != b"mcInfoOverlay_233" && local != b"mcInfoOverlay_388" {
        return false;
    }
    let class_namespace_uri = class_name
        .namespace()
        .as_uri_opt()
        .map(|uri| uri.to_string());
    let class_local_name = local.to_string();
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
    is_aqw_loader_movie(movie_url)
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
    is_aqw_game_movie(movie_url)
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

    /// `Loader3.swf` loads whichever build `api/data/gameversion` names, so the game movie is
    /// called something new every release. Every repair here is gated on recognising it.
    #[test]
    fn a_versioned_game_build_is_recognised_as_the_game() {
        assert!(is_aqw_game_movie(
            "https://game.aq.com/game/gamefiles/Game3098r24.swf?ver=R0047"
        ));
        assert!(is_hosted_aqw_game_movie(
            "https://game.aq.com/game/gamefiles/Game3098r24.swf?ver=R0047"
        ));
        // A later release, and the same file with the case AQW's own loader would not have used.
        assert!(is_aqw_game_movie(
            "https://game.aq.com/game/gamefiles/Game4100r1.swf"
        ));
        assert!(is_aqw_game_movie(
            "https://game.aq.com/game/gamefiles/GAME3098R24.SWF"
        ));
    }

    /// The staging build Aether loaded until 0.5.14. Anyone still pointed at it keeps the repairs.
    #[test]
    fn the_frozen_staging_build_is_still_recognised() {
        assert!(is_aqw_game_movie(
            "https://game.aq.com/game/gamefiles/spider.swf?ver=0.6"
        ));
        assert!(is_hosted_aqw_game_movie(
            "https://game.aq.com/game/gamefiles/spider.swf?ver=0.6"
        ));
    }

    /// The near miss. A spellcraft map begins with the same four letters and is not the game, and
    /// the loaders sit in the same directory as the build they load.
    #[test]
    fn maps_loaders_and_other_hosts_are_not_the_game() {
        assert!(!is_aqw_game_movie(
            "https://game.aq.com/game/gamefiles/maps/tradeskills/spellcraft/game-spellcraftr2.swf"
        ));
        assert!(!is_aqw_game_movie(
            "https://game.aq.com/game/gamefiles/Loader3.swf?ver=a"
        ));
        assert!(!is_aqw_game_movie(
            "https://game.aq.com/game/gamefiles/Loader_Spider.swf"
        ));
        assert!(!is_aqw_game_movie(
            "https://game.aq.com/game/gamefiles/assets/assets_2026.swf"
        ));
        // Named like the game, served by somebody else.
        assert!(!is_hosted_aqw_game_movie(
            "https://example.invalid/gamefiles/Game3098r24.swf"
        ));
        // The shape without the name, and the name without the file.
        assert!(!is_aqw_game_movie(
            "https://game.aq.com/game/gamefiles/.swf"
        ));
        assert!(!is_aqw_game_movie(
            "https://game.aq.com/game/gamefiles/Game"
        ));
    }

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
        // The live loader, which is the one Aether runs from 0.5.14. This repair is about the
        // loader rather than the game, so switching loaders is what would silently retire it.
        assert!(is_aqw_parent_timeline_label_fallback(
            "https://game.aq.com/game/gamefiles/Loader3.swf?ver=a",
            "Login",
            false,
            true,
        ));
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

/// How AQW's on-screen numbers are grouped, if at all.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NumberSeparator {
    /// Leave the number exactly as AQW wrote it.
    None,
    /// `1,250,000`.
    Comma,
    /// `1 250 000`. Some readers find a comma easy to mistake for a decimal point, and much of
    /// the world writes decimals that way, so a space is the less ambiguous choice for them.
    Space,
}

impl NumberSeparator {
    fn character(self) -> Option<char> {
        match self {
            NumberSeparator::None => None,
            NumberSeparator::Comma => Some(','),
            // A plain ASCII space, not a thin or non-breaking one. AQW's embedded fonts cannot be
            // relied on to carry the typographically correct glyphs.
            NumberSeparator::Space => Some(' '),
        }
    }
}

static NUMBER_SEPARATOR: AtomicU8 = AtomicU8::new(0);

/// Group AQW's on-screen numbers for readability.
#[inline]
pub fn set_number_separator(separator: NumberSeparator) {
    NUMBER_SEPARATOR.store(
        match separator {
            NumberSeparator::None => 0,
            NumberSeparator::Comma => 1,
            NumberSeparator::Space => 2,
        },
        Ordering::Relaxed,
    );
}

#[inline]
pub fn number_separator() -> NumberSeparator {
    match NUMBER_SEPARATOR.load(Ordering::Relaxed) {
        1 => NumberSeparator::Comma,
        2 => NumberSeparator::Space,
        _ => NumberSeparator::None,
    }
}

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

/// Group every standalone number inside a line of text, as `Bones 152197/1000000` to
/// `Bones 152,197/1,000,000`.
///
/// A digit run is only grouped when it stands alone as a quantity. A run touching a letter, a dot
/// or a hyphen is left alone, which keeps identifiers, version numbers, decimals and negatives
/// intact. `citadelruins-99922` is the case that motivated the hyphen rule.
pub fn group_number_runs(
    text: &str,
    separator: NumberSeparator,
    min_digits: usize,
) -> Option<String> {
    if separator.character().is_none() || !has_groupable_run(text, min_digits) {
        return None;
    }

    let mut grouped = String::with_capacity(text.len() + text.len() / 3);
    let changed = group_number_runs_into(&mut grouped, text, separator, min_digits);
    changed.then_some(grouped)
}

/// Whether `text` holds a digit run long enough to be worth a closer look.
///
/// This runs on every text assignment AQW makes, including each line of chat and each damage
/// number, and almost none of them contain a long number. Answering no here costs one pass over
/// the bytes and no allocation at all, where the grouping pass would allocate a buffer first and
/// only then discover there was nothing to do.
fn has_groupable_run(text: &str, min_digits: usize) -> bool {
    let mut run = 0;
    for byte in text.bytes() {
        if byte.is_ascii_digit() {
            run += 1;
            if run >= min_digits {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

/// Append `text` to `grouped`, separating any digit run that reads as a quantity. Reports whether
/// anything was actually rewritten.
///
/// A digit run is ASCII by definition, so the scan can index bytes to find one. Everything between
/// the runs is copied back as whole characters: AQW carries accented item names and symbol glyphs,
/// and copying those a byte at a time would turn each one into mojibake.
fn group_number_runs_into(
    grouped: &mut String,
    text: &str,
    separator: NumberSeparator,
    min_digits: usize,
) -> bool {
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut copied_from = 0;
    let mut changed = false;

    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }

        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }

        let joined_before = text[..start]
            .chars()
            .next_back()
            .is_some_and(is_number_glue);
        let joined_after = text[index..].chars().next().is_some_and(is_number_glue);

        if joined_before || joined_after {
            continue;
        }

        if let Some(separated) = group_digits(&text[start..index], separator, min_digits) {
            grouped.push_str(&text[copied_from..start]);
            grouped.push_str(&separated);
            copied_from = index;
            changed = true;
        }
    }

    grouped.push_str(&text[copied_from..]);
    changed
}

/// Group the numbers in a line of Flash HTML, leaving the markup itself alone.
///
/// AQW colours its quest rewards, so a reward line arrives as `<font color="#FFFF00">10000000</font>
/// gold` rather than as plain text. Only the text between the tags is a quantity. A number inside a
/// tag is a colour, a font size, a tab stop or the target of a chat link, and rewriting one would
/// change what the markup means.
///
/// Character entities are skipped for the same reason: `&#8203;` is a single character written as a
/// number, and separating it would print the entity instead of the glyph.
pub fn group_number_runs_in_html(
    html: &str,
    separator: NumberSeparator,
    min_digits: usize,
) -> Option<String> {
    /// `&` plus the longest entity name Flash recognises plus `;`, with room to spare. Bounding
    /// the search stops a bare `&` in chat from swallowing the rest of the line.
    const MAX_ENTITY_LEN: usize = 12;

    if separator.character().is_none() || !has_groupable_run(html, min_digits) {
        return None;
    }

    let bytes = html.as_bytes();
    let mut grouped = String::with_capacity(html.len() + html.len() / 3);
    let mut changed = false;
    let mut index = 0;
    let mut text_from = 0;

    while index < bytes.len() {
        // A tag runs to the next `>`, or to the end of the string if AQW left one unclosed.
        let skip_to = match bytes[index] {
            b'<' => html[index..]
                .find('>')
                .map_or(bytes.len(), |end| index + end + 1),
            // An entity is `&`, up to a handful of name or digit characters, then `;`. A longer
            // run without a `;` is a stray ampersand, which is ordinary text.
            b'&' => {
                let limit = bytes.len().min(index + MAX_ENTITY_LEN);
                match bytes[index..limit].iter().position(|&byte| byte == b';') {
                    Some(end) => index + end + 1,
                    None => {
                        index += 1;
                        continue;
                    }
                }
            }
            _ => {
                index += 1;
                continue;
            }
        };

        changed |=
            group_number_runs_into(&mut grouped, &html[text_from..index], separator, min_digits);
        grouped.push_str(&html[index..skip_to]);
        index = skip_to;
        text_from = skip_to;
    }

    changed |= group_number_runs_into(&mut grouped, &html[text_from..], separator, min_digits);
    changed.then_some(grouped)
}

/// Whether the character next to a digit run means the run is part of a larger token rather than a
/// quantity of its own.
///
/// Letters are tested with the full Unicode rule rather than the ASCII one, so an accented item
/// name glued to digits is treated the same way an unaccented one would be. Symbols are not glue:
/// a currency mark or a bullet sitting against a number still leaves a quantity behind it.
fn is_number_glue(character: char) -> bool {
    character.is_alphabetic() || character == '-' || character == '.' || character == '_'
}

/// The clips AQW puts text written by people inside.
///
/// Grouping is granted to every display field by default and refused under one of these, which is
/// the opposite of how this started. The turn is because the two lists are not the same shape. The
/// places a *number* appears have no end: gold and health were the obvious pair, then quest
/// rewards, then the experience bar, then item stacks, then the reputation panel, each one its own
/// name to discover and each discovery costing a release. The places a *player* writes do end, and
/// this is all of them.
///
/// `nc` and `ncTextLine` hold the chat log. `textLine` and `bmp` hold the same log in AQW's older
/// chat interface, which is still selectable and builds its lines a different way, so both have to
/// be named. `bubble` is the speech balloon over a character's head, the one place chat is drawn
/// outside the chat window.
///
/// None of these names is used for anything else in AQW; `textLine` occurs exactly once in the
/// whole game, as the legacy chat container.
const PLAYER_TEXT_CONTAINERS: [&[u8]; 5] = [b"nc", b"ncTextLine", b"textLine", b"bmp", b"bubble"];

/// Whether a clip on the path down to a field means the field holds what somebody typed.
pub fn is_aqw_player_text_container(name: &crate::string::WStr) -> bool {
    PLAYER_TEXT_CONTAINERS
        .iter()
        .any(|container| name == *container)
}

/// The clips the font override restyles by default, on top of the chat ones above.
///
/// `pname` is the plate over a character's head. Players (`AvatarMC`), monsters (`MonsterMC`) and
/// pets (`PetMC`) all hang their name, guild and type fields inside one, and the game's own font
/// option scopes to exactly this clip: `Game.applyComicSansPname` restyles `pname.ti`, `pname.tg`
/// and `pname.typ` and nothing else. Keying on the container rather than the three field names
/// avoids matching a stray `ti`/`tg` elsewhere, the same way the chat list keys on `nc` rather
/// than the line fields inside it.
const NAMEPLATE_CONTAINERS: [&[u8]; 1] = [b"pname"];

/// Whether a clip on the path down to a field puts the field in the font override's default scope:
/// a chat line, or a name over a character's head.
///
/// The two settings the game ships for this land in the same place. Its "Chat UI" option restyles
/// the chat log, the clips in [`PLAYER_TEXT_CONTAINERS`]; its "Comic Sans Font" option restyles the
/// nameplates, the clips in [`NAMEPLATE_CONTAINERS`]. Together they are what a player reads during
/// play, as opposed to the menus, server list and buttons that make up the rest of the interface --
/// which is why this is the default, and why widening to everything is a separate opt-in.
pub fn is_aqw_scoped_text_container(name: &crate::string::WStr) -> bool {
    PLAYER_TEXT_CONTAINERS
        .iter()
        .chain(NAMEPLATE_CONTAINERS.iter())
        .any(|container| name == *container)
}

/// The line-wrapper classes AQW's chat builds each log line inside.
///
/// A chat line is created, has its text set, and only *then* is named (`"bmp"`) and added to the
/// log -- `Chat` does all three in that order. At the one moment the line lays itself out it is
/// therefore both unparented and unnamed, and the container-name walk finds nothing to match. The
/// class is set from birth, though: the wrapper is a `uiTextLine` (`uiTextLine2` in the alternate
/// interface), so matching on it catches the line at the only layout it gets. Without this a chosen
/// font reaches the nameplates but not fresh chat, which is the timing the container name alone
/// cannot see.
const CHAT_LINE_CLASSES: [&[u8]; 2] = [b"uiTextLine", b"uiTextLine2"];

/// Whether a clip's class name marks it as a chat line wrapper; see [`CHAT_LINE_CLASSES`].
pub fn is_aqw_scoped_text_class(class_local_name: &crate::string::WStr) -> bool {
    CHAT_LINE_CLASSES
        .iter()
        .any(|class| class_local_name == *class)
}

/// The chat log clips, minus the speech balloon.
///
/// A line in the log is one of a stack, and AQW works out where each one sits when it adds it. Its
/// own height is therefore not private to it: laying it out again at a different height leaves it
/// overlapping the lines above and below, because nothing moves them. The balloon is left off this
/// list because it floats on its own over a character and is positioned from its own size.
const STACKED_CHAT_CONTAINERS: [&[u8]; 4] = [b"nc", b"ncTextLine", b"textLine", b"bmp"];

/// Whether a clip on the path down to a field means the game decides where the field sits.
///
/// Used to leave such a field out of a forced relayout: it will pick the new font up when AQW next
/// rebuilds the log, which is the only moment the stack's positions are recomputed.
pub fn is_aqw_game_positioned_container(name: &crate::string::WStr) -> bool {
    STACKED_CHAT_CONTAINERS
        .iter()
        .any(|container| name == *container)
}

/// Group a plain run of digits for readability, as `1250000` to `1,250,000`.
///
/// Requested by a dyslexic player who could read the digits but not the magnitude: with boss health
/// in the millions there is nothing to anchor where the millions place sits, and the number changes
/// several times a second during a fight.
///
/// Only an unbroken run of digits is touched. Anything AQW puts in this field that is not a bare
/// number, such as the literal `X` it writes for a dead target, is passed through unchanged.
pub fn group_digits(text: &str, separator: NumberSeparator, min_digits: usize) -> Option<String> {
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

/// Rewrite a display text assignment with grouped numbers, or `None` to leave it alone.
///
/// Two things have to agree before a digit run is touched, and they guard different mistakes. The
/// caller decides whether the field is one a player could have written into, which is what keeps
/// this out of chat; see [`is_aqw_player_text_container`]. This decides whether the run is a
/// quantity rather than part of a token, which is what keeps `v1000000` and `1.50000` intact, and
/// what keeps a room number safe once it is welded to its map as `citadelruins-99922`.
pub fn aqw_grouped_text(text: &str) -> Option<String> {
    group_number_runs(text, number_separator(), separator_min_digits() as usize)
}

/// The same rewrite for a field being assigned HTML rather than plain text.
pub fn aqw_grouped_html(html: &str) -> Option<String> {
    group_number_runs_in_html(html, number_separator(), separator_min_digits() as usize)
}

#[cfg(test)]
mod digit_grouping_tests {
    use super::{
        NumberSeparator, group_digits, group_number_runs, is_aqw_player_text_container,
        is_aqw_scoped_text_class, is_aqw_scoped_text_container,
    };

    #[test]
    fn boss_health_gets_separators_at_every_thousand() {
        assert_eq!(
            group_digits("1250000", NumberSeparator::Comma, 4).as_deref(),
            Some("1,250,000")
        );
        assert_eq!(
            group_digits("999999999", NumberSeparator::Comma, 4).as_deref(),
            Some("999,999,999")
        );
        assert_eq!(
            group_digits("1000", NumberSeparator::Comma, 4).as_deref(),
            Some("1,000")
        );
        assert_eq!(
            group_digits("12345", NumberSeparator::Comma, 4).as_deref(),
            Some("12,345")
        );
        assert_eq!(
            group_digits("123456", NumberSeparator::Comma, 4).as_deref(),
            Some("123,456")
        );
    }

    #[test]
    fn spaces_group_the_same_places_as_commas() {
        assert_eq!(
            group_digits("1250000", NumberSeparator::Space, 4).as_deref(),
            Some("1 250 000")
        );
        assert_eq!(
            group_digits("12345", NumberSeparator::Space, 4).as_deref(),
            Some("12 345")
        );
        // Off means untouched, whatever the number.
        assert_eq!(group_digits("1250000", NumberSeparator::None, 4), None);
    }

    #[test]
    fn quest_progress_lines_group_both_sides_of_the_slash() {
        assert_eq!(
            group_number_runs(
                "Unleash Doom: Inquisitor Bones 152197/1000000",
                NumberSeparator::Comma,
                4
            )
            .as_deref(),
            Some("Unleash Doom: Inquisitor Bones 152,197/1,000,000")
        );
        assert_eq!(
            group_number_runs("Refined Metal 0/1000000", NumberSeparator::Comma, 4).as_deref(),
            Some("Refined Metal 0/1,000,000")
        );
    }

    fn player_text(name: &[u8]) -> bool {
        is_aqw_player_text_container(crate::string::WStr::from_units(name))
    }

    /// The chat log, in both of AQW's chat interfaces. `nc` and `ncTextLine` are the current one,
    /// `textLine` and `bmp` the older one that is still selectable in the options.
    #[test]
    fn the_chat_log_is_where_players_write() {
        assert!(player_text(b"nc"));
        assert!(player_text(b"ncTextLine"));
        assert!(player_text(b"textLine"));
        assert!(player_text(b"bmp"));
    }

    /// The one place chat is drawn outside the chat window.
    #[test]
    fn the_speech_balloon_is_where_players_write() {
        assert!(player_text(b"bubble"));
    }

    fn scoped_text(name: &[u8]) -> bool {
        is_aqw_scoped_text_container(crate::string::WStr::from_units(name))
    }

    /// The font override's default scope is chat plus the nameplate. `pname` is the plate players,
    /// monsters and pets all share; the chat clips carry over from the list above so a chosen font
    /// reaches both halves of what a player reads during play.
    #[test]
    fn the_font_scope_is_chat_and_the_nameplate() {
        assert!(scoped_text(b"pname"));
        assert!(scoped_text(b"nc"));
        assert!(scoped_text(b"bubble"));
    }

    /// The interface chrome stays in the game's own font by default: the readout containers, and a
    /// near miss on the nameplate name, are all out of scope.
    #[test]
    fn the_interface_chrome_is_left_alone_by_default() {
        assert!(!scoped_text(b"mcGold"));
        assert!(!scoped_text(b"strIntHP"));
        assert!(!scoped_text(b"pnames"));
        assert!(!scoped_text(b"pname_"));
        assert!(!scoped_text(b""));
    }

    fn scoped_class(name: &[u8]) -> bool {
        is_aqw_scoped_text_class(crate::string::WStr::from_units(name))
    }

    /// A fresh chat line is unnamed and unparented at the moment it lays itself out, so it is caught
    /// by the wrapper class instead. Both interfaces' wrappers count; a near miss does not.
    #[test]
    fn a_chat_line_is_caught_by_its_wrapper_class() {
        assert!(scoped_class(b"uiTextLine"));
        assert!(scoped_class(b"uiTextLine2"));
        assert!(!scoped_class(b"uiTextLine3"));
        assert!(!scoped_class(b"TextLine"));
        assert!(!scoped_class(b""));
    }

    /// Everything a number lands in. Naming these was the losing side of this problem: each one
    /// was found by somebody noticing a number that read badly, one release at a time.
    #[test]
    fn the_places_numbers_land_are_not_excluded() {
        for readout in [
            // Gold, health, mana, soul points, the class rank line.
            &b"mcGold"[..],
            b"strIntHP",
            b"strRep",
            // The quest panel, the experience bar, the reputation standings list.
            b"strRew",
            b"strReq",
            b"strXP",
            b"fList",
            // Item stacks, and the numbers that float over an avatar.
            b"strQ",
            b"tQty",
            b"t",
        ] {
            assert!(
                !player_text(readout),
                "{} must stay groupable",
                String::from_utf8_lossy(readout)
            );
        }
    }

    /// A name that merely starts or ends like one on the list is not one on the list.
    #[test]
    fn a_near_miss_on_a_container_name_is_not_a_container() {
        assert!(!player_text(b"ncTextLines"));
        assert!(!player_text(b"mcTextLine"));
        assert!(!player_text(b"bubbles"));
        assert!(!player_text(b"n"));
        assert!(!player_text(b""));
    }

    #[test]
    fn room_numbers_and_other_identifiers_are_never_grouped() {
        // The case from a real screenshot. A room name is not a quantity.
        assert_eq!(
            group_number_runs("1 player in citadelruins-99922", NumberSeparator::Comma, 4),
            None
        );
        // Anything welded to letters, dots or underscores is part of a token, not a count.
        assert_eq!(
            group_number_runs("v1000000", NumberSeparator::Comma, 4),
            None
        );
        assert_eq!(
            group_number_runs("item_10000", NumberSeparator::Comma, 4),
            None
        );
        assert_eq!(
            group_number_runs("1.50000", NumberSeparator::Comma, 4),
            None
        );
        assert_eq!(group_number_runs("-25000", NumberSeparator::Comma, 4), None);
        assert_eq!(
            group_number_runs("spider.swf?ver=0.60398", NumberSeparator::Comma, 4),
            None
        );
    }

    #[test]
    fn grouping_can_start_at_ten_thousand_instead() {
        // 2750 reads fine unseparated; 12750 is where it starts to help.
        assert_eq!(group_digits("2750", NumberSeparator::Comma, 5), None);
        assert_eq!(
            group_digits("12750", NumberSeparator::Comma, 5).as_deref(),
            Some("12,750")
        );
        assert_eq!(
            group_number_runs("Bones 2750/1000000", NumberSeparator::Comma, 5).as_deref(),
            Some("Bones 2750/1,000,000")
        );
    }

    #[test]
    fn short_numbers_are_left_alone() {
        // Grouping these would add noise without making the magnitude any clearer.
        assert_eq!(group_digits("0", NumberSeparator::Comma, 4), None);
        assert_eq!(group_digits("42", NumberSeparator::Comma, 4), None);
        assert_eq!(group_digits("999", NumberSeparator::Comma, 4), None);
    }

    #[test]
    fn anything_that_is_not_a_bare_number_is_untouched() {
        // AQW writes a literal X into this field for a dead target.
        assert_eq!(group_digits("X", NumberSeparator::Comma, 4), None);
        assert_eq!(group_digits("1,250,000", NumberSeparator::Comma, 4), None);
        assert_eq!(group_digits("12000/45000", NumberSeparator::Comma, 4), None);
        assert_eq!(group_digits("-5000", NumberSeparator::Comma, 4), None);
        assert_eq!(group_digits("", NumberSeparator::Comma, 4), None);
        assert_eq!(group_digits("1234 HP", NumberSeparator::Comma, 4), None);
    }
}

#[cfg(test)]
mod number_separator_tests {
    use super::{
        NumberSeparator, aqw_grouped_html, aqw_grouped_text, group_number_runs,
        group_number_runs_in_html,
    };

    /// Every place a screenshot showed an ungrouped number. The field-name allow list this
    /// replaced only reached two of them.
    #[test]
    fn the_readouts_players_actually_look_at_are_grouped() {
        for (before, after) in [
            // The gold total under the character portrait.
            ("1217613", "1,217,613"),
            // The experience bar, and the reputation bar, both hover readouts.
            (
                "Level 65 : 1453237 / 1680000 (86%)",
                "Level 65 : 1,453,237 / 1,680,000 (86%)",
            ),
            (
                "Soul Cleaver, Rank 8 : 32651/72900 (44%)",
                "Soul Cleaver, Rank 8 : 32,651/72,900 (44%)",
            ),
            // Quest item counts, in the tooltip and in the preview panel.
            ("Item - 1/20000", "Item - 1/20,000"),
            ("Quest Item - 1/10000", "Quest Item - 1/10,000"),
            // Inventory stack sizes.
            ("32000", "32,000"),
        ] {
            assert_eq!(
                group_number_runs(before, NumberSeparator::Comma, 4).as_deref(),
                Some(after),
                "{before}"
            );
        }
    }

    /// Quest rewards are coloured, so they arrive as HTML rather than plain text.
    #[test]
    fn quest_reward_html_is_grouped_without_disturbing_the_markup() {
        assert_eq!(
            group_number_runs_in_html(
                "<font color=\"#FFFF00\">10000000</font> gold<br><font color=\"#9900FF\">30000</font> xp",
                NumberSeparator::Comma,
                4,
            )
            .as_deref(),
            Some(
                "<font color=\"#FFFF00\">10,000,000</font> gold<br><font color=\"#9900FF\">30,000</font> xp"
            )
        );
    }

    /// A number inside a tag is markup, not a quantity. Rewriting one would change a colour, a
    /// font size, or the target of a chat link.
    #[test]
    fn numbers_inside_tags_are_left_exactly_as_written() {
        for markup in [
            "<font size=\"10000\">hi</font>",
            "<a href=\"event:trade,1234567\">Player</a>",
            "<textformat tabstops=\"[10000]\">x</textformat>",
            "<img src=\"item12345678.png\">",
        ] {
            assert_eq!(
                group_number_runs_in_html(markup, NumberSeparator::Comma, 4),
                None,
                "{markup}"
            );
        }
    }

    /// `&#12345;` is one character, not a number. Splitting it would print a literal entity.
    #[test]
    fn html_entities_are_left_exactly_as_written() {
        assert_eq!(
            group_number_runs_in_html("&#100000; and 25000", NumberSeparator::Comma, 4).as_deref(),
            Some("&#100000; and 25,000")
        );
        assert_eq!(
            group_number_runs_in_html("&amp;&nbsp;&#8203;", NumberSeparator::Comma, 4),
            None
        );
    }

    /// The scanner walks bytes to find digits. Everything else has to come back out as it went
    /// in, including the multi-byte characters in item and player names.
    #[test]
    fn non_ascii_text_survives_the_scan() {
        assert_eq!(
            group_number_runs("Café Sol 25000", NumberSeparator::Comma, 4).as_deref(),
            Some("Café Sol 25,000")
        );
        assert_eq!(
            group_number_runs("☠ 1250000 ☠", NumberSeparator::Comma, 4).as_deref(),
            Some("☠ 1,250,000 ☠")
        );
        assert_eq!(
            group_number_runs("Café Sol", NumberSeparator::Comma, 4),
            None
        );
    }

    /// The glue rule asks whether the neighbouring character is a letter, in the full Unicode
    /// sense rather than the ASCII one. A symbol is not a letter, so a quantity behind one still
    /// reads as a quantity.
    #[test]
    fn accented_letters_glue_but_symbols_do_not() {
        assert_eq!(
            group_number_runs("Café25000", NumberSeparator::Comma, 4),
            None
        );
        assert_eq!(
            group_number_runs("€1000000", NumberSeparator::Comma, 4).as_deref(),
            Some("€1,000,000")
        );
    }

    /// Grouping already-grouped text has to be a no-op, because AQW reassigns the same fields
    /// every frame and some of its own panels already carry separators.
    #[test]
    fn grouping_the_same_text_twice_changes_nothing() {
        assert_eq!(
            group_number_runs("1,250,000", NumberSeparator::Comma, 4),
            None
        );
        assert_eq!(
            group_number_runs("1 250 000", NumberSeparator::Space, 4),
            None
        );
    }

    #[test]
    fn the_feature_is_inert_until_it_is_switched_on() {
        use super::{set_number_separator, set_separator_min_digits};

        set_number_separator(NumberSeparator::None);
        assert_eq!(aqw_grouped_text("1250000"), None);
        assert_eq!(aqw_grouped_html("<b>1250000</b>"), None);

        set_number_separator(NumberSeparator::Comma);
        assert_eq!(aqw_grouped_text("1250000").as_deref(), Some("1,250,000"));

        set_separator_min_digits(5);
        assert_eq!(aqw_grouped_text("2750"), None);
        set_separator_min_digits(4);

        set_number_separator(NumberSeparator::None);
    }
}

/// AQW's own stage, which every position `ToolTipMC` writes is measured against.
const AQW_TOOLTIP_STAGE_WIDTH: f64 = 960.0;

/// The other half of the same figure, used by the corner AQW pins skill tooltips to.
const AQW_TOOLTIP_STAGE_HEIGHT: f64 = 480.0;

/// How far above the pointer the tooltip sits, matching AQW's own figure for its cursor-following
/// tooltips so the two look the same.
const AQW_TOOLTIP_POINTER_GAP: f64 = 15.0;

static TOOLTIP_FOLLOWS_POINTER: AtomicBool = AtomicBool::new(false);
static SKILL_TOOLTIPS_HIDDEN: AtomicBool = AtomicBool::new(false);

/// Whether the tooltip on screen right now is a skill's, remembered from the frame it opened on.
///
/// AQW writes a tooltip's position once, in `open`, so where it sits is only a reliable answer to
/// "what kind of tooltip is this" before anything here has moved it.
static TOOLTIP_WAS_SHOWING: AtomicBool = AtomicBool::new(false);
static TOOLTIP_IS_A_SKILL: AtomicBool = AtomicBool::new(false);

/// Set while a tooltip is hidden by us rather than by AQW, so it can be put back afterwards.
static TOOLTIP_HIDDEN_BY_US: AtomicBool = AtomicBool::new(false);
static AURA_TOOLTIPS_ALWAYS_SHOWN: AtomicBool = AtomicBool::new(false);

pub fn set_tooltip_follows_pointer_enabled(enabled: bool) {
    TOOLTIP_FOLLOWS_POINTER.store(enabled, Ordering::Relaxed);
}

pub fn tooltip_follows_pointer_enabled() -> bool {
    TOOLTIP_FOLLOWS_POINTER.load(Ordering::Relaxed)
}

pub fn set_skill_tooltips_hidden(hidden: bool) {
    SKILL_TOOLTIPS_HIDDEN.store(hidden, Ordering::Relaxed);
}

pub fn skill_tooltips_hidden() -> bool {
    SKILL_TOOLTIPS_HIDDEN.load(Ordering::Relaxed)
}

pub fn set_aura_tooltips_always_shown(shown: bool) {
    AURA_TOOLTIPS_ALWAYS_SHOWN.store(shown, Ordering::Relaxed);
}

pub fn aura_tooltips_always_shown() -> bool {
    AURA_TOOLTIPS_ALWAYS_SHOWN.load(Ordering::Relaxed)
}

/// Whether a tooltip was placed by AQW's "pin it to the bottom right" branch.
///
/// That branch is what a skill's tooltip uses, and nothing a player hovers in combat uses any
/// other. Buffs and auras take the cursor-following branch instead, which is why they can be kept
/// while skills are hidden: the two are already distinguishable by where AQW put them.
fn was_pinned_to_the_corner(x: f64, y: f64, width: f64, height: f64) -> bool {
    let corner_x = AQW_TOOLTIP_STAGE_WIDTH - width - 4.0;
    let corner_y = AQW_TOOLTIP_STAGE_HEIGHT - height - 4.0;
    (x - corner_x).abs() < 1.5 && (y - corner_y).abs() < 1.5
}

/// Whether this tooltip is the account-safety warning rather than something being hovered.
///
/// `ToolTipMC` paints the warning's background black through a colour transform and leaves every
/// other tooltip's alone. The warning is not attached to anything the pointer is over -- it appears
/// on a chat event and closes itself after ten seconds -- so dragging it to the cursor would move a
/// thing the player is meant to read, and it is left where AQW put it.
fn is_the_account_safety_warning(tooltip: DisplayObject<'_>) -> bool {
    let Some(container) = tooltip.as_container() else {
        return false;
    };
    let Some(content) = container.child_by_name(WStr::from_units(b"cnt"), false) else {
        return false;
    };
    let Some(background) = content
        .as_container()
        .and_then(|content| content.child_by_name(WStr::from_units(b"bg"), false))
    else {
        return false;
    };

    let tint = background.base().color_transform();
    tint.r_multiply.to_f32() == 0.0 && tint.g_multiply.to_f32() == 0.0
}

/// AQW's tooltip, if it is on screen.
fn open_aqw_tooltip<'gc>(context: &mut UpdateContext<'gc>) -> Option<DisplayObject<'gc>> {
    // `Game.ui.ToolTip`, reached by name rather than by walking the tree, because this runs every
    // frame and the tree is thousands of objects deep by the time a map is loaded.
    let stage = context.stage;
    for child in stage.iter_render_list() {
        let Some(game) = child.as_container() else {
            continue;
        };
        let Some(ui) = game.child_by_name(WStr::from_units(b"ui"), false) else {
            continue;
        };
        let Some(tooltip) = ui
            .as_container()
            .and_then(|ui| ui.child_by_name(WStr::from_units(b"ToolTip"), false))
        else {
            continue;
        };

        // `openWith` only starts a timer; `open` is what makes the contents visible, so `cnt` is
        // how to tell a tooltip that is showing from one that is merely constructed.
        //
        // Deliberately not `tooltip.visible()`: hiding one is done by clearing exactly that, and
        // reading it back here would mean a tooltip could never be found again once hidden, and so
        // could never be restored.
        let open = tooltip
            .as_container()
            .and_then(|tooltip| tooltip.child_by_name(WStr::from_units(b"cnt"), false))
            .is_some_and(|content| content.visible());
        if open {
            return Some(tooltip);
        }
    }
    None
}

/// Put AQW's tooltip above the pointer instead of wherever it asked to go.
///
/// AQW positions a skill's tooltip by pinning it to the bottom right of the stage, which puts it
/// over the bag icon rather than near the skill being hovered, and far enough away that players
/// have clicked through it while trying to move. `ToolTipMC` already knows how to sit above a
/// point -- its cursor-following tooltips do exactly that -- so this puts every tooltip there,
/// using AQW's own offset so they match.
pub fn reposition_aqw_tooltip(context: &mut UpdateContext<'_>) {
    let hide_skills = skill_tooltips_hidden();
    let follow_pointer = tooltip_follows_pointer_enabled();
    let keep_auras = aura_tooltips_always_shown();
    // Something we hid has to be put back even after every switch is turned off again, so the flag
    // is part of what decides whether there is work to do.
    let ours_to_undo = TOOLTIP_HIDDEN_BY_US.load(Ordering::Relaxed);
    if !hide_skills && !follow_pointer && !keep_auras && !ours_to_undo {
        TOOLTIP_WAS_SHOWING.store(false, Ordering::Relaxed);
        return;
    }

    let Some(tooltip) = open_aqw_tooltip(context) else {
        TOOLTIP_WAS_SHOWING.store(false, Ordering::Relaxed);
        if ours_to_undo {
            TOOLTIP_HIDDEN_BY_US.store(false, Ordering::Relaxed);
        }
        return;
    };
    if is_the_account_safety_warning(tooltip) {
        return;
    }
    let Some(parent) = tooltip.parent() else {
        return;
    };

    let width = tooltip.width();
    let height = tooltip.height();

    // Classified on the frame it opens, before anything here has moved it.
    if !TOOLTIP_WAS_SHOWING.swap(true, Ordering::Relaxed) {
        let pinned = was_pinned_to_the_corner(
            tooltip.x().to_pixels(),
            tooltip.y().to_pixels(),
            width,
            height,
        );
        TOOLTIP_IS_A_SKILL.store(pinned, Ordering::Relaxed);
    }
    let is_a_skill = TOOLTIP_IS_A_SKILL.load(Ordering::Relaxed);

    let is_an_aura = !is_a_skill;
    if (hide_skills && is_a_skill) || (!keep_auras && is_an_aura) {
        // Hidden rather than closed: AQW owns when a tooltip opens and closes, and taking that over
        // would leave one that never comes back.
        tooltip.set_visible(context, false);
        TOOLTIP_HIDDEN_BY_US.store(true, Ordering::Relaxed);
        return;
    }

    if ours_to_undo {
        TOOLTIP_HIDDEN_BY_US.store(false, Ordering::Relaxed);
    }
    tooltip.set_visible(context, true);

    // A buff's tooltip is the one that says what is on you, and it is worth being able to read that
    // while a skill's is suppressed. It follows the cursor, so it is put on screen here whether or
    // not tooltips in general are being moved.
    if !follow_pointer && !(keep_auras && is_an_aura) {
        return;
    }

    // x and y are read in the parent's space, so the pointer has to be too.
    let pointer = parent.local_mouse_position(context);
    let mut x = pointer.x.to_pixels() - width / 2.0;
    let mut y = pointer.y.to_pixels() - height - AQW_TOOLTIP_POINTER_GAP;

    // Kept on screen the way AQW keeps its own: pushed back inside at the right edge, and dropped
    // below the pointer rather than off the top when there is no room above.
    let rightmost = (AQW_TOOLTIP_STAGE_WIDTH - width - 10.0).max(1.0);
    x = x.clamp(1.0, rightmost);
    if y < 1.0 {
        y = pointer.y.to_pixels() + 10.0;
    }

    tooltip.set_x(Twips::from_pixels(x));
    tooltip.set_y(Twips::from_pixels(y));
}

#[cfg(test)]
mod tooltip_tests {
    use super::*;

    /// Both tests below touch the same process-global atomic, and the test harness runs tests in
    /// parallel, so the default-state assertion raced the toggle test's momentary `true`. A flake
    /// that only appeared when scheduling shifted, which is the worst kind.
    static TOOLTIP_GLOBAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Off unless asked for. It overrules where the game puts its own tooltips, which is not
    /// something to do to someone who has not asked.
    #[test]
    fn repositioning_is_off_by_default() {
        let _lock = TOOLTIP_GLOBAL.lock().unwrap();
        assert!(!tooltip_follows_pointer_enabled());
    }

    #[test]
    fn repositioning_can_be_turned_on_and_off() {
        let _lock = TOOLTIP_GLOBAL.lock().unwrap();
        set_tooltip_follows_pointer_enabled(true);
        assert!(tooltip_follows_pointer_enabled());
        set_tooltip_follows_pointer_enabled(false);
        assert!(!tooltip_follows_pointer_enabled());
    }

    /// A skill's tooltip is the one AQW pins to the corner; a buff's follows the cursor. Telling
    /// them apart is the whole reason one can be hidden while the other is kept.
    #[test]
    fn a_corner_pinned_tooltip_is_a_skills() {
        // 960 - 200 - 4, 480 - 100 - 4
        assert!(was_pinned_to_the_corner(756.0, 376.0, 200.0, 100.0));
        // A pixel of rounding either way is still the corner.
        assert!(was_pinned_to_the_corner(755.4, 376.6, 200.0, 100.0));
    }

    #[test]
    fn a_tooltip_near_the_pointer_is_not_a_skills() {
        assert!(!was_pinned_to_the_corner(120.0, 88.0, 200.0, 100.0));
        // Same height, wrong side of the stage.
        assert!(!was_pinned_to_the_corner(20.0, 376.0, 200.0, 100.0));
    }

    /// Turning the switch off has to bring back anything it hid. Nothing restores a tooltip except
    /// this flag, because AQW never touches `visible` itself.
    #[test]
    fn hiding_is_remembered_so_it_can_be_undone() {
        assert!(!TOOLTIP_HIDDEN_BY_US.load(Ordering::Relaxed));
        TOOLTIP_HIDDEN_BY_US.store(true, Ordering::Relaxed);
        assert!(TOOLTIP_HIDDEN_BY_US.load(Ordering::Relaxed));
        TOOLTIP_HIDDEN_BY_US.store(false, Ordering::Relaxed);
        assert!(!TOOLTIP_HIDDEN_BY_US.load(Ordering::Relaxed));
    }
}

/// The lowest stage quality the movie is allowed to drop to, as a `StageQuality` sample count.
///
/// Zero means the movie decides, which is the ordinary behaviour.
///
/// AQW manages its own quality. `World.as` samples the frame rate and, on an average below 12 fps,
/// steps `stage.quality` down through `["LOW","MEDIUM","HIGH"]`:
///
/// ```text
/// if (avgFps <  12 && idx > 0) stage.quality = arrQuality[idx - 1];
/// if (avgFps >= 12 && idx < 2) stage.quality = arrQuality[idx + 1];
/// ```
///
/// `HIGH` is 4x multisampling, `MEDIUM` is 2x and `LOW` is none at all, so a dip below twelve
/// leaves every piece of vector art -- which is all of the text -- drawn without antialiasing. It
/// climbs back one step per five samples of twenty-four frames, so a moment of slowness costs
/// hundreds of frames of soft, thin text well after the slowness has passed.
///
/// That is reasonable of AQW and unreasonable here: the setting is offered so the player can choose
/// what their card should be asked for, and a transient dip should not overrule them.
static STAGE_QUALITY_FLOOR: AtomicU8 = AtomicU8::new(0);

/// Set the floor from a `StageQuality`'s own discriminant, or clear it with `None`.
pub fn set_stage_quality_floor(floor: Option<u8>) {
    STAGE_QUALITY_FLOOR.store(floor.unwrap_or(0), Ordering::Relaxed);
}

/// The floor, if one is set.
pub fn stage_quality_floor() -> Option<u8> {
    match STAGE_QUALITY_FLOOR.load(Ordering::Relaxed) {
        0 => None,
        floor => Some(floor),
    }
}

#[cfg(test)]
mod stage_quality_floor_tests {
    use super::*;

    #[test]
    fn a_floor_of_zero_means_the_movie_decides() {
        set_stage_quality_floor(None);
        assert_eq!(stage_quality_floor(), None);
        set_stage_quality_floor(Some(4));
        assert_eq!(stage_quality_floor(), Some(4));
        set_stage_quality_floor(None);
    }
}

/// Whether a chosen font should replace the ones a movie embeds.
///
/// AQW embeds the faces it draws its text with, so `resolve_font` finds an embedded match for
/// chat, nameplates and damage numbers and never reaches device font resolution -- which is where
/// a user's choice can be applied. The game's own "Comic Sans Font" option works the same way from
/// the inside, restyling nameplates in ActionScript rather than substituting a face.
///
/// Setting this makes text layout skip the embedded lookup so the request falls through to the
/// system, at the cost of the movie's own glyph metrics: text laid out with a different face
/// occupies a different width, so lines can wrap and centre slightly differently. That is the
/// trade the setting is asking for, which is why it only applies when a font has been chosen.
static UI_FONT_OVERRIDE: AtomicBool = AtomicBool::new(false);

pub fn set_ui_font_override(active: bool) {
    // Logged for the same reason as the aura recolour: a setting that never reaches the core looks
    // identical to a feature that does not work.
    if UI_FONT_OVERRIDE.swap(active, Ordering::Relaxed) != active {
        tracing::info!(
            "AQW text font override is {}",
            if active { "on" } else { "off" }
        );
    }
}

pub fn ui_font_override() -> bool {
    UI_FONT_OVERRIDE.load(Ordering::Relaxed)
}

/// The family a chosen font should resolve to, when one is chosen.
///
/// Held separately from the flag so the common case -- no override -- costs an atomic load and
/// never touches the lock.
static UI_FONT_FAMILY: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub fn set_ui_font_family(family: Option<String>) {
    let active = family.is_some();
    if let Ok(mut slot) = UI_FONT_FAMILY.lock() {
        *slot = family;
    }
    set_ui_font_override(active);
}

/// The chosen family, if any.
pub fn ui_font_family() -> Option<String> {
    if !ui_font_override() {
        return None;
    }
    UI_FONT_FAMILY.lock().ok().and_then(|slot| slot.clone())
}

/// How wide the chosen font reaches.
///
/// The default is the narrow one: only the text a player reads during play. Overriding every field
/// -- the menus, the server list, the buttons -- was the first cut of this feature and it read as a
/// bug, so it is now the opt-in rather than the default. Which fields are "chat and nameplates" is
/// decided by [`is_aqw_scoped_text_container`]; this only decides whether that filter is consulted
/// at all.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UiFontScope {
    /// Chat lines and the names over characters' heads, and nothing else.
    #[default]
    ChatAndNameplates,
    /// Every text field the game draws.
    Everything,
}

static UI_FONT_SCOPE: AtomicU8 = AtomicU8::new(0);

pub fn set_ui_font_scope(scope: UiFontScope) {
    UI_FONT_SCOPE.store(
        match scope {
            UiFontScope::ChatAndNameplates => 0,
            UiFontScope::Everything => 1,
        },
        Ordering::Relaxed,
    );
}

pub fn ui_font_scope() -> UiFontScope {
    match UI_FONT_SCOPE.load(Ordering::Relaxed) {
        1 => UiFontScope::Everything,
        _ => UiFontScope::ChatAndNameplates,
    }
}

/// Whether the chosen font should be asked for at bold weight even where the text is not bold.
///
/// Some families are drawn far lighter than the faces AQW embeds, and against the game's art the
/// regular weight reads as thin and washed out rather than as a different font -- Courier New is
/// the one this was raised for. Asking for the bold cut restores the stroke weight the interface
/// was drawn around. It only applies where the override applies, so it can never embolden text the
/// setting is not already replacing.
static UI_FONT_BOLD: AtomicBool = AtomicBool::new(false);

pub fn set_ui_font_bold(bold: bool) {
    UI_FONT_BOLD.store(bold, Ordering::Relaxed);
}

pub fn ui_font_bold() -> bool {
    UI_FONT_BOLD.load(Ordering::Relaxed)
}
