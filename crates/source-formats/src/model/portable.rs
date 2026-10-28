use std::ffi::CStr;
use std::io;

use crate::keyvalues::{self, Node};

use super::{HardwareCharge, HardwareLimits, Model, hardware::Budget, invalid, reader::Reader};

const HEADER_BYTES: usize = 408;
const MAX_PATH_BYTES: usize = 259;
const MAX_METADATA_BYTES: usize = 4096;
const MAX_METADATA_NODES: usize = 8;
const MAX_PHYSICS_TEXT_BYTES: usize = 64 * 1024;
const MAX_PHYSICS_TEXT_NODES: usize = 16 * 1024;
const ANIMATION_BYTES: usize = 100;
const SEQUENCE_BYTES: usize = 212;
const ALL_ZEROS: i32 = 0x20;
const EMPTY_ANIMATION: [u8; 4] = [u8::MAX, 0, 0, 0];

pub fn validate_portable_static(
    model_path: &str,
    mdl: &[u8],
    vvd: &[u8],
    vtx: &[u8],
    limits: HardwareLimits,
    charge: &mut impl FnMut(HardwareCharge) -> io::Result<()>,
) -> io::Result<Model> {
    let mut budget = Budget::hardware(limits, charge)?;
    let header = crate::mdl::ModelHeader::parse(mdl)?;
    if !header.is_static_prop() {
        return Err(invalid("portable model packing requires a static-prop MDL"));
    }
    let reader = Reader::new(mdl, HEADER_BYTES)?;
    canonical_path(model_path, false)?;
    let name = model_path
        .strip_prefix("models/")
        .filter(|name| name.ends_with(".mdl") && name.len() > 4)
        .ok_or_else(|| invalid("portable static models require a models/*.mdl path"))?;
    let original_name = terminated(reader.bytes(12, 64)?)?;
    if original_name != name {
        return Err(invalid(
            "portable MDL internal name must match its canonical models-relative path",
        ));
    }
    if reader.int(400)? != 0 {
        return Err(invalid(
            "portable static models do not support extended MDL headers or names",
        ));
    }
    for offset in [164, 240, 260, 268, 276, 284, 292, 300, 320, 336, 352, 384] {
        if reader.int(offset)? != 0 {
            return Err(invalid(
                "portable static models do not support external models, animations or specialized controls",
            ));
        }
    }
    let animation_name = reader.count(348, mdl.len())?;
    if animation_name != 0 && !string(&reader, animation_name)?.is_empty() {
        return Err(invalid(
            "portable static models do not support named external animation files",
        ));
    }
    if reader.byte(377)? != 0 {
        return Err(invalid("portable static models require root LOD zero"));
    }
    let bodies = reader.table(0, 232, 236, 16, 1)?;
    if bodies.len != 1 || reader.count(bodies.at(0)? + 4, 1)? != 1 {
        return Err(invalid(
            "portable static models require one bodypart and one body variant",
        ));
    }
    let textures = reader.table(0, 204, 208, 64, super::MAX_MATERIALS)?;
    if textures.len == 0 {
        return Err(invalid("portable static models need declared materials"));
    }
    budget.charge(0, textures.len)?;
    for index in 0..textures.len {
        budget.checkpoint(index)?;
        let base = textures.at(index)?;
        let value = string(&reader, reader.relative(base, 0)?)?;
        canonical_path(value, false)?;
        if value.starts_with("materials/")
            || value
                .rsplit('/')
                .next()
                .is_none_or(|name| name.contains('.'))
        {
            return Err(invalid(
                "portable MDL material names require logical paths without materials/ or an extension",
            ));
        }
    }
    let directories = reader.table(0, 212, 216, 4, super::MAX_MATERIALS)?;
    budget.charge(0, directories.len)?;
    for index in 0..directories.len {
        budget.checkpoint(index)?;
        let value = string(&reader, reader.count(directories.at(index)?, mdl.len())?)?;
        if value == "/" {
            return Err(invalid(
                "portable MDL material directories must be relative",
            ));
        }
        canonical_path(value.strip_suffix('/').unwrap_or(value), true)?;
        if value.starts_with("materials/") {
            return Err(invalid(
                "portable MDL material directories must be relative to materials/",
            ));
        }
    }
    let surface = reader.count(308, mdl.len())?;
    if surface != 0 {
        surface_property(string(&reader, surface)?)?;
    }
    metadata(&reader, &mut budget)?;
    animations(&reader, &mut budget)?;
    bones(&reader, &mut budget)?;
    tangents(vvd, &mut budget)?;
    let (model, _) = Model::parse_inner(mdl, vvd, vtx, &mut budget)?;
    Ok(model)
}

