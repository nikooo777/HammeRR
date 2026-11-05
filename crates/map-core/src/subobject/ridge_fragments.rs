use super::fragments::{self, upper_distance};
use super::*;
use crate::surface::Projection;
use glam::DVec2;
use source_formats::vmf::Side;

pub const MAX_RIDGE_FRAGMENTS: usize = crate::carve::MAX_FRAGMENTS;

#[derive(Clone, Debug, PartialEq)]
pub struct CreaseFragmentFace {
    pub side: u32,
    pub source_side: u32,
    pub exterior: bool,
    pub ridge_side: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CreaseFragment {
    pub source: u32,
    pub owner: ObjectId,
    pub solid: Solid,
    pub faces: Vec<CreaseFragmentFace>,
}

struct Boundary {
    solid: Solid,
    points: Vec<DVec3>,
    anchors: Vec<DVec3>,
    rings: Vec<Vec<usize>>,
}

struct EdgeUse {
    vertices: [usize; 2],
    ridge_side: u32,
    uv: [DVec2; 2],
}

struct Triangle {
    vertices: [usize; 3],
    template: Side,
    source_side: u32,
}

fn projection(
    side: &Side,
    vertices: [usize; 3],
    before: &[DVec3],
    after: &[DVec3],
    attached: bool,
) -> Result<Projection, EditError> {
    Plane::from_points(vertices.map(|index| after[index]))
        .ok_or_else(|| invalid("Ridge triangulation has a collapsed exterior triangle"))?;
    if attached {
        if [side.u_axis, side.v_axis].iter().any(|axis| {
            !axis.axis.is_finite()
                || !axis.offset.is_finite()
                || !axis.scale.is_finite()
                || axis.scale == 0.0
        }) {
            return Err(invalid(
                "Ridge attachment requires finite nonzero texture scales",
            ));
        }
        crate::texture_deform::attach(
            side,
            &vertices.map(|index| before[index]),
            &vertices.map(|index| after[index]),
        )
    } else {
        Ok(Projection {
            u: side.u_axis,
            v: side.v_axis,
            rotation: side.rotation,
        })
    }
}

fn triangles(
    ring: &[usize],
    points: &mut Vec<DVec3>,
    anchors: &mut Vec<DVec3>,
    side: &Side,
    attached: bool,
    source_side: u32,
) -> Result<Vec<Triangle>, EditError> {
    for start in 0..ring.len() {
        let candidate = (1..ring.len() - 1)
            .map(|index| {
                [
                    ring[start],
                    ring[(start + index) % ring.len()],
                    ring[(start + index + 1) % ring.len()],
                ]
            })
            .collect::<Vec<_>>();
        let projections = candidate
            .iter()
            .map(|&triangle| projection(side, triangle, anchors, points, attached))
            .collect::<Result<Vec<_>, _>>();
        if let Ok(projections) = projections {
            return Ok(candidate
                .into_iter()
                .zip(projections)
                .map(|(vertices, projection)| {
                    let mut template = side.clone();
                    template.u_axis = projection.u;
                    template.v_axis = projection.v;
                    Triangle {
                        vertices,
                        template,
                        source_side,
                    }
                })
                .collect());
        }
    }
    let center = points.len();
    points.push(ring.iter().map(|&index| points[index]).sum::<DVec3>() / ring.len() as f64);
    anchors.push(ring.iter().map(|&index| anchors[index]).sum::<DVec3>() / ring.len() as f64);
    let mut result = Vec::with_capacity(ring.len());
    for index in 0..ring.len() {
        let vertices = [center, ring[index], ring[(index + 1) % ring.len()]];
        let fitted = projection(side, vertices, anchors, points, attached)?;
        let mut template = side.clone();
        template.u_axis = fitted.u;
        template.v_axis = fitted.v;
        result.push(Triangle {
            vertices,
            template,
            source_side,
        });
    }
    Ok(result)
}

fn canonical_boundary(
    source: &Solid,
    replacement: &Solid,
    change: &CreaseChange,
) -> Result<Boundary, EditError> {
    let before = topology(source)?;
    let after = topology(replacement)?;
    let mut points = before
        .vertices
        .iter()
        .map(|vertex| vertex.position)
        .collect::<Vec<_>>();
    let mut anchors = points.clone();
    points.extend(change.positions);
    anchors.extend(change.flat_positions);
    if after.vertices.len() != points.len() {
        return Err(invalid(
            "Ridge fragments require the exact planned vertex topology",
        ));
    }
    let mut used = BTreeSet::new();
    let mut mapping = Vec::with_capacity(after.vertices.len());
    for vertex in &after.vertices {
        let matches = points
            .iter()
            .enumerate()
            .filter(|(_, point)| vertex.position.distance(**point) < EPSILON * 10.0)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let [index]: [usize; 1] = matches
            .try_into()
            .map_err(|_| invalid("Ridge vertex has no unique planned source correspondence"))?;
        if !used.insert(index) {
            return Err(invalid("Ridge vertex correspondence is not one-to-one"));
        }
        let mut sides = vertex
            .sides
            .iter()
            .map(|&side| {
                if side == change.new_side {
                    change.source.side
                } else {
                    side
                }
            })
            .collect::<Vec<_>>();
        sides.sort_unstable();
        sides.dedup();
        let expected = if index < before.vertices.len() {
            before.vertices[index].sides.clone()
        } else {
            let mut expected = vec![
                change.source.side,
                change.source.edges[index - before.vertices.len()],
            ];
            expected.sort_unstable();
            expected
        };
        if sides != expected {
            return Err(invalid(
                "Ridge vertex changes its declared source incidence",
            ));
        }
        mapping.push(index);
    }
    let mut boundary = replacement.clone();
    let mut rings = Vec::with_capacity(boundary.sides.len());
    for side in &mut boundary.sides {
        let face = after
            .faces
            .iter()
            .find(|face| face.side == side.id)
            .ok_or_else(|| invalid("Ridge boundary face is missing"))?;
        let ring = face
            .vertices
            .iter()
            .map(|&index| mapping[index])
            .collect::<Vec<_>>();
        side.set_precise_vertices(&ring.iter().map(|&index| points[index]).collect::<Vec<_>>())
            .map_err(|error| invalid(error.to_string()))?;
        rings.push(ring);
    }
    crate::brush::canonicalize_precise(&mut boundary)?;
    topology(&boundary)?;
    Ok(Boundary {
        solid: boundary,
        points,
        anchors,
        rings,
    })
}

fn decompose(
    source: &Solid,
    replacement: &Replacement,
    change: &CreaseChange,
    limit: usize,
) -> Result<Vec<CreaseFragment>, EditError> {
    let Boundary {
        solid: boundary,
        mut points,
        mut anchors,
        rings,
    } = canonical_boundary(source, &replacement.solid, change)?;
    let center = points.iter().sum::<DVec3>() / points.len() as f64;
    let geometry =
        BrushGeometry::from_solid(&boundary).map_err(|error| invalid(error.to_string()))?;
    if geometry
        .faces
        .iter()
        .any(|face| upper_distance(face.plane, center) >= 0.0)
    {
        return Err(invalid(
            "Ridge vertex centroid is not certified strictly inside every boundary plane",
        ));
    }
    let mut exterior = Vec::new();
    for (side, ring) in boundary.sides.iter().zip(&rings) {
        let source_side = if side.id == change.new_side {
            change.source.side
        } else {
            side.id
        };
        exterior.extend(triangles(
            ring,
            &mut points,
            &mut anchors,
            side,
            change.source.edges.contains(&source_side),
            source_side,
        )?);
        if exterior.len() > limit {
            return Err(invalid(
                "Ridge attachment exceeds 256 aggregate convex fragments",
            ));
        }
    }
    let mut edges = BTreeMap::<[usize; 2], Vec<EdgeUse>>::new();
    for triangle in &exterior {
        for index in 0..3 {
            let oriented = [triangle.vertices[index], triangle.vertices[(index + 1) % 3]];
            let mut key = oriented;
            key.sort_unstable();
            edges.entry(key).or_default().push(EdgeUse {
                vertices: oriented,
                ridge_side: triangle.template.id,
                uv: key.map(|vertex| crate::texel_uv(&triangle.template, points[vertex])),
            });
        }
    }
    for faces in edges.values() {
        if faces.len() != 2 || faces[0].vertices != [faces[1].vertices[1], faces[1].vertices[0]] {
            return Err(invalid(
                "Ridge exterior triangulation is not an exact closed oriented boundary",
            ));
        }
        if faces[0].ridge_side == faces[1].ridge_side
            && faces[0].uv.iter().zip(&faces[1].uv).any(|(a, b)| {
                !a.is_finite()
                    || !b.is_finite()
                    || (*a - *b).abs().max_element() > crate::texture_deform::TEXEL_EPSILON
            })
        {
            return Err(invalid(
                "Ridge triangle projections do not preserve their shared texture seam",
            ));
        }
    }
    let triangles = exterior
        .into_iter()
        .map(|triangle| {
            let tag = Some(triangle.template.id);
            fragments::BoundaryTriangle {
                vertices: triangle.vertices,
                template: triangle.template,
                source_side: triangle.source_side,
                tag,
            }
        })
        .collect();
    fragments::tetrahedralize(source, &points, triangles, center, limit).map(|pieces| {
        pieces
            .into_iter()
            .map(|piece| CreaseFragment {
                source: source.id,
                owner: replacement.owner,
                solid: piece.solid,
                faces: piece
                    .faces
                    .into_iter()
                    .map(|face| CreaseFragmentFace {
                        side: face.side,
                        source_side: face.source_side,
                        exterior: face.exterior,
                        ridge_side: face.tag,
                    })
                    .collect(),
            })
            .collect()
    })
}

pub(super) fn attach(
    tx: &mut Transaction,
    plan: &mut CreasePlan,
    originals: &BTreeMap<u32, Solid>,
) -> Result<(), EditError> {
    let mut fragments = Vec::new();
    for replacement in &plan.replacements {
        let source = originals
            .get(&replacement.solid.id)
            .ok_or_else(|| invalid("Ridge source provenance is unavailable"))?;
        fragments::admit_source(source)?;
        let change = plan
            .changes
            .iter()
            .find(|change| change.source.solid == source.id)
            .ok_or_else(|| invalid("Ridge change provenance is unavailable"))?;
        fragments.extend(
            decompose(
                source,
                replacement,
                change,
                MAX_RIDGE_FRAGMENTS - fragments.len(),
            )
            .map_err(|error| invalid(format!("Brush {}: {error}", source.id)))?,
        );
    }
    let mut retained_solids = BTreeSet::new();
    let mut retained_sides = BTreeSet::new();
    for fragment in &mut fragments {
        if !retained_solids.insert(fragment.source) {
            fragment.solid.id = tx.new_object_id()?;
        }
        let ridge_side = fragment.faces[0]
            .ridge_side
            .ok_or_else(|| invalid("Ridge exterior has no declared ridge face identity"))?;
        for (index, (side, face)) in fragment
            .solid
            .sides
            .iter_mut()
            .zip(&mut fragment.faces)
            .enumerate()
        {
            side.id = if index == 0 && retained_sides.insert(ridge_side) {
                ridge_side
            } else {
                tx.new_side_id()?
            };
            face.side = side.id;
        }
    }
    fragments::install(
        tx,
        fragments
            .iter()
            .map(|fragment| (fragment.source, fragment.owner, fragment.solid.clone())),
    )?;
    plan.fragments = fragments;
    Ok(())
}
