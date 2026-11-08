use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::mem::size_of;

use compiler::instances::PreparedInstances;
use map_core::displacement::{lod, native, neighbors, smoothing};
use map_core::surface::FaceId;
use map_core::{BrushGeometry, DisplacementMesh};
use render::scene::TerrainNormalOverride;
use source_formats::{bsp::DisplacementCorner, vmf::Vmf};

use super::{AUXILIARY_BYTES, DVec3, bounded, instances, references};

pub(super) struct Surface {
    pub mesh: DisplacementMesh,
    pub normals: Vec<[f64; 3]>,
    pub triangles: Vec<[usize; 3]>,
    pub edges: Vec<[usize; 2]>,
}

impl Surface {
    pub fn bytes(&self) -> usize {
        self.mesh.positions.capacity() * size_of::<DVec3>()
            + self.mesh.base_positions.capacity() * size_of::<DVec3>()
            + self.mesh.alphas.capacity() * size_of::<f64>()
            + self.normals.capacity() * size_of::<[f64; 3]>()
            + self.triangles.capacity() * size_of::<[usize; 3]>()
            + self.edges.capacity() * size_of::<[usize; 2]>()
            + size_of::<Self>()
    }
}

#[derive(Default)]
pub(super) struct Prepared {
    pub(super) surfaces: BTreeMap<FaceId, Surface>,
    pub(super) parent_normals: BTreeMap<FaceId, TerrainNormalOverride>,
}

pub(super) fn normal_bytes(overrides: &BTreeMap<FaceId, TerrainNormalOverride>) -> Option<usize> {
    overrides.values().try_fold(0usize, |bytes, value| {
        bytes
            .checked_add(size_of::<TerrainNormalOverride>())?
            .checked_add(
                value
                    .positions
                    .capacity()
                    .checked_mul(size_of::<[f32; 3]>())?,
            )?
            .checked_add(
                value
                    .triangles
                    .capacity()
                    .checked_mul(size_of::<[usize; 3]>())?,
            )?
            .checked_add(
                value
                    .normals
                    .capacity()
                    .checked_mul(size_of::<[f64; 3]>())?,
            )
    })
}

struct Grid {
    face: FaceId,
    mesh: DisplacementMesh,
    angle: Option<f64>,
    triangles: Option<Vec<[usize; 3]>>,
}

impl Grid {
    fn indices(&self) -> Cow<'_, [[usize; 3]]> {
        match &self.triangles {
            Some(triangles) => Cow::Borrowed(triangles),
            None => Cow::Owned(self.mesh.triangles().collect()),
        }
    }

    fn same_surface(&self, other: &Self) -> bool {
        self.mesh.positions.len() == other.mesh.positions.len()
            && self
                .mesh
                .positions
                .iter()
                .zip(&other.mesh.positions)
                .all(|(a, b)| a.as_vec3() == b.as_vec3())
            && self.indices() == other.indices()
    }
}

struct Budget<'a> {
    control: &'a compiler::Control,
    work: usize,
}

impl Budget<'_> {
    fn consume(&mut self, amount: usize) -> Result<(), String> {
        self.control.checkpoint()?;
        self.work = self
            .work
            .checked_add(amount)
            .ok_or("Instance terrain work overflow")?;
        if self.work > instances::MAX_GEOMETRY_WORK {
            return Err("Instance terrain exceeds its geometry work budget".into());
        }
        Ok(())
    }
}

fn solids(vmf: &Vmf) -> impl Iterator<Item = &source_formats::vmf::Solid> {
    vmf.world
        .solids
        .iter()
        .chain(
            vmf.entities
                .iter()
                .filter(|entity| {
                    !entity.hidden
                        && entity
                            .classname()
                            .is_some_and(|class| class.eq_ignore_ascii_case("func_detail"))
                })
                .flat_map(|entity| &entity.solids),
        )
        .filter(|solid| !solid.hidden && solid.sides.iter().any(|side| side.displacement.is_some()))
}

fn grid_views(grids: &[Grid]) -> Vec<neighbors::SurfaceGrid<'_>> {
    grids
        .iter()
        .map(|grid| {
            let size = grid.mesh.size;
            neighbors::SurfaceGrid {
                power: (size - 1).ilog2(),
                corners: [0, (size - 1) * size, size * size - 1, size - 1]
                    .map(|index| native::stored_vertex(grid.mesh.base_positions[index])),
                positions: &grid.mesh.positions,
            }
        })
        .collect()
}

struct Context {
    faces: usize,
    scratch_bytes: usize,
    witness_bytes: usize,
}

struct PreparedContext {
    grids: Vec<Grid>,
    links: neighbors::Neighbors,
    corners: Vec<[DisplacementCorner; 4]>,
}

