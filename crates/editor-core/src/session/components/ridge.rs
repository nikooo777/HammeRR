use super::*;
use map_core::surface::FaceId;
use map_core::{context, measurements, partition_metadata};
use source_formats::vmf::{Solid, Vmf};

const MAX_RIDGE_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;

pub(super) fn component_selection(plan: &subobject::CreasePlan) -> BTreeSet<Component> {
    if !plan.fragments.is_empty() {
        return BTreeSet::new();
    }
    plan.changes
        .iter()
        .flat_map(|change| change.selection.iter().cloned())
        .collect()
}

pub(super) fn exterior_faces(plan: &subobject::CreasePlan) -> Vec<FaceId> {
    plan.fragments
        .iter()
        .flat_map(|fragment| {
            fragment.faces.iter().filter_map(|face| {
                face.exterior.then_some(FaceId {
                    solid: fragment.solid.id,
                    side: face.side,
                })
            })
        })
        .collect()
}

impl CreaseDraft {
    pub(super) fn report(&self) -> Value {
        let output_brushes = if self.plan.fragments.is_empty() {
            self.plan.replacements.len()
        } else {
            self.plan.fragments.len()
        };
        json!({"rise":self.rise,"roof_texture_lock":self.roof_texture_lock,"neighbor_texture_lock":self.neighbor_texture_lock,
            "output_brushes":output_brushes,"added_brushes":output_brushes.saturating_sub(self.requests.len()),"fragment_count":self.plan.fragments.len(),"cubemap_bindings":self.cubemap_bindings.report(),
            "pairs":self.requests.iter().map(|request|json!({"solid":request.solid,"edges":request.edges})).collect::<Vec<_>>(),
            "changes":self.plan.changes.iter().map(|change|json!({"solid":change.source.solid,"source_side":change.source.side,"edges":change.source.edges,"positions":change.positions.map(|point|point.to_array()),"flat_positions":change.flat_positions.map(|point|point.to_array()),"new_side":change.new_side,"selection":if self.plan.fragments.is_empty(){change.selection.iter().map(component_json).collect::<Vec<_>>()}else{Vec::new()}})).collect::<Vec<_>>(),
            "selected_exterior_faces":exterior_faces(&self.plan).iter().map(|face|json!({"solid":face.solid,"side":face.side})).collect::<Vec<_>>(),
            "fragments":self.plan.fragments.iter().map(|fragment|json!({"source_solid":fragment.source,"solid":fragment.solid.id,
                "owner":match fragment.owner{ObjectId::Solid(_)=>"world".to_owned(),ObjectId::Entity(id)=>format!("entity:{id}")},
                "bounds":map_core::query::solid_bounds(&fragment.solid).map(|(mins,maxs)|json!({"mins":mins.to_array(),"maxs":maxs.to_array()})),
                "faces":fragment.faces.iter().map(|face|{
                    let side=fragment.solid.sides.iter().find(|side|side.id==face.side).unwrap();
                    json!({"side":face.side,"source_side":face.source_side,"ridge_side":face.ridge_side,"exterior":face.exterior,"material":side.material.to_ascii_lowercase(),"plane":side.plane.0.map(|point|point.to_array()),
                        "uaxis":{"axis":side.u_axis.axis.to_array(),"offset":side.u_axis.offset,"scale":side.u_axis.scale},"vaxis":{"axis":side.v_axis.axis.to_array(),"offset":side.v_axis.offset,"scale":side.v_axis.scale},
                        "rotation":side.rotation,"lightmap_scale":side.lightmap_scale,"smoothing_groups":side.smoothing_groups})
                }).collect::<Vec<_>>() })).collect::<Vec<_>>()})
    }
}

impl Session {
    pub(super) fn fragmented_ridge_preview(&self) -> bool {
        self.geometry
            .components
            .preview
            .as_ref()
            .and_then(|preview| preview.crease.as_ref())
            .is_some_and(|crease| !crease.plan.fragments.is_empty())
    }

