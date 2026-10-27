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
            Self::Target032 => "adf714015cbc1c0bfc4c001474f60d73045a412b5f64221908f16fd11953a379",
            Self::Reference032 => {
                "76ea7591eddd1262cf0c8f409078430c49ab01219695084bbea0554839e6c8c0"
            }
            Self::Target064 => "e829bc5d419cea49f035bba9079c50f33b112c25155ce640ab745fe0c0ce171e",
            Self::Reference064 => {
                "7f0b49548e11bc529d7fdbcdf62a84496c3389693fd4b73df5b3789d099265e5"
            }
            Self::Target096 => "985e2766595ebdc477442d541e31971d0d0b4caa543e589373a81b03ad2b56a2",
            Self::Reference096 => {
                "7d90a2bfb370e166a68ebcca937d4b1791486874792ab41524f7596bab655d19"
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
