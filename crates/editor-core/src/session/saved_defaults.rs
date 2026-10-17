use super::{Actor, Session};
use crate::saved_defaults::{Binding, SavedDefaults};

impl Session {
    pub fn with_saved_defaults(
        game: Result<source_formats::gamefs::GameFs, String>,
        defaults: SavedDefaults,
    ) -> Self {
        let mut session = Self::new(game);
        session.use_application_saved_defaults(defaults);
        session
    }

    pub(crate) fn use_application_saved_defaults(&mut self, defaults: SavedDefaults) {
        self.grid_preferences = Binding::Application(defaults.grid);
        self.preview_preferences = Binding::Application(defaults.preview);
        self.navigation_preferences = Binding::Application(defaults.navigation);
        let grid = self.grid_preferences.authority().snapshot();
        if grid.loaded {
            if let Err(error) =
                self.set_spatial(Actor::System, grid.saved.unwrap_or_default().update())
            {
                self.log(
                    Actor::System,
                    format!("Grid defaults could not apply: {error}"),
                );
            }
        } else if let Some(error) = grid.error.or(grid.storage_error) {
            self.log(
                Actor::System,
                format!("Grid defaults could not load: {error}"),
            );
        }
        let preview = self.preview_preferences.authority().snapshot();
        if preview.loaded {
            if let Err(error) = self.apply_preview_value(preview.saved.unwrap_or_default()) {
                self.log(
                    Actor::System,
                    format!("Preview defaults could not apply: {error}"),
                );
            }
        } else if let Some(error) = preview.error.or(preview.storage_error) {
            self.log(
                Actor::System,
                format!("Preview defaults could not load: {error}"),
            );
        }
        let navigation = self.navigation_preferences.authority().snapshot();
        if navigation.loaded {
            if let Err(error) = self.set_fly_speed(navigation.saved.unwrap_or_default().fly_speed) {
                self.log(
                    Actor::System,
                    format!("Navigation defaults could not apply: {error}"),
                );
            }
        } else if let Some(error) = navigation.error.or(navigation.storage_error) {
            self.log(
                Actor::System,
                format!("Navigation defaults could not load: {error}"),
            );
        }
    }
}
