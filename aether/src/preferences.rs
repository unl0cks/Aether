mod read;
mod write;

pub mod storage;

use crate::aether_settings::{
    AetherExplicitFlags, AetherSettings, OptionsHotkey, ResolvedAetherSettings,
};
use crate::cli::{GameModePreference, OpenUrlMode, Opt};
use crate::gui::ThemePreference;
use crate::log::FilenamePattern;
use crate::preferences::read::read_preferences;
use crate::preferences::write::PreferencesWriter;
use anyhow::{Context, Error};
use ruffle_core::backend::ui::US_ENGLISH;
use ruffle_frontend_utils::bookmarks::{Bookmarks, BookmarksWriter, read_bookmarks};
use ruffle_frontend_utils::parse::DocumentHolder;
use ruffle_frontend_utils::recents::{Recents, RecentsWriter, read_recents};
use ruffle_render_wgpu::clap::{GraphicsBackend, PowerPreference};
use std::sync::{Arc, Mutex};
use sys_locale::get_locale;
use tokio::sync::broadcast;
use tokio::sync::broadcast::{Receiver, Sender};
use unic_langid::LanguageIdentifier;

/// The preferences that relate to the application itself.
///
/// This structure is safe to clone, internally it holds an Arc to any mutable properties.
///
/// The general priority order for preferences should look as follows, where top is "highest priority":
/// - User-selected movie-specific setting (if applicable, such as through Open Advanced)
/// - Movie-specific settings (if applicable and we implement this, stored on disk)
/// - CLI (if applicable)
/// - Persisted preferences (if applicable, saved to toml)
/// - Ruffle defaults
#[derive(Clone)]
pub struct GlobalPreferences {
    /// As the CLI holds properties ranging from initial movie settings (ie url),
    /// to application itself (ie render backend),
    /// this field is available for checking where needed.
    // TODO: This should really not be public and we should split up CLI somehow,
    // or make it all getters in here?
    pub cli: Opt,

    /// The actual, mutable user preferences that are persisted to disk.
    preferences: Arc<Mutex<DocumentHolder<SavedGlobalPreferences>>>,

    bookmarks: Arc<Mutex<DocumentHolder<Bookmarks>>>,

    recents: Arc<Mutex<DocumentHolder<Recents>>>,

    watchers: GlobalPreferencesWatchers,

    /// Which Aether settings the command line pinned, captured before the preset ran. See
    /// [`crate::aether_settings`] for why this cannot be read back off `cli`.
    aether_explicit: AetherExplicitFlags,

    /// Whether the AQW preset applied. The options window needs it because a saved AQW setting
    /// must not take effect when Aether was pointed at an unrelated movie.
    aqw_mode: bool,

    /// The settings this process actually started with. Not an `Arc`, deliberately: it must not
    /// change when the options window saves.
    aether_at_startup: AetherSettings,
}

impl GlobalPreferences {
    pub fn load(
        cli: Opt,
        aether_explicit: AetherExplicitFlags,
        aqw_mode: bool,
    ) -> Result<Self, Error> {
        std::fs::create_dir_all(&cli.config).context("Failed to create configuration directory")?;
        let preferences_path = cli.config.join("preferences.toml");
        let preferences = if preferences_path.exists() {
            let contents = std::fs::read_to_string(&preferences_path)
                .context("Failed to read saved preferences")?;
            let result = read_preferences(&contents);
            for warning in result.warnings {
                // TODO: A way to display warnings to users, generally
                tracing::warn!("{warning}");
            }
            result.result
        } else {
            Default::default()
        };

        let bookmarks_path = cli.config.join("bookmarks.toml");
        let bookmarks = if bookmarks_path.exists() {
            let contents = std::fs::read_to_string(&bookmarks_path)
                .context("Failed to read saved bookmarks")?;
            let result = read_bookmarks(&contents);
            for warning in result.warnings {
                tracing::warn!("{warning}");
            }
            result.result
        } else {
            Default::default()
        };

        let recents_path = cli.config.join("recents.toml");
        let recents = if recents_path.exists() {
            let contents =
                std::fs::read_to_string(&recents_path).context("Failed to read saved recents")?;
            let result = read_recents(&contents);
            for warning in result.warnings {
                tracing::warn!("{warning}");
            }
            result.result
        } else {
            Default::default()
        };

        let preferences_snapshot = preferences.aether;

        Ok(Self {
            cli,
            preferences: Arc::new(Mutex::new(preferences)),
            bookmarks: Arc::new(Mutex::new(bookmarks)),
            recents: Arc::new(Mutex::new(recents)),
            watchers: Default::default(),
            aether_explicit,
            aqw_mode,
            aether_at_startup: preferences_snapshot,
        })
    }

