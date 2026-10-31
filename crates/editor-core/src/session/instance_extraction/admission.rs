use super::*;
use map_core::{context, groups, layers, measurements, partition_metadata};
use source_formats::vmf::{Properties, Solid};

fn editor(properties: Option<&Properties>) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for (key, value) in properties.into_iter().flat_map(Properties::iter) {
        if (key.eq_ignore_ascii_case("visgroupshown")
            || key.eq_ignore_ascii_case("visgroupautoshown"))
            && value != "1"
        {
            return Err("Selected brush/editor visibility must be shown before extraction".into());
        }
        if !["color", "visgroupshown", "visgroupautoshown", "logicalpos"]
            .iter()
            .any(|allowed| key.eq_ignore_ascii_case(allowed))
            || !seen.insert(key.to_ascii_lowercase())
            || value.len() > instances::MAX_FIELD_BYTES
        {
            return Err(format!(
                "Selected editor metadata {key} cannot be preserved by an instance; remove its group/layer/extension first"
            ));
        }
    }
    Ok(())
}

fn solid(solid: &Solid) -> Result<(), String> {
    if solid.hidden {
        return Err("Hidden selected brushes cannot be extracted".into());
    }
    if solid.sides.len() > instances::MAX_BRUSH_SIDES
        || !solid.extra.is_empty()
        || !solid.extra_properties.0.is_empty()
        || solid.sides.iter().any(|side| {
            side.material.len() > instances::MAX_FIELD_BYTES
                || side.displacement.is_some()
                || !side.extra.is_empty()
                || !side.extra_properties.0.is_empty()
        })
    {
        return Err(
            "Selected brush exceeds supported bounded ordinary face/metadata admission".into(),
        );
    }
    editor(solid.editor.as_ref())
}

fn side_bindings(session: &Session, sides: &BTreeSet<u32>) -> Result<(), String> {
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
            if entity.properties.get_all(key).count() != 1 || value.len() > 16 * 1024 {
                return Err(
                    "Ambiguous or oversized inbound side binding; repair it before extraction"
                        .into(),
                );
            }
            let mut seen = BTreeSet::new();
            for token in value.split_whitespace() {
                let id = token
                    .parse::<u32>()
                    .ok()
                    .filter(|id| *id > 0)
                    .ok_or("Malformed inbound side binding; repair it before extraction")?;
                if !seen.insert(id) || seen.len() > 1024 {
                    return Err("Duplicate or oversized inbound side binding".into());
                }
                if !existing.contains(&id) {
                    return Err(format!(
                        "entity:{} {key} binds unavailable root side:{id}; extraction cannot reinterpret it as a child face",
                        entity.id
                    ));
                }
                if sides.contains(&id) {
                    return Err(format!(
                        "entity:{} {key} binds selected side:{id}; extraction cannot retarget child faces",
                        entity.id
                    ));
                }
            }
        }
    }
    Ok(())
}

