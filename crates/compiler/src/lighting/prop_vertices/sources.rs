use std::collections::BTreeMap;
use std::ops::Range;
use std::rc::Rc;

use glam::{DVec2, DVec3};
use source_formats::vhv::Bgra8;

use super::assets::SourceMaterial;
use super::{Allocation, BoundReceiver, Budget, ModelData};
use crate::lighting::props::{PropPose, PropPrimitive, PropTriangle};
use crate::lighting::shadows::{ShadowIndex, ShadowObject};
use crate::{Diagnostic, Report};

pub(in crate::lighting) struct SourceTriangle {
    pub primitive: PropPrimitive,
    pub triangle: PropTriangle,
    pub caster_slot: Option<usize>,
    pub reflectivity: DVec3,
    pub two_sided: bool,
    pub vertices: [u32; 3],
}

pub(super) struct Geometry {
    triangles: Vec<SourceTriangle>,
    _materials: Vec<Rc<SourceMaterial>>,
    _allocations: Vec<Allocation>,
    _allocation: Allocation,
}

struct SnapshotData {
    geometry: Rc<Geometry>,
    colors: Vec<Bgra8>,
    _allocation: Allocation,
}

#[derive(Clone)]
pub(in crate::lighting) struct SourceSnapshot {
    data: Rc<SnapshotData>,
}

impl SourceSnapshot {
    pub(in crate::lighting) fn triangles(&self) -> &[SourceTriangle] {
        &self.data.geometry.triangles
    }

    pub(in crate::lighting) fn colors(&self, triangle: &SourceTriangle) -> [Bgra8; 3] {
        triangle
            .vertices
            .map(|index| self.data.colors[index as usize])
    }

    pub(super) fn same_geometry(&self, geometry: &Rc<Geometry>) -> bool {
        Rc::ptr_eq(&self.data.geometry, geometry)
    }

    pub(super) fn all_colors(&self) -> &[Bgra8] {
        &self.data.colors
    }

    pub(super) fn buffer(
        vertices: usize,
        budget: &Budget<'_>,
    ) -> Result<(Vec<Bgra8>, Allocation), String> {
        let bytes = vertices
            .checked_mul(size_of::<Bgra8>())
            .and_then(|bytes| bytes.checked_add(size_of::<SnapshotData>() + 2 * size_of::<usize>()))
            .ok_or("Reflected prop snapshot allocation overflow")?;
        let allocation = budget.reserve_bytes(bytes)?;
        Ok((Vec::with_capacity(vertices), allocation))
    }

    pub(super) fn new(geometry: Rc<Geometry>, colors: Vec<Bgra8>, allocation: Allocation) -> Self {
        Self {
            data: Rc::new(SnapshotData {
                geometry,
                colors,
                _allocation: allocation,
            }),
        }
    }
}

pub(super) struct UsedModel {
    pub(super) originals: Vec<u32>,
    pub(super) hardware: Vec<u32>,
}

pub(super) struct Plan {
    pub(super) geometry: Rc<Geometry>,
    pub(super) models: Vec<UsedModel>,
    pub(super) ranges: Vec<Range<usize>>,
    pub(super) vertices: usize,
    pub(super) source_props: usize,
    pub(super) source_vertices: usize,
    _allocation: Allocation,
}

