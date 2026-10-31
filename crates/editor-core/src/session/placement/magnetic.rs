use super::*;

pub const MIN_RADIUS: f64 = 0.125;
pub const MAX_RADIUS: f64 = 1024.0;
pub const DEFAULT_RADIUS: f64 = 16.0;

pub(super) struct State {
    enabled: bool,
    radius: f64,
    source: Option<Component>,
    generation: u64,
    revision: u64,
    serial: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            enabled: false,
            radius: DEFAULT_RADIUS,
            source: None,
            generation: 0,
            revision: 0,
            serial: 0,
        }
    }
}

pub struct Report {
    pub token: u64,
    pub proposed: DVec3,
    pub target: Option<DVec3>,
    pub result: Value,
    generation: u64,
    revision: u64,
    scene: u64,
    view: u64,
    settings: u64,
    definitions: u64,
}

impl Session {
    pub fn magnetic_snapping_enabled(&self) -> bool {
        self.placement.magnetic.enabled
    }

    pub fn magnetic_sources_snapshot(&self) -> String {
        format!(
            "magnetic:{}:{}:{}:{}:{:?}",
            self.buffer_review_scope,
            self.document_generation,
            self.document.revision(),
            self.view_serial,
            self.selected_ids()
        )
    }

    pub fn magnetic_snap_sources(&mut self) -> Result<Value, String> {
        self.ensure_not_closing()?;
        let ids = self.selected_ids();
        let sources = self.translation_source_corners(&ids, true)?;
        Ok(json!({"snapshot":self.magnetic_sources_snapshot(),
            "sources":sources.into_iter().map(|source|json!({"owner":object_id(source.owner),
                "solid":source.component.solid,"sides":source.component.sides,"position":source.position.to_array()})).collect::<Vec<_>>(),
            "limits":{"brushes":MAX_SOURCE_BRUSHES,"corners":MAX_SOURCE_CORNERS},"provenance":"committed_authored_geometry"}))
    }

    pub fn magnetic_snapping(&self) -> Value {
        let state = &self.placement.magnetic;
        let preview = self
            .geometry
            .transform_preview
            .as_ref()
            .and_then(|preview| preview.magnetic.as_ref());
        let selected = self.selected_ids();
        let source_stale = state.source.as_ref().is_some_and(|source| {
            state.generation != self.document_generation
                || subobject::solid(&self.document, source.solid).is_err()
                || self
                    .document
                    .owner_of_solid(source.solid)
                    .is_none_or(|owner| {
                        !selected.contains(&owner)
                            && !selected.contains(&ObjectId::Solid(source.solid))
                    })
        });
        json!({"enabled":state.enabled,"radius":state.radius,"radius_units":"world_units",
            "source_corner":state.source.as_ref().map(super::super::components::component_json),
            "source_stale":source_stale,
            "settings_revision":state.revision,"snapshot":self.magnetic_sources_snapshot(),
            "preview":preview.map(|report|json!({"token":report.token.to_string(),"stale":self.magnetic_preview_stale(report),"result":report.result})),
            "limits":{"minimum_radius":MIN_RADIUS,"maximum_radius":MAX_RADIUS},
            "policy":"Move handles capture the nearest real corner of the first hovered visible brush face or prepared terrain/model hit triangle. Radius is source-to-target distance in world units after ordinary grid/constrained movement. A capture translates every selected object/child together and may leave the handle axis/plane. No through-wall or nearby-scene search. Missing surfaces and outside-radius results explicitly preserve ordinary movement; invalid geometry rejects without replacing the last valid preview."})
    }

