use serde::Serialize;

pub const MAX_LIGHTING_CONTRIBUTORS: usize = 64;

#[derive(Clone, Copy)]
pub(super) enum Contributor {
    Unattributed,
    Retained(usize),
    Other,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LightingChannel {
    #[default]
    Preparation,
    Ldr,
    Hdr,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LightingPhase {
    #[default]
    Preparation,
    Direct,
    PointSpot,
    Sun,
    SkyAmbient,
    Bounce,
    PropShadows,
    PropVertices,
    AmbientProbes,
    DetailProps,
    Finalization,
}

const PHASES: [LightingPhase; 11] = [
    LightingPhase::Preparation,
    LightingPhase::Direct,
    LightingPhase::PointSpot,
    LightingPhase::Sun,
    LightingPhase::SkyAmbient,
    LightingPhase::Bounce,
    LightingPhase::PropShadows,
    LightingPhase::PropVertices,
    LightingPhase::AmbientProbes,
    LightingPhase::DetailProps,
    LightingPhase::Finalization,
];

#[derive(Debug, Clone, Serialize)]
pub struct LightingPhaseProgress {
    pub channel: LightingChannel,
    pub phase: LightingPhase,
    pub work: u64,
    pub samples_planned: u64,
    pub samples_completed: u64,
    pub samples_planned_complete: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct LightingContributor {
    pub entity_id: u32,
    pub compiled_entity_id: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<LightingInstanceSource>,
    pub classname: &'static str,
    pub channel: LightingChannel,
    pub phase: LightingPhase,
    pub work: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LightingInstanceSource {
    pub file: String,
    pub original_entity_id: u32,
    pub file_truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct LightingProgress {
    pub limit: u64,
    pub sky_quality: &'static str,
    /// Charged requests include the request that exceeded the limit, before its work ran.
    pub work: u64,
    pub preparation_work: u64,
    pub ldr_work: u64,
    pub hdr_work: u64,
    pub current_channel: LightingChannel,
    pub current_phase: LightingPhase,
    pub completed: bool,
    pub lightmap_samples_planned: u64,
    pub lightmap_samples_completed: u64,
    pub lightmap_samples_planned_complete: bool,
    pub ldr_lightmap_samples_planned: u64,
    pub ldr_lightmap_samples_completed: u64,
    pub hdr_lightmap_samples_planned: u64,
    pub hdr_lightmap_samples_completed: u64,
    pub phases: Vec<LightingPhaseProgress>,
    pub contributors: Vec<LightingContributor>,
    pub unattributed_work: u64,
    pub other_contributors_work: u64,
    pub contributors_truncated: bool,
}

impl LightingProgress {
    pub(super) fn new(limit: u64, quality: crate::SkyQuality) -> Self {
        let phases = [
            LightingChannel::Preparation,
            LightingChannel::Ldr,
            LightingChannel::Hdr,
        ]
        .into_iter()
        .flat_map(|channel| {
            PHASES.map(|phase| LightingPhaseProgress {
                channel,
                phase,
                work: 0,
                samples_planned: 0,
                samples_completed: 0,
                samples_planned_complete: false,
            })
        })
        .collect();
        Self {
            limit,
            sky_quality: quality.name(),
            work: 0,
            preparation_work: 0,
            ldr_work: 0,
            hdr_work: 0,
            current_channel: LightingChannel::Preparation,
            current_phase: LightingPhase::Preparation,
            completed: false,
            lightmap_samples_planned: 0,
            lightmap_samples_completed: 0,
            lightmap_samples_planned_complete: false,
            ldr_lightmap_samples_planned: 0,
            ldr_lightmap_samples_completed: 0,
            hdr_lightmap_samples_planned: 0,
            hdr_lightmap_samples_completed: 0,
            phases,
            contributors: Vec::with_capacity(MAX_LIGHTING_CONTRIBUTORS),
            unattributed_work: 0,
            other_contributors_work: 0,
            contributors_truncated: false,
        }
    }

    pub(super) fn phase_index(&self) -> usize {
        self.current_channel as usize * PHASES.len() + self.current_phase as usize
    }

    pub(super) fn record(&mut self, amount: u64, contributor: Contributor) -> Result<(), String> {
        self.work = self
            .work
            .checked_add(amount)
            .ok_or("Lighting work count overflow")?;
        let channel = match self.current_channel {
            LightingChannel::Preparation => &mut self.preparation_work,
            LightingChannel::Ldr => &mut self.ldr_work,
            LightingChannel::Hdr => &mut self.hdr_work,
        };
        *channel += amount;
        let index = self.phase_index();
        self.phases[index].work += amount;
        match contributor {
            Contributor::Retained(index) => self.contributors[index].work += amount,
            Contributor::Other => self.other_contributors_work += amount,
            Contributor::Unattributed => self.unattributed_work += amount,
        }
        Ok(())
    }
}
