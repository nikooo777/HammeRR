use std::sync::Arc;

use glam::DVec2;
use source_formats::{vmt, vtf::Vtf};

use super::{Allocation, Assets, Budget};

const MAX_TEXTURE_SIZE: u32 = crate::MAX_BRUSH_SHADOW_TEXTURE_SIZE;
const MAX_ALPHA_BYTES: usize = crate::MAX_BRUSH_SHADOW_ALPHA_BYTES;
const CLAMP_S: u32 = 0x0000_0004;
const CLAMP_T: u32 = 0x0000_0008;

#[derive(Debug)]
pub(in crate::lighting) struct Texture {
    dimensions: [usize; 2],
    clamp: [bool; 2],
    alpha: Vec<u8>,
}

#[derive(Debug)]
pub(in crate::lighting) struct Material {
    texture: Arc<Texture>,
    transform: [[f64; 3]; 2],
    reference: f64,
    fractional: bool,
}

impl Material {
    pub(in crate::lighting) fn dimensions(&self) -> [usize; 2] {
        self.texture.dimensions
    }

    pub(in crate::lighting) fn blocks(&self, uv: DVec2) -> Result<bool, String> {
        Ok(self.coverage(uv)? != 0)
    }

    pub(in crate::lighting) fn fractional(&self) -> bool {
        self.fractional
    }

    pub(in crate::lighting) fn coverage(&self, uv: DVec2) -> Result<u8, String> {
        let [s, t] = self.transform;
        let uv = if s[1] == 0.0 && t[0] == 0.0 {
            uv * DVec2::new(s[0], t[1]) + DVec2::new(s[2], t[2])
        } else {
            DVec2::new(
                uv.x * s[0] + uv.y * s[1] + s[2],
                uv.x * t[0] + uv.y * t[1] + t[2],
            )
        };
        if !uv.is_finite() {
            return Err("Static-prop texture-shadow coordinates must be finite".into());
        }
        let texel: [usize; 2] = std::array::from_fn(|axis| {
            let coordinate = if self.texture.clamp[axis] {
                uv[axis].clamp(0.0, 1.0)
            } else {
                uv[axis].rem_euclid(1.0)
            };
            ((coordinate * self.texture.dimensions[axis] as f64).floor() as usize)
                .min(self.texture.dimensions[axis] - 1)
        });
        let alpha = self.texture.alpha[texel[1] * self.texture.dimensions[0] + texel[0]];
        Ok(if self.fractional {
            alpha
        } else if f64::from(alpha) / 255.0 >= self.reference {
            255
        } else {
            0
        })
    }
}

fn scalar(material: &vmt::Material, key: &str, default: f64) -> Result<f64, String> {
    let Some(value) = material.get(key) else {
        return Ok(default);
    };
    let value = value
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("invalid {key}"))?;
    if !value.is_finite() {
        return Err(format!("nonfinite {key}"));
    }
    Ok(value)
}

fn transform(value: Option<&str>) -> Result<[[f64; 3]; 2], String> {
    source_formats::vmt::texture_transform_matrix(
        value,
        source_formats::vmt::DuplicateOperations::Reject,
    )
    .map_err(|error| match error {
        source_formats::vmt::TransformError::Syntax => {
            "unsupported $basetexturetransform syntax".into()
        }
        source_formats::vmt::TransformError::Duplicate => {
            "duplicate $basetexturetransform operation".into()
        }
        source_formats::vmt::TransformError::Incomplete => {
            "incomplete $basetexturetransform".into()
        }
        source_formats::vmt::TransformError::Invalid => "invalid $basetexturetransform".into(),
        source_formats::vmt::TransformError::Bounds => {
            "$basetexturetransform values must be finite and within ±1024".into()
        }
        source_formats::vmt::TransformError::Rotation => {
            "invalid $basetexturetransform rotation".into()
        }
    })
}

