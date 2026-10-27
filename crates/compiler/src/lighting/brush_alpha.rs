use std::{
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    sync::Arc,
};

use glam::{DVec2, DVec3};
use source_formats::{bsp::contents, gamefs::GameFs, vmt};

use super::{
    bake::{Allocation, Budget},
    props::{Assets, alpha},
};
use crate::{asset_sources::BuildMaterialSources, map::Map};

#[derive(Debug, Clone)]
pub(super) struct Face {
    material: Arc<alpha::Material>,
    projection: [[f32; 4]; 2],
}

impl Face {
    pub(super) fn covered(&self, point: DVec3) -> Result<bool, String> {
        let dimensions = self.material.dimensions();
        let coordinate = |axis: usize| {
            let row = self.projection[axis];
            (DVec3::new(f64::from(row[0]), f64::from(row[1]), f64::from(row[2])).dot(point)
                + f64::from(row[3]))
                / dimensions[axis] as f64
        };
        self.material
            .blocks(DVec2::new(coordinate(0), coordinate(1)))
            .map_err(|error| format!("Brush texture shadows: {error}"))
    }
}

pub(super) struct Prepared {
    materials: BTreeMap<usize, Option<Arc<alpha::Material>>>,
    model_alpha: bool,
    global_alpha: BTreeSet<usize>,
    _allocations: Vec<Allocation>,
}

impl Prepared {
    pub(super) fn contains(&self, material: usize) -> bool {
        self.materials.get(&material).is_some_and(Option::is_some)
    }

    pub(super) fn has_model_alpha(&self) -> bool {
        self.model_alpha
    }

    pub(super) fn has_global_alpha(
        &self,
        entity: usize,
        budget: &mut Budget<'_>,
    ) -> Result<bool, String> {
        if self.global_alpha.is_empty() {
            return Ok(false);
        }
        budget.consume(self.global_alpha.len().checked_ilog2().unwrap_or(0) as usize + 1)?;
        Ok(self.global_alpha.contains(&entity))
    }

    pub(super) fn face(&self, material: usize, projection: [[f32; 4]; 2]) -> Option<Face> {
        self.materials
            .get(&material)
            .and_then(Option::as_ref)
            .map(|material| Face {
                material: Arc::clone(material),
                projection,
            })
    }

    pub(super) fn qualify_models(
        &self,
        map: &Map<'_>,
        scopes: &[super::models::Scope],
        receivers: &[bool],
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        if !self.model_alpha {
            return Ok(());
        }
        if scopes.len() != receivers.len() {
            return Err("Brush texture shadow model scopes do not match receivers".into());
        }
        let _allocation = budget.reserve::<bool>(map.entities.len())?;
        budget.consume(map.entities.len())?;
        let mut admitted = vec![false; map.entities.len()];
        for (scope, &receiver) in scopes.iter().zip(receivers) {
            budget.consume(1)?;
            admitted[scope.entity] = receiver
                || crate::brush_shadow_casters::enabled(&map.entities[scope.entity].properties);
        }
        for brush in map.brushes.iter().filter(|brush| brush.entity != 0) {
            for side in brush
                .sides
                .iter()
                .filter(|side| !side.bevel && side.visible)
            {
                budget.consume(self.materials.len().checked_ilog2().unwrap_or(0) as usize + 1)?;
                if self.contains(side.material)
                    && (!admitted[brush.entity]
                        || brush.contents & (contents::SOLID | contents::WINDOW | contents::GRATE)
                            == 0)
                {
                    return Err(format!(
                        "Brush texture shadows entity:{} solid:{} material {} has no applicable baked self-shadow or requested global scope",
                        map.entities[brush.entity].id,
                        brush.id,
                        map.materials.get(side.material).name
                    ));
                }
            }
        }
        Ok(())
    }
}

