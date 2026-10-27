mod common;
#[path = "common/overlays.rs"]
mod overlay_fixture;

use glam::DVec3;
use map_core::{Document, ObjectId, Plane, compile_region, edit};
use source_formats::bsp::{self, Bsp, Face, Leaf, LeafWaterData, TexInfo, lump};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::Vmf;

const TOP: &str = "water/top";
const BOTTOM: &str = "WATER/BELOW";
const SIDE: &str = "water/side";
const SIDE_BOTTOM: &str = "WATER/SIDE_BELOW";
const SIDE_TEXT: &str = "Water { $abovewater 1 $bottommaterial water/side_below }";

fn game() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    let materials = directory.path().join("materials/water");
    std::fs::create_dir_all(&materials).unwrap();
    for (name, text) in [
        (
            "top",
            "Water { $abovewater 1 $bottommaterial water/below $underwateroverlay effects/water_warp01 }",
        ),
        (
            "top_patch",
            "Patch { include materials/water/top.vmt replace { $bottommaterial Materials\\water\\below.vmt } }",
        ),
        (
            "below",
            "Patch { include materials/water/below_base.vmt replace { $abovewater 0 } }",
        ),
        ("below_base", "Water { $abovewater 1 }"),
        ("side", SIDE_TEXT),
        ("side_below", "Water { $abovewater 0 }"),
    ] {
        std::fs::write(materials.join(format!("{name}.vmt")), text).unwrap();
    }
    std::fs::write(
        directory.path().join("materials/nodraw.vmt"),
        "UnlitGeneric { %compilenodraw 1 }",
    )
    .unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().into()),
    });
    (directory, game)
}

fn add_pool(vmf: Vmf, mins: [f64; 3], maxs: [f64; 3], material: &str) -> Vmf {
    let mut document = Document::new(vmf);
    document
        .edit("water surface", |tx| {
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::from_array(mins),
                DVec3::from_array(maxs),
                "nodraw",
            )?
            else {
                unreachable!()
            };
            for side in &mut tx.solid_mut(id)?.sides {
                if Plane::from_points(side.plane.0).unwrap().normal.z > 0.99 {
                    side.material = material.into();
                    side.u_axis.offset = 17.0;
                    side.v_axis.offset = -13.0;
                    side.u_axis.scale = 0.75;
                    side.v_axis.scale = 0.5;
                }
            }
            Ok(())
        })
        .unwrap();
    document.vmf().clone()
}

fn compiled(vmf: &Vmf, game: &GameFs, physics: compiler::PhysicsMode) -> Bsp {
    let output = compiler::compile_with_options(
        vmf,
        game,
        compiler::Options {
            physics,
            ..Default::default()
        },
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.leak.is_none());
    Bsp::parse(&output.bsp.unwrap().to_bytes()).unwrap()
}

fn paint_pool_side(vmf: &mut Vmf, normal: DVec3) {
    for side in &mut vmf.world.solids.last_mut().unwrap().sides {
        if Plane::from_points(side.plane.0)
            .unwrap()
            .normal
            .abs_diff_eq(normal, 1e-8)
        {
            side.material = SIDE.into();
            side.u_axis.offset = 31.0;
            side.v_axis.offset = -29.0;
            side.u_axis.scale = 0.25;
            side.v_axis.scale = -0.75;
        }
    }
}

fn material_faces(bsp: &Bsp, name: &str) -> Vec<Face> {
    let infos = bsp.records::<TexInfo>().unwrap();
    let data = bsp.records::<bsp::TexData>().unwrap();
    let names = bsp.texdata_strings().unwrap();
    bsp.records::<Face>()
        .unwrap()
        .into_iter()
        .filter(|face| {
            names[data[infos[face.texinfo as usize].texdata as usize].name_index as usize]
                .eq_ignore_ascii_case(name)
        })
        .collect()
}

fn face_points(bsp: &Bsp, face: &Face) -> Vec<DVec3> {
    let vertices = bsp.values::<[f32; 3]>(lump::VERTEXES).unwrap();
    let edges = bsp.records::<bsp::Edge>().unwrap();
    let surfedges = bsp.values::<i32>(lump::SURFEDGES).unwrap();
    surfedges[face.first_edge as usize..face.first_edge as usize + face.edge_count as usize]
        .iter()
        .map(|&edge| {
            let vertex = edges[edge.unsigned_abs() as usize].0[usize::from(edge < 0)];
            DVec3::from_array(vertices[vertex as usize].map(f64::from))
        })
        .collect()
}

fn face_leaves(bsp: &Bsp, face: &Face) -> Vec<Leaf> {
    let faces = bsp.records::<Face>().unwrap();
    let id = faces
        .iter()
        .position(|candidate| candidate == face)
        .unwrap() as u16;
    let references = bsp.values::<u16>(lump::LEAFFACES).unwrap();
    bsp.records::<Leaf>()
        .unwrap()
        .into_iter()
        .filter(|leaf| {
            references[leaf.first_leaf_face as usize
                ..leaf.first_leaf_face as usize + leaf.leaf_face_count as usize]
                .contains(&id)
        })
        .collect()
}

fn horizontal_pair(bsp: &Bsp, outward: &str, inward: &str, z: f64, area: f32) {
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let mut polygons = Vec::new();
    for (material, wet) in [(outward, false), (inward, true)] {
        let faces = material_faces(bsp, material)
            .into_iter()
            .filter(|face| face_points(bsp, face).iter().all(|point| point.z == z))
            .collect::<Vec<_>>();
        assert_eq!(faces.iter().map(|face| face.area).sum::<f32>(), area);
        let mut keys = Vec::new();
        for face in faces {
            let normal = if wet { DVec3::Z } else { DVec3::NEG_Z };
            assert_eq!(
                planes[face.plane as usize].normal,
                normal.as_vec3().to_array()
            );
            assert_eq!(
                planes[face.plane as usize].dist,
                if wet { z as f32 } else { -z as f32 }
            );
            let points = face_points(bsp, &face);
            assert!(
                (points[2] - points[0])
                    .cross(points[1] - points[0])
                    .dot(normal)
                    > 0.0
            );
            assert_eq!(face.light_offset, -1);
            assert!(face.fog_volume >= 0);
            let leaves = face_leaves(bsp, &face);
            assert!(!leaves.is_empty());
            assert!(
                leaves
                    .iter()
                    .all(|leaf| (leaf.contents & bsp::contents::WATER != 0) == wet
                        && (!wet || leaf.water_data == face.fog_volume))
            );
            let mut key = points
                .iter()
                .map(|point| point.to_array().map(f64::to_bits))
                .collect::<Vec<_>>();
            key.sort_unstable();
            keys.push(key);
        }
        keys.sort_unstable();
        polygons.push(keys);
    }
    assert_eq!(polygons[0], polygons[1]);
}

