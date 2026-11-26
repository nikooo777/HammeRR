use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use glam::Vec3;
use source_formats::gamefs::GameFs;

mod appearance;
mod simulation;
use simulation::Simulation;

pub const MAX_TIME: f64 = simulation::MAX_SECONDS as f64;
pub const MAX_ENTITIES: usize = 16;
pub(crate) const MAX_QUADS: usize = MAX_ENTITIES * simulation::MAX_PARTICLES;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    Off,
    Selected,
    All,
}

impl Scope {
    pub fn parse(name: &str) -> Result<Self, String> {
        match name {
            "off" => Ok(Self::Off),
            "selected" => Ok(Self::Selected),
            "all" => Ok(Self::All),
            _ => Err("Particle preview must be off, selected or all".into()),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Selected => "selected",
            Self::All => "all",
        }
    }
}

pub fn checked_time(seconds: f64) -> Result<f32, String> {
    if !(0.0..=MAX_TIME).contains(&seconds) {
        return Err(format!(
            "Particle time must be finite and between 0 and {MAX_TIME} seconds"
        ));
    }
    Ok(seconds as f32)
}

#[derive(Debug)]
pub struct Asset {
    simulation: Simulation,
    appearance: appearance::Appearance,
    bytes: usize,
    pub(crate) id: u64,
}

impl Asset {
    pub fn load(
        fs: &GameFs,
        program: &source_formats::particle::Program,
    ) -> Result<Arc<Self>, String> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let simulation = Simulation::compile(program)?;
        let appearance = appearance::Appearance::load(fs, simulation.material())?;
        let texture = appearance
            .material
            .base
            .as_ref()
            .ok_or("Particle preview has no base texture")?;
        let bytes = texture.encoded_size()
            + (0..texture.mip_count)
                .map(|mip| {
                    let (width, height) = texture.mip_size(mip);
                    width as usize * height as usize * 4
                })
                .sum::<usize>();
        Ok(Arc::new(Self {
            simulation,
            appearance,
            bytes,
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        }))
    }

    pub fn retained_bytes(&self) -> usize {
        self.bytes
    }

    pub(crate) fn material(&self) -> &crate::materials::MaterialInfo {
        &self.appearance.material
    }

    pub(crate) fn blend(&self) -> crate::icons::Blend {
        self.appearance.blend
    }

    pub(crate) fn clamp_edges(&self) -> bool {
        self.appearance.clamp_edges
    }
}

#[derive(Debug, Clone)]
pub struct Emitter {
    pub asset: Arc<Asset>,
    pub origin: [f32; 3],
    pub seed: u32,
}

impl Emitter {
    pub(crate) fn sample(&self, camera: &crate::Camera, seconds: f32) -> Result<Vec<Quad>, String> {
        let samples = self.asset.simulation.sample(seconds, self.seed)?;
        if samples.is_empty() {
            return Ok(Vec::new());
        }
        let origin = Vec3::from_array(self.origin);
        let (mins, maxs) = samples
            .iter()
            .map(|particle| origin + Vec3::from_array(particle.offset))
            .fold((Vec3::MAX, Vec3::MIN), |(mins, maxs), center| {
                (mins.min(center), maxs.max(center))
            });
        let padding = Vec3::splat(simulation::BOUNDS_PADDING);
        let mins = mins - padding;
        let maxs = maxs + padding;
        let eye = camera.position;
        let nearest = eye.clamp(mins, maxs);
        if eye.distance_squared(nearest) > self.asset.simulation.max_draw_distance_squared() {
            return Ok(Vec::new());
        }
        let inside_bounds = eye == nearest;
        let forward = camera.forward();
        samples
            .into_iter()
            .filter_map(|particle| {
                let center = origin + Vec3::from_array(particle.offset);
                let alpha = (particle.alpha * 255.0).round_ties_even();
                let facing = !inside_bounds || (center - eye).dot(forward) >= 0.0;
                (facing && alpha > 0.0 && particle.radius != 0.0).then_some((
                    particle,
                    center.to_array(),
                    alpha / 255.0,
                ))
            })
            .map(|(particle, center, opacity)| {
                let corners = appearance::quad(center, particle.radius, particle.rotation)?;
                Ok(Quad {
                    center,
                    corners,
                    radius: particle.radius,
                    opacity,
                    color: particle.color,
                })
            })
            .collect()
    }
}

#[derive(Debug)]
pub(crate) struct Quad {
    pub(crate) center: [f32; 3],
    pub(crate) corners: [[f32; 3]; 4],
    pub(crate) radius: f32,
    pub(crate) opacity: f32,
    pub(crate) color: [u8; 3],
}
