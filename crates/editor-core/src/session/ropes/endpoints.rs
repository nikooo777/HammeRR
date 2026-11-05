use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use compiler::Control;
use glam::{DAffine3, DMat3, DVec3};
use map_core::{Document, ObjectId};
use render::models::{Asset, Instance, Library};
use render::ropes::{
    Endpoint, MAX_EXTENT, MAX_IDENTIFIER_BYTES, MAX_LINKS, ResolvedEndpoints, is_rope,
};
use source_formats::gamefs::GameFs;
use source_formats::vmf::Entity;

use super::super::models::Models;

const FRAME_TOLERANCE: f64 = 0.0001;
const MAX_PARENT_ATTACHMENT: usize = 63;

enum Target<'a> {
    Missing,
    Unique(&'a Entity),
    Ambiguous,
}

pub(in crate::session) fn resolve_required_models(
    document: &Document,
    game: &GameFs,
    included: &HashSet<ObjectId>,
    choice: Option<(&[u32], &Arc<Asset>)>,
    control: &Control,
) -> Result<Models, String> {
    control.checkpoint()?;
    let sources = document
        .vmf()
        .entities
        .iter()
        .filter(|entity| {
            !entity.hidden && is_rope(entity) && included.contains(&ObjectId::Entity(entity.id))
        })
        .take(MAX_LINKS)
        .collect::<Vec<_>>();
    let mut targets = BTreeMap::new();
    for source in &sources {
        if let Ok(Some(next)) = identifier(source, "NextKey")
            && !next.is_empty()
            && !next.contains(['*', '?', '!'])
        {
            targets.insert(next.to_ascii_lowercase(), Target::Missing);
        }
    }
    for entity in &document.vmf().entities {
        control.checkpoint()?;
        for name in entity.properties.get_all("targetname") {
            if name.len() <= MAX_IDENTIFIER_BYTES
                && let Some(target) = targets.get_mut(&name.trim().to_ascii_lowercase())
            {
                *target = match target {
                    Target::Missing => Target::Unique(entity),
                    Target::Unique(_) | Target::Ambiguous => Target::Ambiguous,
                };
            }
        }
    }
    let mut parents = BTreeMap::new();
    for source in sources {
        control.checkpoint()?;
        let next = match identifier(source, "NextKey") {
            Ok(next) => next.unwrap_or(""),
            Err(_) => continue,
        };
        let target = if next.is_empty() {
            if property(source, "Dangling")
                .ok()
                .flatten()
                .and_then(|value| value.parse::<i32>().ok())
                != Some(1)
            {
                continue;
            }
            None
        } else {
            match targets.get(&next.to_ascii_lowercase()) {
                Some(Target::Unique(target))
                    if !target.hidden && included.contains(&ObjectId::Entity(target.id)) =>
                {
                    Some(*target)
                }
                _ => continue,
            }
        };
        let Ok(start_parent) = required_parent(document, source) else {
            continue;
        };
        if let Some(parent) = start_parent {
            parents.insert(parent.id, parent);
        }
        let end_parent = match target
            .map(|entity| required_parent(document, entity))
            .transpose()
        {
            Ok(parent) => parent.flatten(),
            Err(_) => continue,
        };
        if let Some(parent) = end_parent {
            parents.insert(parent.id, parent);
        }
    }
    let mut library = Library::default();
    let mut models = Models {
        instances: Vec::with_capacity(parents.len()),
        issues: BTreeMap::new(),
    };
    for parent in parents.into_values() {
        control.checkpoint()?;
        let result = load_parent_model(parent, game, &mut library, choice);
        control.checkpoint()?;
        match result {
            Ok(instance) => models.instances.push(instance),
            Err(error) => {
                models.issues.insert(parent.id, error);
            }
        }
    }
    Ok(models)
}

fn load_parent_model(
    parent: &Entity,
    game: &GameFs,
    library: &mut Library,
    choice: Option<(&[u32], &Arc<Asset>)>,
) -> Result<Instance, String> {
    let path = property(parent, "model")?
        .filter(|path| !path.is_empty())
        .ok_or("Rope parent needs a model asset path")?;
    let asset = match choice.filter(|(ids, _)| ids.contains(&parent.id)) {
        Some((_, asset)) => asset.clone(),
        None => library.get_geometry(game, path)?,
    };
    Instance::new(parent, asset)
}

