use serde::{Deserialize, Serialize};

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum NameStyle {
    #[default]
    SourceSuffix,
    IndexedPrefix,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum InternalReferences {
    #[default]
    Remap,
    KeepOriginal,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ExternalReferences {
    #[default]
    Keep,
    Reject,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Naming {
    pub style: NameStyle,
    pub prefix: String,
    pub suffix: String,
    pub start_index: u32,
}

impl Default for Naming {
    fn default() -> Self {
        Self {
            style: NameStyle::SourceSuffix,
            prefix: String::new(),
            suffix: "_copy".into(),
            start_index: 1,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Policies {
    pub naming: Naming,
    pub internal: InternalReferences,
    pub external: ExternalReferences,
}

impl Policies {
    pub(super) fn core(&self) -> map_core::repeat::Policies {
        use map_core::repeat as core;
        core::Policies {
            naming: core::Naming {
                style: match self.naming.style {
                    NameStyle::SourceSuffix => core::NameStyle::SourceSuffix,
                    NameStyle::IndexedPrefix => core::NameStyle::IndexedPrefix,
                },
                prefix: self.naming.prefix.clone(),
                suffix: self.naming.suffix.clone(),
                start_index: self.naming.start_index,
            },
            internal: match self.internal {
                InternalReferences::Remap => core::InternalReferences::Remap,
                InternalReferences::KeepOriginal => core::InternalReferences::KeepOriginal,
            },
            external: match self.external {
                ExternalReferences::Keep => core::ExternalReferences::Keep,
                ExternalReferences::Reject => core::ExternalReferences::Reject,
            },
        }
    }
}