fn texture_allocations(
    pixels: usize,
    budget: &Budget<'_>,
) -> Result<(Allocation, Allocation), String> {
    let alpha_bytes = pixels
        .checked_add(size_of::<Texture>() + 2 * size_of::<usize>())
        .ok_or("Texture-shadow alpha allocation size overflow")?;
    let alpha = budget.reserve_bytes(alpha_bytes)?;
    let rgba_bytes = pixels
        .checked_mul(4)
        .ok_or("Texture-shadow image allocation size overflow")?;
    let rgba = budget.reserve_bytes(rgba_bytes)?;
    Ok((alpha, rgba))
}

pub(in crate::lighting) fn prepare(
    material: &vmt::Material,
    assets: &mut Assets<'_, '_>,
    budget: &mut Budget<'_>,
) -> Result<Option<Arc<Material>>, String> {
    prepare_for(material, assets, budget, false)
}

pub(in crate::lighting) fn prepare_brush(
    material: &vmt::Material,
    assets: &mut Assets<'_, '_>,
    budget: &mut Budget<'_>,
) -> Result<Arc<Material>, String> {
    prepare_for(material, assets, budget, true)?
        .ok_or_else(|| "Brush texture shadows requires $alphatest 1".into())
}

fn prepare_for(
    material: &vmt::Material,
    assets: &mut Assets<'_, '_>,
    budget: &mut Budget<'_>,
    brush: bool,
) -> Result<Option<Arc<Material>>, String> {
    if brush {
        if material.shader != "lightmappedgeneric"
            || scalar(material, "$alphatest", 0.0)? != 1.0
            || scalar(material, "$translucent", 0.0)? != 0.0
        {
            return Err("Brush texture shadows supports only binary $alphatest 1 LightmappedGeneric; fractional translucency is unsupported".into());
        }
        for (key, _) in &material.params {
            if !matches!(
                key.as_str(),
                "$basetexture"
                    | "$basetexturetransform"
                    | "$alphatest"
                    | "$alphatestreference"
                    | "$translucent"
                    | "$alpha"
                    | "$frame"
                    | "$surfaceprop"
                    | "$surfaceprop2"
                    | "%keywords"
                    | "%tooltexture"
            ) {
                return Err(format!(
                    "Brush texture shadows does not support alpha material parameter {key}"
                ));
            }
        }
    }
    if !matches!(
        material.shader.as_str(),
        "vertexlitgeneric" | "unlitgeneric" | "lightmappedgeneric"
    ) {
        return Err(format!(
            "unsupported texture-shadow shader {}",
            material.shader
        ));
    }
    if !material.blocks.is_empty() {
        return Err(
            "material animation and conditional blocks are unsupported for texture shadows".into(),
        );
    }
    let translucent = scalar(material, "$translucent", 0.0)?;
    let alpha_test = scalar(material, "$alphatest", 0.0)?;
    let fractional = translucent != 0.0;
    if fractional && (translucent != 1.0 || alpha_test != 0.0) {
        return Err("$translucent texture shadows require value one and no alpha test".into());
    }
    if fractional && material.get("$alphatestreference").is_some() {
        return Err("$alphatestreference is unsupported for $translucent texture shadows".into());
    }
    for key in ["$additive", "$vertexalpha", "$distancealpha"] {
        if scalar(material, key, 0.0)? != 0.0 {
            return Err(format!("{key} is unsupported for texture shadows"));
        }
    }
    if scalar(material, "$alpha", 1.0)? != 1.0 {
        return Err("$alpha modulation is unsupported for texture shadows".into());
    }
    if alpha_test == 0.0 && !fractional {
        return Ok(None);
    }
    for key in ["$selfillum", "$basealphaenvmapmask", "$seamless_scale"] {
        if scalar(material, key, 0.0)? != 0.0 {
            return Err(format!("{key} is unsupported for texture shadows"));
        }
    }
    if !brush
        && !matches!(
            material.shader.as_str(),
            "vertexlitgeneric" | "unlitgeneric"
        )
    {
        return Err(format!(
            "alpha-texture shadows do not support shader {}",
            material.shader
        ));
    }
    for (key, _) in &material.params {
        if fractional
            && !matches!(
                key.as_str(),
                "$basetexture"
                    | "$basetexturetransform"
                    | "$translucent"
                    | "$alphatest"
                    | "$alpha"
                    | "$additive"
                    | "$vertexalpha"
                    | "$distancealpha"
                    | "$selfillum"
                    | "$basealphaenvmapmask"
                    | "$seamless_scale"
                    | "$frame"
                    | "$model"
                    | "$surfaceprop"
                    | "$surfaceprop2"
                    | "%keywords"
                    | "%tooltexture"
            )
        {
            return Err(format!(
                "unsupported $translucent texture-shadow parameter {key}"
            ));
        }
        if !matches!(
            key.as_str(),
            "$basetexture"
                | "$basetexturetransform"
                | "$alphatest"
                | "$alphatestreference"
                | "$alpha"
                | "$translucent"
                | "$additive"
                | "$vertexalpha"
                | "$distancealpha"
                | "$selfillum"
                | "$seamless_scale"
                | "$frame"
                | "$model"
                | "$nocull"
                | "$surfaceprop"
                | "$surfaceprop2"
                | "$color"
                | "$color2"
                | "$halflambert"
                | "$bumpmap"
                | "$envmap"
                | "$envmapmask"
                | "$envmaptint"
                | "$basealphaenvmapmask"
                | "$normalmapalphaenvmapmask"
                | "$nodecal"
                | "$nofog"
                | "%keywords"
                | "%tooltexture"
        ) {
            return Err(format!(
                "unsupported alpha-test texture-shadow parameter {key}"
            ));
        }
    }
    if scalar(material, "$frame", 0.0)? != 0.0 {
        return Err("nonzero $frame is unsupported for texture shadows".into());
    }
    let default_reference = if material.shader == "unlitgeneric" {
        0.7
    } else {
        0.5
    };
    let reference = scalar(material, "$alphatestreference", default_reference)?;
    if !(0.0..=1.0).contains(&reference) {
        return Err("$alphatestreference must be between zero and one".into());
    }
    let reference = if reference == 0.0 { 0.5 } else { reference };
    let transform = transform(material.get("$basetexturetransform"))?;
    let path = material
        .get("$basetexture")
        .filter(|value| !value.trim().is_empty())
        .ok_or("alpha-texture shadows require $basetexture")?;
    let path = vmt::texture_path(path);
    let texture = match assets.textures.get(&path) {
        Some(texture) => Arc::clone(texture),
        None => {
            if brush {
                assets.allocations.push(budget.reserve_bytes(
                    path.len() + 16 * size_of::<(String, Arc<Texture>)>() + 16 * size_of::<usize>(),
                )?);
            }
            let bytes = assets.read(&path, budget)?;
            let vtf = Vtf::parse(bytes).map_err(|error| format!("{path}: {error}"))?;
            if vtf.frames != 1 || vtf.faces != 1 || vtf.depth != 1 {
                return Err(format!(
                    "{path}: animated, cubemap and volume texture shadows are unsupported"
                ));
            }
            if vtf.width > MAX_TEXTURE_SIZE || vtf.height > MAX_TEXTURE_SIZE {
                return Err(format!(
                    "{path}: texture shadows exceed the {MAX_TEXTURE_SIZE} pixel dimension limit"
                ));
            }
            let pixels = vtf.width as usize * vtf.height as usize;
            if pixels > MAX_ALPHA_BYTES - assets.alpha_bytes {
                return Err(
                    "Static-prop texture shadows exceed the 64 MiB decoded alpha limit".into(),
                );
            }
            let (alpha_allocation, _rgba_allocation) = texture_allocations(pixels, budget)?;
            budget.consume(pixels * 2)?;
            let image = vtf
                .decode_rgba8(0, 0, 0)
                .map_err(|error| format!("{path}: {error}"))?;
            if image.pixels.len() != pixels * 4 {
                return Err(format!(
                    "{path}: decoded texture-shadow image has invalid dimensions"
                ));
            }
            let texture = Arc::new(Texture {
                dimensions: [vtf.width as usize, vtf.height as usize],
                clamp: [vtf.flags & CLAMP_S != 0, vtf.flags & CLAMP_T != 0],
                alpha: image
                    .pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|pixel| pixel[3])
                    .collect(),
            });
            assets.allocations.push(alpha_allocation);
            assets.alpha_bytes += pixels;
            assets.textures.insert(path, Arc::clone(&texture));
            texture
        }
    };
    let material_allocation =
        budget.reserve_bytes(size_of::<Material>() + 2 * size_of::<usize>())?;
    assets.allocations.push(material_allocation);
    Ok(Some(Arc::new(Material {
        texture,
        transform,
        reference,
        fractional,
    })))
}