#[test]
fn water_underside_uses_patched_bottom_material_and_keeps_top_projection_flags_and_fog() {
    let (_directory, game) = game();
    for top in [TOP, "water/top_patch"] {
        let vmf = add_pool(
            common::room(),
            [-128.0, -128.0, 0.0],
            [128.0, 128.0, 64.0],
            top,
        );
        let source = vmf.clone();
        let bsp = compiled(&vmf, &game, compiler::PhysicsMode::None);
        let tops = material_faces(&bsp, top);
        let bottoms = material_faces(&bsp, BOTTOM);
        assert_eq!(tops.len(), 1);
        assert_eq!(bottoms.len(), 1);
        let infos = bsp.records::<TexInfo>().unwrap();
        let top_info = infos[tops[0].texinfo as usize];
        let bottom_info = infos[bottoms[0].texinfo as usize];
        assert_eq!(top_info.texture_vecs, bottom_info.texture_vecs);
        assert_eq!(top_info.lightmap_vecs, bottom_info.lightmap_vecs);
        assert_eq!(top_info.flags, bottom_info.flags);
        assert_ne!(top_info.texdata, bottom_info.texdata);
        let planes = bsp.records::<bsp::Plane>().unwrap();
        assert_eq!(planes[tops[0].plane as usize].normal, [0.0, 0.0, 1.0]);
        assert_eq!(planes[bottoms[0].plane as usize].normal, [0.0, 0.0, -1.0]);
        for (face, normal) in [(&tops[0], DVec3::Z), (&bottoms[0], DVec3::NEG_Z)] {
            let points = face_points(&bsp, face);
            assert!(points.iter().all(|point| point.z == 64.0));
            assert!(
                (points[2] - points[0])
                    .cross(points[1] - points[0])
                    .dot(normal)
                    > 0.0
            );
            assert_eq!(face.fog_volume, 0);
            assert_eq!(face.light_offset, -1);
        }
        let water = bsp.records::<LeafWaterData>().unwrap();
        assert_eq!(water.len(), 1);
        assert_eq!(water[0].surface_texinfo, tops[0].texinfo);
        assert_eq!((water[0].surface_z, water[0].min_z), (64.0, 0.0));
        let leaves = bsp.records::<Leaf>().unwrap();
        let leaf_faces = bsp.values::<u16>(lump::LEAFFACES).unwrap();
        let all_faces = bsp.records::<Face>().unwrap();
        let top_id = all_faces.iter().position(|face| face == &tops[0]).unwrap() as u16;
        let bottom_id = all_faces
            .iter()
            .position(|face| face == &bottoms[0])
            .unwrap() as u16;
        let mut top_refs = 0;
        let mut bottom_refs = 0;
        for leaf in leaves {
            let refs = &leaf_faces[leaf.first_leaf_face as usize
                ..leaf.first_leaf_face as usize + leaf.leaf_face_count as usize];
            if refs.contains(&top_id) {
                assert_eq!(leaf.contents & bsp::contents::WATER, 0);
                top_refs += 1;
            }
            if refs.contains(&bottom_id) {
                assert_ne!(leaf.contents & bsp::contents::WATER, 0);
                assert_eq!(leaf.water_data, 0);
                bottom_refs += 1;
            }
        }
        assert!(top_refs > 0 && bottom_refs > 0);
        assert_eq!(all_faces, bsp.values::<Face>(lump::ORIGINALFACES).unwrap());
        assert_eq!(vmf, source);
    }
}

#[test]
fn connected_different_depths_keep_one_region_and_do_not_add_an_internal_water_backside() {
    let (_directory, game) = game();
    let vmf = add_pool(
        common::room(),
        [-128.0, -128.0, 0.0],
        [0.0, 128.0, 64.0],
        TOP,
    );
    let vmf = add_pool(vmf, [0.0, -128.0, 32.0], [128.0, 128.0, 64.0], TOP);
    let bsp = compiled(&vmf, &game, compiler::PhysicsMode::None);
    assert_eq!(bsp.records::<LeafWaterData>().unwrap().len(), 1);
    assert_eq!(bsp.records::<LeafWaterData>().unwrap()[0].min_z, 0.0);
    let area: f32 = material_faces(&bsp, BOTTOM)
        .iter()
        .map(|face| face.area)
        .sum();
    assert_eq!(area, 256.0 * 256.0);

    let vmf = add_pool(
        common::room(),
        [-128.0, -128.0, 0.0],
        [128.0, 128.0, 32.0],
        TOP,
    );
    let vmf = add_pool(vmf, [-128.0, -128.0, 32.0], [128.0, 128.0, 64.0], TOP);
    let bsp = compiled(&vmf, &game, compiler::PhysicsMode::None);
    let bottom_faces = material_faces(&bsp, BOTTOM);
    assert!(!bottom_faces.is_empty());
    assert!(
        bottom_faces
            .iter()
            .all(|face| face_points(&bsp, face).iter().all(|point| point.z == 64.0))
    );
}

#[test]
fn missing_bottom_key_warns_and_explicit_bad_bottom_assets_preserve_prior_output() {
    let (directory, game) = game();
    let vmf = add_pool(
        common::room(),
        [-128.0, -128.0, 0.0],
        [128.0, 128.0, 64.0],
        TOP,
    );
    let source = vmf.clone();
    let top = directory.path().join("materials/water/top.vmt");
    std::fs::write(&top, "Water { $abovewater 1 }").unwrap();
    let output = compiler::compile(&vmf, &game);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("no $bottommaterial"))
    );
    assert!(material_faces(&output.bsp.unwrap(), BOTTOM).is_empty());
    let destination = directory.path().join("previous.bsp");
    std::fs::write(&destination, b"previous successful output").unwrap();
    for bottom in ["water/missing", "water/broken", "../outside", "water/cycle"] {
        std::fs::write(
            &top,
            format!("Water {{ $abovewater 1 $bottommaterial {bottom} }}"),
        )
        .unwrap();
        std::fs::write(
            directory.path().join("materials/water/broken.vmt"),
            "Water { $abovewater }",
        )
        .unwrap();
        std::fs::write(
            directory.path().join("materials/water/cycle.vmt"),
            "Patch { include materials/water/cycle.vmt }",
        )
        .unwrap();
        let output = compiler::compile(&vmf, &game);
        assert!(
            output.report.has_errors(),
            "bottom {bottom}: {:?}",
            output.report
        );
        assert!(output.bsp.is_none());
        assert!(!output.write(&destination).unwrap());
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"previous successful output"
        );
        assert_eq!(vmf, source);
    }
}

