use crate::identity::id_string as object_name;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use map_core::checks::{self, Fix, Issue, Location, Report, Severity};
use map_core::surface::FaceId;
use map_core::{ObjectId, Plane};
use serde_json::{Value, json};
use source_formats::fgd::ClassKind;
use source_formats::vmf::UvAxis;
use source_formats::vmt::material_path;

use super::{Actor, SelectMode, Session};

mod spawns;

const SDK_MAX_BRUSHES: usize = 8192;
const SDK_MAX_ENTITIES: usize = 8192;
const SDK_MAX_BRUSH_SIDES: usize = 65536;

#[derive(Default)]
pub struct State {
    serial: u64,
    report: Option<MapReport>,
    draft: Option<FixDraft>,
}

pub struct MapReport {
    pub token: u64,
    pub generation: u64,
    pub revision: u64,
    pub definitions_revision: u64,
    material_generation: u64,
    map_version: Option<String>,
    instance_source_path: Option<Option<std::path::PathBuf>>,
    pub elapsed_ms: u128,
    pub result: Report,
}

#[derive(Clone)]
pub struct FixDraft {
    pub token: u64,
    pub report: u64,
    pub issues: Vec<usize>,
    pub changes: Vec<Value>,
}

pub fn issue_json(index: usize, issue: &Issue) -> Value {
    json!({"id": index.to_string(), "code": issue.code, "severity": issue.severity.label(), "message": issue.message,
        "object": issue.location.object.map(object_name), "face": issue.location.face.map(|face| json!({"solid":face.solid,"side":face.side})),
        "property": issue.location.property, "connection": issue.location.connection, "position": issue.location.position.map(|point| point.to_array()),
        "path": issue.location.path, "hidden": issue.location.hidden, "ambiguous": issue.location.ambiguous,
        "selectable": issue.location.object.is_some() && !issue.location.ambiguous,
        "fix": issue.fix.map(|fix| match fix { Fix::ResetProjection(_) => "reset_projection", Fix::ResetLightmapScale(_) => "reset_lightmap_scale" })})
}

impl Session {
    pub fn map_check_serial(&self) -> u64 {
        self.checks.serial
    }
    pub fn map_check_report(&self) -> Option<&MapReport> {
        self.checks.report.as_ref()
    }
    pub fn map_fix_draft(&self) -> Option<&FixDraft> {
        self.checks.draft.as_ref()
    }

    pub fn map_checks_stale(&self) -> bool {
        self.checks.report.as_ref().is_some_and(|report| {
            report.generation != self.document_generation
                || report.revision != self.document.revision()
                || report.definitions_revision != self.definitions_serial
                || report.material_generation != self.materials.generation()
                || report.map_version.as_deref()
                    != self.document.vmf().world.properties.get("mapversion")
                || report
                    .instance_source_path
                    .as_ref()
                    .is_some_and(|path| path != &self.path)
        })
    }

