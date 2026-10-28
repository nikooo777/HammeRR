use super::*;
use map_core::surface::{PaintMode, SurfaceSample};

#[derive(Debug, Clone, Copy)]
pub enum GroupProjection {
    Face(Option<FaceId>),
    View,
}

pub struct ChainPaint {
    pub changed: usize,
    pub links: Vec<map_core::texture_chain::Link>,
    pub source_in_targets: bool,
}

pub struct TerrainSew {
    pub changed: usize,
    pub pairs: usize,
    pub corner_joins: usize,
    pub vertices: usize,
    pub interpolated_vertices: usize,
}

pub struct TerrainTriangles {
    pub face: FaceId,
    pub triangles: Vec<map_core::displacement::triangles::Triangle>,
}

pub struct TerrainSubdivision {
    pub changed: usize,
    pub shared_edges: usize,
    pub vertices: usize,
}

impl TerrainSubdivision {
    pub fn summary(&self) -> String {
        format!(
            "Preview: {} surfaces rounded across {} shared edges; {} samples. Apply to replace their sculpted positions.",
            self.changed, self.shared_edges, self.vertices
        )
    }
}

impl TerrainSew {
    pub fn summary(&self) -> String {
        format!(
            "Preview: {} joined borders, {} corner joins across {} surfaces. {} fine border points resampled. Apply to keep this edit.",
            self.pairs, self.corner_joins, self.changed, self.interpolated_vertices
        )
    }
}

impl Session {
    pub fn terrain_triangles(
        &self,
        faces: &[FaceId],
        preview: bool,
    ) -> Result<Vec<TerrainTriangles>, String> {
        let document = if preview {
            self.render_document()
        } else {
            &self.document
        };
        self.terrain_grids(faces, preview)?
            .into_iter()
            .map(|grid| {
                let side = map_core::surface::side(document, grid.face)
                    .ok_or("Terrain face is missing")?;
                let triangles = map_core::displacement::triangles::inspect(
                    side.displacement.as_ref().unwrap(),
                    &grid.mesh,
                )
                .map_err(|error| error.to_string())?;
                Ok(TerrainTriangles {
                    face: grid.face,
                    triangles,
                })
            })
            .collect()
    }

    pub fn preview_terrain_triangle_tags(
        &mut self,
        actor: Actor,
        targets: &[(FaceId, Vec<usize>)],
        settings: map_core::displacement::triangles::Settings,
    ) -> Result<usize, String> {
        let faces = targets.iter().map(|(face, _)| *face).collect::<Vec<_>>();
        self.terrain_grids(&faces, false)?;
        if targets.len()
            != faces
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
        {
            return Err("Use one triangle selection per terrain face".into());
        }
        self.preview_or_apply_faces(
            actor,
            targets
                .iter()
                .map(|(face, triangles)| {
                    (
                        *face,
                        SurfaceEdit {
                            terrain: Some(map_core::displacement::Operation::TriangleTags {
                                triangles: triangles.clone(),
                                settings,
                            }),
                            ..Default::default()
                        },
                    )
                })
                .collect(),
            true,
        )
    }

    pub fn terrain_grids(
        &self,
        faces: &[FaceId],
        preview: bool,
    ) -> Result<Vec<map_core::displacement::edit::TerrainGrid>, String> {
        let document = if preview {
            self.render_document()
        } else {
            &self.document
        };
        map_core::displacement::edit::inspect(document, faces).map_err(|error| error.to_string())
    }

    pub(super) fn terrain_surface_grids(
        &self,
        faces: &[FaceId],
        preview: bool,
    ) -> Result<Vec<map_core::displacement::edit::TerrainGrid>, String> {
        let mut grids = self.terrain_grids(faces, preview)?;
        let document = if preview {
            self.render_document()
        } else {
            &self.document
        };
        let surfaces = map_core::displacement::surface::prepare(document.vmf());
        for grid in &mut grids {
            if let Some(error) = surfaces.issues.get(&grid.face) {
                return Err(error.clone());
            }
            if let Some(mesh) = surfaces.get(&grid.face) {
                grid.mesh = mesh.clone();
            }
        }
        Ok(grids)
    }