pub(super) fn plan(session: &Session, file: &str, pivot_choice: Pivot) -> Result<Plan, String> {
    let vmf = session.document.vmf();
    vmf.validate_ids().map_err(|error| error.to_string())?;
    if vmf.object_ids().take(instances::MAX_OBJECTS + 1).count() > instances::MAX_OBJECTS
        || vmf.side_ids().take(instances::MAX_SIDES + 1).count() > instances::MAX_SIDES
    {
        return Err("Source exceeds bounded instance object/side limits".into());
    }
    if session.face_mode || !session.face_selection.is_empty() {
        return Err(
            "Use Objects mode and select complete world brushes or func_detail owners".into(),
        );
    }
    if session.selection.is_empty() || session.selection.len() > MAX_OBJECTS {
        return Err("Select 1–64 complete world brushes or ordinary func_detail owners".into());
    }
    session.require_root_objects(&session.selected_ids())?;
    if instances::requires_source(vmf) {
        return Err("Extracting a parent with existing visible instance references is not supported; compiler brush order must remain unchanged".into());
    }
    metadata_budget(&vmf.extra, 0, &mut (0, 0))?;
    groups::Graph::read(&session.document).map_err(|error| error.to_string())?;
    layers::Graph::read(&session.document).map_err(|error| error.to_string())?;
    let context = context::Metadata::parse(vmf)?;
    context.validate()?;
    let measurements = measurements::Measurements::parse(vmf)?;
    measurements.validate()?;
    let partitions = partition_metadata::Metadata::parse(vmf)?;
    partitions.validate()?;
    let views = map_core::saved_views::Views::parse(vmf)?;
    views.validate()?;
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
        || vmf.solids().any(|solid| {
            !solid.extra.is_empty()
                || !solid.extra_properties.0.is_empty()
                || solid
                    .sides
                    .iter()
                    .any(|side| !side.extra.is_empty() || !side.extra_properties.0.is_empty())
        })
    {
        return Err("Unknown world, group, object, face or saved-view extension metadata cannot be proven independent of extracted IDs".into());
    }
    let mut layers = vmf.visgroups.iter().flatten().collect::<Vec<_>>();
    while let Some(layer) = layers.pop() {
        if !layer.extra.is_empty() || !layer.extra_properties.0.is_empty() {
            return Err(
                "Unknown layer extension metadata cannot be proven independent of extracted IDs"
                    .into(),
            );
        }
        layers.extend(&layer.children);
    }
    map_core::compile_region::active(vmf).map_err(|error| error.to_string())?;
    if !context.extra.is_empty()
        || context.notes.values().any(|note| !note.extra.is_empty())
        || !partitions.extra.is_empty()
        || partitions
            .entries
            .values()
            .any(|intent| !intent.extra.is_empty())
        || !measurements.extra.is_empty()
        || measurements.measurements.iter().any(|measurement| {
            measurement.extra.iter().any(|node| {
                !matches!(
                    node.key.as_str(),
                    "start_entity_origin" | "end_entity_origin"
                )
            })
        })
    {
        return Err("Unknown context, measurement or partition extension metadata cannot be proven independent of the selection".into());
    }
    for node in &vmf.extra {
        if ![
            context::METADATA_KEY,
            measurements::KEY,
            partition_metadata::KEY,
            map_core::saved_views::KEY,
        ]
        .contains(&node.key.as_str())
        {
            return Err(format!(
                "Unknown root metadata {} cannot be proven independent of extracted IDs",
                node.key
            ));
        }
    }
    let mut child = Document::empty().into_vmf();
    child.version_info = None;
    child.view_settings = None;
    child.world.id = vmf.world.id;
    child.world.properties = Properties::default();
    child.world.properties.set("classname", "worldspawn");
    let mut removed = BTreeSet::new();
    let mut side_ids = BTreeSet::new();
    let mut selected = Vec::new();
    let mut brush_count = 0usize;
    let mut side_count = 0usize;
    let mut world_selected = false;
    for brush in &vmf.world.solids {
        if session.selection.contains(&ObjectId::Solid(brush.id)) {
            world_selected = true;
            solid(brush)?;
            side_count += brush.sides.len();
            if side_count > MAX_SIDES {
                return Err("Extraction exceeds2048 sides".into());
            }
            brush_count += 1;
            if brush_count > MAX_BRUSHES {
                return Err("Extraction exceeds256 brushes".into());
            }
            child.world.solids.push(brush.clone());
            selected.push(ObjectId::Solid(brush.id));
        } else if world_selected && !brush.hidden {
            return Err("Selected world brushes must form the visible world-brush suffix to preserve compiler plane order".into());
        }
    }
    let mut detail_selected = false;
    for entity in &vmf.entities {
        if !session.selection.contains(&ObjectId::Entity(entity.id)) {
            if detail_selected && !entity.hidden && entity.solids.iter().any(|solid| !solid.hidden)
            {
                return Err("Selected detail owners must follow all retained visible brush entities to preserve compiler plane order".into());
            }
            continue;
        }
        if entity.hidden || entity.classname() != Some("func_detail") || entity.solids.is_empty() {
            return Err(
                "Only visible world brushes and whole ordinary func_detail owners can be extracted"
                    .into(),
            );
        }
        if !entity.extra.is_empty()
            || entity
                .connections
                .as_ref()
                .is_some_and(|connections| !connections.is_empty())
            || entity.properties.0.len() > 6
            || entity.properties.iter().any(|(key, value)| {
                !["classname", "origin", "angles", "mindxlevel", "maxdxlevel"]
                    .iter()
                    .any(|allowed| key.eq_ignore_ascii_case(allowed))
                    || value.len() > instances::MAX_FIELD_BYTES
            })
        {
            return Err("Selected detail owner has unsupported properties or metadata".into());
        }
        brush_count = brush_count
            .checked_add(entity.solids.len())
            .ok_or("Brush count overflow")?;
        if brush_count > MAX_BRUSHES {
            return Err("Extraction exceeds256 brushes".into());
        }
        detail_selected = true;
        editor(entity.editor.as_ref())?;
        for brush in &entity.solids {
            solid(brush)?;
            side_count += brush.sides.len();
            if side_count > MAX_SIDES {
                return Err("Extraction exceeds2048 sides".into());
            }
        }
        child.entities.push(entity.clone());
        selected.push(ObjectId::Entity(entity.id));
    }
    if selected.len() != session.selection.len() {
        return Err(
            "Selection contains an owned brush, missing object or unsupported entity".into(),
        );
    }
    let world_brushes = child.world.solids.len();
    let detail_brushes = child
        .entities
        .iter()
        .map(|entity| entity.solids.len())
        .sum::<usize>();
    if world_brushes + detail_brushes > MAX_BRUSHES {
        return Err("Extraction exceeds256 brushes".into());
    }
    for id in &selected {
        removed.insert(match id {
            ObjectId::Solid(id) | ObjectId::Entity(id) => *id,
        });
    }
    for brush in child.solids() {
        removed.insert(brush.id);
        if brush.sides.len() > instances::MAX_BRUSH_SIDES {
            return Err("Extraction brush exceeds128 sides".into());
        }
        side_ids.extend(brush.sides.iter().map(|side| side.id));
    }
    if side_ids.len() > MAX_SIDES {
        return Err("Extraction exceeds2048 sides".into());
    }
    for scope in context.notes.keys() {
        if let context::Scope::Object(ObjectId::Solid(id) | ObjectId::Entity(id)) = scope
            && removed.contains(id)
        {
            return Err(
                "Selected objects have attached notes; extraction cannot preserve child context"
                    .into(),
            );
        }
    }
    for measurement in &measurements.measurements {
        for endpoint in [measurements::Endpoint::Start, measurements::Endpoint::End] {
            if measurement
                .origin_reference(endpoint)?
                .is_some_and(|id| removed.contains(&id))
            {
                return Err("A measurement origin binding refers to selected IDs".into());
            }
        }
    }
    if partitions
        .entries
        .keys()
        .any(|(solid, _)| removed.contains(solid))
    {
        return Err("Selected brushes have partition intent; extraction cannot preserve it".into());
    }
    side_bindings(session, &side_ids)?;
    let added = 1usize + world_brushes + 2 * detail_brushes;
    let retained = vmf
        .object_ids()
        .count()
        .checked_sub(removed.len())
        .ok_or("Object count underflow")?;
    if retained
        .checked_add(added)
        .is_none_or(|count| count > instances::MAX_OBJECTS)
        || vmf.side_ids().count() > instances::MAX_SIDES
    {
        return Err("Prospective instance expansion exceeds combined object or side limits".into());
    }
    if let Some(class) = session.definitions.class("func_instance")
        && (class.properties.len() > 128
            || class.properties.iter().any(|property| {
                property.name.len() > instances::MAX_FIELD_BYTES
                    || property
                        .default
                        .as_ref()
                        .is_some_and(|value| value.len() > instances::MAX_FIELD_BYTES)
                    || property.choices.len() > 128
                    || property
                        .choices
                        .iter()
                        .any(|choice| choice.value.len() > instances::MAX_FIELD_BYTES)
            }))
    {
        return Err("Instance definition defaults exceed the extraction helper budget".into());
    }
    let pivot = match pivot_choice {
        Pivot::WorldOrigin => DVec3::ZERO,
        Pivot::SelectionCentre => session.selection_pivot(&selected)?,
        Pivot::Custom(point) => point,
    };
    if !pivot.is_finite() {
        return Err("Extraction pivot requires finite X, Y and Z coordinates".into());
    }
    let (classname, properties) = session.entity_properties(
        "func_instance",
        ClassKind::Point,
        BTreeMap::from([
            ("file".into(), file.into()),
            ("origin".into(), source_formats::vmf::format_vec3(pivot)),
            ("angles".into(), "0 0 0".into()),
        ]),
    )?;
    let mut helper = Entity::new(0, &classname);
    for (key, value) in properties {
        helper.properties.set(key, value);
    }
    if helper.properties.iter().any(|(key, value)| {
        key.to_ascii_lowercase().starts_with("replace") && !value.trim().is_empty()
    }) {
        return Err("Instance extraction does not support nonempty replacement defaults; author light bindings on an ordinary instance reference".into());
    }
    instances::helper_admission(&helper)?;
    let allocator_metadata = vmf
        .extra
        .iter()
        .filter(|node| matches!(node.key.as_str(), context::METADATA_KEY | measurements::KEY))
        .cloned()
        .collect::<Vec<_>>();
    let allocator_bytes = allocator_metadata.capacity()
        * std::mem::size_of::<source_formats::keyvalues::Node>()
        + allocator_metadata.iter().map(node_bytes).sum::<usize>();
    let source_bytes = (world_brushes + detail_brushes) * std::mem::size_of::<(Solid, bool)>()
        + child.solids().map(solid_bytes).sum::<usize>();
    if plan_bytes(&child, &helper, selected.capacity()) + allocator_bytes + source_bytes
        > MAX_PLAN_BYTES
    {
        return Err("Extraction retained plan exceeds16 MiB".into());
    }
    let mut originals = Vec::with_capacity(world_brushes + detail_brushes);
    originals.extend(
        child
            .world
            .solids
            .iter()
            .cloned()
            .map(|solid| (solid, false)),
    );
    originals.extend(
        child
            .entities
            .iter()
            .flat_map(|entity| entity.solids.iter().cloned().map(|solid| (solid, true))),
    );
    Ok(Plan {
        child,
        originals,
        pivot,
        pivot_mode: pivot_choice.mode(),
        helper,
        selected,
        world_brushes,
        detail_brushes,
        sides: side_ids.len(),
        allocator_metadata,
    })
}