fn required_parent<'a>(
    document: &'a Document,
    entity: &'a Entity,
) -> Result<Option<&'a Entity>, String> {
    endpoint_origin(document, entity)?;
    if !is_rope(entity) {
        point_basis(document, entity)?;
    }
    let Some(binding) = identifier(entity, "parentname")?.filter(|binding| !binding.is_empty())
    else {
        return Ok(None);
    };
    let (parent, attachment) = parent_target(document, entity, binding)?;
    if parent
        .classname()
        .is_some_and(|class| class.eq_ignore_ascii_case("info_target"))
        && attachment.is_none()
    {
        point_basis(document, parent)?;
        return Ok(None);
    }
    model_parent_properties(parent)?;
    Ok(Some(parent))
}

pub(in crate::session) fn resolve(
    document: &Document,
    models: &Models,
    source: &Entity,
    target: Option<&Entity>,
    sources: &mut BTreeMap<String, Arc<Asset>>,
) -> Result<ResolvedEndpoints, String> {
    let next = identifier(source, "NextKey")?.unwrap_or("");
    let start = endpoint(document, models, source, sources)?;
    let end = if let Some(entity) = target {
        Some(
            endpoint(document, models, entity, sources)
                .map_err(|error| format!("NextKey endpoint: {error}"))?,
        )
    } else if !next.is_empty() {
        return Err("Named NextKey endpoint is unresolved".into());
    } else if property(source, "Dangling")?
        .unwrap_or("0")
        .parse::<i32>()
        .map_err(|_| "Dangling must be an integer flag")?
        == 1
    {
        Some(Endpoint {
            position: DVec3::ZERO,
            forward: DVec3::X,
        })
    } else {
        None
    };
    let rope_length = target.map_or(Ok(0), |entity| {
        let source_origin = origin(document, source)?;
        let target_origin = origin(document, entity)?;
        let delta = source_origin.as_vec3() - target_origin.as_vec3();
        Ok::<_, String>((delta.x * delta.x + delta.y * delta.y + delta.z * delta.z).sqrt() as i32)
    })?;
    Ok(ResolvedEndpoints {
        start,
        end,
        rope_length,
    })
}

fn property<'a>(entity: &'a Entity, key: &str) -> Result<Option<&'a str>, String> {
    let mut values = entity
        .properties
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case(key));
    let value = values.next().map(|(_, value)| value.trim());
    if values.next().is_some() {
        return Err(format!(
            "{key} has ambiguous case-insensitive duplicate keys"
        ));
    }
    Ok(value)
}

fn identifier<'a>(entity: &'a Entity, key: &str) -> Result<Option<&'a str>, String> {
    let value = property(entity, key)?;
    if entity
        .properties
        .iter()
        .any(|(name, value)| name.eq_ignore_ascii_case(key) && value != value.trim())
    {
        return Err(format!(
            "{key} identifiers cannot contain leading or trailing whitespace"
        ));
    }
    if value.is_some_and(|value| {
        value.len() > MAX_IDENTIFIER_BYTES || value.chars().any(char::is_control)
    }) {
        return Err(format!(
            "{key} needs an identifier within {MAX_IDENTIFIER_BYTES} bytes without control characters"
        ));
    }
    Ok(value)
}

fn origin(document: &Document, entity: &Entity) -> Result<DVec3, String> {
    if !entity.solids.is_empty() {
        return Err(
            "Rope endpoints need point entities; brush collision centers are unsupported".into(),
        );
    }
    if entity.hidden {
        return Err("Rope endpoints need visible point entities".into());
    }
    let position =
        map_core::measurements::entity_origin(document.vmf(), ObjectId::Entity(entity.id))?;
    Ok(position.as_vec3().as_dvec3())
}

