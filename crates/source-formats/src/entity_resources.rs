use crate::{fgd::Class, gamefs::asset_relative, vmf::Entity};

pub const MAX_REFERENCE_BYTES: usize = 256;

pub fn is_source(class: Option<&Class>) -> bool {
    class
        .and_then(|class| class.property("model"))
        .is_some_and(|property| property.kind == "sprite")
}

fn relative(raw: &str) -> Result<String, String> {
    if raw.is_empty() || raw.len() > MAX_REFERENCE_BYTES || raw.trim() != raw {
        return Err(
            "Sprite resource paths require 1–256 bytes without surrounding whitespace".into(),
        );
    }
    if raw.chars().any(char::is_control)
        || raw
            .split(['/', '\\'])
            .any(|part| part.is_empty() || part == ".")
    {
        return Err("Sprite resource paths require nonempty relative path components".into());
    }
    asset_relative(raw).map_err(|error| error.to_string())
}

fn filename(stem: &str) -> Result<(), String> {
    if stem.is_empty() || stem.contains('.') {
        return Err("Unsupported Sprite filename: the filename stem must be nonempty and contain no dots; directories may contain dots".into());
    }
    Ok(())
}

pub fn material_path(raw: &str) -> Result<String, String> {
    let path = relative(raw)?;
    let (stem, extension) = path.rsplit_once('.').ok_or(
        "Sprite resource requires an explicit .vmt or sprites/<basename>.spr reference; extensionless model values dispatch as studio models",
    )?;
    filename(stem.rsplit('/').next().unwrap_or(stem))?;
    let material = match extension {
        "vmt" if path.contains('/') => {
            let stem = stem.strip_prefix("materials/").unwrap_or(stem);
            format!("materials/{stem}.vmt")
        }
        "vmt" => format!("materials/sprites/{stem}.vmt"),
        "spr" => {
            let basename = stem.strip_prefix("sprites/").filter(|name| !name.contains('/'))
                .ok_or("Only the bounded legacy sprites/<basename>.spr form is supported; other .spr directories remap by basename in the engine")?;
            format!("materials/sprites/{basename}.vmt")
        }
        _ => {
            return Err(
                "Sprite resource requires an explicit .vmt or sprites/<basename>.spr reference"
                    .into(),
            );
        }
    };
    if material.len() > MAX_REFERENCE_BYTES {
        return Err("Resolved Sprite VMT path exceeds 256 bytes".into());
    }
    Ok(material)
}

pub fn browse_reference(catalog_relative: &str) -> Result<String, String> {
    let name = relative(catalog_relative)?;
    filename(name.rsplit('/').next().unwrap_or(&name))?;
    let reference = format!("materials/{name}.vmt");
    if reference.len() > MAX_REFERENCE_BYTES {
        return Err("Full Sprite VMT reference exceeds 256 bytes".into());
    }
    Ok(reference)
}

pub fn picker_name(raw: &str) -> Result<String, String> {
    material_path(raw).map(|path| path["materials/".len()..path.len() - ".vmt".len()].into())
}

pub fn rope_material(entity: &Entity) -> Result<String, String> {
    fn property<'a>(entity: &'a Entity, key: &'a str) -> Result<Option<&'a str>, String> {
        let mut values = entity.properties.get_all(key);
        let value = values.next().map(str::trim);
        if values.next().is_some() {
            return Err(format!(
                "{key} has ambiguous case-insensitive duplicate keys"
            ));
        }
        if value.is_some_and(|value| value.len() > MAX_REFERENCE_BYTES) {
            return Err(format!("{key} exceeds {MAX_REFERENCE_BYTES} bytes"));
        }
        Ok(value)
    }
    let material = property(entity, "RopeMaterial")?.filter(|value| !value.is_empty());
    let legacy = property(entity, "RopeShader")?.map_or(Ok(0), |raw| {
        raw.parse::<i32>()
            .map_err(|_| "RopeShader must be an integer".to_owned())
    })?;
    Ok(material
        .unwrap_or(match legacy {
            0 => "cable/cable",
            1 => "cable/rope",
            _ => "cable/chain",
        })
        .to_owned())
}
