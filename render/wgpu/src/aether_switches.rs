//! Renderer switches that can be changed while Aether is running.
//!
//! These began as `OnceLock` reads of an environment variable, which is fine for a developer
//! measuring something from a shell and useless for anyone else. Turning one on meant knowing the
//! variable existed, closing the client, and starting it again from a command line.
//!
//! Two things wanted more than that. The options window needs to offer them, and comparing a
//! switch on against off is far more trustworthy when both halves come from *one* session: scene
//! composition dominates every measurement here, and two sessions in different rooms are barely
//! comparable at all. A switch that can be flipped in place removes that whole class of error.
//!
//! **The environment variable still wins.** If it is set, it pins the switch and the options window
//! cannot move it. That ordering is deliberate: someone who passed a variable explicitly is
//! measuring something, and a saved preference quietly overriding them would waste a run.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// No A/B is driving this switch.
const AB_NONE: u8 = 0;
const AB_OFF: u8 = 1;
const AB_ON: u8 = 2;

/// A switch with a default, an environment override, and a value the UI may set.
#[derive(Debug)]
pub struct RuntimeSwitch {
    env: &'static str,
    value: AtomicBool,
    /// Set only by the alternating A/B harness, and checked ahead of everything else.
    ///
    /// This deliberately outranks the environment override. Asking for an A/B *on this switch* is
    /// a more specific instruction than having its variable set, and the two arrive by the same
    /// route: a shell. `$env:` persists for the life of a PowerShell window, so a variable left
    /// over from an earlier run silently cancelled a whole 30 minute A/B once. Precedence alone
    /// would not have been enough -- see the warning [`ab_state`] leaves behind.
    ab: AtomicU8,
    /// Resolved once. `None` means the variable was absent or unrecognised, so the switch is the
    /// UI's to decide.
    env_override: OnceLock<Option<bool>>,
}

impl RuntimeSwitch {
    pub const fn new(env: &'static str, default_on: bool) -> Self {
        Self {
            env,
            value: AtomicBool::new(default_on),
            ab: AtomicU8::new(AB_NONE),
            env_override: OnceLock::new(),
        }
    }

    fn env_override(&self) -> Option<bool> {
        *self.env_override.get_or_init(|| {
            match std::env::var(self.env).as_deref() {
                Ok("0") | Ok("off") | Ok("false") | Ok("no") => Some(false),
                Ok("1") | Ok("on") | Ok("true") | Ok("yes") => Some(true),
                // An unrecognised value is not an instruction. Guessing at it would be worse than
                // leaving the switch where the player put it.
                _ => None,
            }
        })
    }

    pub fn enabled(&self) -> bool {
        match self.ab.load(Ordering::Relaxed) {
            AB_ON => return true,
            AB_OFF => return false,
            _ => {}
        }
        self.env_override()
            .unwrap_or_else(|| self.value.load(Ordering::Relaxed))
    }

    /// Put this switch under an alternating A/B, overriding both the saved value and the
    /// environment. Returns whether the environment was being overridden, so the caller can say so.
    pub fn set_ab(&self, on: bool) -> bool {
        self.ab
            .store(if on { AB_ON } else { AB_OFF }, Ordering::Relaxed);
        self.pinned_by_env()
    }

    /// Move the switch, unless the environment pinned it.
    pub fn set(&self, on: bool) {
        self.value.store(on, Ordering::Relaxed);
    }

    /// Whether a command-line variable is holding this switch, so the UI can say so rather than
    /// offering a control that appears to do nothing.
    pub fn pinned_by_env(&self) -> bool {
        self.env_override().is_some()
    }

    pub fn env_name(&self) -> &'static str {
        self.env
    }
}

/// Group several filtered cache surfaces into one atlas and blur them together.
///
/// Measured: cost per filtered cache entry 0.085 ms to 0.025 ms, a 71% cut.
pub static FILTER_ATLAS: RuntimeSwitch = RuntimeSwitch::new("AETHER_FILTER_ATLAS", true);

/// Recycle `cacheAsBitmap` textures instead of asking the driver for a new one each time.
///
/// Measured: 74% reuse, and texture creations per frame essentially eliminated in the band where
/// two sessions could be compared.
pub static CACHE_TEXTURE_POOL: RuntimeSwitch =
    RuntimeSwitch::new("AETHER_CACHE_TEXTURE_POOL", true);

/// Let runs of compatible complex blends share one render pass.
pub static BLEND_BATCHING: RuntimeSwitch = RuntimeSwitch::new("AETHER_BLEND_BATCH", true);

/// Move compatible blends next to each other so the batcher can merge them.
///
/// Off, on measurement: it relocated 11 blends a frame out of 296, about 1.6% of render passes.
/// Kept because the geometry of another scene may be kinder than a packed hub.
pub static BLEND_REORDERING: RuntimeSwitch = RuntimeSwitch::new("AETHER_BLEND_REORDER", false);