fn terminated(bytes: &[u8]) -> io::Result<&str> {
    CStr::from_bytes_until_nul(bytes)
        .map_err(|_| invalid("portable MDL strings need a bounded NUL terminator"))?
        .to_str()
        .map_err(|_| invalid("portable MDL strings must be UTF-8"))
}

fn string<'a>(reader: &Reader<'a>, start: usize) -> io::Result<&'a str> {
    let mut length = 0;
    while length <= MAX_PATH_BYTES {
        if reader.byte(
            start
                .checked_add(length)
                .ok_or_else(|| invalid("MDL string overflow"))?,
        )? == 0
        {
            return terminated(reader.bytes(start, length + 1)?);
        }
        length += 1;
    }
    Err(invalid("portable MDL string exceeds 259 bytes"))
}

fn canonical_path(path: &str, empty: bool) -> io::Result<()> {
    if path.is_empty() && empty {
        return Ok(());
    }
    if path.is_empty()
        || path.len() > MAX_PATH_BYTES
        || !path.bytes().all(|value| {
            value.is_ascii_lowercase()
                || value.is_ascii_digit()
                || matches!(value, b'_' | b'-' | b'.' | b'/')
        })
        || path
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err(invalid(
            "portable MDL paths require canonical lowercase ASCII names",
        ));
    }
    Ok(())
}

fn surface_property(value: &str) -> io::Result<()> {
    if !matches!(value, "default" | "metalvent") {
        return Err(invalid(
            "portable static models require the standard default or metalvent surface property",
        ));
    }
    Ok(())
}

fn metadata(reader: &Reader<'_>, budget: &mut Budget<'_>) -> io::Result<()> {
    let length = reader.count(316, MAX_METADATA_BYTES)?;
    if length == 0 {
        return Ok(());
    }
    let start = reader.count(312, crate::mdl::MAX_MODEL_BYTES)?;
    let bytes = reader.bytes(start, length)?;
    let text = CStr::from_bytes_with_nul(bytes)
        .map_err(|_| invalid("portable MDL metadata needs one terminal NUL"))?
        .to_str()
        .map_err(|_| invalid("portable MDL metadata must be UTF-8"))?;
    budget.charge(
        length * 32 + MAX_METADATA_NODES * std::mem::size_of::<Node>(),
        length,
    )?;
    let nodes = keyvalues::parse_static_material_bounded(text, 3, MAX_METADATA_NODES)
        .map_err(|_| invalid("portable MDL metadata syntax is unsupported"))?;
    let root = only_node(&nodes, "mdlkeyvalue")?;
    let prop = only_node(
        root.children()
            .ok_or_else(|| invalid("MDL metadata must be a block"))?,
        "prop_data",
    )?;
    let base = only_node(
        prop.children()
            .ok_or_else(|| invalid("MDL prop_data must be a block"))?,
        "base",
    )?;
    if base.as_str() != Some("Metal.Medium") {
        return Err(invalid(
            "portable MDL metadata only supports standard prop_data base Metal.Medium",
        ));
    }
    Ok(())
}

fn only_node<'a>(nodes: &'a [Node], key: &str) -> io::Result<&'a Node> {
    if nodes.len() != 1 || !nodes[0].key.eq_ignore_ascii_case(key) {
        return Err(invalid(
            "portable MDL metadata contains unsupported declarations",
        ));
    }
    Ok(&nodes[0])
}

