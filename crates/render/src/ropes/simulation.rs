use std::sync::Arc;

use glam::{DVec3, Vec3};
use map_core::rope_collision::Trace;

use super::{
    Environment, INITIAL_HANG_SECONDS, Link, MAX_BATCH_STEPS, MAX_CURVE_SEGMENTS, MAX_EXTENT,
    MAX_SIMULATION_WORK, MAX_TIME_SECONDS, Prepared, Preview, Progress, STEP_SECONDS,
};

const CONSTRAINT_PASSES: usize = 3;
const COLLISION_ITERATIONS: usize = 10;
const DAMPING: f32 = 0.98;
const COLLISION_FRICTION: f32 = 0.3;
const COLLISION_PUSH: f64 = 2.2;

#[derive(Debug, Clone, Copy)]
struct Node {
    position: Vec3,
    previous: Vec3,
    touching: bool,
}

#[derive(Debug)]
struct State {
    nodes: Vec<Node>,
    spring: f32,
}

#[derive(Debug)]
pub struct Simulation {
    prepared: Arc<Prepared>,
    environment: Environment,
    states: Vec<State>,
    ticks: usize,
    requested: Option<f64>,
    complete: bool,
    work: usize,
    failure: Option<String>,
}

fn charge(work: &mut usize, amount: usize) -> Result<(), String> {
    *work = work
        .checked_add(amount)
        .filter(|work| *work <= MAX_SIMULATION_WORK)
        .ok_or("Rope simulation exceeds its bounded aggregate work")?;
    Ok(())
}

fn position_valid(position: Vec3, width: f64) -> bool {
    position.is_finite() && f64::from(position.abs().max_element()) + width * 0.5 <= MAX_EXTENT
}

fn contact(
    node: &mut Node,
    trace: &mut impl FnMut(DVec3, DVec3) -> Result<Trace, String>,
    work: &mut usize,
) -> Result<(), String> {
    for _ in 0..COLLISION_ITERATIONS {
        charge(work, 1)?;
        let result = trace(node.previous.as_dvec3(), node.position.as_dvec3())?;
        if !result.fraction.is_finite()
            || !(0.0..=1.0).contains(&result.fraction)
            || !result.end.is_finite()
        {
            return Err("Rope collision provider returned an invalid trace".into());
        }
        if result.fraction == 1.0 {
            return Ok(());
        }
        if result.start_solid || result.all_solid || result.fraction == 0.0 {
            node.position = node.previous;
            node.touching = true;
            return Ok(());
        }
        if !result.normal.is_finite()
            || !result.plane_distance.is_finite()
            || (result.normal.length_squared() - 1.0).abs() > 1e-4
        {
            return Err("Rope collision provider returned an invalid contact plane".into());
        }
        node.position -= (node.position - node.previous) * COLLISION_FRICTION;
        let position = node.position.as_dvec3();
        let behind = result.normal.dot(position) - result.plane_distance;
        node.position = (position + result.normal * (-behind + COLLISION_PUSH)).as_vec3();
        node.touching = true;
    }
    node.position = node.previous;
    Ok(())
}

impl Simulation {
    pub fn new(prepared: Arc<Prepared>, environment: Environment) -> Result<Self, String> {
        prepared.validate()?;
        environment.validate()?;
        let mut states = Vec::new();
        states
            .try_reserve_exact(prepared.spans.len())
            .map_err(|_| "Rope state allocation failed")?;
        for span in &prepared.spans {
            let end = span
                .endpoints
                .end
                .ok_or("Rope needs a resolved end anchor")?;
            let start = span.endpoints.start.position.as_vec3();
            let end = end.position.as_vec3();
            let mut nodes = Vec::new();
            nodes
                .try_reserve_exact(span.settings.nodes)
                .map_err(|_| "Rope node allocation failed")?;
            for index in 0..span.settings.nodes {
                let t = index as f32 / (span.settings.nodes - 1) as f32;
                let position = start.lerp(end, t);
                if !position_valid(position, span.settings.width) {
                    return Err(format!(
                        "Rope entity:{} initial nodes exceed finite preview bounds",
                        span.source
                    ));
                }
                nodes.push(Node {
                    position,
                    previous: position,
                    touching: false,
                });
            }
            let spring = ((span.endpoints.rope_length + span.settings.slack - 100)
                / (span.settings.nodes - 1) as i32)
                .max(0) as f32;
            states.push(State { nodes, spring });
        }
        Ok(Self {
            prepared,
            environment,
            states,
            ticks: 0,
            requested: None,
            complete: false,
            work: 0,
            failure: None,
        })
    }

