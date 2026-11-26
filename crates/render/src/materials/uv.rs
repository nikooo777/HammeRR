use source_formats::{
    gamefs::asset_relative,
    vmt::{self, Material, texture_path},
    vtf::Vtf,
};

use super::validate_ordinary_color_texture;

pub(crate) type Matrix = [[f32; 3]; 2];
pub(crate) const IDENTITY: Matrix = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
pub(crate) const IDENTITY_AXIS: [f32; 4] = [1.0, 1.0, 0.0, 0.0];
pub(crate) const FLAG: u32 = 128;

pub(super) fn is_axis_identity(value: Option<&str>) -> bool {
    vmt::texture_transform::<f32>(value, vmt::DuplicateOperations::Reject) == Ok(IDENTITY_AXIS)
}

pub(crate) fn checked_matrix(value: Option<&str>) -> Result<Matrix, String> {
    let rows = vmt::texture_transform_matrix(value, vmt::DuplicateOperations::Reject)
        .map_err(|error| format!("Invalid static texture transform: {error:?}"))?;
    let determinant = rows[0][0] * rows[1][1] - rows[0][1] * rows[1][0];
    if !determinant.is_finite() || determinant.abs() < 1e-12 {
        return Err("Static texture transform must be nonsingular".into());
    }
    Ok(rows.map(|row| row.map(|value| value as f32)))
}

pub(crate) fn gpu_rows(rows: Matrix) -> [[f32; 4]; 2] {
    rows.map(|[u, v, offset]| [u, v, offset, 0.0])
}

pub(crate) fn fits_gpu(rows: Matrix, mut coordinates: impl Iterator<Item = [f32; 2]>) -> bool {
    coordinates.all(|[u, v]| {
        rows.into_iter()
            .all(|[x, y, offset]| (x * u).abs() + (y * v).abs() + offset.abs() <= f32::MAX / 4.0)
    })
}

#[derive(Debug)]
pub(crate) struct BaseTransform {
    pub(crate) rows: Matrix,
    pub(crate) model: bool,
}

pub(super) fn resolve(
    material: &Material,
    base: Option<&Vtf>,
    display_texture: Option<&str>,
) -> Result<Option<BaseTransform>, String> {
    let Some(value) = material.get("$basetexturetransform") else {
        return Ok(None);
    };
    if is_axis_identity(Some(value)) {
        return Ok(None);
    }
    let rows = checked_matrix(Some(value))?;
    if rows == IDENTITY {
        return Ok(None);
    }
    let fields: Vec<_> = value.split_whitespace().take(12).collect();
    if !matches!(
        fields.as_slice(),
        [
            "center",
            _,
            _,
            "scale",
            _,
            _,
            "rotate",
            _,
            "translate",
            _,
            _
        ]
    ) {
        return Err("Use center U V scale U V rotate degrees translate U V for a static base texture transform".into());
    }
    let model = match material.shader.as_str() {
        "lightmappedgeneric" => false,
        "vertexlitgeneric" => true,
        _ => {
            return Err(
                "Base texture transforms require LightmappedGeneric or VertexLitGeneric".into(),
            );
        }
    };
    if !material.blocks.is_empty()
        || material.is_tool()
        || material.translucent()
        || material.alpha_test()
    {
        return Err("Base texture transforms require an opaque non-tool material without proxies or fallback blocks".into());
    }
    if let Some((key, _)) = material.params.iter().find(|(key, _)| {
        !matches!(
            key.as_str(),
            "$basetexture"
                | "$basetexturetransform"
                | "$model"
                | "$surfaceprop"
                | "$surfaceprop2"
                | "$nodecal"
                | "%keywords"
        )
    }) {
        let key: String = key.chars().take(128).collect();
        return Err(format!(
            "Base texture transform does not support parameter {key}"
        ));
    }
    let declared = material
        .get("$basetexture")
        .filter(|value| asset_relative(value.trim()).is_ok())
        .map(texture_path);
    let base = base
        .filter(|_| declared.is_some() && declared.as_deref() == display_texture)
        .ok_or("Base texture transform requires the exact declared base texture")?;
    validate_ordinary_color_texture(base, u32::from(u16::MAX))
        .map_err(|error| format!("Base texture transform: {error}"))?;
    Ok(Some(BaseTransform { rows, model }))
}