#[cfg(test)]
pub(in crate::lighting) fn test_cutout(alpha: [u8; 4]) -> Arc<Material> {
    Arc::new(Material {
        texture: Arc::new(Texture {
            dimensions: [2, 2],
            clamp: [false; 2],
            alpha: alpha.into(),
        }),
        transform: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        reference: 0.5,
        fractional: false,
    })
}

#[cfg(test)]
pub(in crate::lighting) fn fractional_triangle(
    points: [glam::DVec3; 3],
    alpha: u8,
    owner: Option<u32>,
) -> super::PropTriangle {
    let mut triangle = super::PropTriangle::new(points).unwrap();
    triangle.owner = owner;
    triangle.alpha = Some((
        Arc::new(Material {
            texture: Arc::new(Texture {
                dimensions: [1, 1],
                clamp: [false; 2],
                alpha: vec![alpha],
            }),
            transform: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            reference: 0.5,
            fractional: true,
        }),
        [DVec2::ZERO; 3],
    ));
    triangle
}

#[cfg(test)]
mod tests {
    use glam::DVec3;

    use super::*;
    use crate::lighting::{
        props::PropTriangle,
        shadows::{ShadowIndex, ShadowObject},
    };

    fn triangle(clamp: [bool; 2], transform: [f64; 4], reference: f64) -> PropTriangle {
        let mut triangle =
            PropTriangle::new([DVec3::ZERO, DVec3::X * 8.0, DVec3::Y * 8.0]).unwrap();
        triangle.alpha = Some((
            Arc::new(Material {
                texture: Arc::new(Texture {
                    dimensions: [2, 2],
                    clamp,
                    alpha: vec![0, 255, 128, 64],
                }),
                transform: [
                    [transform[0], 0.0, transform[2]],
                    [0.0, transform[1], transform[3]],
                ],
                reference,
                fractional: false,
            }),
            [DVec2::ZERO, DVec2::X * 2.0, DVec2::Y * 2.0],
        ));
        triangle
    }

