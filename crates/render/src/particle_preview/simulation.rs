use std::collections::BTreeMap;

use source_formats::gamefs::asset_relative;
use source_formats::particle::{Operator, Program, Property};
use source_formats::vmt::material_path;

pub(crate) const MAX_SECONDS: f32 = 20.0;
pub(crate) const MAX_PARTICLES: usize = 128;
pub(crate) const BOUNDS_PADDING: f32 = 10.0;
const STEPS_PER_SECOND: u32 = 20;
const STEP: f32 = 1.0 / STEPS_PER_SECOND as f32;
const MAX_RATE: f32 = 6.0;
const NORMALIZE_BYTE: f32 = 1.0 / 255.0;
const OPERATOR_RANDOM_STRIDE: u32 = 17;
const FADE_OUT_MIN: &str = "fade out time min";
const FADE_OUT_MAX: &str = "fade out time max";

type Properties = BTreeMap<String, Property>;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Sample {
    pub(crate) offset: [f32; 3],
    pub(crate) radius: f32,
    pub(crate) alpha: f32,
    pub(crate) rotation: f32,
    pub(crate) color: [u8; 3],
}

#[derive(Debug)]
pub(crate) struct Simulation {
    material: String,
    emission: Emission,
    max_draw_distance_squared: f32,
    capacity: usize,
    lifetime: Bounds,
    alpha: Bounds,
    radius: Bounds,
    distance: Bounds,
    positive_z: bool,
    radius_start: f32,
    radius_end: f32,
    radius_bias: Option<f32>,
    color: Option<ColorRandom>,
    random_rotation: bool,
    random_fades: Option<RandomFades>,
}

#[derive(Debug, Clone, Copy)]
enum Emission {
    Continuous(f32),
    Instantaneous,
}

#[derive(Debug, Clone, Copy)]
struct Bounds {
    min: f32,
    max: f32,
}

impl Bounds {
    fn new(min: f32, max: f32, context: &str) -> Result<Self, String> {
        if min > max {
            return Err(format!("{context} maximum must be at least the minimum"));
        }
        Ok(Self { min, max })
    }

    fn at(self, fraction: f32) -> f32 {
        self.min + (self.max - self.min) * fraction
    }

    fn normalized_byte(self) -> Self {
        Self {
            min: self.min * NORMALIZE_BYTE,
            max: self.max * NORMALIZE_BYTE,
        }
    }
}

#[derive(Debug)]
struct ColorRandom {
    color: [f32; 3],
    color_delta: [f32; 3],
    tint: f32,
}

#[derive(Debug)]
struct RandomFades {
    fade_in: Bounds,
    fade_out: Bounds,
}

#[derive(Debug)]
struct Fade {
    fade_in: f32,
    fade_out: f32,
}

#[derive(Debug)]
struct Particle {
    sample: Sample,
    born: f32,
    lifetime: f32,
    initial_radius: f32,
    initial_alpha: f32,
    fade: Option<Fade>,
}

