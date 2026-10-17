use source_formats::{fgd::Class, gamefs::asset_relative};

pub use map_core::entity_helpers::bounded_preview as preview;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_dispatch_keeps_full_virtual_and_legacy_resource_identity() {
        for (raw, material) in [
            ("sprites/glow01.spr", "materials/sprites/glow01.vmt"),
            ("sprites/glow01.vmt", "materials/sprites/glow01.vmt"),
            (
                "materials/sprites/glow01.vmt",
                "materials/sprites/glow01.vmt",
            ),
            ("glow01.vmt", "materials/sprites/glow01.vmt"),
            ("materials/glow01.vmt", "materials/glow01.vmt"),
            ("materials/materials/foo.vmt", "materials/materials/foo.vmt"),
            (
                "MATERIALS\\Sprites\\Glow01.VMT",
                "materials/sprites/glow01.vmt",
            ),
            ("a.b/foo.vmt", "materials/a.b/foo.vmt"),
        ] {
            assert_eq!(material_path(raw).unwrap(), material, "{raw}");
            assert_eq!(
                browse_reference(&picker_name(raw).unwrap()).unwrap(),
                material
            );
        }
        for raw in [
            "sprites/glow01",
            "custom/glow01.spr",
            "glow01.spr",
            "sprites/sub/glow01.spr",
            "sprites/foo.bar.spr",
            "materials/foo.bar.vmt",
            "../foo.vmt",
            "/foo.vmt",
            "sprites/./foo.vmt",
            "sprites//foo.vmt",
            " sprites/foo.vmt",
            "sprites/foo.vmt ",
        ] {
            assert!(material_path(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn browse_keeps_root_nested_and_repeated_materials_directories() {
        for name in ["foo", "sprites/foo", "materials/foo", "a.b/foo"] {
            let reference = browse_reference(name).unwrap();
            assert_eq!(reference, format!("materials/{name}.vmt"));
            assert_eq!(material_path(&reference).unwrap(), reference);
        }
        for name in [
            "foo.bar",
            "sprites/foo.bar",
            "../foo",
            "/foo",
            "foo.vmt",
            "",
            " foo",
        ] {
            assert!(browse_reference(name).is_err(), "{name}");
        }
        assert!(browse_reference(&"a".repeat(MAX_REFERENCE_BYTES)).is_err());
    }
}