fn properties_bytes(properties: &Properties) -> usize {
    properties.0.capacity() * std::mem::size_of::<(String, String)>()
        + properties
            .0
            .iter()
            .map(|(key, value)| key.capacity() + value.capacity())
            .sum::<usize>()
}

fn solid_bytes(solid: &Solid) -> usize {
    solid.sides.capacity() * std::mem::size_of::<source_formats::vmf::Side>()
        + solid
            .sides
            .iter()
            .map(|side| side.material.capacity())
            .sum::<usize>()
        + solid.editor.as_ref().map_or(0, properties_bytes)
}

fn plan_bytes(child: &Vmf, helper: &Entity, selected_capacity: usize) -> usize {
    std::mem::size_of::<Plan>()
        + selected_capacity * std::mem::size_of::<ObjectId>()
        + child.world.solids.capacity() * std::mem::size_of::<Solid>()
        + properties_bytes(&child.world.properties)
        + child.world.solids.iter().map(solid_bytes).sum::<usize>()
        + child.entities.capacity() * std::mem::size_of::<Entity>()
        + child
            .entities
            .iter()
            .map(|entity| {
                properties_bytes(&entity.properties)
                    + entity.editor.as_ref().map_or(0, properties_bytes)
                    + entity.solids.capacity() * std::mem::size_of::<Solid>()
                    + entity.solids.iter().map(solid_bytes).sum::<usize>()
            })
            .sum::<usize>()
        + std::mem::size_of::<Entity>()
        + properties_bytes(&helper.properties)
}

