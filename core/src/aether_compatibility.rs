//! Narrow compatibility repairs used by Aether without enabling diagnostic instrumentation.

use crate::avm2::{
    Activation as Avm2Activation, Multiname as Avm2Multiname, TObject as _, Value as Avm2Value,
};
use crate::context::UpdateContext;
use crate::display_object::{DisplayObject, MovieClip, TDisplayObject, TDisplayObjectContainer};
use crate::string::AvmString;
use std::sync::atomic::{AtomicBool, Ordering};

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

fn is_timeline_child_rebind_target(
    movie_url: &str,
    class_local_name: &str,
    has_nonempty_class_namespace: bool,
) -> bool {
    contains_ascii_case_insensitive(movie_url, "spider.swf")
        && class_local_name == "mcOption"
        && !has_nonempty_class_namespace
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
}
