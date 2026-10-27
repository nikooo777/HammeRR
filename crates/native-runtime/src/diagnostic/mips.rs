use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::CaptureEnvironment;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PplMips931Case {
    #[serde(rename = "tail-032")]
    Tail032,
    #[serde(rename = "tail-096")]
    Tail096,
    #[serde(rename = "reference-032")]
    Reference032,
    #[serde(rename = "reference-096")]
    Reference096,
    #[serde(rename = "filter-a")]
    FilterA,
    #[serde(rename = "filter-b")]
    FilterB,
    #[serde(rename = "filter-reference-064")]
    FilterReference064,
    #[serde(rename = "filter-reference-096")]
    FilterReference096,
}

impl PplMips931Case {
    pub(super) fn bsp_sha(self) -> &'static str {
        match self {
            Self::Tail032 => "a5ebd9d7d915faa8d56d0aa5b1a366821b53151d731e630960a265e9d0cd3ebc",
            Self::Tail096 => "191d6e17e900fd56ab6713f73c296b8131405957394b3456231f4137ac7f13aa",
            Self::Reference032 => {
                "76ea7591eddd1262cf0c8f409078430c49ab01219695084bbea0554839e6c8c0"
            }
            Self::Reference096 => {
                "7d90a2bfb370e166a68ebcca937d4b1791486874792ab41524f7596bab655d19"
            }
            Self::FilterA => "ac9831337a9811c7008397655c79280b29e2ab34f7de141b7d8069a2f146dc4a",
            Self::FilterB => "a43c084d307163d583894645749db3aeebf9dbedeae438485f0e8c8a0155f029",
            Self::FilterReference064 => {
                "7f0b49548e11bc529d7fdbcdf62a84496c3389693fd4b73df5b3789d099265e5"
            }
            Self::FilterReference096 => {
                "7d90a2bfb370e166a68ebcca937d4b1791486874792ab41524f7596bab655d19"
            }
        }
    }
}

pub struct PplMips931Request {
    pub case: PplMips931Case,
    pub environment: CaptureEnvironment,
    pub bsp: PathBuf,
    pub fixture_game: PathBuf,
    pub observer_library: PathBuf,
    pub artifacts_parent: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct PplMips931Receipt {
    pub case: PplMips931Case,
    pub artifacts: PathBuf,
    pub elapsed_ms: u64,
    pub verified_controls: bool,
    pub hardware_observed: bool,
    pub preservation_passed: bool,
}