#[test]
fn underside_materials_leave_collision_and_water_metadata_bytes_unchanged() {
    let (directory, mut game) = game();
    let mut vmf = add_pool(
        common::room(),
        [-128.0, -128.0, 32.0],
        [128.0, 128.0, 96.0],
        TOP,
    );
    paint_pool_side(&mut vmf, DVec3::NEG_Y);
    paint_pool_side(&mut vmf, DVec3::NEG_Z);
    let native = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .map(|path| path.join(".local/share/Steam/steamapps/common/Counter-Strike Source"))
        .filter(|path| path.join("bin/linux64/vphysics.so").is_file());
    let mut modes = vec![compiler::PhysicsMode::None];
    if let Some(root) = native {
        game.base_dir = root;
        modes.push(compiler::PhysicsMode::Engine);
    }
    for physics in modes {
        if physics == compiler::PhysicsMode::Engine {
            vmf.world
                .properties
                .set(compiler::WATER_CURRENT_VELOCITY_KEY, "32 -16 8");
            vmf.world
                .properties
                .set(compiler::WATER_DAMPING_KEY, "0.005");
        }
        let top = directory.path().join("materials/water/top.vmt");
        let side = directory.path().join("materials/water/side.vmt");
        std::fs::write(&top, "Water { $abovewater 1 }").unwrap();
        std::fs::write(&side, "Water { $abovewater 1 }").unwrap();
        let previous = compiled(&vmf, &game, physics);
        std::fs::write(&top, "Water { $abovewater 1 $bottommaterial water/below }").unwrap();
        std::fs::write(&side, SIDE_TEXT).unwrap();
        let current = compiled(&vmf, &game, physics);
        for lump in [
            lump::LEAFWATERDATA,
            lump::LEAFMINDISTTOWATER,
            lump::BRUSHES,
            lump::BRUSHSIDES,
            lump::PHYSCOLLIDE,
        ] {
            assert_eq!(
                previous.lumps[lump].data,
                current.lumps[lump].data,
                "lump {lump}, physics {}",
                physics.name()
            );
        }
        let metadata = |bsp: &Bsp| {
            bsp.records::<Leaf>()
                .unwrap()
                .into_iter()
                .map(|mut leaf| {
                    leaf.first_leaf_face = 0;
                    leaf.leaf_face_count = 0;
                    leaf
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(metadata(&previous), metadata(&current));
        horizontal_pair(&current, SIDE, SIDE_BOTTOM, 32.0, 256.0 * 256.0);
    }
}

#[test]
fn shared_bottom_material_keeps_distinct_top_regions_and_omits_hidden_or_model_water() {
    let (directory, game) = game();
    let vmf = add_pool(
        common::room(),
        [-128.0, -128.0, 0.0],
        [0.0, 128.0, 64.0],
        TOP,
    );
    let vmf = add_pool(
        vmf,
        [0.0, -128.0, 0.0],
        [128.0, 128.0, 64.0],
        "water/top_patch",
    );
    let bsp = compiled(&vmf, &game, compiler::PhysicsMode::None);
    assert_eq!(bsp.records::<LeafWaterData>().unwrap().len(), 2);
    let bottom = material_faces(&bsp, BOTTOM);
    assert_eq!(bottom.len(), 2);
    assert_ne!(bottom[0].fog_volume, bottom[1].fog_volume);

    std::fs::write(
        directory.path().join("materials/water/below.vmt"),
        "Water { $abovewater }",
    )
    .unwrap();
    for model in [false, true] {
        let mut vmf = add_pool(
            common::room(),
            [-128.0, -128.0, 0.0],
            [128.0, 128.0, 64.0],
            TOP,
        );
        let pool = vmf.world.solids.last_mut().unwrap();
        if model {
            let id = pool.id;
            let mut document = Document::new(vmf);
            document
                .edit("model water", |tx| {
                    edit::tie_to_entity(tx, &[id], "func_brush", &[])
                })
                .unwrap();
            vmf = document.vmf().clone();
        } else {
            pool.hidden = true;
        }
        let bsp = compiled(&vmf, &game, compiler::PhysicsMode::None);
        assert!(material_faces(&bsp, BOTTOM).is_empty());
        assert!(bsp.records::<LeafWaterData>().unwrap().is_empty());
    }
}

#[test]
fn authored_top_overlay_does_not_bind_the_generated_bottom_face() {
    let (_directory, game) = game();
    let vmf = add_pool(
        common::room(),
        [-128.0, -128.0, 0.0],
        [128.0, 128.0, 64.0],
        TOP,
    );
    let top = vmf
        .world
        .solids
        .last()
        .unwrap()
        .sides
        .iter()
        .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    let mut vmf = overlay_fixture::add_overlay(vmf, &[top]);
    vmf.entities
        .last_mut()
        .unwrap()
        .properties
        .set("BasisOrigin", "0 0 64");
    let bsp = compiled(&vmf, &game, compiler::PhysicsMode::None);
    let faces = bsp.records::<Face>().unwrap();
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let overlays = bsp.overlay_data().unwrap().unwrap();
    assert_eq!(overlays.overlays.len(), 1);
    let overlay = overlays.overlays[0];
    assert_eq!(overlay.face_count(), material_faces(&bsp, TOP).len());
    assert!(
        overlay.faces[..overlay.face_count()]
            .iter()
            .all(|&face| { planes[faces[face as usize].plane as usize].normal == [0.0, 0.0, 1.0] })
    );
    assert!(!material_faces(&bsp, BOTTOM).is_empty());
}

#[test]
fn touching_solid_and_water_covers_clip_only_the_covered_underside_area() {
    let (_directory, game) = game();
    for liquid_cover in [false, true] {
        for low in [-32.0, 48.0] {
            let high = low + 64.0;
            let mut vmf = add_pool(
                common::room(),
                [-128.0, -128.0, 0.0],
                [128.0, 128.0, 64.0],
                TOP,
            );
            if liquid_cover {
                vmf = add_pool(vmf, [low, low, 64.0], [high, high, 96.0], TOP);
            } else {
                let mut document = Document::new(vmf);
                document
                    .edit("touching water cover", |tx| {
                        edit::create_block(
                            tx,
                            DVec3::new(low, low, 64.0),
                            DVec3::new(high, high, 96.0),
                            "dev/dev_measuregeneric01",
                        )
                    })
                    .unwrap();
                vmf = document.vmf().clone();
            }
            let bsp = compiled(&vmf, &game, compiler::PhysicsMode::None);
            let below = material_faces(&bsp, BOTTOM);
            let mut lower_area = 0.0;
            let mut upper_area = 0.0;
            for face in below {
                let points = face_points(&bsp, &face);
                if points.iter().all(|point| point.z == 64.0) {
                    lower_area += face.area;
                    assert!(
                        points.iter().all(|point| point.x <= low)
                            || points.iter().all(|point| point.x >= high)
                            || points.iter().all(|point| point.y <= low)
                            || points.iter().all(|point| point.y >= high)
                    );
                } else {
                    assert!(liquid_cover);
                    assert!(points.iter().all(|point| point.z == 96.0));
                    upper_area += face.area;
                }
            }
            assert_eq!(
                lower_area,
                256.0 * 256.0 - 64.0 * 64.0,
                "liquid={liquid_cover}, low={low}"
            );
            assert_eq!(upper_area, if liquid_cover { 64.0 * 64.0 } else { 0.0 });
        }
    }
}

#[test]
fn shared_top_and_bottom_texinfo_keeps_generated_faces_and_source_custody() {
    let (_directory, game) = game();
    let mut document = Document::new(common::room());
    let ObjectId::Solid(id) = document
        .edit("all-water slab", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-128.0, -128.0, 32.0),
                DVec3::new(128.0, 128.0, 64.0),
                TOP,
            )
        })
        .unwrap()
    else {
        unreachable!()
    };
    let map =
        compiler::map::Map::load(document.vmf(), &game, &mut compiler::Report::default()).unwrap();
    let brush = map.brushes.iter().find(|brush| brush.id == id).unwrap();
    let top = brush
        .sides
        .iter()
        .find(|side| map.planes.get(side.plane).normal == DVec3::Z)
        .unwrap();
    let bottom = brush
        .sides
        .iter()
        .find(|side| map.planes.get(side.plane).normal == DVec3::NEG_Z)
        .unwrap();
    assert_eq!(top.texinfo, bottom.texinfo);
    let original_bottom = bottom.source_id.unwrap();
    drop(map);
    let mut source = overlay_fixture::add_overlay(document.into_vmf(), &[original_bottom]);
    let overlay = source.entities.last_mut().unwrap();
    overlay.properties.set("BasisOrigin", "0 0 32");
    overlay.properties.set("BasisNormal", "0 0 -1");
    overlay.properties.set("BasisV", "0 -1 0");
    let before = source.clone();
    let output = compiler::compile(&source, &game);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let bsp = output.bsp.as_ref().unwrap();
    horizontal_pair(bsp, TOP, BOTTOM, 32.0, 256.0 * 256.0);
    let below = material_faces(bsp, BOTTOM);
    assert!(!below.is_empty());
    let planes = bsp.records::<bsp::Plane>().unwrap();
    assert!(
        below
            .iter()
            .any(|face| planes[face.plane as usize].normal[2] != 0.0)
    );
    for face in below {
        let normal = planes[face.plane as usize].normal;
        if normal[2] != 0.0 {
            let z = if normal[2] > 0.0 { 32.0 } else { 64.0 };
            assert_eq!(normal, [0.0, 0.0, if z == 32.0 { 1.0 } else { -1.0 }]);
            assert!(face_points(bsp, &face).iter().all(|point| point.z == z));
        }
    }
    let faces = bsp.records::<Face>().unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let origins = output.face_origins.as_ref().unwrap();
    let outward = material_faces(bsp, TOP)
        .into_iter()
        .find(|face| planes[face.plane as usize].normal == [0.0, 0.0, -1.0])
        .unwrap();
    let inward = material_faces(bsp, BOTTOM)
        .into_iter()
        .find(|face| planes[face.plane as usize].normal == [0.0, 0.0, 1.0])
        .unwrap();
    let corners = face_points(bsp, &inward);
    assert_eq!(corners.len(), 4);
    for point in [
        [-128.0, -128.0, 32.0],
        [-128.0, 128.0, 32.0],
        [128.0, -128.0, 32.0],
        [128.0, 128.0, 32.0],
    ] {
        assert!(corners.contains(&DVec3::from_array(point)));
    }
    let mut inward_info = infos[inward.texinfo as usize];
    inward_info.texdata = infos[outward.texinfo as usize].texdata;
    assert_eq!(inward_info, infos[outward.texinfo as usize]);
    let outward_id = faces.iter().position(|face| face == &outward).unwrap();
    assert_eq!(
        origins
            .contributors(outward_id)
            .unwrap()
            .cloned()
            .collect::<Vec<_>>(),
        [compiler::face_origins::Origin::Root {
            entity: source.world.id,
            solid: id,
            side: original_bottom
        }]
    );
    for face in material_faces(bsp, BOTTOM) {
        let index = faces
            .iter()
            .position(|candidate| candidate == &face)
            .unwrap();
        assert_eq!(origins.contributors(index).unwrap().len(), 0);
        assert_eq!(face.fog_volume, 0);
    }
    let overlays = bsp.overlay_data().unwrap().unwrap();
    assert_eq!(overlays.overlays.len(), 1);
    assert_eq!(overlays.overlays[0].face_count(), 1);
    assert_eq!(overlays.overlays[0].faces[0], outward_id as i32);
    assert_eq!(
        bsp.records::<LeafWaterData>()
            .unwrap()
            .iter()
            .map(|water| (water.surface_z, water.min_z))
            .collect::<Vec<_>>(),
        [(64.0, 32.0)]
    );
    assert_eq!(source, before);
}