/// Report every complex blend as it is encoded. A measurement aid, and noisy.
pub static BLENDCHECK: RuntimeSwitch = RuntimeSwitch::new("AETHER_BLENDCHECK", false);

/// Every switch this module owns, so one can be looked up by the variable name that names it.
pub const ALL: &[&RuntimeSwitch] = &[
    &FILTER_ATLAS,
    &CACHE_TEXTURE_POOL,
    &BLEND_BATCHING,
    &BLEND_REORDERING,
    &BLENDCHECK,
];

/// Find a switch by its environment variable name.
///
/// Used by the alternating A/B harness, which is told which switch to drive by name so that adding
/// a switch does not mean teaching the harness about it.
pub fn by_env_name(name: &str) -> Option<&'static RuntimeSwitch> {
    ALL.iter()
        .copied()
        .find(|switch| switch.env_name().eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_switch_starts_at_its_default_and_can_be_moved() {
        // A name no test process will have set, so this exercises the unpinned path.
        static SWITCH: RuntimeSwitch = RuntimeSwitch::new("AETHER_TEST_SWITCH_UNSET_XYZZY", true);
        assert!(SWITCH.enabled(), "starts at its default");
        assert!(!SWITCH.pinned_by_env());

        SWITCH.set(false);
        assert!(!SWITCH.enabled(), "the UI can move an unpinned switch");
        SWITCH.set(true);
        assert!(SWITCH.enabled());
    }

    /// The whole point of the override: a variable passed on the command line is someone measuring
    /// something, and a saved preference must not be able to move it under them.
    #[test]
    fn the_environment_pins_a_switch_against_the_ui() {
        // SAFETY: single-threaded test, and the name is unique to it.
        unsafe { std::env::set_var("AETHER_TEST_SWITCH_PINNED_XYZZY", "0") };
        static SWITCH: RuntimeSwitch = RuntimeSwitch::new("AETHER_TEST_SWITCH_PINNED_XYZZY", true);

        assert!(SWITCH.pinned_by_env());
        assert!(!SWITCH.enabled(), "the variable wins over the default");
        SWITCH.set(true);
        assert!(!SWITCH.enabled(), "and over the UI");
        unsafe { std::env::remove_var("AETHER_TEST_SWITCH_PINNED_XYZZY") };
    }

    /// The case that cost a real 30 minute run: a leftover variable in the shell pinned the very
    /// switch an A/B had been asked to drive, so nothing alternated.
    #[test]
    fn an_ab_drives_a_switch_even_when_the_environment_pinned_it() {
        // SAFETY: single-threaded test, and the name is unique to it.
        unsafe { std::env::set_var("AETHER_TEST_SWITCH_AB_XYZZY", "1") };
        static SWITCH: RuntimeSwitch = RuntimeSwitch::new("AETHER_TEST_SWITCH_AB_XYZZY", false);

        assert!(SWITCH.enabled(), "pinned on by the variable");
        assert!(
            SWITCH.set_ab(false),
            "reports that it is overriding the environment"
        );
        assert!(!SWITCH.enabled(), "the A/B wins");
        SWITCH.set_ab(true);
        assert!(SWITCH.enabled());
        unsafe { std::env::remove_var("AETHER_TEST_SWITCH_AB_XYZZY") };
    }

    #[test]
    fn every_switch_is_findable_by_its_own_variable_name() {
        // The A/B harness is told which switch to drive by name, so a switch missing from `ALL`
        // would silently measure nothing rather than fail.
        for switch in ALL {
            assert!(
                std::ptr::eq(
                    by_env_name(switch.env_name()).expect("a switch must find itself"),
                    *switch
                ),
                "{} is not reachable by name",
                switch.env_name()
            );
        }
        assert!(by_env_name("AETHER_NOT_A_SWITCH").is_none());
        assert!(
            by_env_name("aether_filter_atlas").is_some(),
            "matched without regard to case, since it is typed by hand"
        );
    }

    #[test]
    fn an_unrecognised_value_is_not_treated_as_an_instruction() {
        // SAFETY: single-threaded test, and the name is unique to it.
        unsafe { std::env::set_var("AETHER_TEST_SWITCH_JUNK_XYZZY", "maybe") };
        static SWITCH: RuntimeSwitch = RuntimeSwitch::new("AETHER_TEST_SWITCH_JUNK_XYZZY", false);

        assert!(!SWITCH.pinned_by_env(), "junk does not pin");
        SWITCH.set(true);
        assert!(SWITCH.enabled(), "so the UI still decides");
        unsafe { std::env::remove_var("AETHER_TEST_SWITCH_JUNK_XYZZY") };
    }
}