impl Simulation {
    pub(crate) fn compile(program: &Program) -> Result<Self, String> {
        if !cfg!(target_arch = "x86_64") {
            return Err("Particle preview requires the installed x86-64 operator math".into());
        }
        use Property::{Bool, Color, Float, Int, Text, Vector};

        let definition = Fields::definition(program);
        definition.validate(
            &[
                "material",
                "max_particles",
                "maximum draw distance",
                "time to sleep when not drawn",
            ],
            &[
                ("preventnamebasedlookup", Bool(false)),
                ("initial_particles", Int(0)),
                ("bounding_box_control_point", Int(0)),
                ("bounding_box_min", Vector([-BOUNDS_PADDING; 3])),
                ("bounding_box_max", Vector([BOUNDS_PADDING; 3])),
                ("cull_radius", Float(0.0)),
                ("cull_cost", Float(1.0)),
                ("cull_control_point", Int(0)),
                ("cull_replacement_definition", Text(String::new())),
                ("radius", Float(5.0)),
                ("color", Color([255; 4])),
                ("rotation", Float(0.0)),
                ("rotation_speed", Float(0.0)),
                ("normal", Vector([0.0, 0.0, 1.0])),
                ("sequence_number", Int(0)),
                ("sequence_number 1", Int(0)),
                ("group id", Int(0)),
                ("maximum time step", Float(0.1)),
                ("maximum sim tick rate", Float(0.0)),
                ("minimum sim tick rate", Float(0.0)),
                ("minimum rendered frames", Int(0)),
                (
                    "control point to disable rendering if it is the camera",
                    Int(-1),
                ),
                ("sort particles", Bool(true)),
                ("batch particle systems", Bool(false)),
                ("view model effect", Bool(false)),
            ],
        )?;
        let material = definition.material()?;
        let capacity = definition.integer("max_particles", 1..=1000, 1000)? as usize;
        let max_draw_distance =
            definition.float("maximum draw distance", 0.0..=100_000.0, 100_000.0)?;
        // Seeking has no runtime visibility history, so it does not sleep the simulation.
        definition.float("time to sleep when not drawn", 0.0..=f32::MAX, 8.0)?;
        for (group, operators) in &program.functions {
            if !operators.is_empty()
                && !matches!(
                    group.as_str(),
                    "emitters" | "initializers" | "operators" | "renderers"
                )
            {
                return Err(format!("Particle preview excludes function group {group}"));
            }
        }

        let emission = Emission::compile(program)?;

        let ([lifetime, alpha, radius, position], [color, rotation]) = operators(
            program,
            "initializers",
            [
                "Lifetime Random",
                "Alpha Random",
                "Radius Random",
                "Position Within Sphere Random",
            ],
            ["Color Random", "Rotation Random"],
        )?;
        lifetime.validate(
            &["lifetime_min", "lifetime_max"],
            &[("lifetime_random_exponent", Float(1.0))],
        )?;
        let lifetime = Bounds::new(
            lifetime.float("lifetime_min", STEP..=MAX_SECONDS, 0.0)?,
            lifetime.float("lifetime_max", STEP..=MAX_SECONDS, 0.0)?,
            lifetime.label,
        )?;
        alpha.validate(
            &["alpha_min", "alpha_max"],
            &[("alpha_random_exponent", Float(1.0))],
        )?;
        let alpha = Bounds::new(
            alpha.integer("alpha_min", 0..=255, 255)? as f32,
            alpha.integer("alpha_max", 0..=255, 255)? as f32,
            alpha.label,
        )?
        .normalized_byte();
        radius.validate(
            &["radius_min", "radius_max"],
            &[("radius_random_exponent", Float(1.0))],
        )?;
        let radius = Bounds::new(
            radius.float("radius_min", 0.01..=64.0, 1.0)?,
            radius.float("radius_max", 0.01..=64.0, 1.0)?,
            radius.label,
        )?;
        let (distance, positive_z) = sphere(position)?;
        let color = color.map(ColorRandom::compile).transpose()?;
        if let Some(rotation) = &rotation {
            rotation.validate(
                &[],
                &[
                    ("rotation_initial", Float(0.0)),
                    ("rotation_offset_min", Float(0.0)),
                    ("rotation_offset_max", Float(360.0)),
                    ("rotation_random_exponent", Float(1.0)),
                ],
            )?;
        }

        let ([decay, fade_out, movement, scaling], [fade_in]) = operators(
            program,
            "operators",
            [
                "Lifespan Decay",
                "Alpha Fade Out Random",
                "Movement Basic",
                "Radius Scale",
            ],
            ["Alpha Fade In Random"],
        )?;
        movement.validate(
            &[],
            &[
                ("gravity", Vector([0.0; 3])),
                ("drag", Float(0.0)),
                ("max constraint passes", Int(3)),
            ],
        )?;
        decay.validate(&[], &[])?;
        match emission {
            Emission::Continuous(_) if color.is_some() != fade_in.is_some() => {
                return Err(
                    "Continuous particle preview requires Color Random and Alpha Fade In Random together"
                        .into(),
                );
            }
            Emission::Instantaneous if fade_in.is_some() || rotation.is_some() => {
                return Err("Instantaneous particle preview excludes fade-in and rotation".into());
            }
            _ => {}
        }
        fade_out.validate(
            &[FADE_OUT_MIN, FADE_OUT_MAX],
            &[
                ("fade out time exponent", Float(1.0)),
                ("proportional 0/1", Bool(true)),
                ("ease in and out", Bool(true)),
                ("fade bias", Float(0.5)),
            ],
        )?;
        let fade_out = fade_out.fade_duration(FADE_OUT_MIN, FADE_OUT_MAX)?;
        let random_fades = match fade_in {
            Some(fade_in) => Some(RandomFades::compile(fade_in, fade_out)?),
            None if fade_out.min == 1.0 && fade_out.max == 1.0 => None,
            None => return Err("Fade-out without fade-in must span the whole lifetime".into()),
        };
        scaling.validate(
            &["radius_start_scale", "radius_end_scale", "scale_bias"],
            &[
                ("start_time", Float(0.0)),
                ("end_time", Float(1.0)),
                ("ease_in_and_out", Bool(false)),
            ],
        )?;
        let radius_start = scaling.float(
            "radius_start_scale",
            match emission {
                Emission::Continuous(_) => 1.0..=1.0,
                Emission::Instantaneous => 0.0..=1.0,
            },
            1.0,
        )?;
        let radius_end = scaling.float("radius_end_scale", 0.125..=8.0, 1.0)?;
        let bias = scaling.float("scale_bias", 0.125..=0.875, 0.5)?;
        let radius_bias = (bias != 0.5).then(|| native_math::bias_parameter(bias));

        let ([renderer], []) = operators(program, "renderers", ["render_animated_sprites"], [])?;
        renderer.validate(
            &[],
            &[
                ("orientation_type", Int(2)),
                ("orientation control point", Int(-1)),
                ("animation rate", Float(0.1)),
                ("second sequence animation rate", Float(0.0)),
                ("animation_fit_lifetime", Bool(false)),
                ("use animation rate as fps", Bool(false)),
                ("visibility proxy input control point number", Int(-1)),
            ],
        )?;
        renderer.require("orientation_type")?;
        Ok(Self {
            material,
            emission,
            max_draw_distance_squared: max_draw_distance * max_draw_distance,
            capacity: capacity.min(MAX_PARTICLES),
            lifetime,
            alpha,
            radius,
            distance,
            positive_z,
            radius_start,
            radius_end,
            radius_bias,
            color,
            random_rotation: rotation.is_some(),
            random_fades,
        })
    }