    pub fn set_magnetic_snapping(
        &mut self,
        actor: Actor,
        enabled: bool,
        radius: f64,
        source: Option<Component>,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        if snapshot != self.magnetic_sources_snapshot() {
            return Err("Map, selection or visibility changed; refresh magnetic sources".into());
        }
        if !radius.is_finite() || !(MIN_RADIUS..=MAX_RADIUS).contains(&radius) {
            return Err(format!(
                "Magnetic radius must be {MIN_RADIUS}–{MAX_RADIUS} world units"
            ));
        }
        if enabled && source.is_none() {
            return Err("Choose a magnetic Source corner before enabling snapping".into());
        }
        let source = source.map(|mut source| {
            source.sides.sort_unstable();
            source
        });
        if let Some(source) = &source {
            self.translation_source_corner(&self.selected_ids(), source, true)?;
        }
        let state = &self.placement.magnetic;
        if state.enabled == enabled
            && state.radius == radius
            && state.source == source
            && (source.is_none() || state.generation == self.document_generation)
        {
            return Ok(self.magnetic_snapping());
        }
        let revision = state
            .revision
            .checked_add(1)
            .ok_or("Magnetic settings revision exhausted")?;
        if self
            .geometry
            .transform_preview
            .as_ref()
            .is_some_and(|preview| preview.magnetic.is_some())
            || self
                .transform_drag
                .as_ref()
                .is_some_and(|drag| drag.mode == render::handles::Mode::Move)
        {
            self.cancel_transform_preview();
        }
        let state = &mut self.placement.magnetic;
        state.enabled = enabled;
        state.radius = radius;
        state.source = source;
        state.generation = self.document_generation;
        state.revision = revision;
        self.view_serial += 1;
        self.log(actor, "changed magnetic snapping");
        Ok(self.magnetic_snapping())
    }

    fn magnetic_translation_plan(
        &mut self,
        ids: &[ObjectId],
        transform: Transform,
        ray: Ray,
    ) -> Result<(Transform, Report), String> {
        self.ensure_not_closing()?;
        let state = &self.placement.magnetic;
        if !state.enabled || state.generation != self.document_generation {
            return Err("Enable magnetic snapping with a current Source corner".into());
        }
        let source = state
            .source
            .clone()
            .ok_or("Choose a magnetic Source corner")?;
        let radius = state.radius;
        let token = state
            .serial
            .checked_add(1)
            .ok_or("Magnetic preview tokens exhausted")?;
        let own = self
            .geometry
            .transform_preview
            .as_ref()
            .is_some_and(|preview| {
                preview.magnetic.is_some()
                    && std::ptr::eq(self.render_document(), &preview.document)
            });
        if !std::ptr::eq(self.render_document(), &self.document) && !own {
            return Err(
                "Apply or cancel the other geometry preview before magnetic movement".into(),
            );
        }
        if self.viewport_pick_conflict() && self.transform_drag.is_none()
            || self.measurement_picking()
            || self.linked_path_picking()
        {
            return Err(
                "Finish or cancel the active viewport tool before magnetic movement".into(),
            );
        }
        if let Some(kind) = self
            .document_close_review()
            .first_draft_outside(&["transform", "transform_drag"])
        {
            return Err(format!(
                "Apply or cancel the active {kind} draft before magnetic movement"
            ));
        }
        let source = self.translation_source_corner(ids, &source, true)?;
        let proposed = source + transform.translation;
        self.object_translation_bounds(ids, transform.translation)?;
        let candidate = self.placement_surface_candidate(ids, ray, true)?;
        let mut final_transform = transform;
        let (reason, target, identity, distance) = if let Some(picked) = candidate {
            let (target, position) = self.placement_corner_target(picked)?;
            let distance = position.distance(proposed);
            if distance <= radius {
                final_transform.translation =
                    corner_translation(source, position, self.spatial.grid_size);
                self.object_translation_bounds(ids, final_transform.translation)?;
                (
                    "captured",
                    Some(position),
                    Some(target.report()),
                    Some(distance),
                )
            } else {
                (
                    "outside_radius",
                    Some(position),
                    Some(target.report()),
                    Some(distance),
                )
            }
        } else {
            ("no_surface", None, None, None)
        };
        let result = json!({"captured":reason=="captured","reason":reason,"radius":radius,
            "distance":distance,"source":source.to_array(),"proposed_source":proposed.to_array(),
            "target":identity,"target_position":target.map(|position|position.to_array()),
            "requested_translation":transform.translation.to_array(),"translation":final_transform.translation.to_array()});
        Ok((
            final_transform,
            Report {
                token,
                proposed,
                target,
                result,
                generation: self.document_generation,
                revision: self.document.revision(),
                scene: 0,
                view: 0,
                settings: self.placement.magnetic.revision,
                definitions: self.definitions_serial,
            },
        ))
    }