    #[test]
    fn alpha_and_decode_admission_precede_allocation_and_release_on_failure() {
        let control = crate::Control::default();
        let budget = Budget::new(&control);
        let pixels = 64usize;
        let (alpha, rgba) = texture_allocations(pixels, &budget).unwrap();
        let total = budget.live_bytes();
        assert!(total >= pixels * 5);
        drop(rgba);
        assert_eq!(
            budget.live_bytes(),
            pixels + size_of::<Texture>() + 2 * size_of::<usize>()
        );
        drop(alpha);
        assert_eq!(budget.live_bytes(), 0);
        let held = budget
            .reserve_bytes(crate::lighting::bake::MAX_LIGHTING_LIVE_BYTES - total + 1)
            .unwrap();
        let before = budget.live_bytes();
        assert!(texture_allocations(pixels, &budget).is_err());
        assert_eq!(budget.live_bytes(), before);
        drop(held);
        assert!(texture_allocations(usize::MAX, &budget).is_err());
        assert_eq!(budget.live_bytes(), 0);
    }

    #[test]
    fn cutout_rays_interpolate_uvs_repeat_clamp_transform_and_apply_the_reference() {
        assert!(
            transform(Some("scale 2 3 scale 4 5"))
                .unwrap_err()
                .contains("duplicate")
        );
        assert!(
            transform(Some("scale 1024.00001 1"))
                .unwrap_err()
                .contains("1024")
        );
        assert_eq!(
            transform(Some("center 0 0 scale -1 1 translate 1 0 rotate 0")).unwrap(),
            [[-1.0, 0.0, 1.0], [0.0, 1.0, 0.0]]
        );

        let identity = [1.0, 1.0, 0.0, 0.0];
        for (clamp, mapping, reference, point, expected) in [
            ([false; 2], identity, 0.5, [1.0, 1.0], false),
            ([false; 2], identity, 0.5, [3.0, 1.0], true),
            ([false; 2], identity, 128.0 / 255.0, [1.0, 3.0], true),
            ([false; 2], identity, 0.75, [1.0, 3.0], false),
            ([false; 2], identity, 0.5, [5.0, 1.0], false),
            ([true, false], identity, 0.5, [5.0, 1.0], true),
            ([false; 2], identity, 0.5, [1.0, 5.0], false),
            ([false, true], identity, 0.5, [1.0, 5.0], true),
            ([false; 2], [1.0, 1.0, -0.5, 0.0], 0.5, [1.0, 1.0], true),
            ([true, false], [1.0, 1.0, -0.5, 0.0], 0.5, [1.0, 1.0], false),
            ([false; 2], [-1.0, 1.0, 1.0, 0.0], 0.5, [1.0, 1.0], true),
        ] {
            let triangle = triangle(clamp, mapping, reference);
            let point = DVec3::new(point[0], point[1], 0.0);
            for (start, end) in [
                (point - DVec3::Z, point + DVec3::Z),
                (point + DVec3::Z, point - DVec3::Z),
            ] {
                assert_eq!(triangle.intersects(start, end).unwrap(), expected);
            }
            assert!(!triangle.intersects(point, point + DVec3::Z).unwrap());
            assert!(!triangle.intersects(point - DVec3::Z, point).unwrap());
        }
    }