    pub(crate) fn material(&self) -> &str {
        &self.material
    }

    pub(crate) fn max_draw_distance_squared(&self) -> f32 {
        self.max_draw_distance_squared
    }

    pub(crate) fn sample(&self, seconds: f32, seed: u32) -> Result<Vec<Sample>, String> {
        if !(0.0..=MAX_SECONDS).contains(&seconds) {
            return Err(format!(
                "Particle preview time must be finite and within 0–{MAX_SECONDS} seconds"
            ));
        }
        if seed == 0 {
            return Err("Particle preview requires a nonzero seed".into());
        }
        let mut state = Timeline {
            particles: Vec::new(),
            random: Random { seed, query: 0 },
            time: 0.0,
            births: 0.0,
            emitted: 0,
            spawned: 0,
        };
        let steps = (f64::from(seconds) * f64::from(STEPS_PER_SECOND)).floor() as u32;
        for _ in 0..steps {
            self.step(&mut state, STEP);
        }
        let remainder =
            (f64::from(seconds) - f64::from(steps) / f64::from(STEPS_PER_SECOND)) as f32;
        if remainder >= 1e-22 {
            self.step(&mut state, remainder);
        }
        Ok(state
            .particles
            .into_iter()
            .map(|particle| particle.sample)
            .collect())
    }

    fn step(&self, state: &mut Timeline, dt: f32) {
        state.time += dt;
        self.emit(state, dt);

        state
            .particles
            .retain(|particle| state.time - particle.born < particle.lifetime);
        for particle in &mut state.particles {
            let life = (state.time - particle.born) * native_math::reciprocal(particle.lifetime);
            if let Some(fade) = &particle.fade {
                let fade_start = 1.0 - fade.fade_out;
                if life > fade_start {
                    let progress = spline(life - fade_start, fade.fade_out);
                    particle.sample.alpha =
                        (particle.initial_alpha - particle.initial_alpha * progress).max(0.0);
                }
                let life = life.clamp(0.0, 1.0);
                if life < fade.fade_in {
                    particle.sample.alpha = particle.initial_alpha * spline(life, fade.fade_in);
                }
            } else if life > 0.0 {
                let fade = spline(life, 1.0);
                particle.sample.alpha =
                    (particle.initial_alpha - particle.initial_alpha * fade).max(0.0);
            }
            if (0.0..1.0).contains(&life) {
                let t = life * native_math::reciprocal_refined(1.0);
                let t = self.radius_bias.map_or(t, |bias| {
                    let denominator = 1.0 + bias * (1.0 - t);
                    t * native_math::reciprocal_refined(denominator)
                });
                let scale = self.radius_start + (self.radius_end - self.radius_start) * t;
                particle.sample.radius = particle.initial_radius * scale;
            }
        }
    }