#[test]
fn vertical_water_faces_use_their_own_material_and_clip_touching_solid_and_wet_covers() {
    let (_directory, game) = game();
    let mut vmf = add_pool(
        common::room(),
        [-128.0, -128.0, 0.0],
        [128.0, 128.0, 64.0],
        TOP,
    );
    paint_pool_side(&mut vmf, DVec3::NEG_Y);
    let mut document = Document::new(vmf);
    document
        .edit("touching vertical water cover", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-32.0, -160.0, 0.0),
                DVec3::new(32.0, -128.0, 64.0),
                "dev/dev_measuregeneric01",
            )
        })
        .unwrap();
    let vmf = add_pool(
        document.vmf().clone(),
        [64.0, -160.0, 0.0],
        [96.0, -128.0, 32.0],
        TOP,
    );
    let source = vmf.clone();
    let bsp = compiled(&vmf, &game, compiler::PhysicsMode::None);
    let inward = material_faces(&bsp, SIDE_BOTTOM);
    assert!(!inward.is_empty());
    assert_eq!(
        inward.iter().map(|face| face.area).sum::<f32>(),
        256.0 * 64.0 - 64.0 * 64.0 - 32.0 * 32.0
    );
    let infos = bsp.records::<TexInfo>().unwrap();
    let outward = material_faces(&bsp, SIDE);
    assert_eq!(
        outward.iter().map(|face| face.area).sum::<f32>(),
        inward.iter().map(|face| face.area).sum::<f32>()
    );
    let side_info = infos[outward[0].texinfo as usize];
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let water = bsp.records::<LeafWaterData>().unwrap();
    let region = water
        .iter()
        .position(|region| region.surface_z == 64.0)
        .unwrap() as i16;
    for face in inward {
        assert_eq!(planes[face.plane as usize].normal, [0.0, 1.0, 0.0]);
        assert_eq!(face.fog_volume, region);
        let info = infos[face.texinfo as usize];
        assert_eq!(info.texture_vecs, side_info.texture_vecs);
        assert_eq!(info.lightmap_vecs, side_info.lightmap_vecs);
        assert_eq!(info.flags, side_info.flags);
        assert_ne!(info.texdata, side_info.texdata);
        let points = face_points(&bsp, &face);
        assert!(points.iter().all(|point| point.y == -128.0));
        assert!(
            (points[2] - points[0])
                .cross(points[1] - points[0])
                .dot(DVec3::Y)
                > 0.0
        );
        assert!(
            points.iter().all(|point| point.x <= -32.0)
                || points.iter().all(|point| point.x >= 32.0)
        );
        assert!(
            points.iter().all(|point| point.x <= 64.0)
                || points.iter().all(|point| point.x >= 96.0)
                || points.iter().all(|point| point.z >= 32.0)
        );
    }
    assert_eq!(vmf, source);
}

