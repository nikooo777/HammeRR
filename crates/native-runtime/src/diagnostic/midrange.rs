use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::CaptureEnvironment;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PplMidrange909Case {
    #[serde(rename = "target-032")]
    Target032,
    #[serde(rename = "reference-032")]
    Reference032,
    #[serde(rename = "target-064")]
    Target064,
    #[serde(rename = "reference-064")]
    Reference064,
    #[serde(rename = "target-096")]
    Target096,
    #[serde(rename = "reference-096")]
    Reference096,
}

impl PplMidrange909Case {
    pub(super) fn bsp_sha(self) -> &'static str {
        match self {
            Self::Target032 => "07c753fed495d190e2c1503d5542e39dd108e5bf5a4e00bd9ceb642ff7f3de64",
            Self::Reference032 => {
                "0c874539fa958dceb88a26ad672e1d9f45381086c8ebf58472ac0cfa112483fd"
            }
            Self::Target064 => "587d5aeb6303eafbf19e9c645eb9f6f87f6c0a6207ef94b03b6d095701b203e3",
            Self::Reference064 => {
                "106a54090ed0a7a43f7a99356400f6a8de74df49cf17c541bbd43ac1078d4f35"
            }
            Self::Target096 => "8777f70084f9b66e52bb7288eb506a20dda64f0315f5508edae0b13a4a609fb7",
            Self::Reference096 => {
                "69ec5d8d64f39c00f1e416cdccfb1fd832a0dc772cd248b5904f94ee898faf7c"
            }
        }
    }
}

pub struct PplMidrange909Request {
    pub case: PplMidrange909Case,
    pub environment: CaptureEnvironment,
    pub bsp: PathBuf,
    pub fixture_game: PathBuf,
    pub observer_library: PathBuf,
    pub artifacts_parent: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct PplMidrange909Receipt {
    pub case: PplMidrange909Case,
    pub artifacts: PathBuf,
    pub elapsed_ms: u64,
    pub verified_controls: bool,
    pub hardware_observed: bool,
    pub preservation_passed: bool,
}