    #[test]
    fn rotated_uv_rays_keep_literal_cutout_and_fractional_texels() {
        for (value, clamp, uv, alpha) in [
            ("rotate 90", [false; 2], [0.125, 0.125], 255),
            ("rotate -90", [false; 2], [0.125, 0.125], 128),
            ("rotate 30", [false; 2], [0.625, 0.125], 255),
            ("rotate -30", [false; 2], [0.625, 0.125], 0),
            (
                "center 0.25 0.75 scale 2 3 rotate 90 translate 0.5 -0.25",
                [false; 2],
                [0.125, 0.625],
                0,
            ),
            (
                "center 0.25 0.75 scale 2 3 rotate 90 translate 0.5 -0.25",
                [true, false],
                [0.125, 0.625],
                255,
            ),
            (
                "center 0 0 scale -2 3 rotate 90 translate 0.25 0.5",
                [false; 2],
                [0.125, 0.125],
                255,
            ),
            (
                "center 0 0 scale -2 3 rotate 90 translate 0.25 0.5",
                [true, false],
                [0.125, 0.125],
                0,
            ),
        ] {
            for fractional in [false, true] {
                let mut triangle = triangle(clamp, [1.0, 1.0, 0.0, 0.0], 0.5);
                let material = Arc::get_mut(&mut triangle.alpha.as_mut().unwrap().0).unwrap();
                material.transform = transform(Some(value)).unwrap();
                material.fractional = fractional;
                let expected = if fractional {
                    alpha
                } else if alpha >= 128 {
                    255
                } else {
                    0
                };
                let point = DVec3::new(uv[0] * 4.0, uv[1] * 4.0, 0.0);
                for (start, end) in [
                    (point - DVec3::Z, point + DVec3::Z),
                    (point + DVec3::Z, point - DVec3::Z),
                ] {
                    assert_eq!(triangle.coverage(start, end).unwrap(), expected, "{value}");
                }
            }
        }
    }