    fn emit(&self, state: &mut Timeline, dt: f32) {
        let Emission::Continuous(rate) = self.emission else {
            if state.spawned == 0 {
                self.spawn(state, 0.0);
            }
            return;
        };

        let previous_time = state.time - dt;
        let elapsed = state.time - previous_time;
        state.births += rate * elapsed;
        let total = state.births.floor() as u32;
        let emit = (total - state.emitted) as usize;
        state.emitted = total;
        let emit = emit.min(self.capacity - state.particles.len());
        if emit != 0 {
            let interval = elapsed / emit as f32;
            let mut born = previous_time;
            for _ in 0..emit {
                born = (born + interval).min(state.time);
                self.spawn(state, born);
            }
        }
    }

    fn spawn(&self, state: &mut Timeline, born: f32) {
        let random = &mut state.random;
        let sequence = state.spawned;
        let lifetime = self.lifetime.at(random.next());
        let alpha = self.alpha.at(random.next());
        let radius = self.radius.at(random.next());
        let offset = random.sphere(self.distance, self.positive_z);
        let color = self
            .color
            .as_ref()
            .map_or([255; 3], |color| color.standalone_color(random.next()));
        let rotation = if self.random_rotation {
            random.next() * std::f32::consts::TAU
        } else {
            0.0
        };
        let fade = self.random_fades.as_ref().map(|fades| Fade {
            fade_out: fades
                .fade_out
                .at(random.for_operator(sequence, OPERATOR_RANDOM_STRIDE)),
            fade_in: fades
                .fade_in
                .at(random.for_operator(sequence, 4 * OPERATOR_RANDOM_STRIDE)),
        });
        state.particles.push(Particle {
            sample: Sample {
                offset,
                alpha,
                radius,
                rotation,
                color,
            },
            born,
            lifetime,
            initial_radius: radius,
            initial_alpha: alpha,
            fade,
        });
        state.spawned += 1;
    }
}

impl Emission {
    fn compile(program: &Program) -> Result<Self, String> {
        use Property::{Bool, Float, Int};

        let [operator] = program
            .functions
            .get("emitters")
            .map(Vec::as_slice)
            .unwrap_or(&[])
        else {
            return Err("Particle preview requires exactly one emitter".into());
        };
        let emitter = Fields::operator(operator);
        match operator.name.as_str() {
            "emit_continuously" => {
                emitter.validate(
                    &["emission_rate"],
                    &[
                        ("emission_start_time", Float(0.0)),
                        ("emission_duration", Float(0.0)),
                        ("scale emission to used control points", Float(0.0)),
                        ("use parent particles for emission scaling", Bool(false)),
                        ("emission count scale control point", Int(-1)),
                        ("emission count scale control point field", Int(0)),
                        ("emit particles for killed parent particles", Bool(false)),
                    ],
                )?;
                let rate = emitter.float("emission_rate", 0.0..=MAX_RATE, 100.0)?;
                if rate == 0.0 {
                    return Err(
                        "Particle preview requires a positive continuous emission rate".into(),
                    );
                }
                Ok(Self::Continuous(rate))
            }
            "emit_instantaneously" => {
                emitter.validate(
                    &[],
                    &[
                        ("emission_start_time", Float(0.0)),
                        ("num_to_emit", Int(1)),
                        ("num_to_emit_minimum", Int(-1)),
                        ("maximum emission per frame", Int(100)),
                        ("emission count scale control point", Int(-1)),
                        ("emission count scale control point field", Int(0)),
                    ],
                )?;
                emitter.require("num_to_emit")?;
                emitter.require("maximum emission per frame")?;
                Ok(Self::Instantaneous)
            }
            _ => Err(format!(
                "Particle preview excludes emitter {}",
                operator.name
            )),
        }
    }
}

fn spline(age: f32, duration: f32) -> f32 {
    let t = (age * native_math::reciprocal(duration)).clamp(0.0, 1.0);
    (t * t) * (3.0 - 2.0 * t)
}