fn preflight(vmf: &Vmf, budget: &mut Budget<'_>) -> Result<Context, String> {
    let mut vertices = 0usize;
    let mut faces = 0usize;
    let mut scratch_bytes = 0usize;
    let mut witness_bytes = 0usize;
    for solid in solids(vmf) {
        budget.consume(1)?;
        if solid.sides.len() > instances::MAX_BRUSH_SIDES {
            return Err("Instance terrain context exceeds its brush side budget".into());
        }
        for info in solid
            .sides
            .iter()
            .filter_map(|side| side.displacement.as_ref())
        {
            budget.consume(1)?;
            if !(2..=4).contains(&info.power) {
                return Err("Instance terrain context requires power 2, 3 or 4".into());
            }
            vertices = vertices
                .checked_add(((1usize << info.power) + 1).pow(2))
                .ok_or("Instance terrain context vertex overflow")?;
            let cells = 1usize << info.power;
            witness_bytes = witness_bytes
                .checked_add(((cells + 1).pow(2)) * size_of::<[f32; 3]>())
                .and_then(|bytes| bytes.checked_add(2 * cells.pow(2) * size_of::<[usize; 3]>()))
                .ok_or("Instance terrain normal witness size overflow")?;
            faces += 1;
            if vertices > references::MAX_VERTICES || faces > source_formats::bsp::MAX_DISPLACEMENTS
            {
                return Err("Instance terrain context exceeds its vertex or patch budget".into());
            }
            let scratch = vertices
                .checked_mul(
                    2 * size_of::<DVec3>()
                        + size_of::<f64>()
                        + size_of::<[f64; 3]>()
                        + 2 * size_of::<[usize; 3]>()
                        + 3 * size_of::<[usize; 2]>(),
                )
                .and_then(|bytes| {
                    bytes.checked_add(
                        faces * (size_of::<Grid>() + size_of::<[DisplacementCorner; 4]>()),
                    )
                })
                .ok_or("Instance terrain context CPU size overflow")?;
            if scratch
                .checked_add(AUXILIARY_BYTES)
                .is_none_or(|bytes| bytes > references::MAX_CPU_BYTES)
            {
                return Err("Instance terrain context exceeds the 64 MiB CPU budget".into());
            }
            scratch_bytes = scratch;
        }
    }
    Ok(Context {
        faces,
        scratch_bytes,
        witness_bytes,
    })
}

fn prepare_context(
    vmf: &Vmf,
    context: &Context,
    budget: &mut Budget<'_>,
) -> Result<PreparedContext, String> {
    let mut grids = Vec::with_capacity(context.faces);
    for solid in solids(vmf) {
        budget.consume(solid.sides.len())?;
        let geometry =
            BrushGeometry::from_solid(solid).map_err(|error| bounded(&error.to_string()))?;
        for side in solid
            .sides
            .iter()
            .filter(|side| side.displacement.is_some())
        {
            let face = geometry
                .faces
                .iter()
                .find(|face| solid.sides[face.side].id == side.id)
                .ok_or("Instance terrain has no brush surface")?;
            let info = side.displacement.as_ref().unwrap();
            let mesh = native::stored_mesh(face, info).map_err(|error| bounded(&error))?;
            budget.consume(3 * mesh.positions.len())?;
            grids.push(Grid {
                face: FaceId {
                    solid: solid.id,
                    side: side.id,
                },
                mesh,
                angle: smoothing::angle(info)?,
                triangles: None,
            });
        }
    }
    let views = grid_views(&grids);
    let (links, neighbors::NativeConnections { corners, .. }) =
        neighbors::native_neighborhood(&views, &mut |amount| budget.consume(amount))?;
    let (patches, _) = lod::prepare(&views, &links.pairs, &mut |amount| budget.consume(amount))?;
    drop(views);
    for (grid, patch) in grids.iter_mut().zip(patches) {
        if let Some(patch) = patch {
            grid.mesh.positions = patch.positions;
            grid.triangles = Some(patch.indices);
        }
    }
    Ok(PreparedContext {
        grids,
        links,
        corners,
    })
}

fn prepare_normals(
    grids: &[Grid],
    links: &neighbors::Neighbors,
    corners: &[[DisplacementCorner; 4]],
    budget: &mut Budget<'_>,
) -> Result<Vec<Vec<[f64; 3]>>, String> {
    let angles = grids.iter().map(|grid| grid.angle).collect::<Vec<_>>();
    let views = grid_views(grids);
    neighbors::vertex_normals_with_corners(&views, &links.pairs, corners, &angles, &mut |amount| {
        budget.consume(amount)
    })
}