    #[test]
    fn transparent_bvh_hits_continue_to_opaque_geometry_and_remain_cancellable() {
        let control = crate::Control::default();
        let mut budget = Budget::new(&control);
        let cutout = triangle([false; 2], [1.0, 1.0, 0.0, 0.0], 0.5);
        let blocker = PropTriangle::new([
            DVec3::new(0.0, 0.0, 0.5),
            DVec3::new(8.0, 0.0, 0.5),
            DVec3::new(0.0, 8.0, 0.5),
        ])
        .unwrap();
        let index =
            ShadowIndex::new(vec![cutout, blocker], &mut |work| budget.consume(work)).unwrap();
        let point = DVec3::new(1.0, 1.0, 0.0);
        assert!(
            index
                .blocked(point - DVec3::Z, point + DVec3::Z, &mut |work| budget
                    .consume(work))
                .unwrap()
        );
        assert!(
            !index
                .blocked(point - DVec3::Z, point + DVec3::Z * 0.25, &mut |work| {
                    budget.consume(work)
                })
                .unwrap()
        );
        control.cancel().unwrap();
        assert!(
            index
                .blocked(point - DVec3::Z, point + DVec3::Z, &mut |work| budget
                    .consume(work))
                .is_err()
        );
    }

    #[test]
    fn fractional_coverage_is_additive_saturated_owner_aware_and_counts_each_primitive() {
        let points = |height| {
            [
                DVec3::new(-8.0, -8.0, height),
                DVec3::new(8.0, -8.0, height),
                DVec3::new(0.0, 8.0, height),
            ]
        };
        for (alphas, expected) in [
            (vec![0], 255),
            (vec![64], 191),
            (vec![128], 127),
            (vec![255], 0),
            (vec![64, 128], 63),
            (vec![128, 64], 63),
            (vec![128, 128], 0),
        ] {
            let index = ShadowIndex::new(
                alphas
                    .into_iter()
                    .enumerate()
                    .map(|(index, alpha)| fractional_triangle(points(index as f64), alpha, Some(7)))
                    .collect(),
                &mut |_| Ok(()),
            )
            .unwrap();
            let received = index
                .visibility_skipping_owner(-DVec3::Z, DVec3::Z * 4.0, None, &mut |_| Ok(()))
                .unwrap();
            assert!((received * 255.0 - f64::from(expected)).abs() < 1e-12);
            assert_eq!(
                index
                    .visibility_skipping_owner(-DVec3::Z, DVec3::Z * 4.0, Some(7), &mut |_| Ok(()))
                    .unwrap(),
                1.0
            );
        }
        for owner in [None, Some(9)] {
            let index = ShadowIndex::new(
                vec![
                    fractional_triangle(points(0.0), 64, Some(7)),
                    fractional_triangle(points(0.0), 128, owner),
                ],
                &mut |_| Ok(()),
            )
            .unwrap();
            assert!(
                (index
                    .visibility_skipping_owner(-DVec3::Z, DVec3::Z, Some(7), &mut |_| Ok(()))
                    .unwrap()
                    * 255.0
                    - 127.0)
                    .abs()
                    < 1e-12
            );
        }
        let shared = [
            [
                DVec3::new(-1.0, -1.0, 0.0),
                DVec3::new(1.0, -1.0, 0.0),
                DVec3::new(1.0, 1.0, 0.0),
            ],
            [
                DVec3::new(-1.0, -1.0, 0.0),
                DVec3::new(1.0, 1.0, 0.0),
                DVec3::new(-1.0, 1.0, 0.0),
            ],
        ];
        let index = ShadowIndex::new(
            shared
                .map(|points| fractional_triangle(points, 64, None))
                .into(),
            &mut |_| Ok(()),
        )
        .unwrap();
        assert!(
            (index
                .visibility_skipping_owner(-DVec3::Z, DVec3::Z, None, &mut |_| Ok(()))
                .unwrap()
                * 255.0
                - 127.0)
                .abs()
                < 1e-12
        );
        assert_eq!(
            index
                .visibility_skipping_owner(DVec3::ZERO, DVec3::Z, None, &mut |_| Ok(()))
                .unwrap(),
            1.0
        );
        assert_eq!(
            index
                .visibility_skipping_owner(-DVec3::Z, DVec3::ZERO, None, &mut |_| Ok(()))
                .unwrap(),
            1.0
        );
        for reverse in [false, true] {
            let mut triangles = vec![
                fractional_triangle(points(0.0), 64, None),
                PropTriangle::new(points(0.0)).unwrap(),
            ];
            if reverse {
                triangles.reverse();
            }
            let index = ShadowIndex::new(triangles, &mut |_| Ok(())).unwrap();
            assert_eq!(
                index
                    .visibility_skipping_owner(-DVec3::Z, DVec3::Z, None, &mut |_| Ok(()))
                    .unwrap(),
                0.0
            );
        }
        let index = ShadowIndex::new(
            vec![
                fractional_triangle(points(0.0), 64, None),
                fractional_triangle(points(0.5), 128, None),
            ],
            &mut |_| Ok(()),
        )
        .unwrap();
        for cancel in [false, true] {
            let control = crate::Control::default();
            let mut budget = Budget::with_limit(&control, 100_000_000);
            if !cancel {
                budget.consume(100_000_000 - 2).unwrap();
            }
            let mut calls = 0;
            assert!(
                index
                    .visibility_skipping_owner(-DVec3::Z, DVec3::Z, None, &mut |work| {
                        calls += 1;
                        if cancel && calls == 3 {
                            control.cancel().unwrap();
                        }
                        budget.consume(work)
                    })
                    .is_err()
            );
        }
    }

