#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LightingChannel {
    #[default]
    Ldr,
    Hdr,
}

impl LightingChannel {
    pub fn name(self) -> &'static str {
        match self {
            Self::Ldr => "ldr",
            Self::Hdr => "hdr",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "ldr" => Ok(Self::Ldr),
            "hdr" => Ok(Self::Hdr),
            _ => Err("Lighting channel must be ldr or hdr".into()),
        }
    }
}
