use std::collections::{HashMap, HashSet};

use glam::DVec3;
use source_formats::bsp::{Visibility as NativeVisibility, VisibilityKind};

use super::bake::{Budget, Emitter};
use crate::{map::Map, tree::Tree, visibility::Visibility};

const MAX_ROW_BYTES: usize = 16 * 1024 * 1024;
const MAX_CANDIDATES: usize = 4 * 1024 * 1024;

#[derive(Debug, Default)]
pub(crate) struct Stats {
    pub evaluated: usize,
    pub pvs_skipped: usize,
    pub range_skipped: usize,
    pub work: usize,
}

pub(super) struct Candidates {
    all: Vec<usize>,
    rows: HashMap<usize, Vec<u8>>,
    cached: HashMap<usize, Vec<usize>>,
    entries: usize,
    enabled: bool,
    pub stats: Stats,
}

impl Candidates {
    pub fn new(
        lights: &[Emitter],
        visibility: &Visibility,
        budget: &mut Budget<'_>,
    ) -> Result<Self, String> {
        let mut index = Self {
            all: (0..lights.len()).collect(),
            rows: HashMap::new(),
            cached: HashMap::new(),
            entries: 0,
            enabled: enabled(),
            stats: Stats::default(),
        };
        if !index.enabled || visibility.data.is_empty() || lights.is_empty() {
            return Ok(index);
        }
        index.consume(visibility.count * 2 + lights.len(), budget)?;
        let native =
            NativeVisibility::parse(&visibility.data).map_err(|error| error.to_string())?;
        if native.clusters() != visibility.count {
            return Err("Lighting visibility cluster count disagrees with the BSP tree".into());
        }
        let width = visibility.count.div_ceil(8);
        let mut decoded = HashSet::new();
        for light in lights {
            let Emitter::Point(light) = light else {
                continue;
            };
            let Ok(cluster) = usize::try_from(light.cluster) else {
                continue;
            };
            if !decoded.insert(cluster) {
                continue;
            }
            if (index.rows.len() + 1) * width > MAX_ROW_BYTES {
                continue;
            }
            index.consume(width * 3, budget)?;
            let row = native
                .row(cluster, VisibilityKind::Visible)
                .map_err(|error| error.to_string())?;
            if row.iter().enumerate().any(|(index, &byte)| {
                let bits = (visibility.count - index * 8).min(8);
                byte != ((1_u16 << bits) - 1) as u8
            }) {
                index.rows.insert(cluster, row);
            }
        }
        Ok(index)
    }

    fn consume(&mut self, work: usize, budget: &mut Budget<'_>) -> Result<(), String> {
        budget.consume(work)?;
        self.stats.work += work;
        Ok(())
    }

    pub fn cluster(
        &mut self,
        point: DVec3,
        map: &Map,
        tree: &Tree,
        visibility: &Visibility,
        budget: &mut Budget<'_>,
    ) -> Result<Option<usize>, String> {
        if self.rows.is_empty() {
            return Ok(None);
        }
        let mut index = 0;
        while let Some((plane, children)) = tree.nodes[index].split {
            self.consume(1, budget)?;
            let distance = map.planes.get(plane).distance(point);
            if distance.abs() <= 1e-6 {
                return Ok(None);
            }
            index = children[usize::from(distance < 0.0)];
        }
        Ok(visibility.clusters[index])
    }

    pub fn for_cluster(
        &mut self,
        cluster: Option<usize>,
        lights: &[Emitter],
        budget: &mut Budget<'_>,
    ) -> Result<&[usize], String> {
        let Some(cluster) = cluster.filter(|_| !self.rows.is_empty()) else {
            return Ok(&self.all);
        };
        if !self.cached.contains_key(&cluster) {
            if self.entries + lights.len() > MAX_CANDIDATES {
                return Ok(&self.all);
            }
            self.consume(lights.len(), budget)?;
            let eligible = lights
                .iter()
                .enumerate()
                .filter_map(|(index, light)| {
                    let visible = match light {
                        Emitter::Sky(_) => true,
                        Emitter::Point(light) => usize::try_from(light.cluster)
                            .ok()
                            .and_then(|source| self.rows.get(&source))
                            .and_then(|row| row.get(cluster / 8))
                            .is_none_or(|byte| byte & (1 << (cluster % 8)) != 0),
                    };
                    visible.then_some(index)
                })
                .collect::<Vec<_>>();
            self.entries += eligible.len();
            self.cached.insert(cluster, eligible);
        }
        let eligible = &self.cached[&cluster];
        self.stats.pvs_skipped += lights.len() - eligible.len();
        Ok(eligible)
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }
}