fn animations(reader: &Reader<'_>, budget: &mut Budget<'_>) -> io::Result<()> {
    let animations = reader.table(0, 180, 184, ANIMATION_BYTES, 1)?;
    budget.charge(0, animations.len)?;
    for index in 0..animations.len {
        let base = animations.at(index)?;
        if reader.relative(base, 0)? != 0 {
            return Err(invalid(
                "portable local rest animation must reference its MDL header",
            ));
        }
        if !matches!(reader.int(base + 12)?, 0 | ALL_ZEROS)
            || reader.int(base + 16)? != 1
            || reader.float(base + 8)? <= 0.0
        {
            return Err(invalid(
                "portable static models only support a trivial local rest animation",
            ));
        }
        for offset in [20, 52, 60, 68, 72, 80, 84, 88, 92] {
            if reader.int(base + offset)? != 0 {
                return Err(invalid(
                    "portable static models do not support external or sectioned animation, movement or IK",
                ));
            }
        }
        let animation = reader.int(base + 56)?;
        if animation != 0 {
            let start = reader.relative(base, 56)?;
            if animation < ANIMATION_BYTES as i32
                || reader.bytes(start, EMPTY_ANIMATION.len())? != EMPTY_ANIMATION
            {
                return Err(invalid(
                    "portable static models require a terminal empty local rest animation",
                ));
            }
        } else if reader.int(base + 12)? != ALL_ZEROS {
            return Err(invalid("portable static rest animation data is missing"));
        }
        string(reader, reader.relative(base, 4)?)?;
    }
    let sequences = reader.table(0, 188, 192, SEQUENCE_BYTES, 1)?;
    budget.charge(0, sequences.len)?;
    if animations.len != sequences.len {
        return Err(invalid(
            "portable static models need matching rest animation and sequence tables",
        ));
    }
    for index in 0..sequences.len {
        let base = sequences.at(index)?;
        if reader.relative(base, 0)? != 0 {
            return Err(invalid(
                "portable rest sequence must reference its MDL header",
            ));
        }
        if reader.int(base + 12)? != 0
            || reader.int(base + 56)? != 1
            || reader.int(base + 68)? != 1
            || reader.int(base + 72)? != 1
        {
            return Err(invalid(
                "portable static models require one non-autoplay rest sequence blend",
            ));
        }
        for offset in [
            24, 64, 112, 116, 120, 124, 128, 132, 136, 140, 144, 148, 164, 176, 180, 188,
        ] {
            if reader.int(base + offset)? != 0 {
                return Err(invalid(
                    "portable static sequences do not support events, autolayers, IK or keyvalues",
                ));
            }
        }
        let blend = reader.relative(base, 60)?;
        if reader.short(blend)? != 0 {
            return Err(invalid(
                "portable rest sequence must reference its local rest animation",
            ));
        }
        string(reader, reader.relative(base, 4)?)?;
        string(reader, reader.relative(base, 8)?)?;
    }
    Ok(())
}

fn tangents(vvd: &[u8], budget: &mut Budget<'_>) -> io::Result<()> {
    let reader = Reader::new(vvd, 64)?;
    let count = reader.count(16, super::MAX_VERTICES)?;
    let start = reader.count(60, vvd.len())?;
    let vertex_start = reader.count(56, vvd.len())?;
    let vertex_end = vertex_start
        .checked_add(count * 48)
        .ok_or_else(|| invalid("portable VVD vertex range overflow"))?;
    if start < vertex_end || vertex_start < 64 {
        return Err(invalid(
            "portable VVD needs a complete separate tangent stream",
        ));
    }
    reader.bytes(start, count * 16)?;
    budget.charge(0, count * 5)?;
    for index in 0..count {
        budget.checkpoint(index)?;
        let offset = start + index * 16;
        let xyz = reader.vector(offset)?;
        let length = xyz.iter().map(|value| value * value).sum::<f32>();
        let sign = reader.float(offset + 12)?;
        if (length - 1.0).abs() > 0.01 || !matches!(sign, -1.0 | 1.0) {
            return Err(invalid(
                "portable VVD tangents must be finite unit vectors with a unit sign",
            ));
        }
    }
    Ok(())
}

fn bones(reader: &Reader<'_>, budget: &mut Budget<'_>) -> io::Result<()> {
    let bones = reader.table(0, 156, 160, 216, super::bones::MAX_BONES)?;
    budget.charge(0, bones.len * (40 + bones.len))?;
    for index in 0..bones.len {
        budget.checkpoint(index)?;
        let base = bones.at(index)?;
        string(reader, reader.relative(base, 0)?)?;
        if reader.int(base + 160)? & 0x1f != 0
            || reader.int(base + 164)? != 0
            || reader.int(base + 168)? != 0
        {
            return Err(invalid(
                "portable static models do not support procedural or screen-aligned bones",
            ));
        }
        for controller in 0..6 {
            if reader.int(base + 8 + controller * 4)? != -1 {
                return Err(invalid(
                    "portable static bones do not support bone controllers",
                ));
            }
        }
        for field in (32..160).step_by(4) {
            reader.float(base + field)?;
        }
        let mut rotation = 0.0;
        for field in (44..60).step_by(4) {
            let value = reader.float(base + field)?;
            rotation += value * value;
        }
        if (rotation - 1.0_f32).abs() > 0.01 {
            return Err(invalid(
                "portable static bone rotations must be unit quaternions",
            ));
        }
        if reader.int(base + 176)? != 0 {
            surface_property(string(reader, reader.relative(base, 176)?)?)?;
        }
        let mut current = index;
        let mut steps = 0;
        loop {
            let parent = reader.int(bones.at(current)? + 4)?;
            if parent == -1 {
                break;
            }
            if parent < 0 || parent as usize >= bones.len || steps >= bones.len {
                return Err(invalid(
                    "portable static bone hierarchy is invalid or cyclic",
                ));
            }
            current = parent as usize;
            steps += 1;
        }
    }
    Ok(())
}

