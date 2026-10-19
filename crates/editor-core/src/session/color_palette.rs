use super::*;
use crate::color::Input;
use crate::color_palette::Authority;

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
    pub(crate) fn use_application_color_palette(&mut self, authority: Authority) {
        self.color_palette = Binding::Application(authority);
    }

    pub(super) fn configure_color_palette(&mut self, directory: Result<PathBuf, String>) {
        if let Binding::Independent(authority) = &self.color_palette {
            let _ = authority.configure(directory);
        }
    }

    pub fn color_palette_state(&self) -> serde_json::Value {
        self.color_palette.authority().report()
    }

    pub fn change_color_palette(
        &mut self,
        action: &str,
        snapshot: &str,
        name: Option<&str>,
        color: Option<Input>,
    ) -> Result<serde_json::Value, String> {
        self.ensure_not_closing()?;
        self.color_palette
            .authority()
            .change(action, snapshot, name, color)
    }

    pub fn rename_color_palette(
        &mut self,
        snapshot: &str,
        name: &str,
        new_name: &str,
    ) -> Result<serde_json::Value, String> {
        self.ensure_not_closing()?;
        self.color_palette
            .authority()
            .rename(snapshot, name, new_name)
    }
}