    pub fn advance_to(
        &mut self,
        time: f64,
        max_steps: usize,
        trace: &mut impl FnMut(DVec3, DVec3) -> Result<Trace, String>,
        checkpoint: &mut impl FnMut() -> Result<(), String>,
    ) -> Result<Progress, String> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = self.advance(time, max_steps, trace, checkpoint);
        if let Err(error) = &result {
            self.failure = Some(error.clone());
            self.complete = false;
        }
        result
    }

    fn advance(
        &mut self,
        time: f64,
        max_steps: usize,
        trace: &mut impl FnMut(DVec3, DVec3) -> Result<Trace, String>,
        checkpoint: &mut impl FnMut() -> Result<(), String>,
    ) -> Result<Progress, String> {
        checkpoint()?;
        if !time.is_finite() || !(0.0..=MAX_TIME_SECONDS).contains(&time) {
            return Err(format!(
                "Rope time must be between 0 and {MAX_TIME_SECONDS} seconds"
            ));
        }
        if !(1..=MAX_BATCH_STEPS).contains(&max_steps) {
            return Err(format!(
                "Rope advance needs 1–{MAX_BATCH_STEPS} fixed steps"
            ));
        }
        if self.requested.is_some_and(|previous| time < previous) {
            return Err("Backward rope seek requires a fresh simulation".into());
        }
        self.requested = Some(time);
        let required = ((INITIAL_HANG_SECONDS + time) / STEP_SECONDS).ceil() as usize;
        let steps = (required - self.ticks).min(max_steps);
        if self.states.is_empty() {
            self.ticks = required;
        } else {
            for _ in 0..steps {
                checkpoint()?;
                self.step(trace)?;
                self.ticks += 1;
            }
        }
        self.complete = self.ticks == required;
        let warmup = (INITIAL_HANG_SECONDS / STEP_SECONDS).ceil() as usize;
        Ok(Progress {
            complete: self.complete,
            time: if self.complete {
                time
            } else {
                (self.ticks as f64 * STEP_SECONDS - INITIAL_HANG_SECONDS)
                    .max(0.0)
                    .min(time)
            },
            steps: if self.states.is_empty() { 0 } else { steps },
            warmup_steps: self.ticks.min(warmup),
            work: self.work,
        })
    }

    fn step(
        &mut self,
        trace: &mut impl FnMut(DVec3, DVec3) -> Result<Trace, String>,
    ) -> Result<(), String> {
        let dt = STEP_SECONDS as f32;
        let multiplier = dt * dt * 0.5;
        let apply_wind = self.ticks >= (INITIAL_HANG_SECONDS / STEP_SECONDS).ceil() as usize;
        for (state, span) in self.states.iter_mut().zip(&self.prepared.spans) {
            charge(&mut self.work, state.nodes.len())?;
            for node in &mut state.nodes {
                let mut acceleration = Vec3::new(0.0, 0.0, -self.environment.gravity as f32);
                if apply_wind && !span.settings.no_wind && !node.touching {
                    acceleration += self.environment.wind.as_vec3();
                }
                let previous = node.position;
                node.position +=
                    (node.position - node.previous) * DAMPING + acceleration * multiplier;
                node.previous = previous;
                node.touching = false;
            }
            for _ in 0..CONSTRAINT_PASSES {
                charge(&mut self.work, state.nodes.len() - 1)?;
                for index in 0..state.nodes.len() - 1 {
                    let delta = state.nodes[index].position - state.nodes[index + 1].position;
                    let distance_squared = delta.length_squared();
                    if !distance_squared.is_finite() {
                        return Err(format!(
                            "Rope entity:{} has a nonfinite spring",
                            span.source
                        ));
                    }
                    if distance_squared > state.spring * state.spring {
                        let adjustment =
                            delta * (1.0 - state.spring / distance_squared.sqrt()) * 0.5;
                        state.nodes[index].position -= adjustment;
                        state.nodes[index + 1].position += adjustment;
                    }
                }
                if span.settings.collide {
                    for (index, node) in state.nodes.iter_mut().enumerate() {
                        contact(node, trace, &mut self.work).map_err(|error| {
                            format!("Rope entity:{} node:{index}: {error}", span.source)
                        })?;
                    }
                }
                charge(&mut self.work, 2)?;
                state.nodes[0].position = span.endpoints.start.position.as_vec3();
                if !span.settings.dangling {
                    let last = state.nodes.len() - 1;
                    state.nodes[last].position = span
                        .endpoints
                        .end
                        .ok_or("Rope lost its end anchor")?
                        .position
                        .as_vec3();
                }
                if state
                    .nodes
                    .iter()
                    .any(|node| !position_valid(node.position, span.settings.width))
                {
                    return Err(format!(
                        "Rope entity:{} simulation exceeds finite ±65536 bounds",
                        span.source
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn snapshot(&self) -> Result<Preview, String> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        if !self.complete {
            return Err("Rope frame is not ready at the requested time".into());
        }
        let time = self
            .requested
            .ok_or("Rope simulation has no requested time")?;
        let alpha = ((INITIAL_HANG_SECONDS + time - (self.ticks as f64 - 1.0) * STEP_SECONDS)
            / STEP_SECONDS) as f32;
        if !alpha.is_finite() || !(0.0..=1.0).contains(&alpha) {
            return Err("Rope frame has an invalid fixed-step interpolant".into());
        }
        let mut links = Vec::new();
        links
            .try_reserve_exact(self.states.len())
            .map_err(|_| "Rope frame allocation failed")?;
        for (state, span) in self.states.iter().zip(&self.prepared.spans) {
            let nodes = state
                .nodes
                .iter()
                .map(|node| node.previous.lerp(node.position, alpha))
                .collect::<Vec<_>>();
            let subdiv = if span.settings.barbed {
                3
            } else {
                span.settings.subdiv
            };
            let mut points = Vec::new();
            points
                .try_reserve_exact((nodes.len() - 1) * (subdiv + 1) + 1)
                .map_err(|_| "Rope curve allocation failed")?;
            for index in 0..nodes.len() {
                points.push(nodes[index].as_dvec3());
                if index + 1 == nodes.len() {
                    continue;
                }
                let before = nodes[index.saturating_sub(1)];
                let start = nodes[index];
                let end = nodes[index + 1];
                let after = nodes[(index + 2).min(nodes.len() - 1)];
                for subdivision in 0..subdiv {
                    let t = if span.settings.barbed {
                        [1.5, -0.5, 0.5][subdivision]
                    } else {
                        (subdivision + 1) as f32 / (subdiv + 1) as f32
                    };
                    let point = (start * 2.0
                        + (end - before) * t
                        + (before * 2.0 - start * 5.0 + end * 4.0 - after) * (t * t)
                        + (-before + start * 3.0 - end * 3.0 + after) * (t * t * t))
                        * 0.5;
                    points.push(point.as_dvec3());
                }
            }
            if points.len() > MAX_CURVE_SEGMENTS + 1
                || points.iter().any(|point| {
                    !point.is_finite()
                        || point.abs().max_element() + span.settings.width * 0.5 > MAX_EXTENT
                })
            {
                return Err(format!(
                    "Rope entity:{} visual curve exceeds preview bounds",
                    span.source
                ));
            }
            if !points.windows(2).any(|pair| pair[0] != pair[1]) {
                return Err(format!(
                    "Rope entity:{} has a fully collapsed curve",
                    span.source
                ));
            }
            links.push(Link {
                source: span.source,
                target: span.target,
                points,
                width: span.settings.width,
                slack: f64::from(span.settings.slack),
                physical_nodes: span.settings.nodes,
                subdiv_authored: span.settings.subdiv_authored,
                subdiv: span.settings.subdiv,
                material: span.settings.material.clone(),
                texture_scale: span.settings.texture_scale,
                texture_length: f64::from(span.endpoints.rope_length + span.settings.slack - 100),
                barbed: span.settings.barbed,
                breakable: span.settings.breakable,
                inert_keys: span.settings.inert_keys.clone(),
                scroll: 0.0,
            });
        }
        Ok(Preview {
            links,
            issues: Vec::new(),
            omitted: 0,
            complete: true,
            time,
        })
    }
}