#[test]
fn shared_vertical_texinfo_keeps_generated_faces_in_their_own_wet_regions() {
    let (_directory, game) = game();
    for (height, top) in [(64.0, "water/top_patch"), (96.0, TOP)] {
        let mut vmf = add_pool(
            common::room(),
            [-128.0, -128.0, 0.0],
            [0.0, 128.0, 64.0],
            TOP,
        );
        paint_pool_side(&mut vmf, DVec3::NEG_Y);
        paint_pool_side(&mut vmf, DVec3::X);
        let first = vmf.world.solids.last().unwrap().id;
        let mut vmf = add_pool(vmf, [0.0, -128.0, 0.0], [128.0, 128.0, height], top);
        paint_pool_side(&mut vmf, DVec3::NEG_Y);
        paint_pool_side(&mut vmf, DVec3::NEG_X);
        let second = vmf.world.solids.last().unwrap().id;
        let map = compiler::map::Map::load(&vmf, &game, &mut compiler::Report::default()).unwrap();
        let front_infos = [first, second].map(|id| {
            map.brushes
                .iter()
                .find(|brush| brush.id == id)
                .unwrap()
                .sides
                .iter()
                .find(|side| map.planes.get(side.plane).normal == DVec3::NEG_Y)
                .unwrap()
                .texinfo
        });
        assert_eq!(front_infos[0], front_infos[1]);
        let bsp = compiled(&vmf, &game, compiler::PhysicsMode::None);
        let records = bsp.records::<LeafWaterData>().unwrap();
        assert_eq!(records.len(), 2);
        let infos = bsp.records::<TexInfo>().unwrap();
        let texdata = bsp.records::<bsp::TexData>().unwrap();
        let names = bsp.texdata_strings().unwrap();
        let region_names: Vec<_> = records
            .iter()
            .map(|region| {
                names[texdata[infos[region.surface_texinfo as usize].texdata as usize].name_index
                    as usize]
                    .as_str()
            })
            .collect();
        let planes = bsp.records::<bsp::Plane>().unwrap();
        let leaves = bsp.records::<Leaf>().unwrap();
        let leaf_faces = bsp.values::<u16>(lump::LEAFFACES).unwrap();
        let all_faces = bsp.records::<Face>().unwrap();
        for (material, wet) in [(SIDE_BOTTOM, true), (SIDE, false)] {
            let mut areas = [0.0; 2];
            for face in material_faces(&bsp, material) {
                let normal = planes[face.plane as usize].normal;
                if normal[0] != 0.0 {
                    assert!(height > 64.0);
                    assert_eq!(normal, [if wet { 1.0 } else { -1.0 }, 0.0, 0.0]);
                    assert_eq!(records[face.fog_volume as usize].surface_z, height as f32);
                    assert!(face_points(&bsp, &face).iter().all(|point| point.z >= 64.0));
                    continue;
                }
                assert_eq!(normal, [0.0, if wet { 1.0 } else { -1.0 }, 0.0]);
                let region = face.fog_volume as usize;
                assert!(region < records.len());
                let points = face_points(&bsp, &face);
                let first_region = if height == 64.0 {
                    region_names[region].eq_ignore_ascii_case(TOP)
                } else {
                    records[region].surface_z == 64.0
                };
                assert!(
                    points
                        .iter()
                        .all(|point| point.y == -128.0
                            && point.z <= records[region].surface_z as f64)
                );
                assert!(points.iter().all(|point| if first_region {
                    point.x <= 0.0
                } else {
                    point.x >= 0.0
                }));
                let id = all_faces
                    .iter()
                    .position(|candidate| candidate == &face)
                    .unwrap() as u16;
                let references: Vec<_> = leaves
                    .iter()
                    .filter(|leaf| {
                        leaf_faces[leaf.first_leaf_face as usize
                            ..leaf.first_leaf_face as usize + leaf.leaf_face_count as usize]
                            .contains(&id)
                    })
                    .collect();
                assert!(!references.is_empty());
                assert!(
                    references
                        .iter()
                        .all(|leaf| (leaf.contents & bsp::contents::WATER != 0) == wet
                            && (!wet || leaf.water_data == face.fog_volume))
                );
                areas[usize::from(!first_region)] += face.area;
            }
            assert_eq!(areas, [128.0 * 64.0, 128.0 * height as f32]);
        }
    }
}

#[test]
fn authored_downward_boundaries_use_their_own_target_and_intersect_dry_and_wet_clipping() {
    let (_directory, game) = game();
    for (cover, low) in [
        ("dry solid", -32.0),
        ("dry solid", 48.0),
        ("wet solid", -32.0),
        ("wet solid", 48.0),
        ("wet neighbor", -32.0),
        ("wet neighbor", 48.0),
        ("crossed", 0.0),
    ] {
        let mut source = add_pool(
            common::room(),
            [-128.0, -128.0, 64.0],
            [128.0, 128.0, 128.0],
            TOP,
        );
        paint_pool_side(&mut source, DVec3::NEG_Z);
        if cover == "wet neighbor" {
            source = add_pool(
                source,
                [low, low, 32.0],
                [low + 64.0, low + 64.0, 64.0],
                TOP,
            );
        } else {
            let mut document = Document::new(source);
            let boxes = if cover == "crossed" {
                vec![
                    ([-96.0, -128.0, 32.0], [-88.0, 128.0, 64.0]),
                    ([-128.0, -48.0, 64.0], [128.0, -40.0, 96.0]),
                ]
            } else {
                let z = if cover == "wet solid" { 64.0 } else { 32.0 };
                vec![([low, low, z], [low + 64.0, low + 64.0, z + 32.0])]
            };
            document
                .edit("bottom obstructions", |tx| {
                    for (mins, maxs) in boxes {
                        edit::create_block(
                            tx,
                            DVec3::from_array(mins),
                            DVec3::from_array(maxs),
                            "dev/dev_measuregeneric01",
                        )?;
                    }
                    Ok(())
                })
                .unwrap();
            source = document.into_vmf();
        }
        let original = source.clone();
        let bsp = compiled(&source, &game, compiler::PhysicsMode::None);
        horizontal_pair(
            &bsp,
            SIDE,
            SIDE_BOTTOM,
            64.0,
            if cover == "crossed" {
                248.0 * 248.0
            } else {
                256.0 * 256.0 - 64.0 * 64.0
            },
        );
        let infos = bsp.records::<TexInfo>().unwrap();
        let outward = material_faces(&bsp, SIDE);
        let inward = material_faces(&bsp, SIDE_BOTTOM);
        let source_info = infos[outward[0].texinfo as usize];
        assert_eq!(
            source_info.texture_vecs,
            [[4.0, 0.0, 0.0, 31.0], [0.0, 4.0 / 3.0, 0.0, -29.0]]
        );
        let water = bsp.records::<LeafWaterData>().unwrap();
        for face in inward {
            assert_eq!(water[face.fog_volume as usize].surface_z, 128.0);
            let mut info = infos[face.texinfo as usize];
            assert_ne!(info.texdata, source_info.texdata);
            info.texdata = source_info.texdata;
            assert_eq!(info, source_info);
            let points = face_points(&bsp, &face);
            if cover == "crossed" {
                assert!(
                    points.iter().all(|point| point.x <= -96.0)
                        || points.iter().all(|point| point.x >= -88.0)
                );
                assert!(
                    points.iter().all(|point| point.y <= -48.0)
                        || points.iter().all(|point| point.y >= -40.0)
                );
            } else {
                assert!(
                    points.iter().all(|point| point.x <= low)
                        || points.iter().all(|point| point.x >= low + 64.0)
                        || points.iter().all(|point| point.y <= low)
                        || points.iter().all(|point| point.y >= low + 64.0)
                );
            }
        }
        assert_eq!(source, original);
    }
}

#[test]
fn coplanar_authored_bottoms_with_shared_texinfo_keep_distinct_wet_regions() {
    let (_directory, game) = game();
    for height in [96.0, 128.0] {
        let mut source = add_pool(
            common::room(),
            [-128.0, -128.0, 32.0],
            [0.0, 128.0, 96.0],
            "water/top_patch",
        );
        paint_pool_side(&mut source, DVec3::NEG_Z);
        let first = source.world.solids.last().unwrap().id;
        source = add_pool(source, [0.0, -128.0, 32.0], [128.0, 128.0, height], TOP);
        paint_pool_side(&mut source, DVec3::NEG_Z);
        let second = source.world.solids.last().unwrap().id;
        let map =
            compiler::map::Map::load(&source, &game, &mut compiler::Report::default()).unwrap();
        let bottom_infos = [first, second].map(|id| {
            map.brushes
                .iter()
                .find(|brush| brush.id == id)
                .unwrap()
                .sides
                .iter()
                .find(|side| map.planes.get(side.plane).normal == DVec3::NEG_Z)
                .unwrap()
                .texinfo
        });
        assert_eq!(bottom_infos[0], bottom_infos[1]);
        let bsp = compiled(&source, &game, compiler::PhysicsMode::None);
        horizontal_pair(&bsp, SIDE, SIDE_BOTTOM, 32.0, 256.0 * 256.0);
        let records = bsp.records::<LeafWaterData>().unwrap();
        assert_eq!(records.len(), 2);
        let infos = bsp.records::<TexInfo>().unwrap();
        let texdata = bsp.records::<bsp::TexData>().unwrap();
        let names = bsp.texdata_strings().unwrap();
        for material in [SIDE, SIDE_BOTTOM] {
            let mut areas = [0.0; 2];
            for face in material_faces(&bsp, material) {
                let region = records[face.fog_volume as usize];
                assert_eq!(region.min_z, 32.0);
                let name = &names[texdata[infos[region.surface_texinfo as usize].texdata as usize]
                    .name_index as usize];
                let left = name.eq_ignore_ascii_case("water/top_patch");
                assert!(left || name.eq_ignore_ascii_case(TOP));
                assert_eq!(region.surface_z, if left { 96.0 } else { height as f32 });
                assert!(
                    face_points(&bsp, &face).iter().all(|point| point.z == 32.0
                        && if left { point.x <= 0.0 } else { point.x >= 0.0 })
                );
                areas[usize::from(!left)] += face.area;
            }
            assert_eq!(areas, [128.0 * 256.0, 128.0 * 256.0]);
        }
    }
}