    pub(super) fn capture_ridge_fragment_materials(
        &self,
        requests: &[Crease],
        plan: &subobject::CreasePlan,
        neighbor_texture_lock: bool,
    ) -> Result<Option<compiler::asset_sources::AssetSourceGuard>, String> {
        if !neighbor_texture_lock {
            return Ok(None);
        }
        self.capture_fragment_materials(
            &requests
                .iter()
                .map(|request| request.solid)
                .collect::<Vec<_>>(),
            plan.fragments.iter().flat_map(|fragment| {
                fragment
                    .solid
                    .sides
                    .iter()
                    .map(|side| side.material.as_str())
            }),
        )
        .map(Some)
    }

    pub(super) fn capture_fragment_materials<'a>(
        &'a self,
        sources: &[u32],
        fragment_materials: impl IntoIterator<Item = &'a str>,
    ) -> Result<compiler::asset_sources::AssetSourceGuard, String> {
        let mut names = BTreeSet::new();
        for &id in sources {
            let (_, solid) =
                subobject::solid(&self.document, id).map_err(|error| error.to_string())?;
            names.extend(solid.sides.iter().map(|side| side.material.as_str()));
        }
        names.extend(fragment_materials);
        compiler::materials::capture_solid_fragment_materials(
            &self.game,
            names,
            &compiler::Control::default(),
        )
    }

    pub(super) fn ridge_stale_error(&self, crease: &CreaseDraft) -> Option<String> {
        if let Some(error) = crease.cubemap_bindings.stale_error(self) {
            return Some(error);
        }
        if crease.definitions != self.definitions_serial {
            return Some("Definitions changed; preview ridge attachment again".into());
        }
        if !crease.neighbor_texture_lock {
            return None;
        }
        if crease.material_generation != self.materials.generation() {
            return Some("Material assets were reloaded; preview ridge attachment again".into());
        }
        match &crease.material_sources {
            Some(sources) => sources
                .verify_metadata()
                .err()
                .map(|error| error.to_string()),
            None => Some("Ridge fragment material sources are missing; preview again".into()),
        }
    }

    pub(super) fn check_ridge_fragment_references(
        &self,
        requests: &[Crease],
        consume_source_partition: bool,
    ) -> Result<(), String> {
        let objects = requests.iter().map(|request| request.solid).collect();
        let consumed = requests
            .iter()
            .filter(|_| consume_source_partition)
            .map(|request| (request.solid, request.side))
            .collect();
        self.check_fragment_references(&objects, &consumed, &BTreeSet::new())
    }

    pub(super) fn check_fragment_references(
        &self,
        objects: &BTreeSet<u32>,
        consumed_partitions: &BTreeSet<(u32, u32)>,
        consumed_free_partitions: &BTreeSet<(u32, u32)>,
    ) -> Result<(), String> {
        self.document
            .vmf()
            .validate_ids()
            .map_err(|error| error.to_string())?;
        map_core::references::admit_snapshot_storage(
            self.document.vmf(),
            MAX_RIDGE_SNAPSHOT_BYTES,
            &mut || Ok(()),
        )?;
        map_core::groups::Graph::read(&self.document).map_err(|error| error.to_string())?;
        map_core::layers::Graph::read(&self.document).map_err(|error| error.to_string())?;
        require_known_face_topology_metadata(self.document.vmf())?;
        let mut sides = BTreeSet::new();
        for &id in objects {
            let (owner, solid) =
                subobject::solid(&self.document, id).map_err(|error| error.to_string())?;
            let mut editor_keys = BTreeSet::new();
            for (key, value) in solid.editor.iter().flat_map(|editor| editor.iter()) {
                if ![
                    "color",
                    "groupid",
                    "visgroupid",
                    "visgroupshown",
                    "visgroupautoshown",
                    "logicalpos",
                ]
                .iter()
                .any(|allowed| key.eq_ignore_ascii_case(allowed))
                    || (!key.eq_ignore_ascii_case("visgroupid")
                        && !editor_keys.insert(key.to_ascii_lowercase()))
                    || value.len() > compiler::instances::MAX_FIELD_BYTES
                {
                    return Err(format!(
                        "Brush {}: editor metadata {key} cannot be copied to convex fragments",
                        solid.id
                    ));
                }
            }
            if let ObjectId::Entity(id) = owner {
                let class = self
                    .document
                    .entity(id)
                    .and_then(|entity| entity.classname())
                    .unwrap_or_default();
                if !["func_detail", "func_brush", "func_wall"]
                    .iter()
                    .any(|allowed| class.eq_ignore_ascii_case(allowed))
                {
                    return Err(format!(
                        "entity:{id}: convex fragmentation supports world, func_detail, func_brush and func_wall brushes"
                    ));
                }
            }
            sides.extend(solid.sides.iter().map(|side| side.id));
        }
        if let Some(ordering) = map_core::instance_order::read(self.document.vmf())?
            && ordering
                .origin_custody
                .iter()
                .any(|custody| custody.volumes.iter().any(|id| objects.contains(id)))
        {
            return Err("A source brush is retained origin-volume custody; it cannot become convex fragments".into());
        }
        check_side_references(self, &sides, true)?;
        map_core::measurements::Measurements::parse(self.document.vmf())?
            .validate_retained_references(objects, &sides)?;
        let context = map_core::context::Metadata::parse(self.document.vmf())?;
        context.validate()?;
        if !context.extra.is_empty() || context.notes.values().any(|note| !note.extra.is_empty()) {
            return Err(
                "Unknown context metadata cannot be proven independent of convex fragments".into(),
            );
        }
        if objects.iter().any(|id| {
            context
                .notes
                .contains_key(&map_core::context::Scope::Object(ObjectId::Solid(*id)))
        }) {
            return Err("A source brush has an attached note; detach it before replacing that brush with convex fragments".into());
        }
        let metadata = map_core::partition_metadata::Metadata::parse(self.document.vmf())?;
        metadata.validate()?;
        if !metadata.extra.is_empty()
            || metadata.entries.iter().any(|(&(solid, side), intent)| {
                !intent.extra.is_empty()
                    || (objects.contains(&solid) && !consumed_partitions.contains(&(solid, side)))
            })
        {
            return Err("Saved partition intent cannot be rebound to convex fragments; remove other pieces on the source brushes first".into());
        }
        let free = map_core::free_partition_metadata::Metadata::parse(self.document.vmf())?;
        free.validate()?;
        if !free.extra.is_empty()
            || free.entries.iter().any(|(&(solid, side), intent)| {
                !intent.extra.is_empty()
                    || (objects.contains(&solid)
                        && !consumed_free_partitions.contains(&(solid, side)))
            })
        {
            return Err("Saved free cuts cannot be rebound to convex fragments; remove other cuts on the source brushes first".into());
        }
        Ok(())
    }

    pub(super) fn check_ordinary_ridge_references(
        &self,
        plan: &subobject::CreasePlan,
    ) -> Result<(), String> {
        let sides = plan
            .changes
            .iter()
            .map(|change| change.source.side)
            .collect();
        check_side_references(self, &sides, true)
    }
}