impl ColorRandom {
    fn compile(color: Fields<'_>) -> Result<Self, String> {
        use Property::{Color, Float, Int};

        color.validate(
            &["color1", "color2", "tint_perc"],
            &[
                ("tint control point", Int(0)),
                ("tint clamp min", Color([0; 4])),
                ("tint clamp max", Color([255; 4])),
                ("tint update movement threshold", Float(32.0)),
            ],
        )?;
        color.require("tint_perc")?;
        let tint = color.float("tint_perc", 0.0..=1.0, 0.0)?;
        let second = color.rgb("color2")?;
        let color = color.rgb("color1")?;
        let color_delta = std::array::from_fn(|axis| second[axis] - color[axis]);
        Ok(Self {
            color,
            color_delta,
            tint,
        })
    }

    fn standalone_color(&self, fraction: f32) -> [u8; 3] {
        // Source's standalone particle lighting query returns white (client.so:00914e40).
        std::array::from_fn(|axis| {
            let rgb = self.color[axis] + self.color_delta[axis] * fraction;
            let rgb = rgb + (1.0 - rgb) * self.tint;
            (rgb * 255.0).round_ties_even() as u8
        })
    }
}

impl RandomFades {
    fn compile(fade_in: Fields<'_>, fade_out: Bounds) -> Result<Self, String> {
        use Property::{Bool, Float};
        fade_in.validate(
            &["fade in time min", "fade in time max"],
            &[
                ("fade in time exponent", Float(1.0)),
                ("proportional 0/1", Bool(true)),
                ("fade in curve exponent", Float(0.0)),
            ],
        )?;
        let fade_in = fade_in.fade_duration("fade in time min", "fade in time max")?;
        if fade_in.min == fade_in.max || fade_out.min == fade_out.max {
            return Err("Particle random fades require nonconstant durations".into());
        }
        Ok(Self { fade_in, fade_out })
    }
}

struct Timeline {
    particles: Vec<Particle>,
    random: Random,
    time: f32,
    births: f32,
    emitted: u32,
    spawned: u32,
}

struct Random {
    seed: u32,
    query: u32,
}

impl Random {
    fn at(&self, relative: u32) -> f32 {
        Self::value(self.seed.wrapping_add(self.query).wrapping_add(relative))
    }

    fn for_operator(&self, sequence: u32, offset: u32) -> f32 {
        let particle_id = self.seed.wrapping_add(sequence);
        Self::value(self.seed.wrapping_add(particle_id).wrapping_add(offset))
    }

    fn value(index: u32) -> f32 {
        const BINS: u32 = 4096;
        const MASK: u32 = BINS - 1;
        let mut index = index & MASK;
        index = (index ^ (index >> 5)).wrapping_mul(0xaad) & MASK;
        index = (index ^ (index >> 7)).wrapping_mul(0xd97) & MASK;
        let bin = index ^ (index >> 5);
        let millionths = (bin * 1_000_000 + BINS / 2) / BINS;
        (f64::from(millionths) / 1_000_000.0) as f32
    }

    fn next(&mut self) -> f32 {
        let value = self.at(0);
        self.query = self.query.wrapping_add(1);
        value
    }

    fn sphere(&mut self, distance: Bounds, positive_z: bool) -> [f32; 3] {
        let polar = (1.0 - 2.0 * (self.at(0) * 0.9999 + 0.0001)).acos();
        let azimuth = (f64::from(self.at(1) * 0.9999 + 0.0001) * std::f64::consts::TAU) as f32;
        let radius = (self.at(2) * 0.9999 + 0.0001).cbrt();
        self.query = self.query.wrapping_add(1);
        let (sin_polar, cos_polar) = polar.sin_cos();
        let (sin_azimuth, cos_azimuth) = azimuth.sin_cos();
        let xy = radius * sin_polar;
        let [x, y, mut z] = [xy * cos_azimuth, xy * sin_azimuth, radius * cos_polar];
        if positive_z {
            z = z.abs();
        }
        let length_squared = (z * z + 1e-10) + x * x + y * y;
        let direction_scale = native_math::inverse_sqrt_refined(length_squared);
        let distance = distance.at(radius);
        self.query = self.query.wrapping_add(3);
        [
            x * direction_scale * distance,
            y * direction_scale * distance,
            distance * z * direction_scale,
        ]
    }
}