    pub fn preview_magnetic_translation(
        &mut self,
        actor: Actor,
        translation: DVec3,
        ray: Ray,
        snapshot: &str,
    ) -> Result<Value, String> {
        if snapshot != self.magnetic_sources_snapshot() {
            return Err(
                "Map, selection or visibility changed; inspect magnetic snapping again".into(),
            );
        }
        let ids = self.selected_ids();
        let transform = Transform {
            translation,
            pivot: self.selection_pivot(&ids)?,
            texture_lock: self.transform_tool.texture_lock,
            texture_scale_lock: self.transform_tool.texture_scale_lock,
            ..Transform::default()
        };
        self.preview_magnetic_transform(actor, &ids, transform, ray)?;
        Ok(self.magnetic_snapping())
    }

    pub(crate) fn preview_magnetic_transform(
        &mut self,
        actor: Actor,
        ids: &[ObjectId],
        transform: Transform,
        ray: Ray,
    ) -> Result<Transform, String> {
        let (transform, mut report) = self.magnetic_translation_plan(ids, transform, ray)?;
        self.preview_transform(actor, ids, transform, Some(transform.pivot))?;
        self.scene_serial += 1;
        self.view_serial += 1;
        report.scene = self.scene_serial;
        report.view = self.view_serial;
        self.placement.magnetic.serial = report.token;
        self.geometry
            .transform_preview
            .as_mut()
            .ok_or("Magnetic transform preview did not open")?
            .magnetic = Some(report);
        Ok(transform)
    }

    pub(crate) fn magnetic_preview_stale(&self, report: &Report) -> bool {
        report.generation != self.document_generation
            || report.revision != self.document.revision()
            || report.scene != self.scene_serial
            || report.view != self.view_serial
            || report.settings != self.placement.magnetic.revision
            || report.definitions != self.definitions_serial
    }

    pub fn commit_magnetic_translation(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        let report = self
            .geometry
            .transform_preview
            .as_ref()
            .and_then(|preview| preview.magnetic.as_ref())
            .filter(|report| report.token == token)
            .ok_or("Magnetic preview was closed or replaced")?;
        if self.magnetic_preview_stale(report) {
            return Err(
                "Magnetic source, target or visibility changed; preview movement again".into(),
            );
        }
        let applied = report.result.clone();
        self.commit_transform_preview(actor)?;
        let mut state = self.magnetic_snapping();
        state["applied"] = json!({"token":token.to_string(),"result":applied});
        Ok(state)
    }

    pub fn cancel_magnetic_translation(&mut self, token: u64) -> Result<Value, String> {
        self.ensure_not_closing()?;
        if !self
            .geometry
            .transform_preview
            .as_ref()
            .and_then(|preview| preview.magnetic.as_ref())
            .is_some_and(|report| report.token == token)
        {
            return Err("Magnetic preview was closed or replaced".into());
        }
        self.cancel_transform_preview();
        Ok(self.magnetic_snapping())
    }

    pub(crate) fn append_magnetic_snapping_guide(&self, mesh: &mut render::SceneMesh) {
        let Some(report) = self
            .geometry
            .transform_preview
            .as_ref()
            .and_then(|preview| preview.magnetic.as_ref())
        else {
            return;
        };
        if self.magnetic_preview_stale(report) {
            return;
        }
        for position in [Some(report.proposed), report.target].into_iter().flatten() {
            for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                for point in [position - axis * 3.0, position + axis * 3.0] {
                    mesh.lines.push(render::scene::LineVertex {
                        position: point.as_vec3().to_array(),
                        color: [255, 220, 95, 255],
                    });
                }
            }
        }
    }
}