fn endpoint(
    document: &Document,
    models: &Models,
    entity: &Entity,
    sources: &mut BTreeMap<String, Arc<Asset>>,
) -> Result<Endpoint, String> {
    let origin = endpoint_origin(document, entity)?;
    let forward = if let Some(parent) = identifier(entity, "parentname")?
        && !parent.is_empty()
    {
        let frame = parent_frame(document, models, entity, parent, sources)?;
        let local = frame.inverse().transform_point3(origin);
        let position = frame.transform_point3(local);
        if !local.is_finite()
            || local.abs().max_element() > MAX_EXTENT
            || !position.is_finite()
            || position.distance(origin) > FRAME_TOLERANCE
        {
            return Err("Parent attachment offset exceeds the static rope bounds".into());
        }
        if is_rope(entity) {
            frame.matrix3.x_axis
        } else {
            point_basis(document, entity)?.x_axis
        }
    } else if is_rope(entity) {
        DVec3::X
    } else {
        point_basis(document, entity)?.x_axis
    };
    Ok(Endpoint {
        position: origin,
        forward: forward
            .try_normalize()
            .ok_or("Rope endpoint orientation is degenerate")?,
    })
}

fn endpoint_origin(document: &Document, entity: &Entity) -> Result<DVec3, String> {
    let origin = origin(document, entity)?;
    let classname = identifier(entity, "classname")?.ok_or("Rope endpoint needs a classname")?;
    identifier(entity, "targetname")?;
    if !is_rope(entity) && !classname.eq_ignore_ascii_case("info_target") {
        return Err("Static rope endpoints support rope nodes and info_target; other entities need their runtime collision center or motion".into());
    }
    if property(entity, "model")?.is_some_and(|value| !value.is_empty()) {
        return Err("Model endpoint collision centers are unsupported; parent a rope node to a fixed model attachment instead".into());
    }
    for key in ["StartAttachment", "EndAttachment"] {
        if property(entity, key)?.is_some_and(|value| !value.is_empty() && value != "0") {
            return Err(format!(
                "{key} is not an authored rope attachment binding; use parentname with an attachment name"
            ));
        }
    }
    if property(entity, "parentattachment")?.is_some_and(|value| !value.is_empty() && value != "0")
    {
        return Err("Rope attachment names belong in parentname, not parentattachment".into());
    }
    Ok(origin)
}

fn point_basis(document: &Document, entity: &Entity) -> Result<DMat3, String> {
    property(entity, "angles")?;
    property(entity, "angle")?;
    map_core::query::point_entity_angles(entity)
        .filter(|angles| angles.as_vec3().is_finite())
        .ok_or("Rope point endpoint needs a finite native orientation")?;
    let (_, basis) =
        map_core::measurements::entity_local_pose(document.vmf(), ObjectId::Entity(entity.id))?;
    Ok(basis)
}

fn parent_target<'a>(
    document: &'a Document,
    child: &Entity,
    binding: &'a str,
) -> Result<(&'a Entity, Option<&'a str>), String> {
    let (name, attachment) = binding
        .split_once(',')
        .map_or((binding, None), |(name, attachment)| {
            (name, Some(attachment))
        });
    if name.is_empty()
        || name.contains(['*', '?', '!'])
        || attachment.is_some_and(|attachment| attachment.is_empty() || attachment.contains(','))
    {
        return Err(
            "parentname needs one exact entity name and an optional attachment name".into(),
        );
    }
    let mut parent = None;
    for entity in &document.vmf().entities {
        for (key, value) in entity.properties.iter() {
            if key.eq_ignore_ascii_case("targetname")
                && value.eq_ignore_ascii_case(name)
                && parent.replace(entity).is_some()
            {
                return Err("Rope parentname matches multiple targetname entries".into());
            }
        }
    }
    let parent = parent.ok_or("Rope parentname has no authored target")?;
    identifier(parent, "targetname")?;
    if parent.id == child.id {
        return Err("Rope endpoint cannot parent to itself".into());
    }
    origin(document, parent)?;
    if identifier(parent, "parentname")?.is_some_and(|parent| !parent.is_empty()) {
        return Err("Rope endpoint parent chains are unsupported".into());
    }
    let classname = identifier(parent, "classname")?.ok_or("Rope parent needs a classname")?;
    if classname.eq_ignore_ascii_case("info_target") && attachment.is_none() {
        if property(parent, "model")?.is_some_and(|value| !value.is_empty()) {
            return Err("A static point parent cannot carry a model".into());
        }
        return Ok((parent, attachment));
    }
    if !["prop_dynamic", "prop_dynamic_override"]
        .iter()
        .any(|allowed| classname.eq_ignore_ascii_case(allowed))
    {
        return Err("Rope parents need a fixed root prop_dynamic model or an unattached info_target; static props and runtime-moving classes cannot bind".into());
    }
    Ok((parent, attachment))
}

