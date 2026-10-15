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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terrain_subdivision_shares_markers_owned_previews_and_history_for_both_actors() {
        for actor in [Actor::User, Actor::Agent] {
            let mut session = Session::new(Err("no assets".into()));
            let ObjectId::Solid(solid) = session
                .create_block(actor, DVec3::ZERO, DVec3::splat(64.0), "brick/test", false)
                .unwrap()
            else {
                unreachable!()
            };
            let side = session
                .document
                .solid(solid)
                .unwrap()
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                .unwrap()
                .id;
            let face = FaceId { solid, side };
            session.create_terrain(actor, &[face], 2, 2).unwrap();
            let before = session.document.vmf().clone();
            let history = session.document.undo_labels().count();
            session
                .preview_terrain_resolution(actor, &[face], 3)
                .unwrap();
            let foreign = session.face_preview_token();
            assert!(
                session
                    .preview_terrain_subdivision(
                        actor,
                        &[
                            face,
                            FaceId {
                                solid,
                                side: u32::MAX
                            }
                        ]
                    )
                    .is_err()
            );
            assert_eq!(session.face_preview_token(), foreign);
            let preview = session.preview_terrain_subdivision(actor, &[face]).unwrap();
            assert_eq!(
                (preview.changed, preview.shared_edges, preview.vertices),
                (1, 0, 25)
            );
            assert!(session.terrain_grids(&[face], true).unwrap()[0].subdivided);
            assert_eq!(session.document.vmf(), &before);
            assert_eq!(session.document.undo_labels().count(), history);
            session.cancel_face_preview();
            assert_eq!(session.document.vmf(), &before);
            session.preview_terrain_subdivision(actor, &[face]).unwrap();
            session.commit_face_preview(actor).unwrap();
            let rounded = session.document.vmf().clone();
            assert_eq!(session.document.undo_labels().count(), history + 1);
            assert!(session.terrain_grids(&[face], false).unwrap()[0].subdivided);
            assert_eq!(
                session
                    .preview_terrain_subdivision(actor, &[face])
                    .unwrap()
                    .changed,
                0
            );
            session.cancel_face_preview();
            session.undo(actor).unwrap();
            assert_eq!(session.document.vmf(), &before);
            session.redo(actor).unwrap();
            assert_eq!(session.document.vmf(), &rounded);
        }
    }

    #[test]
    fn terrain_creation_is_shared_atomic_and_undoable_without_changing_selection() {
        for actor in [Actor::User, Actor::Agent] {
            let mut session = Session::new(Err("no assets needed".into()));
            let ObjectId::Solid(solid) = session
                .create_block(actor, DVec3::ZERO, DVec3::splat(64.0), "brick/test", false)
                .unwrap()
            else {
                unreachable!()
            };
            let side = session
                .document()
                .solid(solid)
                .unwrap()
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                .unwrap()
                .id;
            let target = FaceId { solid, side };
            session
                .select_faces(&[target], SelectMode::Replace)
                .unwrap();
            let before = session.document().vmf().clone();
            assert!(session.terrain_creation_error().is_none());
            let corners = session.terrain_creation_faces(&[target]).unwrap()[0].corners;
            assert!(session.create_terrain(actor, &[target], 4, 4).is_err());
            assert!(session.create_terrain(actor, &[target], 5, 0).is_err());
            assert_eq!(*session.document().vmf(), before);
            session.create_terrain(actor, &[target], 4, 3).unwrap();
            assert_eq!(session.selected_faces(), vec![target]);
            assert!(session.terrain_creation_error().is_some());
            assert!(session.terrain_removal_error().is_none());
            assert_eq!(
                session
                    .document()
                    .solid(solid)
                    .unwrap()
                    .sides
                    .iter()
                    .find(|side| side.id == target.side)
                    .unwrap()
                    .displacement
                    .as_ref()
                    .unwrap()
                    .start_position,
                corners[3]
            );
            let created = session.document().vmf().clone();
            session.remove_terrain(actor, &[target]).unwrap();
            assert_eq!(*session.document().vmf(), before);
            session.undo(actor).unwrap();
            assert_eq!(*session.document().vmf(), created);
            session.undo(actor).unwrap();
            assert_eq!(*session.document().vmf(), before);
            assert!(session.terrain_creation_error().is_none());
        }
    }

    #[test]
    fn terrain_resolution_previews_are_shared_atomic_and_keep_source_until_commit() {
        for actor in [Actor::User, Actor::Agent] {
            let mut session = Session::new(Err("no assets needed".into()));
            let ObjectId::Solid(solid) = session
                .create_block(actor, DVec3::ZERO, DVec3::splat(64.0), "brick/test", false)
                .unwrap()
            else {
                unreachable!()
            };
            let shape = session.document().solid(solid).unwrap();
            let target = FaceId {
                solid,
                side: shape
                    .sides
                    .iter()
                    .find(|side| {
                        map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99
                    })
                    .unwrap()
                    .id,
            };
            let ordinary = FaceId {
                solid,
                side: shape
                    .sides
                    .iter()
                    .find(|side| side.id != target.side)
                    .unwrap()
                    .id,
            };
            session
                .select_faces(&[target], SelectMode::Replace)
                .unwrap();
            session.create_terrain(actor, &[target], 2, 3).unwrap();
            let before = session.document().vmf().clone();
            let history = session.document().undo_labels().count();
            session
                .preview_terrain_resolution(actor, &[target], 4)
                .unwrap();
            assert_eq!(*session.document().vmf(), before);
            assert_eq!(session.document().undo_labels().count(), history);
            assert_eq!(session.terrain_grids(&[target], true).unwrap()[0].power, 4);
            let token = session.face_preview_token();
            assert!(
                session
                    .preview_terrain_resolution(actor, &[target, ordinary], 3)
                    .is_err()
            );
            assert_eq!(session.face_preview_token(), token);
            assert_eq!(session.terrain_grids(&[target], true).unwrap()[0].power, 4);
            session.cancel_face_preview();
            assert_eq!(*session.document().vmf(), before);
            session
                .preview_terrain_resolution(actor, &[target], 3)
                .unwrap();
            session.commit_face_preview(actor).unwrap();
            assert_eq!(session.document().undo_labels().count(), history + 1);
            assert_eq!(session.selected_faces(), vec![target]);
            session.undo(actor).unwrap();
            assert_eq!(*session.document().vmf(), before);
        }
    }

    #[test]
    fn terrain_brush_previews_share_actors_commit_cancel_and_atomic_failures() {
        use map_core::displacement::{
            Operation,
            brush::{Axis, Blend, Brush, Sculpt},
        };
        for actor in [Actor::User, Actor::Agent] {
            let mut session = Session::new(Err("no assets needed".into()));
            let ObjectId::Solid(solid) = session
                .create_block(
                    actor,
                    DVec3::splat(-64.0),
                    DVec3::splat(64.0),
                    "brick/test",
                    false,
                )
                .unwrap()
            else {
                unreachable!()
            };
            let target = FaceId {
                solid,
                side: session
                    .document()
                    .solid(solid)
                    .unwrap()
                    .sides
                    .iter()
                    .find(|side| {
                        map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99
                    })
                    .unwrap()
                    .id,
            };
            session
                .select_faces(&[target], SelectMode::Replace)
                .unwrap();
            session.create_terrain(actor, &[target], 2, 0).unwrap();
            let before = session.document().vmf().clone();
            let brush = Brush {
                center: DVec3::new(0.0, 0.0, 64.0),
                radius: 32.0,
                axis: Axis::WorldZ,
            };
            session
                .preview_terrain_edit(
                    actor,
                    &[target],
                    Operation::Sculpt {
                        brush,
                        operation: Sculpt::Raise(16.0),
                    },
                )
                .unwrap();
            let token = session.face_preview_token();
            assert_eq!(
                session.terrain_grids(&[target], true).unwrap()[0]
                    .mesh
                    .positions[12]
                    .z,
                80.0
            );
            assert!(
                session
                    .preview_terrain_edit(
                        actor,
                        &[target],
                        Operation::Sculpt {
                            brush,
                            operation: Sculpt::Raise(4097.0)
                        }
                    )
                    .is_err()
            );
            assert_eq!(session.face_preview_token(), token);
            assert_eq!(*session.document().vmf(), before);
            session.cancel_face_preview();
            session
                .preview_terrain_edit(
                    actor,
                    &[target],
                    Operation::Blend {
                        brush,
                        operation: Blend::Fill(255.0),
                    },
                )
                .unwrap();
            assert_eq!(*session.document().vmf(), before);
            session.commit_face_preview(actor).unwrap();
            assert!(
                session.terrain_grids(&[target], false).unwrap()[0]
                    .mesh
                    .alphas
                    .iter()
                    .all(|alpha| *alpha == 255.0)
            );
            assert_eq!(
                session.document().undo_labels().next().unwrap(),
                "edit terrain blend"
            );
            session.undo(actor).unwrap();
            assert_eq!(*session.document().vmf(), before);
        }
    }

    #[test]
    fn terrain_sewing_and_collision_previews_share_actor_history_and_atomicity() {
        for actor in [Actor::User, Actor::Agent] {
            let mut session = Session::new(Err("no assets needed".into()));
            let mut faces = Vec::new();
            for x in [-64.0, 0.0] {
                let ObjectId::Solid(solid) = session
                    .create_block(
                        actor,
                        DVec3::new(x, 0.0, 0.0),
                        DVec3::new(x + 64.0, 64.0, 16.0),
                        "brick/test",
                        false,
                    )
                    .unwrap()
                else {
                    unreachable!()
                };
                let face = FaceId {
                    solid,
                    side: session
                        .document()
                        .solid(solid)
                        .unwrap()
                        .sides
                        .iter()
                        .find(|side| {
                            map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99
                        })
                        .unwrap()
                        .id,
                };
                session
                    .create_terrain(
                        actor,
                        &[face],
                        if faces.is_empty() { 2 } else { 4 },
                        faces.len() as u32,
                    )
                    .unwrap();
                faces.push(face);
            }
            session.select_faces(&faces, SelectMode::Replace).unwrap();
            session
                .edit(actor, "different heights", |tx| {
                    let solid = tx.solid_mut(faces[1].solid)?;
                    solid
                        .sides
                        .iter_mut()
                        .find(|side| side.id == faces[1].side)
                        .unwrap()
                        .displacement
                        .as_mut()
                        .unwrap()
                        .elevation = 16.0;
                    Ok(())
                })
                .unwrap();
            let before = session.document().vmf().clone();
            let result = session.preview_terrain_sew(actor, &faces, false).unwrap();
            assert_eq!(result.pairs, 1);
            assert_eq!(result.interpolated_vertices, 12);
            assert_eq!(*session.document().vmf(), before);
            let token = session.face_preview_token();
            assert!(
                session
                    .preview_terrain_sew(actor, &faces[..1], true)
                    .is_err()
            );
            assert_eq!(session.face_preview_token(), token);
            session.commit_face_preview(actor).unwrap();
            assert_eq!(
                session.document().undo_labels().next().unwrap(),
                "sew terrain edges"
            );
            session.undo(actor).unwrap();
            assert_eq!(*session.document().vmf(), before);
            session
                .preview_terrain_edit(
                    actor,
                    &faces,
                    map_core::displacement::Operation::Collision(
                        map_core::displacement::collision::Settings {
                            physics: Some(false),
                            player: Some(false),
                            ray: Some(false),
                        },
                    ),
                )
                .unwrap();
            assert!(
                session
                    .terrain_grids(&faces, true)
                    .unwrap()
                    .iter()
                    .all(|grid| grid.flags == 14)
            );
            assert_eq!(*session.document().vmf(), before);
            session.commit_face_preview(actor).unwrap();
            session.undo(actor).unwrap();
            assert_eq!(*session.document().vmf(), before);
        }
    }

    #[test]
    fn connected_wrapping_is_guarded_atomic_and_invalidated_by_resampling() {
        for actor in [Actor::User, Actor::Agent] {
            let mut session = Session::new(Err("no assets needed".into()));
            let ObjectId::Solid(solid) = session
                .create_block(actor, DVec3::ZERO, DVec3::splat(64.0), "brick/test", false)
                .unwrap()
            else {
                unreachable!()
            };
            let face = |normal: DVec3| FaceId {
                solid,
                side: session
                    .document()
                    .solid(solid)
                    .unwrap()
                    .sides
                    .iter()
                    .find(|side| {
                        map_core::Plane::from_points(side.plane.0).unwrap().normal == normal
                    })
                    .unwrap()
                    .id,
            };
            let source = face(DVec3::Z);
            let targets = [face(DVec3::NEG_Y), face(DVec3::NEG_Z)];
            session
                .edit_faces(
                    actor,
                    &[source],
                    SurfaceEdit {
                        material: Some("metal/sample".into()),
                        shift_u: Some(13.0),
                        rotation: Some(27.0),
                        ..Default::default()
                    },
                    false,
                    false,
                )
                .unwrap();
            session.sample_face_texture(source).unwrap();
            session.select_faces(&targets, SelectMode::Replace).unwrap();
            let selected = session.selected_faces();
            let before = session.document().vmf().clone();
            let history = session.document().undo_labels().count();
            let result = session
                .propagate_face_texture(actor, &targets, true)
                .unwrap();
            assert_eq!(result.changed, 2);
            assert_eq!(result.links.len(), 2);
            let preview = session.face_preview_document().unwrap().vmf().clone();
            let token = session.face_preview_token();
            let closed = session
                .document()
                .solid(solid)
                .unwrap()
                .sides
                .iter()
                .map(|side| FaceId {
                    solid,
                    side: side.id,
                })
                .collect::<Vec<_>>();
            assert!(
                session
                    .propagate_face_texture(actor, &closed, true)
                    .is_err()
            );
            assert_eq!(session.face_preview_token(), token);
            assert_eq!(session.face_preview_document().unwrap().vmf(), &preview);
            assert_eq!(session.document().vmf(), &before);
            session.cancel_face_preview();
            session
                .propagate_face_texture(actor, &targets, true)
                .unwrap();
            session.commit_face_preview(actor).unwrap();
            assert_eq!(session.document().vmf(), &preview);
            assert_eq!(session.selected_faces(), selected);
            assert_eq!(session.document().undo_labels().count(), history + 1);
            session.undo(actor).unwrap();
            assert_eq!(session.document().vmf(), &before);
            session.redo(actor).unwrap();
            assert_eq!(session.document().vmf(), &preview);
            session.undo(actor).unwrap();
            session
                .propagate_face_texture(actor, &targets, true)
                .unwrap();
            let token = session.face_preview_token();
            session.sample_face_texture(source).unwrap();
            assert!(!session.has_face_preview());
            assert!(session.check_face_preview_token(token).is_err());
            assert_eq!(session.document().vmf(), &before);
        }
    }

    #[test]
    fn grouped_projection_shares_previews_history_and_material_validation() {
        for actor in [Actor::User, Actor::Agent] {
            let (_directory, game) = crate::test_assets::game();
            let mut session = Session::new(Ok(game));
            let mut faces = Vec::new();
            for x in [0.0, 128.0] {
                let ObjectId::Solid(solid) = session
                    .create_block(
                        actor,
                        DVec3::new(x, 0.0, 0.0),
                        DVec3::new(x + 128.0, 64.0, 16.0),
                        "test/checker",
                        false,
                    )
                    .unwrap()
                else {
                    panic!()
                };
                let side = session
                    .document
                    .solid(solid)
                    .unwrap()
                    .sides
                    .iter()
                    .find(|side| {
                        map_core::Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z
                    })
                    .unwrap()
                    .id;
                faces.push(FaceId { solid, side });
            }
            session.select_faces(&faces, SelectMode::Replace).unwrap();
            let before = session.document.vmf().clone();
            let history = session.document.undo_labels().count();
            session
                .project_face_group(
                    actor,
                    &faces,
                    GroupProjection::Face(None),
                    None,
                    DVec2::ONE,
                    true,
                )
                .unwrap();
            let token = session.face_preview_token();
            assert_eq!(
                session.preview_side(faces[0]).unwrap().u_axis,
                session.preview_side(faces[1]).unwrap().u_axis
            );
            assert_eq!(session.document.vmf(), &before);
            assert_eq!(session.document.undo_labels().count(), history);
            assert!(
                session
                    .project_face_group(
                        actor,
                        &faces,
                        GroupProjection::Face(None),
                        Some("missing/material"),
                        DVec2::ONE,
                        true
                    )
                    .is_err()
            );
            assert_eq!(session.face_preview_token(), token);
            session.cancel_face_preview();
            assert_eq!(session.document.vmf(), &before);
            session
                .project_face_group(
                    actor,
                    &faces,
                    GroupProjection::Face(None),
                    None,
                    DVec2::new(2.0, 1.0),
                    true,
                )
                .unwrap();
            session.commit_face_preview(actor).unwrap();
            assert_eq!(session.document.undo_labels().count(), history + 1);
            session.undo(actor).unwrap();
            assert_eq!(session.document.vmf(), &before);
            session
                .edit_faces(
                    actor,
                    &faces[..1],
                    SurfaceEdit {
                        material: Some("test/other".into()),
                        ..Default::default()
                    },
                    false,
                    false,
                )
                .unwrap();
            assert!(
                session
                    .project_face_group(
                        actor,
                        &faces,
                        GroupProjection::Face(None),
                        None,
                        DVec2::ONE,
                        false
                    )
                    .is_err()
            );
            session
                .project_face_group(
                    actor,
                    &faces,
                    GroupProjection::View,
                    Some("test/checker"),
                    DVec2::ONE,
                    true,
                )
                .unwrap();
            session.select_faces(&[], SelectMode::Replace).unwrap();
            assert!(session.commit_face_preview(actor).is_err());
        }
    }

    #[test]
    fn lighting_previews_tokens_and_diagnostic_settings_share_session_state() {
        for actor in [Actor::User, Actor::Agent] {
            let mut session = Session::new(Err("no assets needed".into()));
            let ObjectId::Solid(id) = session
                .create_block(actor, DVec3::ZERO, DVec3::splat(64.0), "brick/test", false)
                .unwrap()
            else {
                panic!()
            };
            let face = FaceId {
                solid: id,
                side: session.document().solid(id).unwrap().sides[0].id,
            };
            session.select_faces(&[face], SelectMode::Replace).unwrap();
            let before = session.document().vmf().clone();
            let history = session.document().undo_labels().count();
            let patch = SurfaceEdit {
                lightmap_scale: Some(8),
                smoothing_add: surface::smoothing_mask(&[1, 32]).unwrap(),
                ..Default::default()
            };
            session
                .edit_faces(actor, &[face], patch.clone(), false, true)
                .unwrap();
            let token = session.face_preview_token();
            assert_eq!(session.preview_side(face).unwrap().lightmap_scale, 8);
            assert_eq!(session.document().vmf(), &before);
            assert_eq!(session.document().undo_labels().count(), history);
            session
                .set_surface_view(render::SurfaceView::LightmapDensity, 32)
                .unwrap();
            assert_eq!(session.face_preview_token(), token);
            assert_eq!(session.view().surface, render::SurfaceView::LightmapDensity);
            let view = session.view();
            assert!(
                session
                    .set_surface_view(render::SurfaceView::Textured, 0)
                    .is_err()
            );
            assert_eq!(session.view(), view);
            session
                .edit_faces(
                    actor,
                    &[face],
                    SurfaceEdit {
                        lightmap_scale: Some(4),
                        ..patch
                    },
                    false,
                    true,
                )
                .unwrap();
            assert!(session.check_face_preview_token(token).is_err());
            session
                .check_face_preview_token(session.face_preview_token())
                .unwrap();
            session.commit_face_preview(actor).unwrap();
            assert_eq!(session.document().undo_labels().count(), history + 1);
            assert_eq!(
                surface::side(session.document(), face)
                    .unwrap()
                    .smoothing_groups,
                Some(0x8000_0001)
            );
            session.undo(actor).unwrap();
            assert_eq!(session.document().vmf(), &before);
            assert!(session.check_face_preview_token(token).is_err());
        }
    }

    #[test]
    fn sampling_is_read_only_and_paint_previews_are_atomic_and_cancellable() {
        let mut session = Session::new(Err("no assets needed".into()));
        let ObjectId::Solid(id) = session
            .create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/source",
                false,
            )
            .unwrap()
        else {
            panic!()
        };
        let brush = session.document().solid(id).unwrap();
        let top = FaceId {
            solid: id,
            side: brush
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
                .unwrap()
                .id,
        };
        let wall = FaceId {
            solid: id,
            side: brush
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal == DVec3::Y)
                .unwrap()
                .id,
        };
        session
            .edit_faces(
                Actor::User,
                &[top],
                SurfaceEdit {
                    material: Some("metal/sample".into()),
                    shift_u: Some(13.0),
                    ..Default::default()
                },
                false,
                false,
            )
            .unwrap();
        let revision = session.document().revision();
        session.sample_face_texture(top).unwrap();
        assert_eq!(session.document().revision(), revision);
        session.select_faces(&[wall], SelectMode::Replace).unwrap();
        let original = session.document().vmf().clone();
        assert!(
            session
                .paint_face_texture(Actor::Agent, &[wall], PaintMode::Projection, true)
                .is_err()
        );
        assert!(!session.has_face_preview());
        session
            .paint_face_texture(Actor::Agent, &[wall], PaintMode::Wrap, true)
            .unwrap();
        assert_eq!(session.document().vmf(), &original);
        assert_eq!(session.preview_side(wall).unwrap().material, "METAL/SAMPLE");
        let invalid = FaceId {
            solid: id,
            side: u32::MAX,
        };
        assert!(
            session
                .paint_face_texture(Actor::Agent, &[wall, invalid], PaintMode::Material, true)
                .is_err()
        );
        assert!(session.has_face_preview());
        assert_eq!(session.preview_faces(), vec![wall]);
        session.cancel_face_preview();
        assert_eq!(session.document().vmf(), &original);
        session
            .paint_face_texture(Actor::User, &[wall], PaintMode::Wrap, true)
            .unwrap();
        session.commit_face_preview(Actor::Agent).unwrap();
        assert_eq!(
            surface::side(session.document(), wall).unwrap().material,
            "METAL/SAMPLE"
        );
        session.undo(Actor::User).unwrap();
        assert_eq!(session.document().vmf(), &original);
        session.redo(Actor::User).unwrap();
        session
            .edit_faces(
                Actor::User,
                &[top],
                SurfaceEdit {
                    material: Some("wood/new".into()),
                    ..Default::default()
                },
                false,
                false,
            )
            .unwrap();
        assert_eq!(session.surface_sample().unwrap().material, "METAL/SAMPLE");
        session
            .paint_face_texture(Actor::Agent, &[wall], PaintMode::Material, true)
            .unwrap();
        session.select_faces(&[top], SelectMode::Replace).unwrap();
        assert!(!session.has_face_preview());
        session.replace_document(Document::empty(), None);
        assert!(session.surface_sample().is_none());
    }
}
#[test]
fn terrain_smoothing_preview_repairs_fields_and_preserves_geometry_for_both_actors() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("no assets needed".into()));
        let ObjectId::Solid(solid) = session
            .create_block(
                actor,
                glam::DVec3::ZERO,
                glam::DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        let side = session
            .document()
            .solid(solid)
            .unwrap()
            .sides
            .iter()
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id;
        let face = FaceId { solid, side };
        session.create_terrain(actor, &[face], 2, 0).unwrap();
        let source = session.document().vmf().clone();
        let before = session
            .terrain_grids(&[face], false)
            .unwrap()
            .remove(0)
            .mesh;
        session
            .preview_terrain_edit(
                actor,
                &[face],
                map_core::displacement::Operation::Smoothing(Some(45.5)),
            )
            .unwrap();
        let preview = session.terrain_grids(&[face], true).unwrap().remove(0);
        assert_eq!(preview.smoothing_angle, Some(45.5));
        assert_eq!(preview.mesh, before);
        let token = session.face_preview_token();
        assert!(
            session
                .preview_terrain_edit(
                    actor,
                    &[face],
                    map_core::displacement::Operation::Smoothing(Some(181.0))
                )
                .is_err()
        );
        assert_eq!(session.face_preview_token(), token);
        session.commit_face_preview(actor).unwrap();
        assert_eq!(
            session.document().undo_labels().next(),
            Some("edit terrain border smoothing")
        );
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &source);
        session
            .edit(actor, "invalid smooth field", |tx| {
                tx.solid_mut(solid)?
                    .sides
                    .iter_mut()
                    .find(|side| side.id == face.side)
                    .unwrap()
                    .displacement
                    .as_mut()
                    .unwrap()
                    .extra_properties
                    .set("smooth", "invalid");
                Ok(())
            })
            .unwrap();
        assert!(
            session.terrain_grids(&[face], false).unwrap()[0]
                .smoothing_error
                .is_some()
        );
        session
            .preview_terrain_edit(
                actor,
                &[face],
                map_core::displacement::Operation::Smoothing(None),
            )
            .unwrap();
        assert!(
            session.terrain_grids(&[face], true).unwrap()[0]
                .smoothing_error
                .is_none()
        );
        session.cancel_face_preview();
        assert!(
            session.terrain_grids(&[face], false).unwrap()[0]
                .smoothing_error
                .is_some()
        );
    }
}