#[test]
fn public_face_build_discovers_regions_for_bottom_only_generated_surfaces() {
    let (directory, game) = game();
    std::fs::write(
        directory.path().join("materials/water/top.vmt"),
        "Water { $abovewater 1 }",
    )
    .unwrap();
    let mut source = add_pool(
        common::room(),
        [-128.0, -128.0, 32.0],
        [128.0, 128.0, 96.0],
        TOP,
    );
    paint_pool_side(&mut source, DVec3::NEG_Z);
    let mut report = compiler::Report::default();
    let mut map = compiler::map::Map::load(&source, &game, &mut report).unwrap();
    let mut tree =
        compiler::tree::Tree::build(&map.brushes, &map.entities[0].brushes, &mut map.planes)
            .unwrap();
    let portals = compiler::portals::build(&tree, &map.planes).unwrap();
    assert!(
        compiler::portals::flood(&mut tree, &map.planes, &portals, &map.entities, &mut report)
            .is_none()
    );
    tree.prune();
    assert!(!report.has_errors(), "{report:?}");
    let faces =
        compiler::faces::build_controlled(&map, &tree, 0, &compiler::Control::default()).unwrap();
    let generated = faces
        .iter()
        .filter(|face| {
            let info = map.texinfos.infos[face.texinfo];
            map.materials
                .get(map.texdata[info.texdata as usize])
                .name
                .eq_ignore_ascii_case(SIDE_BOTTOM)
        })
        .collect::<Vec<_>>();
    assert!(!generated.is_empty());
    assert_eq!(
        generated
            .iter()
            .map(|face| face.winding.area())
            .sum::<f64>(),
        256.0 * 256.0
    );
    for face in generated {
        assert_eq!(map.planes.get(face.plane).normal, DVec3::Z);
        assert!(face.winding.0.iter().all(|point| point.z == 32.0));
        assert!(face.source_sides.is_empty());
        assert_eq!(face.water_region, Some(0));
        assert!(!face.leaves.is_empty());
        assert!(
            face.leaves
                .iter()
                .all(|leaf| tree.nodes[*leaf].contents & bsp::contents::WATER != 0)
        );
    }
}

fn lighting_materials(directory: &std::path::Path) {
    for folder in ["dev", "tools", "skybox"] {
        std::fs::create_dir_all(directory.join("materials").join(folder)).unwrap();
    }
    for (path, text) in [
        ("dev/dev_measuregeneric01", "LightmappedGeneric {}"),
        ("tools/toolsblack", "UnlitGeneric {}"),
    ] {
        std::fs::write(directory.join(format!("materials/{path}.vmt")), text).unwrap();
    }
    for suffix in ["rt", "lf", "bk", "ft", "up", "dn"] {
        std::fs::write(
            directory.join(format!("materials/skybox/sky_day01_01{suffix}.vmt")),
            "UnlitGeneric {}",
        )
        .unwrap();
    }
}

