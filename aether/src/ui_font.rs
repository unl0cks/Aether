//! The font AQW's text is drawn with.
//!
//! AQW embeds the faces it draws its text with, so the substitution happens in the core text
//! layout rather than here: [`ruffle_core::aether_compatibility::set_ui_font_family`] hands the
//! chosen family to `EditText::relayout`, which applies it per field. This module is just the menu
//! of choices and how they are written to the preferences file.
//!
//! A fixed list rather than every family the system can offer. Enumerating `fontdb` would put
//! several hundred entries in a dropdown, most of them symbol and CJK faces that render AQW's Latin
//! text as boxes, and the setting has to survive being written to a preferences file and read back
//! on a machine where that font may no longer exist. A named list keeps the file portable and the
//! menu readable; `Default` is always available because it means "do not override at all".
//!
//! Tahoma and Segoe UI are deliberately absent. Both drew AQW's server list as unreadable fragments
//! when the override reached it, for a reason not yet pinned down, so they are held back rather than
//! offered as choices that are known to break in one place.

use std::str::FromStr;

/// The font family AQW's text is drawn with.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UiFont {
    /// Resolve as normal, including AQW's own font aliases.
    #[default]
    Default,
    Arial,
    Verdana,
    TrebuchetMs,
    Georgia,
    TimesNewRoman,
    CourierNew,
    Consolas,
    ComicSansMs,
}

impl UiFont {
    /// Every choice, in the order they are offered.
    pub const ALL: [UiFont; 9] = [
        UiFont::Default,
        UiFont::Arial,
        UiFont::Verdana,
        UiFont::TrebuchetMs,
        UiFont::Georgia,
        UiFont::TimesNewRoman,
        UiFont::CourierNew,
        UiFont::Consolas,
        UiFont::ComicSansMs,
    ];

    /// The label shown in the menu.
    pub fn name(self) -> &'static str {
        match self {
            UiFont::Default => "Default",
            UiFont::Arial => "Arial",
            UiFont::Verdana => "Verdana",
            UiFont::TrebuchetMs => "Trebuchet MS",
            UiFont::Georgia => "Georgia",
            UiFont::TimesNewRoman => "Times New Roman",
            UiFont::CourierNew => "Courier New",
            UiFont::Consolas => "Consolas",
            UiFont::ComicSansMs => "Comic Sans MS",
        }
    }

    /// The family to ask `fontdb` for, or `None` to resolve as normal.
    pub fn family(self) -> Option<&'static str> {
        match self {
            UiFont::Default => None,
            other => Some(other.name()),
        }
    }

    /// Whether this family should be asked for at its bold weight throughout.
    ///
    /// Courier New's regular cut is drawn much lighter than the faces AQW embeds, so against the
    /// game's art it reads as thin and washed out rather than as a different font. Its bold cut
    /// carries the stroke weight the interface was drawn around. Nothing else on the list is light
    /// enough to need this.
    pub fn force_bold(self) -> bool {
        matches!(self, UiFont::CourierNew)
    }

    /// How this is written to the preferences file.
    ///
    /// Deliberately not the display name: the file wants something stable and case-insensitive
    /// that survives a display label being reworded later.
    pub fn key(self) -> &'static str {
        match self {
            UiFont::Default => "default",
            UiFont::Arial => "arial",
            UiFont::Verdana => "verdana",
            UiFont::TrebuchetMs => "trebuchet_ms",
            UiFont::Georgia => "georgia",
            UiFont::TimesNewRoman => "times_new_roman",
            UiFont::CourierNew => "courier_new",
            UiFont::Consolas => "consolas",
            UiFont::ComicSansMs => "comic_sans_ms",
        }
    }
}

impl std::fmt::Display for UiFont {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.key())
    }
}

impl FromStr for UiFont {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        UiFont::ALL
            .into_iter()
            .find(|font| font.key().eq_ignore_ascii_case(text))
            .ok_or(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_choice_survives_a_write_and_read_of_the_preferences_file() {
        // The settings round trip is the bug class this project has been bitten by before: a value
        // that writes one spelling and parses another silently resets to the default on restart,
        // and the user reports it as the setting "not saving".
        for font in UiFont::ALL {
            assert_eq!(
                UiFont::from_str(&font.to_string()),
                Ok(font),
                "{} did not survive the round trip",
                font.name()
            );
        }
    }

    #[test]
    fn keys_are_parsed_case_insensitively() {
        assert_eq!(UiFont::from_str("TREBUCHET_MS"), Ok(UiFont::TrebuchetMs));
        assert_eq!(UiFont::from_str("Comic_Sans_MS"), Ok(UiFont::ComicSansMs));
    }

    #[test]
    fn an_unknown_key_is_rejected_rather_than_silently_defaulted() {
        // Returning `Default` here would make a typo in the preferences file indistinguishable from
        // a deliberate choice, and the caller wants to keep whatever it already had. A retired
        // choice reads as unknown too, so an old file naming Tahoma keeps its current value rather
        // than adopting a font that is no longer offered.
        assert_eq!(UiFont::from_str("wingdings"), Err(()));
        assert_eq!(UiFont::from_str("tahoma"), Err(()));
        assert_eq!(UiFont::from_str("segoe_ui"), Err(()));
        assert_eq!(UiFont::from_str(""), Err(()));
    }

    #[test]
    fn only_the_default_declines_to_name_a_family() {
        assert_eq!(UiFont::Default.family(), None);
        for font in UiFont::ALL.into_iter().filter(|f| *f != UiFont::Default) {
            assert_eq!(
                font.family(),
                Some(font.name()),
                "a named choice must ask fontdb for that family"
            );
        }
    }

    #[test]
    fn only_the_light_family_is_asked_for_bold() {
        // Emboldening a family that is already the right weight would make the override look like a
        // bold-text bug rather than a font choice, so this stays a named exception.
        assert!(UiFont::CourierNew.force_bold());
        for font in UiFont::ALL.into_iter().filter(|f| *f != UiFont::CourierNew) {
            assert!(
                !font.force_bold(),
                "{} must keep its own weight",
                font.name()
            );
        }
    }

    #[test]
    fn keys_and_labels_are_both_unique() {
        // Two choices sharing a key would make one of them unreachable after a restart.
        let keys: std::collections::HashSet<&str> = UiFont::ALL.iter().map(|f| f.key()).collect();
        let names: std::collections::HashSet<&str> = UiFont::ALL.iter().map(|f| f.name()).collect();
        assert_eq!(keys.len(), UiFont::ALL.len());
        assert_eq!(names.len(), UiFont::ALL.len());
    }
}