fn node_bytes(node: &source_formats::keyvalues::Node) -> usize {
    node.key.capacity()
        + match &node.value {
            source_formats::keyvalues::Value::String(value) => value.capacity(),
            source_formats::keyvalues::Value::Block(children) => {
                children.capacity() * std::mem::size_of::<source_formats::keyvalues::Node>()
                    + children.iter().map(node_bytes).sum::<usize>()
            }
        }
}

fn metadata_budget(
    nodes: &[source_formats::keyvalues::Node],
    depth: usize,
    budget: &mut (usize, usize),
) -> Result<(), String> {
    if depth > 64 {
        return Err("Extraction metadata exceeds64 levels".into());
    }
    for node in nodes {
        budget.0 += 1;
        budget.1 = budget
            .1
            .checked_add(
                std::mem::size_of::<source_formats::keyvalues::Node>()
                    + node.key.len()
                    + node.as_str().map_or(0, str::len),
            )
            .ok_or("Extraction metadata size overflow")?;
        if budget.0 > 32768 || budget.1 > 8 * 1024 * 1024 {
            return Err("Extraction metadata exceeds32768 nodes or8 MiB".into());
        }
        if let Some(children) = node.children() {
            metadata_budget(children, depth + 1, budget)?;
        }
    }
    Ok(())
}