fn sphere(position: Fields<'_>) -> Result<(Bounds, bool), String> {
    use Property::{Bool, Float, Int, Vector};

    position.validate(
        &[
            "distance_min",
            "distance_max",
            "distance_bias_absolute_value",
        ],
        &[
            ("control_point_number", Int(0)),
            ("create in model", Int(0)),
            ("bias in local system", Bool(false)),
            ("distance_bias", Vector([1.0; 3])),
            (
                "randomly distribute to highest supplied control point",
                Bool(false),
            ),
            ("randomly distribution growth time", Float(0.0)),
            ("speed_min", Float(0.0)),
            ("speed_max", Float(0.0)),
            ("speed_random_exponent", Float(1.0)),
            ("speed_in_local_coordinate_system_min", Vector([0.0; 3])),
            ("speed_in_local_coordinate_system_max", Vector([0.0; 3])),
        ],
    )?;
    let positive_z = match position.values.get("distance_bias_absolute_value") {
        None | Some(Vector([0.0, 0.0, 0.0])) => false,
        Some(Vector([0.0, 0.0, 1.0])) => true,
        _ => return Err("Position Within Sphere Random supports a Z-only absolute bias".into()),
    };
    let distance = Bounds::new(
        position.float("distance_min", 0.0..=128.0, 0.0)?,
        position.float("distance_max", 0.0..=128.0, 0.0)?,
        position.label,
    )?;
    Ok((distance, positive_z))
}

fn operators<'a, const N: usize, const T: usize>(
    program: &'a Program,
    group: &str,
    names: [&str; N],
    optional: [&str; T],
) -> Result<([Fields<'a>; N], [Option<Fields<'a>>; T]), String> {
    let entries = program
        .functions
        .get(group)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let (entries, trailing) = entries
        .split_at_checked(N)
        .filter(|(_, trailing)| trailing.len() <= T)
        .ok_or_else(|| format!("Particle preview has an unsupported number of {group}"))?;
    let entries: &[Operator; N] = entries.try_into().expect("split at base operator count");
    if entries
        .iter()
        .zip(names)
        .any(|(entry, name)| entry.name != name)
    {
        return Err(format!(
            "Particle preview requires {group} in this order: {}",
            names.join(", ")
        ));
    }
    if trailing
        .iter()
        .zip(optional)
        .any(|(entry, name)| entry.name != name)
    {
        return Err(format!("Particle preview has unsupported trailing {group}"));
    }
    Ok((
        entries.each_ref().map(Fields::operator),
        std::array::from_fn(|index| trailing.get(index).map(Fields::operator)),
    ))
}

const FADE_PARAMETERS: &[&str] = &[
    "operator end fadein",
    "operator end fadeout",
    "operator fade oscillate",
    "operator start fadein",
    "operator start fadeout",
];

struct Fields<'a> {
    label: &'a str,
    values: &'a Properties,
    operator: bool,
}

impl<'a> Fields<'a> {
    fn definition(program: &'a Program) -> Self {
        Self {
            label: &program.name,
            values: &program.properties,
            operator: false,
        }
    }

    fn operator(operator: &'a Operator) -> Self {
        Self {
            label: &operator.name,
            values: &operator.properties,
            operator: true,
        }
    }

    fn require(&self, key: &str) -> Result<(), String> {
        if !self.values.contains_key(key) {
            return Err(format!("{} requires an explicit {key}", self.label));
        }
        Ok(())
    }

    fn fade_duration(&self, min: &str, max: &str) -> Result<Bounds, String> {
        self.require(min)?;
        self.require(max)?;
        Bounds::new(
            self.float(min, 0.01..=1.0, 0.25)?,
            self.float(max, 0.01..=1.0, 0.25)?,
            self.label,
        )
    }

    fn rgb(&self, key: &str) -> Result<[f32; 3], String> {
        let Some(Property::Color([red, green, blue, 255])) = self.values.get(key) else {
            return Err(format!("{} requires an opaque {key} color", self.label));
        };
        Ok([*red, *green, *blue].map(|byte| f32::from(byte) * NORMALIZE_BYTE))
    }

