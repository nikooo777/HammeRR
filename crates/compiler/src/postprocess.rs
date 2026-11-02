use serde::Serialize;

use crate::{LightingMode, PhysicsMode};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PostprocessOrder {
    #[default]
    LightingThenPhysics,
    PhysicsThenLighting,
}

impl PostprocessOrder {
    pub const ALL: [Self; 2] = [Self::LightingThenPhysics, Self::PhysicsThenLighting];

    pub const fn name(self) -> &'static str {
        match self {
            Self::LightingThenPhysics => "lighting_then_physics",
            Self::PhysicsThenLighting => "physics_then_lighting",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::LightingThenPhysics => "Lighting then physics",
            Self::PhysicsThenLighting => "Physics then lighting",
        }
    }

    pub fn parse(name: &str) -> Result<Self, String> {
        match name.trim().to_ascii_lowercase().as_str() {
            "lighting_then_physics" => Ok(Self::LightingThenPhysics),
            "physics_then_lighting" => Ok(Self::PhysicsThenLighting),
            _ => Err(
                "Postprocess order must be lighting_then_physics or physics_then_lighting".into(),
            ),
        }
    }

    pub const fn stages(self) -> [PostprocessStage; 2] {
        match self {
            Self::LightingThenPhysics => [PostprocessStage::Lighting, PostprocessStage::Physics],
            Self::PhysicsThenLighting => [PostprocessStage::Physics, PostprocessStage::Lighting],
        }
    }

    fn plan(self, lighting: LightingMode, physics: PhysicsMode) -> PostprocessPlan {
        let steps = self.stages().map(|stage| {
            let skip_reason = match stage {
                PostprocessStage::Lighting if !lighting.performs_lighting() => Some("Fullbright"),
                PostprocessStage::Physics if physics != PhysicsMode::Engine => {
                    Some("Physics mode is None")
                }
                _ => None,
            };
            PostprocessStep {
                stage,
                enabled: skip_reason.is_none(),
                skip_reason,
            }
        });
        let fixed_before = "Geometry, VIS, static props, details and requested asset preflight";
        let fixed_after = "Asset packing and reflections when requested; guarded output writing";
        let labels = steps.map(|step| match step.skip_reason {
            Some(reason) => format!("{} skipped ({reason})", step.stage.label()),
            None => step.stage.label().into(),
        });
        PostprocessPlan {
            requested_order: self,
            effective_stages: steps
                .iter()
                .filter(|step| step.enabled)
                .map(|step| step.stage)
                .collect(),
            steps,
            fixed_before,
            fixed_after,
            summary: format!("{fixed_before} → {} → {fixed_after}", labels.join(" → ")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PostprocessStage {
    Lighting,
    Physics,
}

impl PostprocessStage {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Lighting => "lighting",
            Self::Physics => "physics",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Lighting => "Lighting",
            Self::Physics => "Physics",
        }
    }

    pub(super) const fn progress(self) -> crate::Stage {
        match self {
            Self::Lighting => crate::Stage::Lighting,
            Self::Physics => crate::Stage::Physics,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct PostprocessStep {
    pub stage: PostprocessStage,
    pub enabled: bool,
    pub skip_reason: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PostprocessPlan {
    pub requested_order: PostprocessOrder,
    pub effective_stages: Vec<PostprocessStage>,
    pub steps: [PostprocessStep; 2],
    pub fixed_before: &'static str,
    pub fixed_after: &'static str,
    pub summary: String,
}

impl Default for PostprocessPlan {
    fn default() -> Self {
        crate::Options::default().postprocess_plan()
    }
}

impl crate::Options {
    pub fn postprocess_plan(self) -> PostprocessPlan {
        self.postprocess_order.plan(self.lighting, self.physics)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PostprocessStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl PostprocessStatus {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct PostprocessExecution {
    pub stage: PostprocessStage,
    pub status: PostprocessStatus,
}

#[derive(Debug, Default, Serialize)]
pub struct PostprocessReport {
    pub plan: PostprocessPlan,
    pub stages: Vec<PostprocessExecution>,
}

impl PostprocessReport {
    pub(super) fn new(options: crate::Options) -> Self {
        Self {
            plan: options.postprocess_plan(),
            stages: Vec::new(),
        }
    }

    pub(super) fn begin(&mut self, stage: PostprocessStage) {
        self.stages.push(PostprocessExecution {
            stage,
            status: PostprocessStatus::Running,
        });
    }

    pub(super) fn finish(&mut self, cancelled: bool, failed: bool) {
        if let Some(execution) = self.stages.last_mut() {
            execution.status = if cancelled {
                PostprocessStatus::Cancelled
            } else if failed {
                PostprocessStatus::Failed
            } else {
                PostprocessStatus::Completed
            };
        }
    }
}
