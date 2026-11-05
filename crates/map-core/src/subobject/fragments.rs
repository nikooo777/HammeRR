use super::*;
use source_formats::vmf::{Properties, Side, UvAxis};

pub struct BoundaryTriangle {
    pub vertices: [usize; 3],
    pub template: Side,
    pub source_side: u32,
    pub tag: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FragmentFace {
    pub side: u32,
    pub source_side: u32,
    pub exterior: bool,
    pub tag: Option<u32>,
}

pub struct Tetrahedron {
    pub solid: Solid,
    pub faces: Vec<FragmentFace>,
}

pub(super) fn upper_distance(plane: Plane, point: DVec3) -> f64 {
    let products = (plane.normal * point).map(f64::next_up);
    (((products.x + products.y).next_up() + products.z).next_up() - plane.dist).next_up()
}

fn internal_side(points: [DVec3; 3], id: u32) -> Result<Side, EditError> {
    let plane = Plane::from_points(points)
        .ok_or_else(|| invalid("Fragment has a collapsed internal face"))?;
    let (u, v) = crate::edit::world_texture_axes(plane.normal);
    Ok(Side {
        id,
        plane: source_formats::vmf::Plane(points),
        material: "TOOLS/TOOLSNODRAW".into(),
        u_axis: UvAxis {
            axis: u,
            offset: 0.0,
            scale: crate::edit::DEFAULT_TEXTURE_SCALE,
        },
        v_axis: UvAxis {
            axis: v,
            offset: 0.0,
            scale: crate::edit::DEFAULT_TEXTURE_SCALE,
        },
        rotation: 0.0,
        lightmap_scale: crate::edit::DEFAULT_LIGHTMAP_SCALE,
        smoothing_groups: Some(0),
        displacement: None,
        extra_properties: Properties::default(),
        extra: Vec::new(),
    })
}

pub fn tetrahedralize(
    source: &Solid,
    points: &[DVec3],
    triangles: Vec<BoundaryTriangle>,
    center: DVec3,
    limit: usize,
) -> Result<Vec<Tetrahedron>, EditError> {
    if triangles.is_empty()
        || triangles.len() > limit.min(crate::carve::MAX_FRAGMENTS)
        || points.len() > crate::carve::MAX_FRAGMENTS * 3
        || !center.is_finite()
        || center.abs().max_element() > MAX_COORDINATE
        || points
            .iter()
            .any(|point| !point.is_finite() || point.abs().max_element() > MAX_COORDINATE)
    {
        return Err(invalid(
            "Convex fragments exceed their geometry or resource bounds",
        ));
    }
    let mut edges = BTreeMap::<[usize; 2], Vec<[usize; 2]>>::new();
    for triangle in &triangles {
        if triangle.vertices.iter().any(|&index| index >= points.len())
            || triangle.vertices.into_iter().collect::<BTreeSet<_>>().len() != 3
        {
            return Err(invalid("Fragment exterior has invalid triangle vertices"));
        }
        for index in 0..3 {
            let edge = [triangle.vertices[index], triangle.vertices[(index + 1) % 3]];
            let mut key = edge;
            key.sort_unstable();
            edges.entry(key).or_default().push(edge);
        }
    }
    if edges
        .values()
        .any(|uses| uses.len() != 2 || uses[0] != [uses[1][1], uses[1][0]])
    {
        return Err(invalid(
            "Fragment exterior is not an exact closed oriented boundary",
        ));
    }
    let center_id = points.len();
    let mut vertices = points.to_vec();
    vertices.push(center);
    let mut internals = BTreeMap::<[usize; 3], Vec<[usize; 3]>>::new();
    let mut result = Vec::with_capacity(triangles.len());
    for triangle in triangles {
        let [a, b, c] = triangle.vertices;
        let mut template = triangle.template;
        template
            .clear_precise_vertices()
            .map_err(|error| invalid(error.to_string()))?;
        template.id = 1;
        template.plane.0 = [vertices[a], vertices[b], vertices[c]];
        let exterior = Plane::from_points(template.plane.0)
            .ok_or_else(|| invalid("Fragment exterior triangle is collapsed"))?;
        if upper_distance(exterior, center) >= 0.0 {
            return Err(invalid(
                "Fragment center is not strictly inside its exterior boundary",
            ));
        }
        let mut sides = vec![template];
        let mut faces = vec![FragmentFace {
            side: 1,
            source_side: triangle.source_side,
            exterior: true,
            tag: triangle.tag,
        }];
        for (mut indices, opposite) in [
            ([a, b, center_id], c),
            ([b, c, center_id], a),
            ([c, a, center_id], b),
        ] {
            indices.sort_unstable();
            let key = indices;
            let mut plane = Plane::from_points(indices.map(|index| vertices[index]))
                .ok_or_else(|| invalid("Fragment has a collapsed internal face"))?;
            if upper_distance(plane, vertices[opposite]) >= 0.0 {
                indices.swap(0, 2);
                plane = Plane::from_points(indices.map(|index| vertices[index])).unwrap();
            }
            if upper_distance(plane, vertices[opposite]) >= 0.0 {
                return Err(invalid("Fragment has no certified positive interior"));
            }
            internals.entry(key).or_default().push(indices);
            let id = sides.len() as u32 + 1;
            sides.push(internal_side(indices.map(|index| vertices[index]), id)?);
            faces.push(FragmentFace {
                side: id,
                source_side: 0,
                exterior: false,
                tag: None,
            });
        }
        for side in &mut sides {
            let vertices = side.plane.0;
            side.set_precise_vertices(&vertices)
                .map_err(|error| invalid(error.to_string()))?;
        }
        let mut solid = source.clone();
        solid.sides = sides;
        crate::brush::canonicalize_precise(&mut solid)?;
        let shape = topology(&solid)?;
        if shape.vertices.len() != 4 || shape.faces.len() != 4 {
            return Err(invalid("Fragment is not a positive closed tetrahedron"));
        }
        result.push(Tetrahedron { solid, faces });
    }
    if internals
        .values()
        .any(|uses| uses.len() != 2 || uses[0] != [uses[1][2], uses[1][1], uses[1][0]])
    {
        return Err(invalid(
            "Fragments do not have exact opposite shared internal faces",
        ));
    }
    Ok(result)
}

pub fn install(
    tx: &mut Transaction,
    fragments: impl IntoIterator<Item = (u32, ObjectId, Solid)>,
) -> Result<(), EditError> {
    let ordering = crate::instance_order::read(tx.vmf()).map_err(invalid)?;
    let mut pieces = BTreeMap::<u32, (ObjectId, Vec<Solid>)>::new();
    let mut identities = BTreeSet::new();
    for (source, owner, solid) in fragments {
        if identities.len() >= crate::carve::MAX_FRAGMENTS || !identities.insert(solid.id) {
            return Err(invalid(
                "Convex fragments exceed their count or have duplicate identities",
            ));
        }
        let (expected_owner, _) = super::solid(tx.document(), source)?;
        if owner != expected_owner {
            return Err(invalid("Convex fragment has inconsistent source ownership"));
        }
        pieces
            .entry(source)
            .or_insert_with(|| (owner, Vec::new()))
            .1
            .push(solid);
    }
    if pieces.is_empty() {
        return Err(invalid("Convex fragment installation requires a source"));
    }
    let mut replacements = BTreeMap::new();
    for (&source, (owner, solids)) in &pieces {
        if solids[0].id != source {
            return Err(invalid(
                "Convex fragments did not retain their source brush identity",
            ));
        }
        match owner {
            ObjectId::Solid(_) => {
                let index = tx
                    .vmf()
                    .world
                    .solids
                    .iter()
                    .position(|solid| solid.id == source)
                    .ok_or(EditError::NotFound(ObjectId::Solid(source)))?;
                *tx.solid_mut(source)? = solids[0].clone();
                for (offset, solid) in solids.iter().enumerate().skip(1) {
                    tx.insert_solid(index + offset, solid.clone())?;
                }
                replacements.insert(
                    source,
                    solids.iter().map(|solid| solid.id).collect::<Vec<_>>(),
                );
            }
            ObjectId::Entity(owner) => {
                let entity = tx.entity_mut(*owner)?;
                let index = entity
                    .solids
                    .iter()
                    .position(|solid| solid.id == source)
                    .ok_or(EditError::NotFound(ObjectId::Solid(source)))?;
                entity.solids.splice(index..=index, solids.iter().cloned());
            }
        }
    }
    if let Some(mut ordering) = ordering {
        let mut changed = false;
        for slot in &mut ordering.slots {
            let crate::instance_order::Content::Materialized { world, .. } = &mut slot.content
            else {
                continue;
            };
            let mut count = 0usize;
            for id in world.iter() {
                count = count
                    .checked_add(replacements.get(id).map_or(1, Vec::len))
                    .filter(|count| *count <= crate::instance_order::MAX_IDS)
                    .ok_or_else(|| {
                        invalid("Convex fragments exceed instance ordering's referenced-ID limit")
                    })?;
            }
            let mut expanded = Vec::with_capacity(count);
            for id in world.iter() {
                expanded.extend_from_slice(
                    replacements
                        .get(id)
                        .map_or(std::slice::from_ref(id), Vec::as_slice),
                );
            }
            if expanded != *world {
                *world = expanded;
                changed = true;
            }
        }
        if changed {
            crate::instance_order::install(tx, &ordering)?;
        }
    }
    Ok(())
}

pub fn admit_source(source: &Solid) -> Result<(), EditError> {
    if !source.extra.is_empty()
        || !source.extra_properties.0.is_empty()
        || source.sides.iter().any(|side| {
            !side.extra_properties.0.is_empty()
                || side
                    .extra
                    .iter()
                    .any(|node| !source_formats::vmf::is_precise_vertices_key(&node.key))
        })
    {
        return Err(invalid(format!(
            "Brush {}: opaque geometry metadata cannot be remapped to convex fragments",
            source.id
        )));
    }
    crate::brush::precise::ensure_editable(source)?;
    Ok(())
}
