use source_formats::vmf::parse_vec3;
use source_formats::vmt::Material;

const PARAMETERS: [&str; 4] = [
    "$phong",
    "$phongexponent",
    "$phongboost",
    "$phongfresnelranges",
];

#[derive(Debug, Clone, Copy)]
pub(crate) struct ModelPhong {
    exponent: f32,
    boost: f32,
    fresnel: [f32; 3],
}

impl ModelPhong {
    pub(crate) fn uniform(self) -> [[f32; 4]; 2] {
        let [minimum, middle, maximum] = self.fresnel;
        [
            [
                2.0 * (middle - minimum),
                middle,
                2.0 * (maximum - middle),
                self.boost,
            ],
            [self.exponent, 0.0, 0.0, 0.0],
        ]
    }

    pub(crate) fn gain_bound(self) -> f64 {
        f64::from(self.boost) * f64::from(self.fresnel[2])
    }
}

pub(super) fn requested(material: &Material) -> bool {
    material.shader == "vertexlitgeneric"
        && material.params.iter().any(|(key, value)| {
            (key.starts_with("$phong") || key.contains("phongmask"))
                && !disabled_parameter(key, value)
        })
}

pub(super) fn allowed_parameter(key: &str, value: &str, phong: bool) -> bool {
    disabled_parameter(key, value) || phong && PARAMETERS.contains(&key)
}

fn disabled_parameter(key: &str, value: &str) -> bool {
    key == "$phong" && value.trim().parse::<i32>() == Ok(0)
}

pub(super) fn resolve(material: &Material) -> Result<ModelPhong, String> {
    if let Some((key, _)) = material.params.iter().find(|(key, _)| {
        (key.starts_with("$phong") || key.contains("phongmask"))
            && !PARAMETERS.contains(&key.as_str())
    }) {
        let key: String = key.chars().take(256).collect();
        return Err(format!("Model Phong excludes unsupported parameter {key}"));
    }
    if material
        .get("$phong")
        .and_then(|value| value.trim().parse::<i32>().ok())
        != Some(1)
    {
        return Err("Model Phong requires an explicit $phong 1".into());
    }
    if material
        .get("$bumpmap")
        .is_none_or(|value| value.trim().is_empty())
    {
        return Err("Model Phong requires one declared $bumpmap for its normal and mask".into());
    }
    for key in ["$basetexturetransform", "$bumptransform"] {
        if !super::uv::is_axis_identity(material.get(key)) {
            return Err(format!("Model Phong requires an identity {key}"));
        }
    }
    let exponent = scalar(material, "$phongexponent", 1.0 / 1024.0..=1024.0)?;
    let boost = scalar(material, "$phongboost", 0.0..=64.0)?;
    let fresnel = material
        .get("$phongfresnelranges")
        .map(str::trim)
        .and_then(|value| value.strip_prefix('[')?.strip_suffix(']'))
        .filter(|value| !value.contains(['[', ']', '(', ')', '{', '}']))
        .and_then(parse_vec3)
        .filter(|value| {
            0.0 <= value.x && value.x <= value.y && value.y <= value.z && value.z <= 1.0
        })
        .map(|value| value.as_vec3().to_array())
        .ok_or(
            "Model Phong requires explicit nondecreasing $phongfresnelranges [min middle max] in 0–1",
        )?;
    Ok(ModelPhong {
        exponent,
        boost,
        fresnel,
    })
}

fn scalar(
    material: &Material,
    key: &str,
    range: std::ops::RangeInclusive<f32>,
) -> Result<f32, String> {
    material
        .get(key)
        .and_then(|value| value.trim().parse().ok())
        .filter(|value| range.contains(value))
        .ok_or_else(|| {
            format!(
                "Model Phong requires an explicit finite {key} in {}–{}",
                range.start(),
                range.end()
            )
        })
}