    /// Every Aether setting as it applies to this session, with where each answer came from.
    pub fn aether(&self) -> ResolvedAetherSettings {
        let saved = self
            .preferences
            .lock()
            .expect("Preferences is not reentrant")
            .aether;

        ResolvedAetherSettings::resolve(self.aether_explicit, saved)
    }

    /// The settings as they were when the process started.
    ///
    /// This is what the renderer and the player were actually built with, so it is what the
    /// options window has to compare against to decide whether a change needs a restart. Reading
    /// the saved file instead would forget as soon as Save wrote to it: change the sample count,
    /// see the warning, reopen the window, and the warning would be gone while the renderer was
    /// still running on the old value.
    pub fn aether_at_startup(&self) -> AetherSettings {
        self.aether_at_startup
    }

    /// The saved settings on their own, which is what the options window edits.
    pub fn aether_saved(&self) -> AetherSettings {
        self.preferences
            .lock()
            .expect("Preferences is not reentrant")
            .aether
    }

    pub fn aqw_mode(&self) -> bool {
        self.aqw_mode
    }

    pub fn aether_options_hotkey(&self) -> OptionsHotkey {
        self.preferences
            .lock()
            .expect("Preferences is not reentrant")
            .aether_options_hotkey
    }

    pub fn graphics_backends(&self) -> GraphicsBackend {
        self.cli.graphics.unwrap_or_else(|| {
            self.preferences
                .lock()
                .expect("Preferences is not reentrant")
                .graphics_backend
        })
    }

    pub fn graphics_power_preference(&self) -> PowerPreference {
        self.cli.power.unwrap_or_else(|| {
            self.preferences
                .lock()
                .expect("Preferences is not reentrant")
                .graphics_power_preference
        })
    }

    pub fn gamemode_preference(&self) -> GameModePreference {
        self.cli.gamemode.unwrap_or_else(|| {
            self.preferences
                .lock()
                .expect("Non-poisoned preferences")
                .gamemode_preference
        })
    }

    pub fn language(&self) -> LanguageIdentifier {
        self.preferences
            .lock()
            .expect("Preferences is not reentrant")
            .language
            .clone()
    }

    pub fn output_device_name(&self) -> Option<String> {
        self.preferences
            .lock()
            .expect("Preferences is not reentrant")
            .output_device
            .clone()
    }

    pub fn mute(&self) -> bool {
        self.preferences
            .lock()
            .expect("Preferences is not reentrant")
            .mute
    }

    pub fn preferred_volume(&self) -> f32 {
        self.cli.volume.unwrap_or_else(|| {
            self.preferences
                .lock()
                .expect("Preferences is not reentrant")
                .volume
        })
    }

    pub fn openh264_enabled(&self) -> bool {
        self.preferences
            .lock()
            .expect("Preferences is not reentrant")
            .enable_openh264
    }

    pub fn log_filename_pattern(&self) -> FilenamePattern {
        self.preferences
            .lock()
            .expect("Preferences is not reentrant")
            .log
            .filename_pattern
    }

    pub fn bookmarks(&self, fun: impl FnOnce(&Bookmarks)) {
        fun(&self.bookmarks.lock().expect("Bookmarks is not reentrant"))
    }

    pub fn have_bookmarks(&self) -> bool {
        let bookmarks = &self.bookmarks.lock().expect("Bookmarks is not reentrant");

        !bookmarks.is_empty() && !bookmarks.iter().all(|x| x.is_invalid())
    }

    pub fn storage_backend(&self) -> storage::StorageBackend {
        self.cli.storage.unwrap_or_else(|| {
            self.preferences
                .lock()
                .expect("Preferences is not reentrant")
                .storage
                .backend
        })
    }

    pub fn recent_limit(&self) -> usize {
        self.preferences
            .lock()
            .expect("Preferences is not reentrant")
            .recent_limit
    }

    pub fn theme_preference(&self) -> ThemePreference {
        self.preferences
            .lock()
            .expect("Non-poisoned preferences")
            .theme_preference
    }