    fn validate(&self, variable: &[&str], fixed: &[(&str, Property)]) -> Result<(), String> {
        for (key, value) in self.values {
            if let Some((_, expected)) = fixed.iter().find(|(name, _)| name == &key.as_str()) {
                if same_property(value, expected) {
                    continue;
                }
            } else if variable.contains(&key.as_str())
                || self.operator
                    && FADE_PARAMETERS.contains(&key.as_str())
                    && matches!(value, Property::Float(0.0))
            {
                continue;
            }
            return Err(format!("{} has unsupported parameter {key}", self.label));
        }
        Ok(())
    }

    fn float(
        &self,
        key: &str,
        range: std::ops::RangeInclusive<f32>,
        default: f32,
    ) -> Result<f32, String> {
        let value = match self.values.get(key) {
            Some(Property::Float(value)) => *value,
            None => default,
            _ => return Err(format!("{} requires a float {key}", self.label)),
        };
        if !range.contains(&value) || value.is_subnormal() {
            return Err(format!(
                "{} requires {key} in {}..={}",
                self.label,
                range.start(),
                range.end()
            ));
        }
        Ok(value)
    }

    fn integer(
        &self,
        key: &str,
        range: std::ops::RangeInclusive<i32>,
        default: i32,
    ) -> Result<i32, String> {
        let value = match self.values.get(key) {
            Some(Property::Int(value)) => *value,
            None => default,
            _ => return Err(format!("{} requires an integer {key}", self.label)),
        };
        if !range.contains(&value) {
            return Err(format!(
                "{} requires {key} in {}..={}",
                self.label,
                range.start(),
                range.end()
            ));
        }
        Ok(value)
    }

    fn material(&self) -> Result<String, String> {
        let Some(Property::Text(name)) = self.values.get("material") else {
            return Err("Particle preview requires an explicit mounted material".into());
        };
        let normalized =
            asset_relative(name).map_err(|error| format!("Particle material: {error}"))?;
        let stem = normalized.strip_prefix("materials/").unwrap_or(&normalized);
        let stem = stem.strip_suffix(".vmt").unwrap_or(stem);
        if stem.is_empty()
            || normalized.len() > 240
            || !stem
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-'))
        {
            return Err("Particle preview requires a canonical mounted material path".into());
        }
        Ok(material_path(&normalized))
    }
}

fn same_property(actual: &Property, expected: &Property) -> bool {
    match (actual, expected) {
        (Property::Float(a), Property::Float(b)) => a == b,
        (Property::Int(a), Property::Int(b)) => a == b,
        (Property::Bool(a), Property::Bool(b)) => a == b,
        (Property::Text(a), Property::Text(b)) => a == b,
        (Property::Color(a), Property::Color(b)) => a == b,
        (Property::Vector(a), Property::Vector(b)) => a == b,
        _ => false,
    }
}

mod native_math {
    #[cfg(target_arch = "x86_64")]
    pub(super) fn reciprocal(value: f32) -> f32 {
        use core::arch::x86_64::{_mm_cvtss_f32, _mm_rcp_ss, _mm_set_ss};
        // SSE is guaranteed by x86-64, and each intrinsic operates on a scalar lane.
        unsafe { _mm_cvtss_f32(_mm_rcp_ss(_mm_set_ss(value))) }
    }

    #[cfg(not(target_arch = "x86_64"))]
    pub(super) fn reciprocal(_: f32) -> f32 {
        unreachable!("Particle preview requires x86-64");
    }

    pub(super) fn reciprocal_refined(value: f32) -> f32 {
        let estimate = reciprocal(value);
        (estimate + estimate) - ((value * estimate) * estimate)
    }

    pub(super) fn bias_parameter(value: f32) -> f32 {
        let estimate = reciprocal(value);
        (estimate + estimate) - ((value * estimate) * estimate + 2.0)
    }

    #[cfg(target_arch = "x86_64")]
    pub(super) fn inverse_sqrt_refined(value: f32) -> f32 {
        use core::arch::x86_64::{_mm_cvtss_f32, _mm_rsqrt_ss, _mm_set_ss};
        // SSE is guaranteed by x86-64, and each intrinsic operates on a scalar lane.
        let estimate = unsafe { _mm_cvtss_f32(_mm_rsqrt_ss(_mm_set_ss(value))) };
        (estimate * (3.0 - estimate * estimate * value)) * 0.5
    }

    #[cfg(not(target_arch = "x86_64"))]
    pub(super) fn inverse_sqrt_refined(_: f32) -> f32 {
        unreachable!("Particle preview requires x86-64");
    }
}
