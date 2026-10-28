use std::path::PathBuf;

use super::{Actor, Session};
use crate::material_favorites::{Authority, Snapshot, Status};

pub(super) enum Binding {
    Independent(Authority),
    Application(Authority),
}

impl Default for Binding {
    fn default() -> Self {
        Self::Independent(Authority::default())
    }
}

impl Binding {
    fn authority(&self) -> &Authority {
        match self {
            Self::Independent(authority) | Self::Application(authority) => authority,
        }
    }
}

impl Session {
    pub fn with_material_favorites(
        game: Result<source_formats::gamefs::GameFs, String>,
        authority: Authority,
    ) -> Self {
        let mut session = Self::new(game);
        session.use_application_material_favorites(authority);
        session
    }

    pub(crate) fn use_application_material_favorites(&mut self, authority: Authority) {
        self.material_favorites = Binding::Application(authority);
        if let Some(error) = self.material_favorite_status().error {
            self.log(Actor::System, error);
        }
    }

    pub(super) fn configure_material_favorites(&mut self, directory: Result<PathBuf, String>) {
        if let Binding::Independent(authority) = &self.material_favorites
            && let Err(error) = authority.configure(directory)
        {
            self.log(Actor::System, error);
        }
    }

    pub fn material_favorites_snapshot(&self) -> Snapshot {
        self.material_favorites.authority().snapshot()
    }

    pub fn material_favorites_serial(&self) -> u64 {
        self.material_favorites.authority().serial()
    }

    pub fn reload_material_favorites(&mut self) -> Result<Status, String> {
        let result = self.material_favorites.authority().reload();
        if let Err(error) = &result
            && error != crate::material_favorites::RELOAD_UNAVAILABLE
        {
            self.log(Actor::System, error);
        }
        result
    }

    pub fn material_favorite_status(&self) -> Status {
        self.material_favorites.authority().status()
    }

    pub fn favorite_material(&mut self, material: &str, favorite: bool) -> Result<(), String> {
        let result = self.material_favorites.authority().set(material, favorite);
        if let Err(error) = &result {
            self.log(Actor::System, error);
        }
        result
    }

    pub fn is_material_favorite(&self, material: &str) -> bool {
        self.material_favorites.authority().contains(material)
    }
}
