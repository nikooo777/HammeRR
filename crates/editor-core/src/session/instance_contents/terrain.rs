use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::mem::size_of;

use compiler::instances::PreparedInstances;
use map_core::displacement::{lod, neighbors, smoothing};
use map_core::surface::FaceId;
use map_core::{BrushGeometry, DisplacementMesh};
use source_formats::vmf::Vmf;

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
                    .map(|index| grid.mesh.base_positions[index].as_vec3().as_dvec3()),
                positions: &grid.mesh.positions,
            }
        })
        .collect()
}

struct Context {
    faces: usize,
    scratch_bytes: usize,
}

fn preflight(vmf: &Vmf, budget: &mut Budget<'_>) -> Result<Context, String> {
    let mut vertices = 0usize;
    let mut faces = 0usize;
    let mut scratch_bytes = 0usize;
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
                .and_then(|bytes| bytes.checked_add(faces * size_of::<Grid>()))
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
    })
}

fn prepare_context(
    vmf: &Vmf,
    context: &Context,
    budget: &mut Budget<'_>,
) -> Result<(Vec<Grid>, neighbors::Neighbors), String> {
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
            let mesh =
                DisplacementMesh::build(face, info).map_err(|error| bounded(&error.to_string()))?;
            budget.consume(mesh.positions.len())?;
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
    let links = neighbors::find_spans(&views, &mut |amount| budget.consume(amount))?;
    let (patches, _) = lod::prepare(&views, &links.pairs, &mut |amount| budget.consume(amount))?;
    drop(views);
    for (grid, patch) in grids.iter_mut().zip(patches) {
        if let Some(patch) = patch {
            grid.mesh.positions = patch.positions;
            grid.triangles = Some(patch.indices);
        }
    }
    Ok((grids, links))
}

pub(super) fn prepare(
    prepared: &PreparedInstances<'_>,
    original: &Vmf,
    control: &compiler::Control,
    report: &mut compiler::Report,
) -> Result<BTreeMap<FaceId, Surface>, String> {
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
        return Ok(BTreeMap::new());
    }
    let vmf = prepared.vmf();
    let mut budget = Budget { control, work: 0 };
    let expanded = preflight(vmf, &mut budget)?;
    let ordinary = preflight(original, &mut budget)?;
    if expanded
        .scratch_bytes
        .checked_add(ordinary.scratch_bytes)
        .and_then(|bytes| bytes.checked_add(AUXILIARY_BYTES))
        .is_none_or(|bytes| bytes > references::MAX_CPU_BYTES)
    {
        return Err("Instance terrain comparison exceeds the 64 MiB CPU budget".into());
    }
    let (grids, links) = prepare_context(vmf, &expanded, &mut budget)?;
    if ordinary.faces > 0 {
        let (baseline, _) = prepare_context(original, &ordinary, &mut budget)?;
        let baseline = baseline
            .into_iter()
            .map(|grid| (grid.face, grid))
            .collect::<BTreeMap<_, _>>();
        for grid in grids.iter().filter(|grid| !generated.contains(&grid.face)) {
            budget.consume(grid.mesh.positions.len())?;
            let baseline = baseline
                .get(&grid.face)
                .ok_or("Instance terrain ordinary parent surface is missing")?;
            if !grid.same_surface(baseline) {
                return Err("Instance contents would require replacing ordinary parent terrain LOD; compilation remains available".into());
            }
        }
    }
    if links.ambiguous + links.mismatched != 0 {
        report.diagnostics.push(compiler::Diagnostic::warning(format!("Terrain shared edges left unlinked: {} ambiguous, {} mismatched surfaces. Native full/half edges require matching common samples; other partial spans are not connected", links.ambiguous, links.mismatched), None, None));
    }
    let angles = grids.iter().map(|grid| grid.angle).collect::<Vec<_>>();
    let views = grid_views(&grids);
    let normals =
        neighbors::vertex_normals_with_smoothing(&views, &links.pairs, &angles, &mut |amount| {
            budget.consume(amount)
        })?;
    drop(views);
    let mut surfaces = BTreeMap::new();
    for (grid, normals) in grids.into_iter().zip(normals) {
        budget.consume(grid.mesh.positions.len())?;
        if !generated.contains(&grid.face) {
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
    Ok(surfaces)
}
