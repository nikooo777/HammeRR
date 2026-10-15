use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::*;

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    #[default]
    Map,
    SelectedObjects,
    SelectedFaces,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct Query {
    pub material: String,
    #[serde(default)]
    pub scope: Scope,
    #[serde(default)]
    pub include_hidden: bool,
}

#[derive(Default)]
pub struct State {
    serial: u64,
    pub report: Option<Report>,
}

pub struct Report {
    pub token: u64,
    pub query: Query,
    pub faces: Vec<FaceId>,
    pub hidden_matches: usize,
    pub hidden_excluded: usize,
    matched_objects: usize,
    generation: u64,
    revision: u64,
    objects: Vec<ObjectId>,
    selection: Vec<FaceId>,
}

pub fn material_name(value: &str) -> Result<String, String> {
    let name = value.trim().replace('\\', "/").to_ascii_lowercase();
    let name = name.strip_prefix("materials/").unwrap_or(&name);
    let name = name.strip_suffix(".vmt").unwrap_or(name);
    if name.is_empty()
        || name.chars().any(char::is_control)
        || name.contains(['*', '?'])
        || name
            .split('/')
            .any(|part| part.is_empty() || part == ".." || part == ".")
    {
        return Err(
            "Enter a material path, without wildcards (for example brick/brickwall001a)".into(),
        );
    }
    Ok(name.into())
}

impl Session {
    pub fn material_search_report(&self) -> Option<&Report> {
        self.material_search.report.as_ref()
    }

    pub fn material_search_stale(&self) -> bool {
        self.material_search.report.as_ref().is_some_and(|report| {
            report.generation != self.document_generation()
                || report.revision != self.document.revision()
                || (report.query.scope == Scope::SelectedObjects
                    && report.objects != self.selected_ids())
                || (report.query.scope == Scope::SelectedFaces
                    && report.selection != self.selected_faces())
        })
    }

    fn checked_material_search(&self, token: u64) -> Result<&Report, String> {
        let report = self
            .material_search_report()
            .ok_or("Find material faces first")?;
        if report.token != token || self.material_search_stale() {
            return Err("Material results changed or became stale. Find faces again.".into());
        }
        Ok(report)
    }

    pub fn find_material_faces(&mut self, mut query: Query) -> Result<u64, String> {
        query.material = material_name(&query.material)?;
        let objects = self.selected_ids();
        let selection = self.selected_faces();
        if (query.scope == Scope::SelectedObjects && objects.is_empty())
            || (query.scope == Scope::SelectedFaces && selection.is_empty())
        {
            return Err("The chosen search scope has no selection".into());
        }
        let mut faces = Vec::new();
        let mut hidden_matches = 0;
        let mut hidden_excluded = 0;
        let mut matched_objects = BTreeSet::new();
        let vmf = self.document.vmf();
        let solids = vmf
            .world
            .solids
            .iter()
            .map(|solid| (ObjectId::Solid(solid.id), false, solid))
            .chain(vmf.entities.iter().flat_map(|entity| {
                entity
                    .solids
                    .iter()
                    .map(move |solid| (ObjectId::Entity(entity.id), entity.hidden, solid))
            }));
        for (owner, owner_hidden, solid) in solids {
            if query.scope == Scope::SelectedObjects && objects.binary_search(&owner).is_err() {
                continue;
            }
            let hidden = solid.hidden || owner_hidden;
            for side in &solid.sides {
                let face = FaceId {
                    solid: solid.id,
                    side: side.id,
                };
                if query.scope == Scope::SelectedFaces && selection.binary_search(&face).is_err() {
                    continue;
                }
                if material_name(&side.material).ok().as_deref() != Some(&query.material) {
                    continue;
                }
                if hidden && !query.include_hidden {
                    hidden_excluded += 1;
                    continue;
                }
                hidden_matches += usize::from(hidden);
                matched_objects.insert(owner);
                faces.push(face);
            }
        }
        faces.sort_unstable();
        self.material_search.serial += 1;
        let token = self.material_search.serial;
        self.material_search.report = Some(Report {
            token,
            query,
            faces,
            hidden_matches,
            hidden_excluded,
            matched_objects: matched_objects.len(),
            generation: self.document_generation(),
            revision: self.document.revision(),
            objects,
            selection,
        });
        self.view_serial += 1;
        Ok(token)
    }