fn requested(material: &vmt::Material) -> Result<bool, String> {
    if !material.blocks.is_empty() {
        return Err(
            "Brush texture shadows cannot classify conditional or animated material blocks".into(),
        );
    }
    let mut requested = false;
    for (key, neutral) in [
        ("$alphatest", 0.0),
        ("$translucent", 0.0),
        ("$additive", 0.0),
        ("$vertexalpha", 0.0),
        ("$distancealpha", 0.0),
        ("$alpha", 1.0),
    ] {
        if let Some(value) = material.get(key) {
            let value = value
                .trim()
                .parse::<f64>()
                .map_err(|_| format!("Brush texture shadows cannot classify invalid {key}"))?;
            if !value.is_finite() {
                return Err(format!(
                    "Brush texture shadows cannot classify nonfinite {key}"
                ));
            }
            requested |= value != neutral;
        }
    }
    Ok(requested)
}

pub(super) fn prepare(
    map: &Map<'_>,
    fs: &GameFs,
    sources: &BuildMaterialSources,
    budget: &mut Budget<'_>,
) -> Result<Prepared, String> {
    budget.consume(0)?;
    if !map.materials.uses_build_sources(sources) {
        return Err("Brush texture shadows require the original prepared material owner".into());
    }
    let sides = map
        .brushes
        .iter()
        .try_fold(0usize, |count, brush| {
            count.checked_add(brush.sides.iter().filter(|side| !side.bevel).count())
        })
        .ok_or("Brush texture shadows boundary count overflow")?;
    if sides > crate::MAX_BRUSH_SHADOW_SIDES {
        return Err("Brush texture shadows exceeds 65,536 boundary sides".into());
    }
    let _read_metadata = budget
        .reserve::<Allocation>(2 * (crate::MAX_BRUSH_SHADOW_MATERIAL_LOOKUPS + 5 * sides + 2))?;
    let hull_bytes = map
        .brushes
        .len()
        .checked_mul(size_of::<super::OpticalHull>())
        .ok_or("Brush texture shadows hull allocation overflow")?;
    let allocation = budget.reserve_bytes(
        sides
            .checked_mul(
                size_of::<Option<Face>>()
                    + 4 * size_of::<(usize, Option<Arc<alpha::Material>>)>()
                    + 16 * size_of::<usize>(),
            )
            .and_then(|bytes| bytes.checked_add(hull_bytes))
            .ok_or("Brush texture shadows boundary allocation overflow")?,
    )?;
    budget.consume(sides)?;
    let mut materials = BTreeMap::new();
    let mut model_alpha = false;
    let mut global_alpha = BTreeSet::new();
    let mut global_allocations = Vec::new();
    let mut assets = Assets::for_brushes(fs, sources);
    for brush in &map.brushes {
        let mut brush_alpha = false;
        for side in brush
            .sides
            .iter()
            .filter(|side| !side.bevel && side.visible)
        {
            let info = map.materials.get(side.material);
            budget.consume(materials.len().checked_ilog2().unwrap_or(0) as usize + 1)?;
            if let Entry::Vacant(entry) = materials.entry(side.material) {
                let _parsed_material = budget.reserve_bytes(
                    crate::MAX_BRUSH_SHADOW_MATERIAL_BYTES * 128 * (vmt::MAX_PATCH_DEPTH + 1),
                )?;
                let material = assets
                    .brush_material(&info.name, budget)
                    .map_err(|error| format!("solid:{}: {error}", brush.id))?;
                budget.consume(material.params.len() + 1)?;
                let alpha = if requested(&material).map_err(|error| {
                    format!("solid:{} material {}: {error}", brush.id, info.name)
                })? {
                    Some(
                        alpha::prepare_brush(&material, &mut assets, budget).map_err(|error| {
                            format!(
                                "Brush texture shadows solid:{} material {}: {error}",
                                brush.id, info.name
                            )
                        })?,
                    )
                } else {
                    None
                };
                entry.insert(alpha);
            }
            let model_alpha_side = brush.entity != 0 && materials[&side.material].is_some();
            model_alpha |= model_alpha_side;
            brush_alpha |= model_alpha_side;
        }
        if brush_alpha {
            let properties = &map.entities[brush.entity].properties;
            budget.consume(properties.len())?;
            if crate::brush_shadow_casters::enabled(properties) {
                budget.consume(global_alpha.len().checked_ilog2().unwrap_or(0) as usize + 1)?;
                if !global_alpha.contains(&brush.entity) {
                    global_allocations.push(budget.reserve_bytes(16 * size_of::<usize>())?);
                    global_alpha.insert(brush.entity);
                }
            }
        }
    }
    let mut allocations = assets.into_allocations();
    allocations.extend(global_allocations);
    allocations.push(allocation);
    allocations.push(_read_metadata);
    Ok(Prepared {
        materials,
        model_alpha,
        global_alpha,
        _allocations: allocations,
    })
}