    pub fn run_map_checks(&mut self, actor: Actor) -> u64 {
        let start = Instant::now();
        let vmf = self.document.vmf();
        let mut result = checks::check(vmf);
        let mut counts = BTreeMap::<u32, usize>::new();
        for id in std::iter::once(vmf.world.id)
            .chain(vmf.world.groups.iter().map(|group| group.id))
            .chain(vmf.solids().map(|solid| solid.id))
            .chain(vmf.entities.iter().map(|entity| entity.id))
        {
            *counts.entry(id).or_default() += 1;
        }
        let mut face_counts = BTreeMap::<u32, usize>::new();
        for side in vmf.solids().flat_map(|solid| &solid.sides) {
            *face_counts.entry(side.id).or_default() += 1;
        }
        let mut asset_checks = self.game_error.is_none();
        let mut material_errors = BTreeMap::<String, Option<String>>::new();
        let (materials, models) = if asset_checks {
            match self.game.list("materials", ".vmt").and_then(|materials| {
                self.game
                    .list("models", ".mdl")
                    .map(|models| (materials, models))
            }) {
                Ok((materials, models)) => (
                    materials.into_iter().collect::<BTreeSet<_>>(),
                    models.into_iter().collect::<BTreeSet<_>>(),
                ),
                Err(error) => {
                    asset_checks = false;
                    result.add(
                        Severity::Error,
                        "asset_scan_failed",
                        &Location::default(),
                        format!("Game content could not be inspected: {error}"),
                        None,
                    );
                    (BTreeSet::new(), BTreeSet::new())
                }
            }
        } else {
            (BTreeSet::new(), BTreeSet::new())
        };
        if !asset_checks {
            result.add(
                Severity::Warning,
                "assets_unchecked",
                &Location::default(),
                "Game content could not be fully inspected. Material and model existence checks were skipped.",
                None,
            );
        }
        let has_definitions = self.definitions.classes().next().is_some();
        if !has_definitions {
            result.add(
                Severity::Warning,
                "definitions_unchecked",
                &Location::default(),
                "No FGD is loaded. Entity class and property checks were skipped.",
                None,
            );
        }
        if let Some(class) = self.definitions.class("worldspawn") {
            for (key, value) in vmf.world.properties.iter() {
                if let Some(property) = class.property(key)
                    && let Err(error) = property.validate(value)
                {
                    result.add(
                        Severity::Warning,
                        "property_value",
                        &Location {
                            path: "world".into(),
                            property: Some(key.into()),
                            ..Default::default()
                        },
                        error,
                        None,
                    );
                }
            }
        }
        for (index, entity) in vmf.entities.iter().enumerate() {
            let location = Location {
                object: Some(ObjectId::Entity(entity.id)),
                position: entity.origin().filter(|point| point.is_finite()),
                hidden: entity.hidden,
                ambiguous: entity.id == 0 || counts[&entity.id] != 1,
                path: format!("entity[{index}]"),
                ..Default::default()
            };
            let classname = entity.classname().unwrap_or_default();
            if let Some(class) = self.definitions.class(classname) {
                if (class.kind == ClassKind::Solid && entity.solids.is_empty())
                    || (class.kind == ClassKind::Point && !entity.solids.is_empty())
                {
                    result.add(
                        Severity::Warning,
                        "entity_geometry",
                        &location,
                        format!(
                            "{classname} is a {} entity in the FGD, but contains {} brushes.",
                            class.kind.label(),
                            entity.solids.len()
                        ),
                        None,
                    );
                }
                for (key, value) in entity.properties.iter() {
                    if key.eq_ignore_ascii_case("origin") {
                        continue;
                    }
                    if let Some(property) = class.property(key)
                        && let Err(error) = property.validate(value)
                    {
                        let mut property_location = location.clone();
                        property_location.property = Some(key.into());
                        result.add(
                            Severity::Warning,
                            "property_value",
                            &property_location,
                            error,
                            None,
                        );
                    }
                }
            } else if has_definitions && !classname.is_empty() {
                result.add(Severity::Warning, "unknown_class", &location, format!("{classname} is not declared in the loaded FGD. Custom classes are preserved; verify support in the game."), None);
            }
            if !entity.hidden && classname.eq_ignore_ascii_case("func_instance") {
                let admission=compiler::instances::helper_admission(entity).and_then(|()|self.path.as_ref().map(|_|()).ok_or_else(||"Save the parent VMF before compiling instances; an output BSP path is not a source directory".into()));
                let (severity,message)=match admission {
                    Ok(())=>(Severity::Info,"Brush-only instance helper admitted; referenced geometry and dependencies are checked during compilation from the saved parent directory. This check does not load the child or expand the viewport.".into()),
                    Err(error)=>(Severity::Error,error),
                };
                result.add(severity, "compiler_entity", &location, message, None);
            } else if let Some(diagnostic) = compiler::map::unresolved_instance_diagnostic(entity) {
                result.add(
                    Severity::Error,
                    "compiler_entity",
                    &location,
                    diagnostic.message,
                    None,
                );
            } else if compiler::map::unsupported_entity_class(classname) && !entity.hidden {
                result.add(
                    Severity::Warning,
                    "compiler_entity",
                    &location,
                    format!("The native compiler currently omits {classname}."),
                    None,
                );
            }
            if let Some(model) = entity
                .properties
                .get("model")
                .filter(|value| !value.is_empty() && !value.starts_with('*'))
                && asset_checks
            {
                let normalized = model.replace('\\', "/").to_ascii_lowercase();
                let studio = self
                    .definitions
                    .class(classname)
                    .and_then(|class| class.property("model"))
                    .is_some_and(|property| property.kind == "studio");
                if (studio || normalized.ends_with(".mdl")) && !models.contains(&normalized) {
                    let mut model_location = location.clone();
                    model_location.property = Some("model".into());
                    result.add(
                        Severity::Warning,
                        "missing_model",
                        &model_location,
                        format!("Model {model:?} was not found on the game search path."),
                        None,
                    );
                }
            }
            for (connection_index, connection) in entity.connections.iter().flatten().enumerate() {
                let mut connection_location = location.clone();
                connection_location.connection = Some(connection_index);
                match map_core::connections::parse(connection).and_then(|spec| {
                    spec.validate(map_core::connections::separator(connection))?;
                    Ok(spec)
                }) {
                    Ok(spec) => {
                        for warning in self.connection_problems(entity.id, &spec) {
                            result.add(
                                Severity::Warning,
                                "connection",
                                &connection_location,
                                warning,
                                None,
                            );
                        }
                    }
                    Err(error) => result.add(
                        Severity::Error,
                        "invalid_connection",
                        &connection_location,
                        error,
                        None,
                    ),
                }
            }
        }
        for reference in map_core::references::scan(&self.document, &self.definitions) {
            if !reference.declared || reference.target.starts_with('!') {
                continue;
            }
            let map_core::references::Location::Property { key, .. } = reference.location else {
                continue;
            };
            if map_core::connections::targets(&self.document, reference.source, &reference.target)
                .is_empty()
            {
                let Some(index) = vmf
                    .entities
                    .iter()
                    .position(|entity| entity.id == reference.source)
                else {
                    continue;
                };
                let entity = &vmf.entities[index];
                let location = Location {
                    object: Some(ObjectId::Entity(entity.id)),
                    property: Some(key),
                    hidden: entity.hidden,
                    ambiguous: counts[&entity.id] != 1,
                    path: format!("entity[{index}]"),
                    position: entity.origin().filter(|point| point.is_finite()),
                    ..Default::default()
                };
                result.add(
                    Severity::Warning,
                    "missing_reference",
                    &location,
                    format!(
                        "No visible map entity matches {:?}; it may be created at runtime.",
                        reference.target
                    ),
                    None,
                );
            }
        }
        for solid in vmf.solids() {
            let owner = self.document.owner_of_solid(solid.id);
            let hidden = solid.hidden || owner.is_some_and(|owner| matches!(owner, ObjectId::Entity(id) if self.document.entity(id).is_some_and(|entity|entity.hidden)));
            let base = Location {
                object: owner,
                hidden,
                ambiguous: solid.id == 0
                    || counts[&solid.id] != 1
                    || owner.is_some_and(|id| {
                        let (ObjectId::Entity(id) | ObjectId::Solid(id)) = id;
                        counts[&id] != 1
                    }),
                path: format!("solid:{}", solid.id),
                ..Default::default()
            };
            if solid.sides.iter().any(|side| side.displacement.is_some()) && !hidden {
                let detail_owner=owner.is_some_and(|owner| matches!(owner,ObjectId::Entity(id) if self.document.entity(id).is_some_and(|entity|entity.classname()==Some("func_detail") && entity.properties.get_all("classname").count()==1)));
                if matches!(owner, Some(ObjectId::Entity(_))) && !detail_owner {
                    result.add(
                    Severity::Warning,
                    "compiler_displacement",
                    &base,
                    "Terrain on runtime brush entities is not supported by the native compiler. Detach its parent to world geometry or use func_detail before building.",
                    None,
                );
                }
                for side in &solid.sides {
                    let Some(info) = &side.displacement else {
                        continue;
                    };
                    let mut location = base.clone();
                    location.face = Some(FaceId {
                        solid: solid.id,
                        side: side.id,
                    });
                    location.ambiguous |= side.id == 0 || face_counts[&side.id] != 1;
                    if info.flags.unwrap_or(0) & !14 != 0 {
                        result.add(Severity::Warning,"compiler_displacement_flags",&location,"Terrain has unsupported native collision/bump flags. Keep supported physics, player and ray collision settings before building.",None);
                    }
                    if info.triangle_tags.as_ref().is_some_and(|rows| {
                        rows.iter().flatten().any(|tag| {
                            tag & i32::from(map_core::displacement::triangles::REMOVE) != 0
                        })
                    }) {
                        result.add(Severity::Warning,"compiler_displacement_triangles",&location,"Removed terrain triangles are not supported by the native compiler yet.",None);
                    }
                }
            }
            if asset_checks {
                for side in &solid.sides {
                    if side.material.trim().is_empty() {
                        continue;
                    }
                    let path = material_path(&side.material);
                    let mut location = base.clone();
                    location.ambiguous |= side.id == 0 || face_counts[&side.id] != 1;
                    location.face = Some(FaceId {
                        solid: solid.id,
                        side: side.id,
                    });
                    if !materials.contains(&path) {
                        result.add(
                            Severity::Warning,
                            "missing_material",
                            &location,
                            format!(
                                "Material {:?} was not found on the game search path.",
                                side.material
                            ),
                            None,
                        );
                    } else if let Some(error) = material_errors.entry(path).or_insert_with(|| {
                        render::materials::MaterialInfo::load(&self.game, &side.material).error
                    }) {
                        result.add(
                            Severity::Error,
                            "invalid_material",
                            &location,
                            error.clone(),
                            None,
                        );
                    }
                }
            }
        }
        for issue in render::scene::terrain_issues(vmf) {
            if result.issues.iter().any(|existing| {
                existing.location.face == Some(issue.face)
                    && matches!(
                        existing.code,
                        "invalid_displacement" | "displacement_smoothing" | "invalid_brush"
                    )
            }) {
                continue;
            }
            let owner = self.document.owner_of_solid(issue.face.solid);
            let location = Location {
                object: owner,
                face: Some(issue.face),
                ambiguous: owner.is_none_or(|owner| {
                    let id = match owner {
                        ObjectId::Solid(id) | ObjectId::Entity(id) => id,
                    };
                    id == 0 || counts[&id] != 1
                }) || issue.face.solid == 0
                    || counts[&issue.face.solid] != 1
                    || issue.face.side == 0
                    || face_counts[&issue.face.side] != 1,
                ..Default::default()
            };
            result.add(
                Severity::Error,
                "terrain_preview",
                &location,
                issue.message,
                None,
            );
        }
        spawns::check(
            vmf,
            asset_checks.then_some(&self.game),
            &counts,
            &mut result,
        );
        for (label, count, limit) in [
            ("brushes", vmf.solids().count(), SDK_MAX_BRUSHES),
            ("entities", vmf.entities.len() + 1, SDK_MAX_ENTITIES),
            (
                "brush sides",
                vmf.solids().map(|solid| solid.sides.len()).sum(),
                SDK_MAX_BRUSH_SIDES,
            ),
        ] {
            if count >= limit {
                result.add(Severity::Warning, "sdk_reference_limit", &Location::default(), format!("{count} {label} reaches the Source SDK 2013 reference limit of {limit}. Compiled counts and CS:S limits may differ."), None);
            }
        }
        result.issues.sort_by_key(|issue| issue.severity);
        self.checks.serial += 1;
        let token = self.checks.serial;
        self.checks.draft = None;
        self.checks.report = Some(MapReport {
            token,
            generation: self.document_generation,
            revision: self.document.revision(),
            definitions_revision: self.definitions_serial,
            material_generation: self.materials.generation(),
            map_version: self
                .document
                .vmf()
                .world
                .properties
                .get("mapversion")
                .map(str::to_owned),
            instance_source_path: compiler::instances::requires_source(self.document.vmf())
                .then(|| self.path.clone()),
            elapsed_ms: start.elapsed().as_millis(),
            result,
        });
        self.log(
            actor,
            format!(
                "checked map: {} issues",
                self.checks.report.as_ref().unwrap().result.issues.len()
            ),
        );
        token
    }