#[test]
fn downward_faces_preserve_region_metadata_and_unlit_ldr_hdr() {
    let (directory, game) = game();
    lighting_materials(directory.path());
    for regional in [false, true] {
        let mut source = add_pool(
            common::room(),
            [-128.0, -128.0, 32.0],
            [128.0, 128.0, 96.0],
            TOP,
        );
        paint_pool_side(&mut source, DVec3::NEG_Z);
        for side in source
            .world
            .solids
            .iter_mut()
            .flat_map(|solid| &mut solid.sides)
        {
            side.lightmap_scale = 128;
        }
        if regional {
            let mut document = Document::new(source);
            document
                .edit("contained pool", |tx| {
                    compile_region::apply(
                        tx,
                        Some(compile_region::Definition {
                            enabled: true,
                            bounds: compile_region::Bounds {
                                mins: DVec3::new(-192.0, -192.0, 4.0),
                                maxs: DVec3::new(192.0, 192.0, 192.0),
                            },
                        }),
                    )
                })
                .unwrap();
            source = document.into_vmf();
        }
        let original = source.clone();
        for lighting in [
            compiler::LightingMode::Fullbright,
            compiler::LightingMode::DirectLdr,
            compiler::LightingMode::DirectLdrHdr,
        ] {
            let options = compiler::Options {
                lighting,
                ..Default::default()
            };
            std::fs::write(
                directory.path().join("materials/water/side.vmt"),
                "Water { $abovewater 1 }",
            )
            .unwrap();
            let before = compiler::compile_with_options(&source, &game, options);
            assert!(!before.report.has_errors(), "{:?}", before.report);
            assert!(before.report.diagnostics.iter().any(|diagnostic| {
                diagnostic
                    .message
                    .to_ascii_lowercase()
                    .contains("water/side")
                    && diagnostic.message.contains("no $bottommaterial")
            }));
            std::fs::write(directory.path().join("materials/water/side.vmt"), SIDE_TEXT).unwrap();
            let after = compiler::compile_with_options(&source, &game, options);
            assert!(!after.report.has_errors(), "{:?}", after.report);
            let previous = before.bsp.as_ref().unwrap();
            let current = after.bsp.as_ref().unwrap();
            horizontal_pair(current, SIDE, SIDE_BOTTOM, 32.0, 256.0 * 256.0);
            assert_eq!(
                current
                    .records::<LeafWaterData>()
                    .unwrap()
                    .iter()
                    .map(|region| (region.surface_z, region.min_z))
                    .collect::<Vec<_>>(),
                [(96.0, 32.0)]
            );
            for index in [
                lump::PLANES,
                lump::BRUSHES,
                lump::BRUSHSIDES,
                lump::LEAFBRUSHES,
                lump::LEAFWATERDATA,
                lump::LEAFMINDISTTOWATER,
                lump::VISIBILITY,
                lump::PHYSCOLLIDE,
                lump::LIGHTING,
                lump::LIGHTING_HDR,
            ] {
                assert_eq!(
                    previous.lumps[index], current.lumps[index],
                    "regional={regional}, {lighting:?}, lump={index}"
                );
            }
            let leaves = |bsp: &Bsp| {
                bsp.records::<Leaf>()
                    .unwrap()
                    .into_iter()
                    .map(|mut leaf| {
                        leaf.first_leaf_face = 0;
                        leaf.leaf_face_count = 0;
                        leaf
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(leaves(previous), leaves(current));
            let infos = current.records::<TexInfo>().unwrap();
            let data = current.records::<bsp::TexData>().unwrap();
            let names = current.texdata_strings().unwrap();
            for channel in [lump::FACES, lump::FACES_HDR] {
                let water_faces = current
                    .values::<Face>(channel)
                    .unwrap()
                    .into_iter()
                    .filter(|face| {
                        let name = &names[data[infos[face.texinfo as usize].texdata as usize]
                            .name_index as usize];
                        name.eq_ignore_ascii_case(SIDE) || name.eq_ignore_ascii_case(SIDE_BOTTOM)
                    })
                    .collect::<Vec<_>>();
                if channel == lump::FACES || lighting == compiler::LightingMode::DirectLdrHdr {
                    assert!(!water_faces.is_empty());
                }
                assert!(water_faces.iter().all(|face| face.light_offset == -1));
            }
        }
        assert_eq!(source, original);
    }
}

#[test]
fn ineligible_liquid_sources_never_acquire_a_downward_target_or_read_its_invalid_asset() {
    let (directory, game) = game();
    std::fs::write(
        directory.path().join("materials/water/side_below.vmt"),
        "Water { $abovewater }",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("materials/slime.vmt"),
        "LightmappedGeneric { %compileslime 1 $bottommaterial slime }",
    )
    .unwrap();
    for case in [
        "hidden",
        "nodraw bottom",
        "no top",
        "detail",
        "model",
        "sloped pool",
        "sloped bottom",
        "slime",
    ] {
        let mut source = add_pool(
            common::room(),
            [-128.0, -128.0, 32.0],
            [128.0, 128.0, 96.0],
            TOP,
        );
        paint_pool_side(&mut source, DVec3::NEG_Z);
        let pool = source.world.solids.last_mut().unwrap();
        let id = pool.id;
        match case {
            "hidden" => pool.hidden = true,
            "nodraw bottom" => pool.sides[1].material = "nodraw".into(),
            "no top" => pool.sides[0].material = "nodraw".into(),
            "slime" => pool.sides[0].material = "slime".into(),
            "sloped bottom" => {
                for point in &mut pool.sides[1].plane.0 {
                    point.z += point.x / 16.0;
                }
            }
            "detail" | "model" | "sloped pool" => {
                let mut document = Document::new(source);
                document
                    .edit("liquid eligibility", |tx| {
                        if case == "sloped pool" {
                            edit::transform(
                                tx,
                                &[ObjectId::Solid(id)],
                                map_core::transform::Transform {
                                    rotation: DVec3::new(0.0, 15.0, 0.0),
                                    pivot: DVec3::new(0.0, 0.0, 64.0),
                                    ..Default::default()
                                },
                            )
                        } else {
                            edit::tie_to_entity(
                                tx,
                                &[id],
                                if case == "detail" {
                                    "func_detail"
                                } else {
                                    "func_brush"
                                },
                                &[],
                            )
                            .map(|_| ())
                        }
                    })
                    .unwrap();
                source = document.into_vmf();
            }
            _ => unreachable!(),
        }
        let before = source.clone();
        let output = compiler::compile(&source, &game);
        assert!(
            !output
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("$bottommaterial")
                    && diagnostic.message.contains("side_below")),
            "{case}: {:?}",
            output.report
        );
        if case == "slime" {
            assert!(output.bsp.is_none() && output.report.has_errors());
            assert!(output.report.diagnostics.iter().any(|diagnostic| {
                diagnostic
                    .message
                    .contains("cannot combine WATER and SLIME")
            }));
        } else if matches!(case, "no top" | "sloped pool") {
            assert!(output.bsp.is_none() && output.report.has_errors());
            assert!(
                output
                    .report
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.message.contains("visible horizontal top")),
                "{case}: {:?}",
                output.report
            );
        } else {
            assert!(!output.report.has_errors(), "{case}: {:?}", output.report);
            let bsp = output.bsp.as_ref().unwrap();
            assert!(material_faces(bsp, SIDE_BOTTOM).is_empty(), "{case}");
            if matches!(case, "hidden" | "detail" | "model") {
                assert!(bsp.records::<LeafWaterData>().unwrap().is_empty(), "{case}");
            }
        }
        assert_eq!(source, before);
    }
}

#[test]
fn downward_target_failures_crossing_regions_and_cancellation_preserve_previous_outputs_and_source()
{
    let (directory, game) = game();
    lighting_materials(directory.path());
    let mut source = add_pool(
        common::room(),
        [-128.0, -128.0, 32.0],
        [128.0, 128.0, 96.0],
        TOP,
    );
    paint_pool_side(&mut source, DVec3::NEG_Z);
    let owner = source.world.solids.last().unwrap().id;
    let original = source.clone();
    let path = directory.path().join("previous.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    std::fs::write(path.with_extension("lin"), b"previous LIN").unwrap();
    for target in ["water/missing", "water/broken"] {
        std::fs::write(
            directory.path().join("materials/water/side.vmt"),
            format!("Water {{ $abovewater 1 $bottommaterial {target} }}"),
        )
        .unwrap();
        std::fs::write(
            directory.path().join("materials/water/broken.vmt"),
            "Water { $abovewater }",
        )
        .unwrap();
        let output = compiler::compile(&source, &game);
        assert!(output.bsp.is_none() && output.report.has_errors());
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(
                    |diagnostic| diagnostic.object == Some(format!("solid:{owner}"))
                        && diagnostic.message.contains(target)
                ),
            "{:?}",
            output.report
        );
        assert!(!output.write(&path).unwrap());
    }
    std::fs::write(directory.path().join("materials/water/side.vmt"), SIDE_TEXT).unwrap();
    let mut crossing = Document::new(source.clone());
    crossing
        .edit("crossing pool", |tx| {
            compile_region::apply(
                tx,
                Some(compile_region::Definition {
                    enabled: true,
                    bounds: compile_region::Bounds {
                        mins: DVec3::new(-192.0, -192.0, 4.0),
                        maxs: DVec3::new(64.0, 192.0, 192.0),
                    },
                }),
            )
        })
        .unwrap();
    let before = crossing.vmf().clone();
    let output = compiler::compile(crossing.vmf(), &game);
    assert!(output.bsp.is_none() && output.report.has_errors());
    assert!(
        output.report.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("crosses a compile-region boundary")
            && diagnostic.message.contains(&format!("solid:{owner}"))),
        "{:?}",
        output.report
    );
    assert!(!output.write(&path).unwrap());
    assert_eq!(crossing.vmf(), &before);
    let control = compiler::Control::default();
    control.cancel().unwrap();
    let output = compiler::compile_controlled(&source, &game, &control);
    assert!(output.bsp.is_none());
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    assert_eq!(
        std::fs::read(path.with_extension("lin")).unwrap(),
        b"previous LIN"
    );
    assert_eq!(source, original);
}