pub(in crate::session) fn require_unbound_side_references(
    session: &Session,
    sides: &BTreeSet<u32>,
) -> Result<(), String> {
    check_side_references(session, sides, false)
}

fn check_side_references(
    session: &Session,
    sides: &BTreeSet<u32>,
    cubemaps: bool,
) -> Result<(), String> {
    let existing = session.document.vmf().side_ids().collect::<BTreeSet<_>>();
    for entity in &session.document.vmf().entities {
        for (key, value) in entity.properties.iter() {
            let typed = session
                .definitions
                .class(entity.classname().unwrap_or_default())
                .and_then(|class| class.property(key))
                .is_some_and(|property| matches!(property.kind.as_str(), "sidelist" | "side_list"));
            if !key.eq_ignore_ascii_case("sides") && !typed {
                continue;
            }
            let ids = parsed_side_references(entity, key, value, &existing)?;
            if ids.iter().any(|id| sides.contains(id))
                && !(cubemaps
                    && key.eq_ignore_ascii_case("sides")
                    && super::ridge_bindings::is_cubemap(entity))
            {
                return Err(format!(
                    "entity:{} {key} binds a selected face; the operation requires one-to-many face rebinding",
                    entity.id
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn parsed_side_references(
    entity: &source_formats::vmf::Entity,
    key: &str,
    value: &str,
    existing: &BTreeSet<u32>,
) -> Result<Vec<u32>, String> {
    if entity.properties.get_all(key).count() != 1 || value.len() > 16 * 1024 {
        return Err(
            "Ambiguous or oversized inbound side binding; repair it before changing face topology"
                .into(),
        );
    }
    let mut seen = BTreeSet::new();
    let mut ids = Vec::new();
    for token in value.split_whitespace() {
        let id = token
            .parse::<u32>()
            .ok()
            .filter(|id| *id > 0)
            .ok_or("Malformed inbound side binding; repair it before changing face topology")?;
        if !seen.insert(id) || seen.len() > 1024 {
            return Err("Duplicate or oversized inbound side binding".into());
        }
        if !existing.contains(&id) {
            return Err(format!(
                "entity:{} {key} binds unavailable root side:{id}; the operation cannot reinterpret its face binding",
                entity.id
            ));
        }
        ids.push(id);
    }
    Ok(ids)
}

fn face_metadata(solid: &Solid) -> Result<(), String> {
    let precise = solid
        .precise_vertices()
        .map_err(|error| error.to_string())?;
    if precise
        .as_ref()
        .is_some_and(|rings| rings.iter().any(|ring| ring.has_opaque_fields))
        || solid.sides.iter().any(|side| {
            !side.extra_properties.0.is_empty()
                || side
                    .extra
                    .iter()
                    .any(|node| !source_formats::vmf::is_precise_vertices_key(&node.key))
        })
    {
        return Err(
            "Unknown face metadata cannot be proven independent of changed face IDs".into(),
        );
    }
    Ok(())
}

pub(in crate::session) fn require_known_face_topology_metadata(vmf: &Vmf) -> Result<(), String> {
    let views = map_core::saved_views::Views::parse(vmf)?;
    views.validate()?;
    for solid in vmf.solids() {
        face_metadata(solid)?;
    }
    if !vmf.world.extra.is_empty()
        || !vmf.visgroups_extra.is_empty()
        || !views.extra.is_empty()
        || views.views.iter().any(|view| !view.extra.is_empty())
        || vmf.world.groups.iter().any(|group| {
            !group.extra.is_empty()
                || !group.extra_properties.0.is_empty()
                || group.editor.as_ref().is_some_and(|editor| {
                    editor.iter().any(|(key, _)| {
                        ![
                            "name",
                            "color",
                            "groupid",
                            "logicalpos",
                            "visgroupshown",
                            "visgroupautoshown",
                        ]
                        .iter()
                        .any(|allowed| key.eq_ignore_ascii_case(allowed))
                    })
                })
        })
        || vmf.entities.iter().any(|entity| !entity.extra.is_empty())
        || vmf
            .solids()
            .any(|solid| !solid.extra.is_empty() || !solid.extra_properties.0.is_empty())
    {
        return Err("Unknown world, group, object, face or saved-view extension metadata cannot be proven independent of changed face IDs".into());
    }
    let mut layers = vmf.visgroups.iter().flatten().collect::<Vec<_>>();
    while let Some(layer) = layers.pop() {
        if !layer.extra.is_empty() || !layer.extra_properties.0.is_empty() {
            return Err(
                "Unknown layer extension metadata cannot be proven independent of changed face IDs"
                    .into(),
            );
        }
        layers.extend(&layer.children);
    }
    for node in &vmf.extra {
        if ![
            context::METADATA_KEY,
            measurements::KEY,
            partition_metadata::KEY,
            map_core::free_partition_metadata::KEY,
            map_core::saved_views::KEY,
        ]
        .contains(&node.key.as_str())
        {
            return Err(format!(
                "Unknown root metadata {} cannot be proven independent of changed face IDs",
                node.key
            ));
        }
    }
    Ok(())
}