pub(super) fn prepare(
    prepared: &PreparedInstances<'_>,
    original: &Vmf,
    control: &compiler::Control,
    report: &mut compiler::Report,
) -> Result<Prepared, String> {
    let generated = prepared
        .generated_brushes()
        .flat_map(|brush| {
            brush
                .solid
                .sides
                .iter()
                .filter(|side| side.displacement.is_some())
                .map(move |side| FaceId {
                    solid: brush.solid.id,
                    side: side.id,
                })
        })
        .collect::<BTreeSet<_>>();
    if generated.is_empty() {
        return Ok(Prepared::default());
    }
    let vmf = prepared.vmf();
    let mut budget = Budget { control, work: 0 };
    let expanded = preflight(vmf, &mut budget)?;
    let ordinary = preflight(original, &mut budget)?;
    if expanded
        .scratch_bytes
        .checked_add(ordinary.scratch_bytes)
        .and_then(|bytes| bytes.checked_add(ordinary.witness_bytes))
        .and_then(|bytes| bytes.checked_add(AUXILIARY_BYTES))
        .is_none_or(|bytes| bytes > references::MAX_CPU_BYTES)
    {
        return Err("Instance terrain comparison exceeds the 64 MiB CPU budget".into());
    }
    let PreparedContext {
        grids,
        links,
        corners,
    } = prepare_context(vmf, &expanded, &mut budget)?;
    let normals = prepare_normals(&grids, &links, &corners, &mut budget)?;
    let mut changed_parents = BTreeSet::new();
    if ordinary.faces > 0 {
        let baseline = prepare_context(original, &ordinary, &mut budget)?;
        let baseline_normals = prepare_normals(
            &baseline.grids,
            &baseline.links,
            &baseline.corners,
            &mut budget,
        )?;
        let baseline = baseline
            .grids
            .into_iter()
            .zip(baseline_normals)
            .map(|(grid, normals)| (grid.face, (grid, normals)))
            .collect::<BTreeMap<_, _>>();
        for (grid, normals) in grids
            .iter()
            .zip(&normals)
            .filter(|(grid, _)| !generated.contains(&grid.face))
        {
            budget.consume(2 * grid.mesh.positions.len())?;
            let (baseline, baseline_normals) = baseline
                .get(&grid.face)
                .ok_or("Instance terrain ordinary parent surface is missing")?;
            if !grid.same_surface(baseline) {
                return Err("Instance contents would require replacing ordinary parent terrain LOD; compilation remains available".into());
            }
            if normals.len() != baseline_normals.len() {
                return Err("Instance terrain parent normals have an incomplete baseline".into());
            }
            if normals
                .iter()
                .zip(baseline_normals)
                .any(|(a, b)| DVec3::from_array(*a).as_vec3() != DVec3::from_array(*b).as_vec3())
            {
                changed_parents.insert(grid.face);
            }
        }
    }
    if links.ambiguous + links.mismatched != 0 {
        report.diagnostics.push(compiler::Diagnostic::warning(format!("Terrain shared edges left unlinked: {} ambiguous, {} mismatched surfaces. Native full/half edges require matching common samples; other partial spans are not connected", links.ambiguous, links.mismatched), None, None));
    }
    let mut surfaces = BTreeMap::new();
    let mut parent_normals = BTreeMap::new();
    for (grid, normals) in grids.into_iter().zip(normals) {
        budget.consume(grid.mesh.positions.len())?;
        if !generated.contains(&grid.face) {
            if changed_parents.contains(&grid.face) {
                budget.consume(2 * (grid.mesh.size - 1).pow(2))?;
                parent_normals.insert(
                    grid.face,
                    TerrainNormalOverride {
                        positions: grid
                            .mesh
                            .positions
                            .iter()
                            .map(|point| point.as_vec3().to_array())
                            .collect(),
                        triangles: grid.mesh.triangles().collect(),
                        normals,
                    },
                );
            }
            continue;
        }
        let triangles = grid
            .triangles
            .unwrap_or_else(|| grid.mesh.triangles().collect());
        let mut edges = BTreeSet::new();
        for &[a, b, c] in &triangles {
            budget.consume(1)?;
            for (a, b) in [(a, b), (b, c), (c, a)] {
                edges.insert([a.min(b), a.max(b)]);
            }
        }
        surfaces.insert(
            grid.face,
            Surface {
                mesh: grid.mesh,
                normals,
                triangles,
                edges: edges.into_iter().collect(),
            },
        );
    }
    if surfaces.len() != generated.len() {
        return Err("Instance terrain has no complete prepared surface".into());
    }
    let bytes = surfaces.values().map(Surface::bytes).sum::<usize>();
    if normal_bytes(&parent_normals)
        .and_then(|normals| bytes.checked_add(normals))
        .and_then(|bytes| bytes.checked_add(AUXILIARY_BYTES))
        .is_none_or(|bytes| bytes > references::MAX_CPU_BYTES)
    {
        return Err("Instance terrain exceeds the retained CPU capacity budget".into());
    }
    Ok(Prepared {
        surfaces,
        parent_normals,
    })
}