pub(super) fn outside_range(light: &Emitter, point: DVec3) -> bool {
    matches!(light, Emitter::Point(light) if light.falloff.outside_range(light.origin.distance(point)))
}

#[cfg(not(test))]
fn enabled() -> bool {
    true
}

#[cfg(test)]
thread_local! {
    static ENABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

#[cfg(test)]
fn enabled() -> bool {
    ENABLED.get()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_common as common;
    use crate::{Control, LightingMode, Options, VisibilityMode};
    use map_core::{BrushGeometry, Document, ObjectId, edit};
    use source_formats::vmf::{DispInfo, Vmf};

    fn source(cluster: i32, properties: &[(&str, &str)]) -> Emitter {
        let entity = crate::map::MapEntity {
            id: 1,
            classname: "light".into(),
            origin: Some(DVec3::new(0.0, 0.0, 100.0)),
            properties: std::iter::once(("_light".into(), "255 255 255 200".into()))
                .chain(
                    properties
                        .iter()
                        .map(|(key, value)| (key.to_string(), value.to_string())),
                )
                .collect(),
            brushes: Vec::new(),
        };
        Emitter::Point(super::super::PointLight::from_entity(&entity, cluster).unwrap())
    }

    #[test]
    fn uses_source_rows_in_order_and_keeps_unknown_receivers_and_cache_fallbacks() {
        let lights = [source(0, &[]), source(1, &[]), source(-1, &[])];
        let control = Control::default();
        let mut budget = Budget::new(&control);
        let mut index = Candidates {
            all: vec![0, 1, 2],
            rows: HashMap::from([(0, vec![0b01]), (1, vec![0b11])]),
            cached: HashMap::new(),
            entries: 0,
            enabled: true,
            stats: Stats::default(),
        };
        assert_eq!(
            index.for_cluster(Some(1), &lights, &mut budget).unwrap(),
            [1, 2]
        );
        assert_eq!(
            index.for_cluster(Some(0), &lights, &mut budget).unwrap(),
            [0, 1, 2]
        );
        assert_eq!(
            index.for_cluster(None, &lights, &mut budget).unwrap(),
            [0, 1, 2]
        );
        index.cached.clear();
        index.entries = MAX_CANDIDATES;
        assert_eq!(
            index.for_cluster(Some(1), &lights, &mut budget).unwrap(),
            [0, 1, 2]
        );
        assert_eq!(index.stats.pvs_skipped, 1);
    }

    #[test]
    fn range_culling_matches_exact_falloff_endpoints_and_keeps_soft_tails() {
        let cases = [
            (
                vec![("_distance", "100")],
                vec![(99.999, false), (100.0, false), (100.001, true)],
            ),
            (
                vec![
                    ("_fifty_percent_distance", "100"),
                    ("_zero_percent_distance", "200"),
                    ("_hardfalloff", "1"),
                ],
                vec![(199.999, false), (200.0, true), (200.001, true)],
            ),
            (
                vec![
                    ("_fifty_percent_distance", "100"),
                    ("_zero_percent_distance", "200"),
                ],
                vec![(200.0, false), (400.0, false), (10000.0, false)],
            ),
        ];
        for (properties, distances) in cases {
            let Emitter::Point(light) = source(0, &properties) else {
                unreachable!()
            };
            for (distance, skipped) in distances {
                assert_eq!(light.falloff.outside_range(distance), skipped);
                if skipped {
                    assert_eq!(light.falloff.factor(distance).unwrap(), 0.0);
                } else {
                    assert!(light.falloff.factor(distance).unwrap() > 0.0);
                }
            }
        }
    }

    fn fixture(connected: bool, styled: bool, finite: bool) -> Vmf {
        let mut document = Document::new(common::room());
        document
            .edit("culling fixture", |tx| {
                if !connected {
                    edit::create_block(
                        tx,
                        DVec3::new(-16.0, -256.0, 0.0),
                        DVec3::new(16.0, 256.0, 256.0),
                        "dev/dev_measuregeneric01",
                    )?;
                } else {
                    for (mins, maxs) in [
                        ([-16.0, -256.0, 0.0], [16.0, -64.0, 256.0]),
                        ([-16.0, 64.0, 0.0], [16.0, 256.0, 256.0]),
                    ] {
                        edit::create_block(
                            tx,
                            DVec3::from_array(mins),
                            DVec3::from_array(maxs),
                            "dev/dev_measuregeneric01",
                        )?;
                    }
                }
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::new(64.0, 64.0, 16.0),
                    DVec3::new(112.0, 112.0, 64.0),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                edit::tie_to_entity(
                    tx,
                    &[id],
                    "func_brush",
                    &[("origin".into(), "90.5 91.25 30.125".into())],
                )?;
                let ObjectId::Solid(terrain) = edit::create_block(
                    tx,
                    DVec3::new(-224.0, 64.0, 0.0),
                    DVec3::new(-64.0, 224.0, 16.0),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                let solid = tx.solid_mut(terrain)?;
                let geometry = BrushGeometry::from_solid(solid).unwrap();
                let top = geometry
                    .faces
                    .iter()
                    .find(|face| face.plane.normal.z > 0.99)
                    .unwrap();
                solid.sides[top.side].displacement = Some(DispInfo {
                    power: 2,
                    start_position: top.vertices[0],
                    flags: Some(0),
                    elevation: 0.0,
                    subdivided: false,
                    normals: Some(vec![vec![DVec3::Z; 5]; 5]),
                    distances: Some(
                        (0..5)
                            .map(|row| (0..5).map(|col| (row * col) as f64).collect())
                            .collect(),
                    ),
                    offsets: None,
                    offset_normals: None,
                    alphas: None,
                    triangle_tags: None,
                    allowed_verts: None,
                    extra_properties: Default::default(),
                    extra: Vec::new(),
                });
                for (x, color) in [(-128.0, "255 0 0 200"), (128.0, "0 0 255 200")] {
                    let mut properties = vec![("_light".into(), color.into())];
                    if styled {
                        properties.push(("targetname".into(), format!("switch_{x}")));
                    }
                    if finite {
                        properties.push(("_distance".into(), "140".into()));
                    }
                    edit::create_entity(
                        tx,
                        "light",
                        Some(DVec3::new(x, 0.0, 128.0)),
                        &properties,
                        Vec::new(),
                    )?;
                }
                Ok(())
            })
            .unwrap();
        document.vmf().clone()
    }

    #[test]
    fn native_output_matches_unculled_reference_for_rooms_styles_and_fractional_models() {
        let mut removed = 0;
        let mut ranged = 0;
        for connected in [false, true] {
            for styled in [false, true] {
                for finite in [false, true] {
                    let vmf = fixture(connected, styled, finite);
                    for visibility in [VisibilityMode::Fast, VisibilityMode::Detailed] {
                        let options = Options {
                            lighting: LightingMode::DirectLdr,
                            visibility,
                            ..Options::default()
                        };
                        let result =
                            crate::compile_with_options(&vmf, &common::empty_game(), options);
                        assert!(!result.report.has_errors(), "{:?}", result.report);
                        ENABLED.set(false);
                        let reference =
                            crate::compile_with_options(&vmf, &common::empty_game(), options);
                        ENABLED.set(true);
                        assert!(!reference.report.has_errors(), "{:?}", reference.report);
                        assert!(
                            result.bsp.unwrap().to_bytes() == reference.bsp.unwrap().to_bytes(),
                            "Culling changed native output: connected={connected}, styled={styled}, finite={finite}, visibility={visibility:?}"
                        );
                        assert!(
                            result.stats.light_evaluations <= reference.stats.light_evaluations
                        );
                        removed += result.stats.light_pvs_skipped;
                        ranged += result.stats.light_range_skipped;
                    }
                }
            }
        }
        assert!(removed > 0);
        assert!(ranged > 0);
    }
}