    pub fn theme_preference_watcher(&self) -> Receiver<ThemePreference> {
        self.watchers.theme_preference_watcher.subscribe()
    }

    pub fn open_url_mode(&self) -> OpenUrlMode {
        self.cli.open_url_mode.unwrap_or_else(|| {
            self.preferences
                .lock()
                .expect("Non-poisoned preferences")
                .open_url_mode
        })
    }

    pub fn ime_enabled(&self) -> Option<bool> {
        self.preferences
            .lock()
            .expect("Non-poisoned preferences")
            .ime_enabled
    }

    pub fn recents<R>(&self, fun: impl FnOnce(&Recents) -> R) -> R {
        fun(&self.recents.lock().expect("Recents is not reentrant"))
    }

    pub fn write_preferences(&self, fun: impl FnOnce(&mut PreferencesWriter)) -> Result<(), Error> {
        let mut preferences = self
            .preferences
            .lock()
            .expect("Preferences is not reentrant");

        let mut writer = PreferencesWriter::new(&mut preferences);
        writer.set_watchers(&self.watchers);
        fun(&mut writer);

        let serialized = preferences.serialize();
        std::fs::write(self.cli.config.join("preferences.toml"), serialized)
            .context("Could not write preferences to disk")
    }

    pub fn write_bookmarks(&self, fun: impl FnOnce(&mut BookmarksWriter)) -> Result<(), Error> {
        let mut bookmarks = self.bookmarks.lock().expect("Bookmarks is not reentrant");

        let mut writer = BookmarksWriter::new(&mut bookmarks);
        fun(&mut writer);

        let serialized = bookmarks.serialize();
        std::fs::write(self.cli.config.join("bookmarks.toml"), serialized)
            .context("Could not write bookmarks to disk")
    }

    pub fn write_recents(&self, fun: impl FnOnce(&mut RecentsWriter)) -> Result<(), Error> {
        let mut recents = self.recents.lock().expect("Recents is not reentrant");

        let mut writer = RecentsWriter::new(&mut recents);
        fun(&mut writer);

        let serialized = recents.serialize();
        std::fs::write(self.cli.config.join("recents.toml"), serialized)
            .context("Could not write recents to disk")
    }
}

#[derive(PartialEq, Debug)]
pub struct SavedGlobalPreferences {
    pub graphics_backend: GraphicsBackend,
    pub graphics_power_preference: PowerPreference,
    pub gamemode_preference: GameModePreference,
    pub language: LanguageIdentifier,
    pub output_device: Option<String>,
    pub mute: bool,
    pub volume: f32,
    pub enable_openh264: bool,
    pub recent_limit: usize,
    pub log: LogPreferences,
    pub storage: StoragePreferences,
    pub theme_preference: ThemePreference,
    pub open_url_mode: OpenUrlMode,
    pub ime_enabled: Option<bool>,
    pub aether: AetherSettings,
    pub aether_options_hotkey: OptionsHotkey,
}

impl Default for SavedGlobalPreferences {
    fn default() -> Self {
        let preferred_locale = get_locale();
        let locale = preferred_locale
            .and_then(|l| l.parse().ok())
            .unwrap_or_else(|| US_ENGLISH.clone());

        Self {
            graphics_backend: Default::default(),
            graphics_power_preference: Default::default(),
            gamemode_preference: Default::default(),
            language: locale,
            output_device: None,
            mute: false,
            volume: 1.0,
            enable_openh264: true,
            recent_limit: 10,
            log: Default::default(),
            storage: Default::default(),
            theme_preference: Default::default(),
            open_url_mode: Default::default(),
            ime_enabled: None,
            aether: Default::default(),
            aether_options_hotkey: Default::default(),
        }
    }
}

#[derive(PartialEq, Debug, Default)]
pub struct LogPreferences {
    pub filename_pattern: FilenamePattern,
}

#[derive(PartialEq, Debug, Default)]
pub struct StoragePreferences {
    pub backend: storage::StorageBackend,
}

#[derive(Clone)]
pub struct GlobalPreferencesWatchers {
    theme_preference_watcher: Arc<Sender<ThemePreference>>,
}

impl Default for GlobalPreferencesWatchers {
    fn default() -> Self {
        Self {
            theme_preference_watcher: Arc::new(broadcast::channel(1).0),
        }
    }
}