fn parent_frame(
    document: &Document,
    models: &Models,
    child: &Entity,
    binding: &str,
    sources: &mut BTreeMap<String, Arc<Asset>>,
) -> Result<DAffine3, String> {
    let (parent, attachment) = parent_target(document, child, binding)?;
    if parent
        .classname()
        .is_some_and(|class| class.eq_ignore_ascii_case("info_target"))
        && attachment.is_none()
    {
        return Ok(DAffine3::from_mat3_translation(
            point_basis(document, parent)?,
            origin(document, parent)?,
        ));
    }
    let instance = fixed_model(models, parent, sources)?;
    let frame = match attachment {
        Some(name) => {
            let mut attachments = instance
                .asset
                .model
                .attachments
                .iter()
                .enumerate()
                .filter(|(_, attachment)| attachment.name.eq_ignore_ascii_case(name));
            let (index, _) = attachments
                .next()
                .ok_or("Rope parent model has no attachment with the authored name")?;
            if attachments.next().is_some() {
                return Err("Rope parent model attachment name is ambiguous".into());
            }
            if index >= MAX_PARENT_ATTACHMENT {
                return Err(
                    "Rope parent attachment index exceeds the native parent attachment field"
                        .into(),
                );
            }
            instance
                .attachment(index + 1)
                .ok_or("Rope parent model attachment transform is unavailable")?
        }
        None => DAffine3::from_rotation_translation(instance.rotation, instance.origin),
    };
    let basis = frame.matrix3;
    if !frame.is_finite()
        || frame.translation.abs().max_element() > MAX_EXTENT
        || !(basis.transpose() * basis).abs_diff_eq(DMat3::IDENTITY, FRAME_TOLERANCE)
        || (basis.determinant() - 1.0).abs() > FRAME_TOLERANCE
    {
        return Err(
            "Rope parent attachment needs a finite proper rigid frame within ±65536".into(),
        );
    }
    Ok(frame)
}

fn model_parent_properties(entity: &Entity) -> Result<(), String> {
    for key in [
        "model",
        "origin",
        "angles",
        "angle",
        "modelscale",
        "skin",
        "body",
    ] {
        property(entity, key)?;
    }
    if property(entity, "DefaultAnim")?.is_some_and(|value| !value.is_empty())
        || property(entity, "RandomAnimation")?.is_some_and(|value| value != "0")
        || property(entity, "parentattachment")?
            .is_some_and(|value| !value.is_empty() && value != "0")
    {
        return Err("Animated or separately attached model parents are unsupported".into());
    }
    if render::models::scale(entity)? != 1.0 {
        return Err(
            "Scaled rope attachment parents are outside the fixed rigid binding profile".into(),
        );
    }
    Ok(())
}

fn fixed_model<'a>(
    models: &'a Models,
    entity: &Entity,
    sources: &mut BTreeMap<String, Arc<Asset>>,
) -> Result<&'a Instance, String> {
    model_parent_properties(entity)?;
    let mut instances = models
        .instances
        .iter()
        .filter(|instance| instance.id == ObjectId::Entity(entity.id));
    let instance = instances.next().ok_or_else(|| {
        models
            .issues
            .get(&entity.id)
            .cloned()
            .unwrap_or("Rope parent needs an available cached model instance".into())
    })?;
    if instances.next().is_some() {
        return Err("Rope parent model instance is ambiguous".into());
    }
    let asset = &instance.asset;
    if sources
        .get(&asset.path)
        .is_some_and(|previous| !Arc::ptr_eq(previous, asset))
    {
        return Err("Rope parents refer to different cached sources of the same model".into());
    }
    sources.insert(asset.path.clone(), asset.clone());
    if !instance.asset.model.header.is_static_prop() {
        return Err("Rope parent model needs fixed static-prop bones; reference poses of animated models are unsupported".into());
    }
    if instance.scale != 1.0 {
        return Err(
            "Scaled rope attachment parents are outside the fixed rigid binding profile".into(),
        );
    }
    Ok(instance)
}