    pub(super) fn terrain_surface_triangles(
        &self,
        faces: &[FaceId],
        preview: bool,
    ) -> Result<Vec<TerrainTriangles>, String> {
        let mut rows = self.terrain_triangles(faces, preview)?;
        let grids = self.terrain_surface_grids(faces, preview)?;
        for (row, grid) in rows.iter_mut().zip(grids) {
            for triangle in &mut row.triangles {
                triangle.positions = triangle.indices.map(|index| grid.mesh.positions[index]);
                triangle.centroid = triangle.positions.into_iter().sum::<DVec3>() / 3.0;
            }
        }
        Ok(rows)
    }

    pub fn preview_terrain_sew(
        &mut self,
        actor: Actor,
        faces: &[FaceId],
        blend: bool,
    ) -> Result<TerrainSew, String> {
        let plan = map_core::displacement::sew::plan(&self.document, faces, blend)
            .map_err(|error| error.to_string())?;
        let edits = plan
            .edits
            .into_iter()
            .map(|(face, info)| {
                (
                    face,
                    SurfaceEdit {
                        terrain: Some(map_core::displacement::Operation::Sewn(Box::new(info))),
                        ..Default::default()
                    },
                )
            })
            .collect();
        let changed = self.preview_or_apply_faces(actor, edits, true)?;
        Ok(TerrainSew {
            changed,
            pairs: plan.pairs,
            corner_joins: plan.corner_joins,
            vertices: plan.vertices,
            interpolated_vertices: plan.interpolated_vertices,
        })
    }

    pub fn preview_terrain_subdivision(
        &mut self,
        actor: Actor,
        faces: &[FaceId],
    ) -> Result<TerrainSubdivision, String> {
        let plan = map_core::displacement::subdivide::plan(&self.document, faces)
            .map_err(|error| error.to_string())?;
        let edits = plan
            .edits
            .into_iter()
            .map(|(face, info)| {
                (
                    face,
                    SurfaceEdit {
                        terrain: Some(map_core::displacement::Operation::Subdivided(Box::new(
                            info,
                        ))),
                        ..Default::default()
                    },
                )
            })
            .collect();
        let changed = self.preview_or_apply_faces(actor, edits, true)?;
        Ok(TerrainSubdivision {
            changed,
            shared_edges: plan.shared_edges,
            vertices: plan.vertices,
        })
    }

    pub fn preview_terrain_resolution(
        &mut self,
        actor: Actor,
        faces: &[FaceId],
        power: u32,
    ) -> Result<usize, String> {
        self.preview_terrain_edit(
            actor,
            faces,
            map_core::displacement::Operation::Resolution(power),
        )
    }

    pub fn preview_terrain_edit(
        &mut self,
        actor: Actor,
        faces: &[FaceId],
        operation: map_core::displacement::Operation,
    ) -> Result<usize, String> {
        self.terrain_grids(faces, false)?;
        self.edit_faces(
            actor,
            faces,
            SurfaceEdit {
                terrain: Some(operation),
                ..Default::default()
            },
            false,
            true,
        )
    }

    pub fn terrain_creation_faces(
        &self,
        faces: &[FaceId],
    ) -> Result<Vec<map_core::displacement::edit::CreationFace>, String> {
        map_core::displacement::edit::creation_faces(&self.document, faces)
            .map_err(|error| error.to_string())
    }

    pub fn terrain_creation_error(&self) -> Option<String> {
        map_core::displacement::edit::validate(&self.document, &self.selected_faces())
            .err()
            .map(|error| error.to_string())
    }

    pub fn terrain_removal_error(&self) -> Option<String> {
        map_core::displacement::edit::validate_removal(&self.document, &self.selected_faces())
            .err()
            .map(|error| error.to_string())
    }

    pub fn remove_terrain(&mut self, actor: Actor, faces: &[FaceId]) -> Result<usize, String> {
        self.edit(actor, "remove terrain", |tx| {
            map_core::displacement::edit::remove(tx, faces)
        })
    }

    pub fn create_terrain(
        &mut self,
        actor: Actor,
        faces: &[FaceId],
        power: u32,
        start_corner: u32,
    ) -> Result<usize, String> {
        self.edit(actor, "create terrain", |tx| {
            map_core::displacement::edit::create(tx, faces, power, start_corner)
        })
    }