#[test]
fn terrain_triangle_subsets_share_actors_and_preserve_failed_drafts_and_history() {
    use map_core::displacement::triangles::{Choice, Settings};
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("no assets needed".into()));
        let mut faces = Vec::new();
        for power in [2, 3] {
            let ObjectId::Solid(solid) = session
                .create_block(
                    actor,
                    glam::DVec3::ZERO,
                    glam::DVec3::splat(64.0),
                    "brick/test",
                    false,
                )
                .unwrap()
            else {
                unreachable!()
            };
            let side = session
                .document()
                .solid(solid)
                .unwrap()
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                .unwrap()
                .id;
            let face = FaceId { solid, side };
            session
                .create_terrain(actor, &[face], power, power - 2)
                .unwrap();
            faces.push(face);
        }
        session.select_faces(&faces, SelectMode::Replace).unwrap();
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let settings = Settings {
            walkable: Some(Choice::Disabled),
            buildable: Some(Choice::Enabled),
        };
        let targets = vec![(faces[0], vec![0, 31]), (faces[1], vec![7, 127])];
        assert_eq!(
            session
                .preview_terrain_triangle_tags(actor, &targets, settings)
                .unwrap(),
            2
        );
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
        let preview = session.face_preview_token();
        let rows = session.terrain_triangles(&faces, true).unwrap();
        for (row, (_, selected)) in rows.iter().zip(&targets) {
            for triangle in &row.triangles {
                assert_eq!(triangle.walkable, !selected.contains(&triangle.id));
                assert_eq!(
                    triangle.buildable_override,
                    selected.contains(&triangle.id).then_some(true)
                );
            }
        }
        assert!(
            session
                .preview_terrain_triangle_tags(
                    actor,
                    &[(faces[0], vec![1]), (faces[1], vec![128])],
                    settings
                )
                .is_err()
        );
        assert!(
            session
                .preview_terrain_triangle_tags(
                    actor,
                    &[(faces[0], vec![1]), (faces[0], vec![2])],
                    settings
                )
                .is_err()
        );
        assert_eq!(session.face_preview_token(), preview);
        session.cancel_face_preview();
        assert_eq!(session.document().vmf(), &source);
        session
            .preview_terrain_triangle_tags(actor, &targets, settings)
            .unwrap();
        session.commit_face_preview(actor).unwrap();
        assert_eq!(session.document().undo_labels().count(), history + 1);
        assert_eq!(
            session.document().undo_labels().next(),
            Some("edit terrain triangle tags")
        );
        let changed = session.document().vmf().clone();
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &source);
        session.redo(actor).unwrap();
        assert_eq!(session.document().vmf(), &changed);
    }
}

#[cfg(test)]
#[path = "surface_vectors_tests.rs"]
mod vector_tests;