pub fn validate_portable_physics(data: &[u8], checksum: i32) -> io::Result<()> {
    let count = crate::mdl::validate_physics(data, checksum)?;
    let reader = Reader::new(data, 16)?;
    let mut offset = 16;
    for _ in 0..count {
        let length = reader.count(offset, crate::bsp::MAX_COLLISION_BYTES)?;
        offset = offset
            .checked_add(4 + length)
            .ok_or_else(|| invalid("portable PHY solid range overflow"))?;
    }
    let bytes = &data[offset..];
    if bytes.len() > MAX_PHYSICS_TEXT_BYTES {
        return Err(invalid("portable PHY text exceeds 64 KiB"));
    }
    let text = CStr::from_bytes_with_nul(bytes)
        .map_err(|_| invalid("portable PHY text needs one terminal NUL"))?
        .to_str()
        .map_err(|_| invalid("portable PHY text must be UTF-8"))?;
    let nodes = keyvalues::parse_static_material_bounded(text, 2, MAX_PHYSICS_TEXT_NODES)
        .map_err(|_| invalid("portable PHY text syntax is unsupported"))?;
    let mut solids = 0;
    let mut edits = false;
    for node in &nodes {
        let children = node
            .children()
            .ok_or_else(|| invalid("portable PHY roots must be blocks"))?;
        if node.key.eq_ignore_ascii_case("solid") {
            validate_physics_solid(children, solids)?;
            solids += 1;
        } else if node.key.eq_ignore_ascii_case("editparams") && !edits {
            edits = true;
            validate_editparams(children)?;
        } else {
            return Err(invalid(
                "portable PHY contains an unsupported or duplicate root",
            ));
        }
    }
    if solids != count {
        return Err(invalid(
            "portable PHY text must describe every collision solid",
        ));
    }
    Ok(())
}

fn scalar(children: &[Node], index: usize) -> io::Result<&str> {
    let node = &children[index];
    if children[..index]
        .iter()
        .any(|old| old.key.eq_ignore_ascii_case(&node.key))
    {
        return Err(invalid("portable PHY scalar declarations must be unique"));
    }
    node.as_str()
        .ok_or_else(|| invalid("portable PHY nested declarations are unsupported"))
}

fn nonnegative(value: &str) -> io::Result<()> {
    if value
        .parse::<f64>()
        .ok()
        .is_none_or(|number| !number.is_finite() || number < 0.0)
    {
        return Err(invalid(
            "portable PHY numeric scalars must be finite and nonnegative",
        ));
    }
    Ok(())
}

fn validate_physics_solid(children: &[Node], expected: usize) -> io::Result<()> {
    if children.len() > 9 {
        return Err(invalid("portable PHY solid has too many declarations"));
    }
    let mut found_index = false;
    for (index, node) in children.iter().enumerate() {
        let value = scalar(children, index)?;
        match node.key.to_ascii_lowercase().as_str() {
            "index" if value.parse::<usize>().ok() == Some(expected) => found_index = true,
            "name"
                if value.len() <= 128
                    && value.is_ascii()
                    && !value.bytes().any(|byte| byte < b' ' || byte == 127) => {}
            "surfaceprop" => surface_property(value)?,
            "mass" | "damping" | "rotdamping" | "inertia" | "volume" => nonnegative(value)?,
            _ => return Err(invalid("portable PHY solid contains an unsupported scalar")),
        }
    }
    if !found_index {
        return Err(invalid(
            "portable PHY solids require ordered explicit indexes",
        ));
    }
    Ok(())
}

fn validate_editparams(children: &[Node]) -> io::Result<()> {
    if children.len() > 2 {
        return Err(invalid("portable PHY editparams has too many declarations"));
    }
    for (index, node) in children.iter().enumerate() {
        let value = scalar(children, index)?;
        match node.key.to_ascii_lowercase().as_str() {
            "rootname" if value.is_empty() => {}
            "totalmass" => nonnegative(value)?,
            _ => {
                return Err(invalid(
                    "portable PHY editparams contains an unsupported scalar",
                ));
            }
        }
    }
    Ok(())
}
