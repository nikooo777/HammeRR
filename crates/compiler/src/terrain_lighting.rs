#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerrainLightingNormals {
    #[default]
    Authored,
    NativeClient,
}

impl TerrainLightingNormals {
    pub fn name(self) -> &'static str {
        match self {
            Self::Authored => "authored",
            Self::NativeClient => "native_client",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Authored => "Authored smoothing",
            Self::NativeClient => "Native client normals",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "authored" => Ok(Self::Authored),
            "native_client" => Ok(Self::NativeClient),
            _ => Err("Terrain lighting normals must be authored or native_client".into()),
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Authored => {
                "Use terrain Automatic, Hard borders and Angle limit smoothing. This is the default."
            }
            Self::NativeClient => {
                "Use native client normals for unbumped terrain. Ignore authored border settings. Requires Direct LDR or Direct LDR + HDR. Editor shading stays authored."
            }
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct TerrainNormalInputs {
    pub mode: TerrainLightingNormals,
    pub status: &'static str,
    pub sampling: &'static str,
    pub displacement_count: usize,
    pub sample_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native: Option<NativeTerrainNormalInputs>,
}

#[derive(Debug, serde::Serialize)]
pub struct NativeTerrainNormalInputs {
    pub implementation: &'static str,
    pub engine_sha256: &'static str,
    pub cpu_vendor: String,
    pub cpu_brand: String,
    pub cpu_signature: u32,
    pub rsqrt_probe_bits: [u32; 8],
    pub position_rounding: &'static str,
    pub basis_rounding: &'static str,
    pub flush_to_zero: bool,
    pub denormals_are_zero: bool,
    pub caller_mxcsr: u32,
    pub selected_mxcsr: u32,
    pub local_digest: u32,
    pub t_junction_digest: u32,
    pub corner_digest: u32,
    pub final_digest: u32,
    pub normal_sha256: String,
    pub work_units: usize,
    pub kernel_work_limit_units: usize,
    pub scratch_reserved_bytes: usize,
    pub caller_scratch_reserved_bytes: usize,
    pub retained_bytes: usize,
}