fn water_texture(cube: bool, hdr: bool) -> Vec<u8> {
    let header = if cube { 88_u32 } else { 80 };
    let mut bytes = vec![0; header as usize];
    bytes[..4].copy_from_slice(b"VTF\0");
    for (offset, value) in [
        (4, 7_u32),
        (8, if cube { 4 } else { 2 }),
        (12, header),
        (20, if cube { 0x4000 } else { 0 }),
        (52, if hdr { 24 } else { 0 }),
        (57, u32::MAX),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for offset in [16, 18, 24, 63] {
        bytes[offset..offset + 2].copy_from_slice(&1_u16.to_le_bytes());
    }
    bytes[56] = 1;
    if cube {
        bytes[68..72].copy_from_slice(&1_u32.to_le_bytes());
        bytes[80..84].copy_from_slice(&[0x30, 0, 0, 0]);
        bytes[84..88].copy_from_slice(&header.to_le_bytes());
    }
    for _ in 0..if cube { 7 } else { 1 } {
        if hdr {
            for value in [0x3c00_u16, 0, 0x4000, 0x3c00] {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        } else {
            bytes.extend_from_slice(&[127, 127, 255, 255]);
        }
    }
    bytes
}

#[test]
fn downward_own_target_keeps_original_water_packing_and_pending_reflection_aliases() {
    use compiler::water_assets::{read_packed_water, verify_packed_water};
    use std::collections::BTreeSet;

    let (directory, game) = game();
    let output_directory = tempfile::tempdir().unwrap();
    lighting_materials(directory.path());
    let prefix = "$normalmap water/normal $envmap water/cube $underwateroverlay 0";
    for (name, body) in [
        (
            "top",
            format!("{prefix} $abovewater 1 $bottommaterial water/below"),
        ),
        ("below", format!("{prefix} $abovewater 0")),
        (
            "side",
            format!("{prefix} $abovewater 1 $bottommaterial water/side_below"),
        ),
        ("side_below", format!("{prefix} $abovewater 0")),
    ] {
        std::fs::write(
            directory.path().join(format!("materials/water/{name}.vmt")),
            format!("Water {{ {body} }}"),
        )
        .unwrap();
    }
    for (name, bytes) in [
        ("normal.vtf", water_texture(false, false)),
        ("cube.vtf", water_texture(true, false)),
        ("cube.hdr.vtf", water_texture(true, true)),
    ] {
        std::fs::write(directory.path().join("materials/water").join(name), bytes).unwrap();
    }
    let mut source = add_pool(
        common::room(),
        [-128.0, -128.0, 32.0],
        [128.0, 128.0, 96.0],
        TOP,
    );
    paint_pool_side(&mut source, DVec3::NEG_Z);
    let bottom = source.world.solids.last().unwrap().sides[1].id;
    let original = source.clone();
    let baseline = compiled(&source, &game, compiler::PhysicsMode::None);
    let packed = compiler::compile_with_options(
        &source,
        &game,
        compiler::Options {
            pack_water_assets: true,
            ..Default::default()
        },
    );
    assert!(!packed.report.has_errors(), "{:?}", packed.report);
    let bsp = packed.bsp.as_ref().unwrap();
    let plan = packed.water_assets.as_ref().unwrap();
    let expected = [
        "materials/water/top.vmt",
        "materials/water/below.vmt",
        "materials/water/side.vmt",
        "materials/water/side_below.vmt",
        "materials/water/normal.vtf",
        "materials/water/cube.vtf",
        "materials/water/cube.hdr.vtf",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<BTreeSet<_>>();
    assert_eq!(
        plan.entries()
            .iter()
            .map(|entry| entry.name.clone())
            .collect::<BTreeSet<_>>(),
        expected
    );
    let archive = bsp.pak_data(bsp::PakLimits::default()).unwrap();
    for entry in plan.entries() {
        assert_eq!(
            entry.data,
            std::fs::read(directory.path().join(&entry.name)).unwrap()
        );
        assert_eq!(
            archive
                .entries
                .iter()
                .find(|candidate| candidate.name == entry.name)
                .unwrap()
                .data,
            entry.data
        );
    }
    for index in 0..bsp::LUMP_COUNT {
        if index != lump::PAKFILE {
            assert_eq!(bsp.lumps[index], baseline.lumps[index]);
        }
    }
    assert_eq!(bsp.game_lumps, baseline.game_lumps);
    horizontal_pair(bsp, SIDE, SIDE_BOTTOM, 32.0, 256.0 * 256.0);
    let packed_bsp = bsp.clone();
    let manifest = plan.manifest();
    let previous = output_directory.path().join("previous.bsp");
    std::fs::write(&previous, b"previous BSP").unwrap();
    std::fs::write(previous.with_extension("lin"), b"previous LIN").unwrap();
    let staged = packed
        .prepare_write_controlled(&previous, &compiler::Control::default())
        .unwrap()
        .unwrap();
    let bottom_path = directory.path().join("materials/water/side_below.vmt");
    let original_bottom = std::fs::read(&bottom_path).unwrap();
    let mut changed = original_bottom.clone();
    changed.push(b'\n');
    std::fs::write(&bottom_path, changed).unwrap();
    assert!(staged.commit(&compiler::Control::default()).is_err());
    assert!(packed.write(&previous).is_err());
    std::fs::write(&bottom_path, original_bottom).unwrap();
    std::fs::write(directory.path().join("materials/water/side.vmt"), "Water { $normalmap water/normal $envmap env_cubemap $underwateroverlay 0 $abovewater 1 $bottommaterial water/side_below }").unwrap();
    std::fs::write(
        &bottom_path,
        "Water { $normalmap water/normal $envmap env_cubemap $underwateroverlay 0 $abovewater 0 }",
    )
    .unwrap();
    let helper = source.object_ids().chain(source.side_ids()).max().unwrap() + 1;
    let mut sample = source_formats::vmf::Entity::new(helper, "env_cubemap");
    sample.properties.set("origin", "192 0 128");
    sample.properties.set("cubemapsize", "1");
    sample.properties.set("sides", bottom.to_string());
    source.entities.push(sample);
    for lighting in [
        compiler::LightingMode::DirectLdr,
        compiler::LightingMode::DirectLdrHdr,
    ] {
        let pending = compiler::compile_with_context_controlled(
            &source,
            &game,
            compiler::Options {
                lighting,
                generate_reflections: true,
                ..Default::default()
            },
            Some(&compiler::CompileContext::new("downward_water").unwrap()),
            &compiler::Control::default(),
        );
        assert!(!pending.report.has_errors(), "{:?}", pending.report);
        let reflections = pending.cubemap_assets.as_ref().unwrap();
        assert_eq!(reflections.phase, compiler::CubemapAssetPhase::Pending);
        assert_eq!(reflections.samples.len(), 1);
        assert_eq!(
            reflections.hdr_samples.len(),
            usize::from(lighting == compiler::LightingMode::DirectLdrHdr)
        );
        assert_eq!(reflections.assignments.len(), 1);
        assert_eq!(
            (
                reflections.assignments[0].face_id,
                reflections.assignments[0].helper_id,
                reflections.assignments[0].manual
            ),
            (bottom, helper, true)
        );
        assert_eq!(
            reflections
                .aliases
                .iter()
                .map(|alias| alias.material.as_str())
                .collect::<BTreeSet<_>>(),
            [SIDE, "water/side_below"].into_iter().collect()
        );
        let outward = reflections
            .aliases
            .iter()
            .find(|alias| alias.material == SIDE)
            .unwrap();
        let inward = reflections
            .aliases
            .iter()
            .find(|alias| alias.material == "water/side_below")
            .unwrap();
        assert_eq!(reflections.assignments[0].alias, outward.alias);
        assert_ne!(outward.alias, inward.alias);
        horizontal_pair(
            pending.bsp.as_ref().unwrap(),
            &outward.alias,
            &inward.alias,
            32.0,
            256.0 * 256.0,
        );
        assert!(
            pending
                .write(&previous)
                .unwrap_err()
                .to_string()
                .contains("pending native capture")
        );
    }
    source.entities.pop();
    assert_eq!(source, original);
    std::fs::remove_dir_all(directory.path().join("materials")).unwrap();
    assert_eq!(
        read_packed_water(&packed_bsp, &compiler::Control::default()).unwrap(),
        Some(manifest.clone())
    );
    verify_packed_water(&packed_bsp, &manifest, &compiler::Control::default()).unwrap();
    assert_eq!(std::fs::read(&previous).unwrap(), b"previous BSP");
    assert_eq!(
        std::fs::read(previous.with_extension("lin")).unwrap(),
        b"previous LIN"
    );
}