    pub fn material_search_json(&self, offset: usize, limit: usize) -> Value {
        self.material_search_report().map_or(Value::Null, |report| {
            json!({"token":report.token.to_string(), "query":report.query,
                "generation":report.generation, "revision":report.revision, "stale":self.material_search_stale(),
                "total":report.faces.len(), "objects":report.matched_objects, "hidden_matches":report.hidden_matches,
                "hidden_excluded":report.hidden_excluded, "offset":offset,
                "faces":report.faces.iter().skip(offset).take(limit.clamp(1,500)).map(|face| json!({"solid":face.solid,"side":face.side})).collect::<Vec<_>>()})
        })
    }

    pub fn select_material_faces(&mut self, token: u64, mode: SelectMode) -> Result<usize, String> {
        let faces = self.checked_material_search(token)?.faces.clone();
        self.select_faces(&faces, mode)?;
        Ok(faces.len())
    }

    pub fn replace_materials(
        &mut self,
        actor: Actor,
        token: u64,
        material: &str,
        preview: bool,
    ) -> Result<usize, String> {
        let material = material_name(material)?;
        let report = self.checked_material_search(token)?;
        if report.faces.is_empty() {
            return Err("No matching faces to replace".into());
        }
        if material == report.query.material {
            return Err("Choose a different replacement material".into());
        }
        let faces = report.faces.clone();
        self.edit_faces(
            actor,
            &faces,
            SurfaceEdit {
                material: Some(material),
                ..Default::default()
            },
            false,
            preview,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(session: &mut Session, material: &str) -> ObjectId {
        session
            .create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::splat(64.0),
                material,
                false,
            )
            .unwrap()
    }

    fn query(scope: Scope, include_hidden: bool) -> Query {
        Query {
            material: "Materials\\BRICK/Test.vmt".into(),
            scope,
            include_hidden,
        }
    }

    #[test]
    fn replacement_includes_nested_hidden_faces_only_when_requested_and_preserves_metadata() {
        let mut session = Session::new(Err("no assets".into()));
        let visible = block(&mut session, "brick/test");
        let hidden = block(&mut session, "brick/test");
        session
            .set_visibility(Actor::User, "hide", &[hidden])
            .unwrap();
        let child = block(&mut session, "brick/test");
        session
            .tie_to_entity(Actor::User, &[child], "func_detail", BTreeMap::new())
            .unwrap();
        let ObjectId::Entity(entity) = session.selected_ids()[0] else {
            panic!()
        };
        session
            .edit(Actor::User, "hide child", |tx| {
                tx.entity_mut(entity)?.solids[0].hidden = true;
                Ok(())
            })
            .unwrap();
        let hidden_entity_brush = block(&mut session, "brick/test");
        session
            .tie_to_entity(
                Actor::User,
                &[hidden_entity_brush],
                "func_detail",
                BTreeMap::new(),
            )
            .unwrap();
        let hidden_entity = session.selected_ids()[0];
        session
            .set_visibility(Actor::User, "hide", &[hidden_entity])
            .unwrap();
        block(&mut session, "brick/test_extra");
        let ObjectId::Solid(id) = visible else {
            panic!()
        };
        let face = FaceId {
            solid: id,
            side: session.document.solid(id).unwrap().sides[0].id,
        };
        session
            .edit_faces(
                Actor::User,
                &[face],
                SurfaceEdit {
                    lightmap_scale: Some(8),
                    smoothing_groups: Some(0x8000_0001),
                    shift_u: Some(13.0),
                    ..Default::default()
                },
                false,
                false,
            )
            .unwrap();
        let before = session.document.vmf().clone();
        let history = session.document.undo_labels().count();
        let token = session
            .find_material_faces(query(Scope::Map, false))
            .unwrap();
        let report = session.material_search_report().unwrap();
        assert_eq!(report.faces.len(), 6);
        assert_eq!(report.hidden_excluded, 18);
        assert_eq!(report.hidden_matches, 0);
        assert!(
            session
                .replace_materials(Actor::Agent, token, "brick/test", true)
                .is_err()
        );
        assert_eq!(
            session
                .replace_materials(Actor::Agent, token, "metal/new", true)
                .unwrap(),
            6
        );
        assert_eq!(session.document.vmf(), &before);
        assert_eq!(session.document.undo_labels().count(), history);
        session.cancel_face_preview();
        let token = session
            .find_material_faces(query(Scope::Map, true))
            .unwrap();
        assert_eq!(session.material_search_report().unwrap().hidden_matches, 18);
        assert_eq!(
            session
                .replace_materials(Actor::User, token, "Materials/METAL/new.vmt", true)
                .unwrap(),
            24
        );
        session.commit_face_preview(Actor::User).unwrap();
        let mut expected = before.clone();
        for solid in expected.world.solids.iter_mut().chain(
            expected
                .entities
                .iter_mut()
                .flat_map(|entity| &mut entity.solids),
        ) {
            for side in &mut solid.sides {
                if side.material == "BRICK/TEST" {
                    side.material = "METAL/NEW".into();
                }
            }
        }
        assert_eq!(session.document.vmf(), &expected);
        assert_eq!(session.document.undo_labels().count(), history + 1);
        assert!(session.material_search_stale());
        session.undo(Actor::User).unwrap();
        assert_eq!(session.document.vmf(), &before);
        session.redo(Actor::Agent).unwrap();
        assert_eq!(session.document.vmf(), &expected);
        let parsed = Vmf::parse(&session.document.vmf().write(&WriteOptions::default())).unwrap();
        assert_eq!(parsed, expected);
    }

    #[test]
    fn selection_scopes_and_search_tokens_reject_stale_or_replaced_results() {
        let mut session = Session::new(Err("no assets".into()));
        let a = block(&mut session, "brick/test");
        let b = block(&mut session, "brick/test");
        session.select(&[a], SelectMode::Replace).unwrap();
        let first = session
            .find_material_faces(query(Scope::SelectedObjects, false))
            .unwrap();
        assert_eq!(session.material_search_report().unwrap().faces.len(), 6);
        session.select(&[b], SelectMode::Replace).unwrap();
        assert!(
            session
                .replace_materials(Actor::Agent, first, "metal/test", false)
                .is_err()
        );
        let token = session
            .find_material_faces(query(Scope::SelectedObjects, false))
            .unwrap();
        assert!(
            session
                .select_material_faces(first, SelectMode::Replace)
                .is_err()
        );
        session
            .select_material_faces(token, SelectMode::Replace)
            .unwrap();
        assert_eq!(session.selected_faces().len(), 6);
        assert!(session.material_search_stale());
        let face = session.selected_faces()[0];
        session.select_faces(&[face], SelectMode::Replace).unwrap();
        let token = session
            .find_material_faces(query(Scope::SelectedFaces, false))
            .unwrap();
        assert_eq!(session.material_search_report().unwrap().faces, [face]);
        session
            .replace_materials(Actor::Agent, token, "metal/test", false)
            .unwrap();
        assert!(
            session
                .replace_materials(Actor::Agent, token, "wood/test", false)
                .is_err()
        );
        session.new_map(Actor::User, true).unwrap();
        assert!(session.material_search_report().is_none());
        block(&mut session, "brick/test");
        let new_token = session
            .find_material_faces(query(Scope::Map, false))
            .unwrap();
        assert_ne!(new_token, first);
        assert!(
            session
                .select_material_faces(first, SelectMode::Replace)
                .is_err()
        );
    }

    #[test]
    fn queries_validate_paths_and_page_results_without_editing_history() {
        let mut session = Session::new(Err("no assets".into()));
        for invalid in [
            "",
            "*",
            "brick/?",
            "../brick/test",
            "materials/.vmt",
            "brick//test",
        ] {
            assert!(
                session
                    .find_material_faces(Query {
                        material: invalid.into(),
                        ..query(Scope::Map, false)
                    })
                    .is_err()
            );
        }
        assert!(
            session
                .find_material_faces(query(Scope::SelectedObjects, false))
                .is_err()
        );
        block(&mut session, "brick/test");
        let before = session.document.vmf().clone();
        let history = session.document.undo_labels().count();
        let token = session
            .find_material_faces(query(Scope::Map, false))
            .unwrap();
        let page = session.material_search_json(2, 2);
        assert_eq!(page["total"], 6);
        assert_eq!(page["faces"].as_array().unwrap().len(), 2);
        session
            .select_material_faces(token, SelectMode::Replace)
            .unwrap();
        assert_eq!(session.selected_faces().len(), 6);
        assert!(!session.material_search_stale());
        session
            .select_material_faces(token, SelectMode::Toggle)
            .unwrap();
        assert!(session.selected_faces().is_empty());
        assert_eq!(session.document.vmf(), &before);
        assert_eq!(session.document.undo_labels().count(), history);
    }
}