    #[test]
    fn fractional_uv_sampling_keeps_raw_bytes_and_legacy_visibility_keeps_work() {
        let mut fractional = triangle([false; 2], [1.0, 1.0, 0.0, 0.0], 0.75);
        Arc::get_mut(&mut fractional.alpha.as_mut().unwrap().0)
            .unwrap()
            .fractional = true;
        for (point, expected) in [
            (DVec3::new(1.0, 3.0, 0.0), 128),
            (DVec3::new(3.0, 3.0, 0.0), 64),
            (DVec3::new(5.0, 1.0, 0.0), 0),
        ] {
            assert_eq!(
                fractional
                    .coverage(point - DVec3::Z, point + DVec3::Z)
                    .unwrap(),
                expected
            );
        }
        for triangles in [
            vec![triangle([false; 2], [1.0, 1.0, 0.0, 0.0], 0.5)],
            vec![PropTriangle::new([DVec3::ZERO, DVec3::X * 8.0, DVec3::Y * 8.0]).unwrap()],
        ] {
            let index = ShadowIndex::new(triangles, &mut |_| Ok(())).unwrap();
            assert!(!index.has_fractional());
            let start = DVec3::new(1.0, 1.0, -1.0);
            let end = start + DVec3::Z * 2.0;
            for owner in [None, Some(7)] {
                let mut before = 0;
                let blocked = index
                    .blocked_skipping_owner(start, end, owner, &mut |work| {
                        before += work;
                        Ok(())
                    })
                    .unwrap();
                let mut after = 0;
                let visibility = index
                    .visibility_skipping_owner(start, end, owner, &mut |work| {
                        after += work;
                        Ok(())
                    })
                    .unwrap();
                assert_eq!(visibility, if blocked { 0.0 } else { 1.0 });
                assert_eq!(before, after);
            }
        }
    }
}