    pub fn project_face_group(
        &mut self,
        actor: Actor,
        faces: &[FaceId],
        projection: GroupProjection,
        material: Option<&str>,
        repeats: DVec2,
        preview: bool,
    ) -> Result<usize, String> {
        let faces: Vec<_> = faces
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let first = *faces.first().ok_or("Select at least one face")?;
        let material = match material {
            Some(material) => material_search::material_name(material)?,
            None => {
                let mut names = BTreeSet::new();
                for face in &faces {
                    let side =
                        surface::side(&self.document, *face).ok_or("Target face does not exist")?;
                    names.insert(material_search::material_name(&side.material)?);
                }
                if names.len() != 1 {
                    return Err(
                        "Choose one shared material for faces with different materials".into(),
                    );
                }
                names.pop_first().unwrap()
            }
        };
        let texture = self.material_preview(&material)?;
        if texture.missing {
            return Err(format!(
                "Cannot fit {material}: texture dimensions unavailable"
            ));
        }
        let basis = match projection {
            GroupProjection::Face(reference) => {
                map_core::surface_group::Basis::Face(reference.unwrap_or(first))
            }
            GroupProjection::View => map_core::surface_group::Basis::View {
                right: self.camera().right().as_dvec3(),
                up: self.camera().up().as_dvec3(),
            },
        };
        let mut edits = map_core::surface_group::project(
            &self.document,
            &faces,
            basis,
            [texture.width, texture.height],
            repeats,
        )
        .map_err(|error| error.to_string())?;
        for (_, edit) in &mut edits {
            edit.material = Some(material.clone());
        }
        self.preview_or_apply_faces(actor, edits, preview)
    }

    pub fn surface_view_options(
        &self,
        mode: Option<render::SurfaceView>,
        group: Option<u32>,
    ) -> Result<render::ViewOptions, String> {
        let mut view = self.view();
        if let Some(mode) = mode {
            view.surface = mode;
        }
        if let Some(group) = group {
            if !(1..=32).contains(&group) {
                return Err("Choose a smoothing group from 1 to 32".into());
            }
            view.smoothing_group = group;
        }
        Ok(view)
    }

    pub fn set_surface_view(
        &mut self,
        mode: render::SurfaceView,
        group: u32,
    ) -> Result<(), String> {
        let view = self.surface_view_options(Some(mode), Some(group))?;
        self.set_view(view);
        Ok(())
    }

    pub fn sample_face_texture(&mut self, face: FaceId) -> Result<(), String> {
        let sample = surface::sample(&self.document, face).map_err(|error| error.to_string())?;
        if self
            .paint_stroke()
            .is_some_and(|stroke| stroke.texture.is_some())
        {
            self.clear_paint_stroke();
        }
        self.cancel_face_preview();
        self.surface_sample = Some(sample);
        self.view_serial += 1;
        Ok(())
    }

    pub fn surface_sample(&self) -> Option<&SurfaceSample> {
        self.surface_sample.as_ref()
    }

    pub fn propagate_face_texture(
        &mut self,
        actor: Actor,
        faces: &[FaceId],
        preview: bool,
    ) -> Result<ChainPaint, String> {
        let sample = self
            .surface_sample
            .as_ref()
            .ok_or("Sample a source face texture first")?;
        let chain = map_core::texture_chain::wrap(&self.document, faces, sample)
            .map_err(|error| error.to_string())?;
        let changed = self.preview_or_apply_faces(actor, chain.edits, preview)?;
        Ok(ChainPaint {
            changed,
            links: chain.links,
            source_in_targets: chain.source_in_targets,
        })
    }

    pub fn paint_face_texture(
        &mut self,
        actor: Actor,
        faces: &[FaceId],
        mode: PaintMode,
        preview: bool,
    ) -> Result<usize, String> {
        if faces.is_empty() {
            return Err("select at least one target face".into());
        }
        let sample = self
            .surface_sample
            .as_ref()
            .ok_or("sample a face texture first")?;
        let mut edits = Vec::new();
        for face in faces.iter().copied().collect::<BTreeSet<_>>() {
            let edit = surface::paint_edit(&self.document, face, sample, mode)
                .map_err(|error| error.to_string())?;
            edits.push((face, edit));
        }
        self.preview_or_apply_faces(actor, edits, preview)
    }
}
