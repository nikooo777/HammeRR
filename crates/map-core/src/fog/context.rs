use super::Profile;

pub const CSS_OFFICIAL_MAP_CATALOG: [&str; 18] = [
    "cs_assault",
    "cs_compound",
    "cs_havana",
    "cs_italy",
    "cs_militia",
    "cs_office",
    "de_aztec",
    "de_cbble",
    "de_chateau",
    "de_dust",
    "de_dust2",
    "de_inferno",
    "de_nuke",
    "de_piranesi",
    "de_port",
    "de_prodigy",
    "de_tides",
    "de_train",
];

pub const INITIAL_RADIAL_CONTEXTS: [InitialRadialContext; 2] = [
    InitialRadialContext::Authored,
    InitialRadialContext::CssOfficialMap,
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InitialRadialContext {
    #[default]
    Authored,
    CssOfficialMap,
}

impl InitialRadialContext {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "authored" => Ok(Self::Authored),
            "css_official_map" => Ok(Self::CssOfficialMap),
            _ => Err("Initial radial context must be authored or css_official_map".into()),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Authored => "authored",
            Self::CssOfficialMap => "css_official_map",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Authored => "Authored values",
            Self::CssOfficialMap => "CS:S official-map rule",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            Self::Authored => {
                "Preserve each controller's authored initial radial value. Later SetRadial inputs can change it."
            }
            Self::CssOfficialMap => {
                "Start every controller with radial fog enabled, as in CS:S official-map initialization. Later SetRadial inputs can change it. No map name is inferred."
            }
        }
    }

    pub fn initial_profile(self, mut profile: Profile) -> Profile {
        if self == Self::CssOfficialMap {
            profile.radial = true;
        }
        profile
    }
}