    pub fn map_checks_json(
        &self,
        severity: Option<&str>,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Value, String> {
        if severity.is_some_and(|severity| !matches!(severity, "error" | "warning" | "info")) {
            return Err("Severity must be error, warning or info".into());
        }
        let Some(report) = &self.checks.report else {
            return Ok(
                json!({"report":null,"issues":[],"total":0,"stale":false,"fix_preview":null}),
            );
        };
        let query = query.to_ascii_lowercase();
        let matching = report
            .result
            .issues
            .iter()
            .enumerate()
            .filter(|(_, issue)| {
                severity.is_none_or(|severity| severity == issue.severity.label())
                    && format!(
                        "{} {} {} {}",
                        issue.code,
                        issue.message,
                        issue.location.path,
                        issue.location.object.map(object_name).unwrap_or_default()
                    )
                    .to_ascii_lowercase()
                    .contains(&query)
            })
            .collect::<Vec<_>>();
        let issues = matching
            .iter()
            .skip(offset)
            .take(limit.clamp(1, 200))
            .map(|(index, issue)| issue_json(*index, issue))
            .collect::<Vec<_>>();
        Ok(
            json!({"report":report.token.to_string(),"generation":report.generation,"revision":report.revision,"definitions_revision":report.definitions_revision,"elapsed_ms":report.elapsed_ms,
            "stale":self.map_checks_stale(), "total":report.result.issues.len(),"matching":matching.len(),"offset":offset,"issues":issues,"omitted":report.result.omitted,
            "counts":{"error":report.result.issues.iter().filter(|issue|issue.severity==Severity::Error).count(),"warning":report.result.issues.iter().filter(|issue|issue.severity==Severity::Warning).count(),"info":report.result.issues.iter().filter(|issue|issue.severity==Severity::Info).count()},
            "fix_preview":self.map_fix_preview_json(),"notes":["Checks inspect applied map data, including hidden objects; pending edit previews are excluded.",spawns::SCOPE,"These checks do not prove the map is leak-free or playable. Compile and test in CS:S.","Native builds use conservative visibility and shared Fullbright or Direct LDR lighting settings. Asset checks verify material/model paths, not complete dependencies."]}),
        )
    }

    fn check_map_report(&self, token: u64) -> Result<&MapReport, String> {
        let report = self.checks.report.as_ref().ok_or("Run map checks first")?;
        if report.token != token {
            return Err("Map check report was replaced; read the current report".into());
        }
        if self.map_checks_stale() {
            return Err("Check results are stale; run map checks again".into());
        }
        Ok(report)
    }

    pub fn select_map_issue(
        &mut self,
        actor: Actor,
        token: u64,
        index: usize,
        frame: bool,
        aspect: f32,
    ) -> Result<(), String> {
        let issue = self
            .check_map_report(token)?
            .result
            .issues
            .get(index)
            .ok_or("Issue does not exist")?
            .clone();
        if issue.location.ambiguous {
            return Err(
                "This issue has duplicate or invalid IDs; it cannot be selected unambiguously"
                    .into(),
            );
        }
        let object = issue
            .location
            .object
            .ok_or("This is a map-wide issue with no selectable object")?;
        if frame && (!aspect.is_finite() || aspect <= 0.0) {
            return Err("View aspect must be finite and positive".into());
        }
        if let Some(face) = issue.location.face {
            self.select_faces(&[face], SelectMode::Replace)?;
        } else {
            self.set_face_mode(false);
            self.select(&[object], SelectMode::Replace)?;
        }
        if frame {
            self.frame_selection(actor, aspect)?;
        }
        Ok(())
    }

    pub fn preview_map_fixes(&mut self, token: u64, indices: &[usize]) -> Result<(), String> {
        if indices.is_empty() || indices.len() > 200 {
            return Err("Choose between 1 and 200 issues to repair".into());
        }
        let report = self.check_map_report(token)?;
        let indices = indices
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let mut changes = Vec::new();
        for index in &indices {
            let issue = report
                .result
                .issues
                .get(*index)
                .ok_or("Issue does not exist")?;
            let fix = issue
                .fix
                .ok_or("This issue has no automatic repair; inspect and edit it manually")?;
            let (Fix::ResetProjection(face) | Fix::ResetLightmapScale(face)) = fix;
            let side =
                map_core::surface::side(&self.document, face).ok_or("Face no longer exists")?;
            let (before, after) = match fix {
                Fix::ResetProjection(_) => {
                    let normal = Plane::from_points(side.plane.0)
                        .ok_or("Face plane is invalid")?
                        .normal;
                    let (u, v) = map_core::edit::world_texture_axes(normal);
                    (
                        json!({"u":side.u_axis.axis.to_array(),"v":side.v_axis.axis.to_array(),"scale":[side.u_axis.scale,side.v_axis.scale],"shift":[side.u_axis.offset,side.v_axis.offset],"rotation":side.rotation}),
                        json!({"u":u.to_array(),"v":v.to_array(),"scale":([map_core::edit::DEFAULT_TEXTURE_SCALE;2]),"shift":[0,0],"rotation":0}),
                    )
                }
                Fix::ResetLightmapScale(_) => (
                    json!({"lightmap_scale":side.lightmap_scale}),
                    json!({"lightmap_scale":map_core::edit::DEFAULT_LIGHTMAP_SCALE}),
                ),
            };
            changes.push(json!({"issue":index.to_string(),"face":{"solid":face.solid,"side":face.side},"description":issue.message,"before":before,"after":after}));
        }
        self.checks.serial += 1;
        self.checks.draft = Some(FixDraft {
            token: self.checks.serial,
            report: token,
            issues: indices,
            changes,
        });
        Ok(())
    }

    pub fn map_fix_preview_json(&self) -> Value {
        self.checks.draft.as_ref().map_or(Value::Null,|draft|json!({"token":draft.token.to_string(),"report":draft.report.to_string(),"stale":self.map_checks_stale(),"changes":draft.changes}))
    }

    pub fn cancel_map_fixes(&mut self, token: u64) -> Result<(), String> {
        if self
            .checks
            .draft
            .as_ref()
            .is_none_or(|draft| draft.token != token)
        {
            return Err("Repair preview was closed or replaced".into());
        }
        self.checks.draft = None;
        self.checks.serial += 1;
        Ok(())
    }

    pub fn apply_map_fixes(&mut self, actor: Actor, token: u64) -> Result<usize, String> {
        let draft = self
            .checks
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Repair preview was closed or replaced")?
            .clone();
        let report = self.check_map_report(draft.report)?;
        let fixes = draft
            .issues
            .iter()
            .map(|index| report.result.issues[*index].fix.unwrap())
            .collect::<Vec<_>>();
        self.edit(actor, "repair map issues", |tx| {
            for fix in &fixes {
                let (Fix::ResetProjection(face) | Fix::ResetLightmapScale(face)) = *fix;
                let owner = tx
                    .document()
                    .owner_of_solid(face.solid)
                    .ok_or_else(|| map_core::EditError::Invalid("Brush no longer exists".into()))?;
                let solid = match owner {
                    ObjectId::Solid(id) => tx.solid_mut(id)?,
                    ObjectId::Entity(id) => tx
                        .entity_mut(id)?
                        .solids
                        .iter_mut()
                        .find(|solid| solid.id == face.solid)
                        .ok_or_else(|| {
                            map_core::EditError::Invalid("Brush no longer exists".into())
                        })?,
                };
                let side = solid
                    .sides
                    .iter_mut()
                    .find(|side| side.id == face.side)
                    .ok_or_else(|| map_core::EditError::Invalid("Face no longer exists".into()))?;
                match fix {
                    Fix::ResetProjection(_) => {
                        let normal = Plane::from_points(side.plane.0)
                            .ok_or_else(|| {
                                map_core::EditError::Invalid("Invalid face plane".into())
                            })?
                            .normal;
                        let (u, v) = map_core::edit::world_texture_axes(normal);
                        side.u_axis = UvAxis {
                            axis: u,
                            offset: 0.0,
                            scale: map_core::edit::DEFAULT_TEXTURE_SCALE,
                        };
                        side.v_axis = UvAxis {
                            axis: v,
                            offset: 0.0,
                            scale: map_core::edit::DEFAULT_TEXTURE_SCALE,
                        };
                        side.rotation = 0.0;
                    }
                    Fix::ResetLightmapScale(_) => {
                        side.lightmap_scale = map_core::edit::DEFAULT_LIGHTMAP_SCALE
                    }
                }
            }
            Ok(())
        })?;
        self.checks.draft = None;
        self.run_map_checks(actor);
        Ok(fixes.len())
    }
}

impl State {
    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.draft
            .as_ref()
            .map(|draft| serde_json::json!([self.serial, draft.token, draft.report, draft.issues]))
    }
}