impl Plan {
    pub(super) fn triangle_count(&self) -> usize {
        self.geometry.triangles.len()
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn prepare(
    models: &[ModelData],
    receivers: &[BoundReceiver],
    materials: BTreeMap<(usize, usize), Rc<SourceMaterial>>,
    shadows: &ShadowIndex<PropTriangle>,
    allocations: Vec<Allocation>,
    report: &mut Report,
    budget: &mut Budget<'_>,
) -> Result<Plan, String> {
    let mapping_bytes = models.iter().try_fold(
        models.len() * size_of::<UsedModel>() + receivers.len() * size_of::<Range<usize>>(),
        |bytes, data| {
            bytes
                .checked_add(data.layout.vertices.len() * 2 * size_of::<u32>())
                .ok_or("Reflected prop vertex mapping allocation overflow")
        },
    )?;
    let mapping_allocation = budget.reserve_bytes(mapping_bytes)?;
    let mut mappings = Vec::with_capacity(models.len());
    for data in models {
        let count = data.layout.vertices.len();
        let levels = count.checked_ilog2().unwrap_or(0) as usize + 1;
        budget.consume(count * (2 * levels + 1))?;
        let mut originals = data.layout.vertices.clone();
        originals.sort_unstable();
        originals.dedup();
        let mut hardware = Vec::with_capacity(count);
        for original in &data.layout.vertices {
            let index = originals
                .binary_search(original)
                .map_err(|_| "Reflected prop hardware vertex has no original mapping")?;
            hardware.push(index as u32);
        }
        for part in data.model.parts_for_body(0) {
            budget.consume(part.indices.len() * levels)?;
            if part
                .indices
                .iter()
                .any(|index| originals.binary_search(index).is_err())
            {
                return Err(
                    "Reflected root-LOD source vertex has no generated hardware color".into(),
                );
            }
        }
        mappings.push(UsedModel {
            originals,
            hardware,
        });
    }
    let mut ranges = Vec::with_capacity(receivers.len());
    let mut vertices = 0usize;
    let mut triangle_count = 0usize;
    for receiver in receivers {
        budget.consume(1)?;
        let first = vertices;
        vertices = vertices
            .checked_add(mappings[receiver.model].originals.len())
            .filter(|count| *count <= super::MAX_VERTICES)
            .ok_or("Reflected prop compact vertices exceed the hardware vertex limit")?;
        ranges.push(first..vertices);
        for part in models[receiver.model].model.parts_for_body(0) {
            triangle_count = triangle_count
                .checked_add(part.indices.len() / 3)
                .filter(|count| *count <= PropTriangle::MAX_OBJECTS)
                .ok_or("Reflected static props exceed the root-LOD triangle limit")?;
        }
    }
    let geometry_bytes = triangle_count
        .checked_mul(size_of::<SourceTriangle>())
        .and_then(|bytes| bytes.checked_add(materials.len() * size_of::<Rc<SourceMaterial>>()))
        .and_then(|bytes| bytes.checked_add(allocations.capacity() * size_of::<Allocation>()))
        .and_then(|bytes| bytes.checked_add(size_of::<Geometry>() + 2 * size_of::<usize>()))
        .ok_or("Reflected prop source geometry allocation overflow")?;
    let geometry_allocation = budget.reserve_bytes(geometry_bytes)?;
    let _lookup_allocation = budget.reserve::<(PropPrimitive, usize)>(shadows.objects().len())?;
    let levels = shadows.objects().len().checked_ilog2().unwrap_or(0) as usize + 1;
    budget.consume(shadows.objects().len() * (levels + 1))?;
    let mut lookup = shadows
        .objects()
        .iter()
        .enumerate()
        .map(|(slot, triangle)| (triangle.primitive(), slot))
        .collect::<Vec<_>>();
    lookup.sort_unstable_by_key(|entry| entry.0);
    if lookup.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err("Reflected prop caster primitive identities are ambiguous".into());
    }
    let mut triangles = Vec::with_capacity(triangle_count);
    let mut source_props = 0usize;
    let mut source_vertices = 0usize;
    for (receiver_index, receiver) in receivers.iter().enumerate() {
        let data = &models[receiver.model];
        let used = &mappings[receiver.model];
        let _scratch = budget.reserve::<bool>(used.originals.len())?;
        budget.consume(used.originals.len() + 32)?;
        let mut root_vertices = vec![false; used.originals.len()];
        let before = triangles.len();
        let result = (|| {
            let skin = data
                .model
                .skins
                .get(receiver.prop.skin as usize)
                .ok_or("Reflected prop selected skin is unavailable")?;
            let pose = PropPose::new(&receiver.prop);
            let mut ordinal = 0usize;
            for part in data.model.parts_for_body(0) {
                let selected = *skin
                    .get(part.material)
                    .ok_or("Reflected prop source skin binding is unavailable")?;
                let material = materials
                    .get(&(receiver.model, selected))
                    .ok_or("Reflected prop source has no checked selected material")?;
                for indices in part.indices.as_chunks::<3>().0 {
                    budget.consume(
                        48 + levels
                            + 3 * (used.originals.len().checked_ilog2().unwrap_or(0) as usize + 1),
                    )?;
                    let primitive = PropPrimitive {
                        owner: receiver.metadata.ordinal,
                        triangle: ordinal,
                    };
                    ordinal += 1;
                    let found = lookup
                        .binary_search_by_key(&primitive, |entry| entry.0)
                        .ok()
                        .map(|index| lookup[index].1);
                    let mut triangle = if receiver.prop.flags & crate::lighting::props::NO_SHADOW
                        == 0
                    {
                        if let Some(slot) = found {
                            shadows.objects()[slot].clone()
                        } else {
                            let points = pose.points(
                                indices.map(|index| data.model.vertices[index as usize].position),
                            )?;
                            if (points[1] - points[0])
                                .cross(points[2] - points[0])
                                .length_squared()
                                == 0.0
                            {
                                continue;
                            }
                            return Err(
                                "Reflected casting prop primitive has no actual shadow triangle"
                                    .into(),
                            );
                        }
                    } else {
                        if found.is_some() {
                            return Err(
                                "Reflected noncasting prop unexpectedly has a shadow primitive"
                                    .into(),
                            );
                        }
                        let points = pose.points(
                            indices.map(|index| data.model.vertices[index as usize].position),
                        )?;
                        if (points[1] - points[0])
                            .cross(points[2] - points[0])
                            .length_squared()
                            == 0.0
                        {
                            continue;
                        }
                        PropTriangle::new(points, primitive).map_err(|error| {
                            format!("{} ({}): {error}", material.name, material.base)
                        })?
                    };
                    let coordinates = indices.map(|index| {
                        DVec2::from_array(data.model.vertices[index as usize].uv.map(f64::from))
                    });
                    triangle.set_alpha(material.alpha.clone(), coordinates);
                    let vertices = indices.map(|index| {
                        used.originals
                            .binary_search(&index)
                            .map(|compact| {
                                root_vertices[compact] = true;
                                (ranges[receiver_index].start + compact) as u32
                            })
                            .map_err(|_| {
                                "Reflected prop source lost its compact vertex mapping".to_string()
                            })
                    });
                    let [a, b, c] = vertices;
                    triangles.push(SourceTriangle {
                        primitive,
                        triangle,
                        caster_slot: found,
                        reflectivity: material.reflectivity,
                        two_sided: material.two_sided,
                        vertices: [a?, b?, c?],
                    });
                }
            }
            Ok::<(), String>(())
        })();
        if let Err(message) = result {
            report.diagnostics.push(Diagnostic::error(
                message,
                Some(format!("entity:{}", receiver.source)),
                Some(DVec3::from_array(receiver.prop.origin.map(f64::from))),
            ));
            return Err("Static-prop reflected source preparation failed".into());
        }
        if triangles.len() != before {
            source_props += 1;
            source_vertices += root_vertices.into_iter().filter(|used| *used).count();
        }
    }
    if triangles
        .windows(2)
        .any(|pair| pair[0].primitive >= pair[1].primitive)
    {
        return Err("Reflected prop source primitive order is not unique".into());
    }
    Ok(Plan {
        geometry: Rc::new(Geometry {
            triangles,
            _materials: materials.into_values().collect(),
            _allocations: allocations,
            _allocation: geometry_allocation,
        }),
        models: mappings,
        ranges,
        vertices,
        source_props,
        source_vertices,
        _allocation: mapping_allocation,
    })
}