#[cfg(test)]
pub(super) fn test_face(alpha: [u8; 4]) -> Face {
    Face {
        material: alpha::test_cutout(alpha),
        projection: [[0.0, 2.0, 0.0, 0.0], [0.0, 0.0, 2.0, 0.0]],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cutout_preparation_replays_original_patch_sources_and_keeps_publication_guard() {
        use crate::{Control, Report};
        use source_formats::gamefs::{Mount, MountedPath};

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/validation/931-brush-material-replay");
        std::fs::create_dir_all(&root).unwrap();
        let directory = tempfile::tempdir_in(root).unwrap();
        let materials = directory.path().join("materials/dev");
        std::fs::create_dir_all(&materials).unwrap();
        let patch = "Patch { include materials/dev/base.vmt replace { $alphatest 1 } }";
        let base = "LightmappedGeneric { $basetexture dev/mask $alphatest 0 }";
        let patch_path = materials.join("dev_measuregeneric01.vmt");
        let base_path = materials.join("base.vmt");
        std::fs::write(&patch_path, patch).unwrap();
        std::fs::write(&base_path, base).unwrap();
        let mut texture = vec![0; 80];
        texture[..4].copy_from_slice(b"VTF\0");
        for (offset, value) in [(4, 7_u32), (8, 2), (12, 80)] {
            texture[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        for offset in [16, 18] {
            texture[offset..offset + 2].copy_from_slice(&2_u16.to_le_bytes());
        }
        for offset in [24, 63] {
            texture[offset..offset + 2].copy_from_slice(&1_u16.to_le_bytes());
        }
        texture[20..24].copy_from_slice(&source_formats::vtf::FLAG_NOMIP.to_le_bytes());
        texture[56] = 1;
        texture[57..61].copy_from_slice(&(-1_i32).to_le_bytes());
        for alpha in [0, 0, 255, 255] {
            texture.extend_from_slice(&[255, 255, 255, alpha]);
        }
        std::fs::write(materials.join("mask.vtf"), texture).unwrap();
        let mut game = crate::test_common::empty_game();
        game.mounts.push(MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(directory.path().to_owned()),
        });
        let vmf = crate::test_common::room();
        let control = Control::default();
        let sources = BuildMaterialSources::default();
        let map = Map::load_with_material_sources_controlled(
            &vmf,
            &game,
            &mut Report::default(),
            &control,
            None,
            &sources,
        )
        .unwrap();
        let missing = BuildMaterialSources::without_retained_payloads();
        let unretained = Map::load_with_material_sources_controlled(
            &vmf,
            &game,
            &mut Report::default(),
            &control,
            None,
            &missing,
        )
        .unwrap();
        let mut budget = Budget::new(&control);
        let error = prepare(&unretained, &game, &missing, &mut budget)
            .err()
            .unwrap();
        assert!(error.contains("original material dependency was not retained"));
        assert_eq!(budget.live_bytes(), 0);
        std::fs::write(&patch_path, "Patch { include materials/dev/missing.vmt }").unwrap();
        std::fs::write(&base_path, "LightmappedGeneric { $alphatest 0 }").unwrap();
        let prepared = prepare(&map, &game, &sources, &mut budget).unwrap();
        assert_eq!(prepared.materials.len(), 1);
        let material = prepared
            .materials
            .values()
            .next()
            .unwrap()
            .as_ref()
            .unwrap();
        assert!(!material.blocks(DVec2::new(0.25, 0.25)).unwrap());
        assert!(material.blocks(DVec2::new(0.25, 0.75)).unwrap());
        assert!(sources.source_guard(&game, &control).is_err());
        assert_eq!(
            sources
                .retained("materials/dev/dev_measuregeneric01.vmt")
                .unwrap()
                .unwrap()
                .bytes(),
            patch.as_bytes()
        );
        drop(prepared);
        assert_eq!(budget.live_bytes(), 0);
    }

    #[test]
    fn preparation_requires_the_original_material_capture_before_reading_or_allocating() {
        use crate::{Control, Report};
        use source_formats::gamefs::{Mount, MountedPath};

        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/validation/901");
        std::fs::create_dir_all(&root).unwrap();
        let directory = tempfile::tempdir_in(root).unwrap();
        let materials = directory.path().join("materials/dev");
        std::fs::create_dir_all(&materials).unwrap();
        std::fs::write(
            materials.join("dev_measuregeneric01.vmt"),
            "LightmappedGeneric { }",
        )
        .unwrap();
        let game = GameFs {
            game_dir: directory.path().to_owned(),
            base_dir: directory.path().to_owned(),
            mounts: vec![MountedPath {
                ids: vec!["game".into()],
                mount: Mount::Directory(directory.path().to_owned()),
            }],
        };
        let vmf = crate::test_common::room();
        let control = Control::default();
        let sources = BuildMaterialSources::default();
        let map = Map::load_with_material_sources_controlled(
            &vmf,
            &game,
            &mut Report::default(),
            &control,
            None,
            &sources,
        )
        .unwrap();
        let mut budget = Budget::new(&control);
        let prepared = prepare(&map, &game, &sources.clone(), &mut budget).unwrap();
        assert_eq!(prepared.materials.len(), 1);
        assert!(prepared.materials.values().all(Option::is_none));
        drop(prepared);
        assert_eq!(budget.live_bytes(), 0);

        let foreign = BuildMaterialSources::default();
        let path = "materials/dev/dev_measuregeneric01.vmt";
        let same_bytes = foreign.read_asset(&game, path, 4096).unwrap().unwrap();
        let original = sources.retained(path).unwrap().unwrap();
        assert_eq!(same_bytes.bytes(), original.bytes());
        assert_eq!(same_bytes.origin(), original.origin());
        let work = budget.work();
        let error = prepare(&map, &game, &foreign, &mut budget).err().unwrap();
        assert!(error.contains("original prepared material owner"));
        assert_eq!(budget.work(), work);
        assert_eq!(budget.live_bytes(), 0);

        let untracked = Map::load(&vmf, &game, &mut Report::default()).unwrap();
        let error = prepare(&untracked, &game, &sources, &mut budget)
            .err()
            .unwrap();
        assert!(error.contains("original prepared material owner"));
        assert_eq!(budget.work(), work);
        assert_eq!(budget.live_bytes(), 0);

        control.cancel().unwrap();
        assert_eq!(
            prepare(&map, &game, &foreign, &mut budget).err().unwrap(),
            "Compilation cancelled"
        );
        assert_eq!(budget.work(), work);
        assert_eq!(budget.live_bytes(), 0);
    }

    #[test]
    fn model_alpha_requires_an_actual_receiver_scope_and_existing_shadow_contents() {
        use super::super::models::Scope;
        use crate::{Control, Report, brush_model_pose::Pose, map::MapEntity};

        let game = crate::test_common::empty_game();
        let mut map =
            Map::load(&crate::test_common::room(), &game, &mut Report::default()).unwrap();
        let entity = map.entities.len();
        map.entities.push(MapEntity {
            id: 777,
            classname: "func_brush".into(),
            properties: Vec::new(),
            brushes: vec![0],
            origin: None,
        });
        map.entities[0].brushes.retain(|&brush| brush != 0);
        map.brushes[0].entity = entity;
        let material = map.brushes[0]
            .sides
            .iter()
            .find(|side| side.visible && !side.bevel)
            .unwrap()
            .material;
        let mut prepared = Prepared {
            materials: BTreeMap::from([(material, Some(alpha::test_cutout([0; 4])))]),
            model_alpha: true,
            global_alpha: BTreeSet::new(),
            _allocations: Vec::new(),
        };
        let scope = Scope {
            entity,
            pose: Pose {
                offset: DVec3::ZERO,
                rotation: None,
            },
            faces: 0..1,
        };
        let control = Control::default();
        let mut budget = Budget::new(&control);
        let error = prepared
            .qualify_models(&map, &[], &[], &mut budget)
            .unwrap_err();
        assert!(
            error.contains("entity:777")
                && error.contains("no applicable baked self-shadow or requested global scope")
        );
        assert_eq!(budget.live_bytes(), 0);
        assert!(
            prepared
                .qualify_models(&map, std::slice::from_ref(&scope), &[false], &mut budget)
                .is_err()
        );
        prepared
            .qualify_models(&map, std::slice::from_ref(&scope), &[true], &mut budget)
            .unwrap();
        assert_eq!(budget.live_bytes(), 0);
        map.entities[entity]
            .properties
            .push(("vrad_brush_cast_shadows".into(), "1".into()));
        prepared
            .qualify_models(&map, std::slice::from_ref(&scope), &[false], &mut budget)
            .unwrap();
        map.entities[entity].properties.clear();
        map.brushes[0].contents = contents::WATER;
        assert!(
            prepared
                .qualify_models(&map, &[scope], &[true], &mut budget)
                .is_err()
        );
        prepared.model_alpha = false;
        let work = budget.work();
        prepared
            .qualify_models(&map, &[], &[], &mut budget)
            .unwrap();
        assert_eq!(budget.work(), work);
    }

    #[test]
    fn classification_uses_numeric_neutral_values_and_refuses_unknown_opacity() {
        let opaque = vmt::Material::parse(
            "LightmappedGeneric { $alpha 1.0 $alphatest 0e0 $translucent -0.0 }",
        )
        .unwrap();
        assert!(!requested(&opaque).unwrap());
        for value in ["0", "0.5", "2"] {
            let material =
                vmt::Material::parse(&format!("LightmappedGeneric {{ $alpha {value} }}")).unwrap();
            assert!(requested(&material).unwrap());
        }
        for text in [
            "LightmappedGeneric { $alpha NaN }",
            "LightmappedGeneric { $alphatest invalid }",
            "LightmappedGeneric { Proxies { Add { resultVar $alpha } } }",
        ] {
            assert!(requested(&vmt::Material::parse(text).unwrap()).is_err());
        }
    }

    #[test]
    fn diffuse_projection_uses_texture_dimensions_before_alpha_sampling() {
        let mut face = test_face([0, 127, 128, 255]);
        for (point, covered) in [
            (DVec3::new(0.0, 0.25, 0.25), false),
            (DVec3::new(0.0, 0.75, 0.25), false),
            (DVec3::new(0.0, 0.25, 0.75), true),
            (DVec3::new(0.0, 0.75, 0.75), true),
            (DVec3::new(100.0, -0.25, 1.75), true),
        ] {
            assert_eq!(face.covered(point).unwrap(), covered);
        }
        face.projection[1][3] = 1.0;
        assert!(face.covered(DVec3::new(0.0, 0.25, 0.25)).unwrap());
        face.projection[0][0] = f32::NAN;
        assert!(face.covered(DVec3::ZERO).is_err());
    }
}
