mod common;

use std::path::PathBuf;
use std::process::Command;

use compiler::{Options, PhysicsMode};
use glam::DVec3;
use map_core::{BrushGeometry, Document, ObjectId, edit};
use source_formats::bsp::{Bsp, CollisionHeader, Model, lump};
use source_formats::gamefs::{Mount, MountedPath};

fn installed_game() -> Option<PathBuf> {
    let root = std::env::var_os("CSS_DIR").map(PathBuf::from).or_else(|| {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home).join(".local/share/Steam/steamapps/common/Counter-Strike Source")
        })
    })?;
    root.join("bin/linux64/vphysics.so")
        .is_file()
        .then_some(root)
}

#[test]
fn engine_worker_preserves_translated_narrow_lantern_boxes() {
    let Some(root) = installed_game() else {
        eprintln!("CS:S not installed; skipping native lantern fixture");
        return;
    };
    let mut document = Document::new(common::room());
    document
        .edit("translated narrow lantern boxes", |transaction| {
            let mut solids = Vec::new();
            for (mins, maxs) in [
                ([106.0, -334.0, 58.0], [114.0, -326.0, 166.0]),
                ([106.0, -352.0, 158.0], [114.0, -334.0, 166.0]),
                ([-64.0, -504.0, 11.0], [-56.0, -496.0, 119.0]),
                ([-64.0, -522.0, 111.0], [-56.0, -504.0, 119.0]),
                ([-334.0, -654.0, -22.0], [-326.0, -646.0, 86.0]),
                ([-334.0, -672.0, 78.0], [-326.0, -654.0, 86.0]),
                ([-564.0, -724.0, -83.0], [-556.0, -716.0, 25.0]),
                ([-564.0, -742.0, 17.0], [-556.0, -724.0, 25.0]),
            ] {
                let ObjectId::Solid(id) = edit::create_block(
                    transaction,
                    DVec3::from_array(mins),
                    DVec3::from_array(maxs),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                solids.push(id);
            }
            edit::tie_to_entity(
                transaction,
                &solids,
                "func_detail",
                &[("targetname".into(), "detail_misc".into())],
            )?;
            Ok(())
        })
        .unwrap();
    let before = document.vmf().write(&Default::default());
    let mut game = common::empty_game();
    game.base_dir = root;
    let output = compiler::compile_with_options(
        document.vmf(),
        &game,
        Options {
            physics: PhysicsMode::Engine,
            ..Options::default()
        },
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.physics_convexes, 14);
    assert_eq!(document.vmf().write(&Default::default()), before);
}

#[test]
fn engine_worker_preserves_curved_carved_fragments_from_supporting_planes() {
    let Some(root) = installed_game() else {
        eprintln!("CS:S not installed; skipping native curved-fragment fixtures");
        return;
    };
    let fixtures = [
        r#"
world { "id" "1" "classname" "worldspawn" }
entity
{
    "id" "282"
    "classname" "func_detail"
    "targetname" "detail_pumpkin_front"
    solid
    {
        "id" "150"
        side
        {
            "id" "1096"
            "plane" "(382.7129026269652 29.566191478555055 262.6972118647972) (390.86761704288983 60 262.6972118647972) (390.86761704288983 60 230.26278813520275)"
            "material" "DEV/DEV_MEASUREGENERIC01"
            "uaxis" "[0 1 0 0] 3"
            "vaxis" "[0 0 -1 0] 3"
            "rotation" "0"
            "lightmapscale" "16"
            "smoothing_groups" "0"
        }
        side
        {
            "id" "1098"
            "plane" "(355.9570448490476 159.37840270747085 -16.56441888656122) (371.9570448490475 99.66558978636878 0.000000000000048096310517768615) (367.6698577701496 115.66558978636873 61.81925288250044)"
            "material" "DEV/DEV_MEASUREGENERIC01"
            "uaxis" "[0 1 0 0] 3"
            "vaxis" "[0 0 -1 0] 3"
            "rotation" "0"
            "lightmapscale" "16"
            "smoothing_groups" "0"
        }
        side
        {
            "id" "1102"
            "plane" "(200.5793085538404 -136.5793085538406 263.60330066355834) (189.38632235212313 -189.3863223521233 229.2210908218954) (136.5793085538404 -200.57930855384058 263.60330066355834)"
            "material" "DEV/DEV_MEASUREGENERIC01"
            "uaxis" "[1 0 0 0] 3"
            "vaxis" "[0 -1 0 0] 3"
            "rotation" "0"
            "lightmapscale" "16"
            "smoothing_groups" "0"
        }
        side
        {
            "id" "1103"
            "plane" "(138.9165013247127 -202.9165013247127 -45.254833995939045) (170.9165013247127 -170.9165013247127 0) (138.9165013247127 -202.9165013247127 45.254833995939045)"
            "material" "DEV/DEV_MEASUREGENERIC01"
            "uaxis" "[1 0 0 0] 3"
            "vaxis" "[0 0 -1 0] 3"
            "rotation" "0"
            "lightmapscale" "16"
            "smoothing_groups" "0"
        }
        side
        {
            "id" "1435"
            "plane" "(155.43827202076292 -9.403912719979957 356.7829597521494) (153.55204595610203 53.93356986061436 347.79377449508047) (94.92224251666595 55.81979592527524 373.386513697559)"
            "material" "CS_ITALY/YELLOW"
            "uaxis" "[1 0 0 0] 0.25"
            "vaxis" "[0 -1 0 0] 0.25"
            "rotation" "0"
            "lightmapscale" "16"
            "smoothing_groups" "0"
        }
    }
}
"#,
        r#"
world { "id" "1" "classname" "worldspawn" }
entity
{
    "id" "282"
    "classname" "func_detail"
    "targetname" "detail_pumpkin_front"
    solid
    {
        "id" "209"
        side
        {
            "id" "1479"
            "plane" "(360.4338085214449 7.287097373034754 230.26278813520275) (382.7129026269652 29.566191478555055 230.26278813520275) (362.57836546912813 41.19087192664084 204.02278813520277)"
            "material" "DEV/DEV_MEASUREGENERIC01"
            "uaxis" "[1 0 0 0] 3"
            "vaxis" "[0 -1 0 0] 3"
            "rotation" "0"
            "lightmapscale" "16"
            "smoothing_groups" "0"
        }
        side
        {
            "id" "1480"
            "plane" "(382.7129026269652 29.566191478555055 230.26278813520275) (390.86761704288983 60 230.26278813520275) (367.6182561467183 60 204.02278813520277)"
            "material" "DEV/DEV_MEASUREGENERIC01"
            "uaxis" "[0 1 0 0] 3"
            "vaxis" "[0 0 -1 0] 3"
            "rotation" "0"
            "lightmapscale" "16"
            "smoothing_groups" "0"
        }
        side
        {
            "id" "1481"
            "plane" "(109.01088766879462 -34.79062068294675 -104.21275694948159) (103.41439456793601 27.70980351023184 -91.62799470918566) (60.300791164549125 33.306296611090445 -138.59496679114457)"
            "material" "DEV/DEV_MEASUREGENERIC01"
            "uaxis" "[0 1 0 0] 3"
            "vaxis" "[0 0 -1 0] 3"
            "rotation" "0"
            "lightmapscale" "16"
            "smoothing_groups" "0"
        }
        side
        {
            "id" "1482"
            "plane" "(-23.725313608487394 24.544075812380264 106.91353463854338) (-18.128820507628756 67.65767921576713 59.94656255658444) (-80.62924470080733 62.061186114908494 47.36180031628846)"
            "material" "DEV/DEV_MEASUREGENERIC01"
            "uaxis" "[1 0 0 0] 3"
            "vaxis" "[0 0 -1 0] 3"
            "rotation" "0"
            "lightmapscale" "16"
            "smoothing_groups" "0"
        }
        side
        {
            "id" "1483"
            "plane" "(202.9165013247127 -138.9165013247127 45.254833995939045) (170.9165013247127 -170.9165013247127 0) (138.9165013247127 -202.9165013247127 45.254833995939045)"
            "material" "DEV/DEV_MEASUREGENERIC01"
            "uaxis" "[1 0 0 0] 3"
            "vaxis" "[0 0 -1 0] 3"
            "rotation" "0"
            "lightmapscale" "16"
            "smoothing_groups" "0"
        }
        side
        {
            "id" "1484"
            "plane" "(28.38243167456983 35.61756832543017 -55.187215150255575) (17.18944547285255 -17.18944547285255 -20.80500530859262) (-35.617568325430184 -28.38243167456983 -55.187215150255575)"
            "material" "DEV/DEV_MEASUREGENERIC01"
            "uaxis" "[1 0 0 0] 3"
            "vaxis" "[0 -1 0 0] 3"
            "rotation" "0"
            "lightmapscale" "16"
            "smoothing_groups" "0"
        }
        side
        {
            "id" "1485"
            "plane" "(0 64 231) (0 0 231) (-64 0 231)"
            "material" "CS_ITALY/YELLOW"
            "uaxis" "[1 0 0 0] 0.25"
            "vaxis" "[0 -1 0 0] 0.25"
            "rotation" "0"
            "lightmapscale" "16"
            "smoothing_groups" "0"
        }
        side
        {
            "id" "1486"
            "plane" "(0 -64 219) (0 0 219) (-64 0 219)"
            "material" "CS_ITALY/YELLOW"
            "uaxis" "[1 0 0 0] 0.25"
            "vaxis" "[0 -1 0 0] 0.25"
            "rotation" "0"
            "lightmapscale" "16"
            "smoothing_groups" "0"
        }
        side
        {
            "id" "1487"
            "plane" "(370 64 0) (370 0 0) (370 0 -64)"
            "material" "CS_ITALY/YELLOW"
            "uaxis" "[0 1 0 0] 0.25"
            "vaxis" "[0 0 -1 0] 0.25"
            "rotation" "0"
            "lightmapscale" "16"
            "smoothing_groups" "0"
        }
    }
}
"#,
        r#"
world { "id" "1" "classname" "worldspawn" }
entity
{
    "id" "285"
    "classname" "func_detail"
    "targetname" "detail_misc"
    solid
    {
        "id" "149"
        side { "id" "1084" "plane" "(348.80912807335915 27.421634530871863 288.93721186479723) (362.57836546912813 41.19087192664084 288.93721186479723) (382.7129026269652 29.566191478555055 262.6972118647972)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 3" "vaxis" "[0 -1 0 0] 3" }
        side { "id" "1085" "plane" "(360.4338085214449 7.2870973730347615 262.6972118647972) (382.7129026269652 29.566191478555055 262.6972118647972) (382.7129026269652 29.566191478555055 230.26278813520275)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 3" "vaxis" "[0 0 -1 0] 3" }
        side { "id" "1086" "plane" "(362.57836546912813 41.19087192664084 288.93721186479723) (367.6182561467183 60 288.93721186479723) (390.86761704288983 60 262.6972118647972)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[0 1 0 0] 3" "vaxis" "[0 0 -1 0] 3" }
        side { "id" "1087" "plane" "(382.7129026269652 29.566191478555055 262.6972118647972) (390.86761704288983 60 262.6972118647972) (390.86761704288983 60 230.26278813520275)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[0 1 0 0] 3" "vaxis" "[0 0 -1 0] 3" }
        side { "id" "1088" "plane" "(333.0432097365025 153.2386590935652 287.459317982193) (338.63970283736114 90.73823490038663 300.044080222489) (295.52609943397425 96.33472800124528 347.0110523044479)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[0 1 0 0] 3" "vaxis" "[0 0 -1 0] 3" }
        side { "id" "1089" "plane" "(355.9570448490476 159.37840270747085 -16.56441888656122) (371.9570448490475 99.66558978636878 0.000000000000048096310517768615) (367.6698577701496 115.66558978636873 61.81925288250044)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[0 1 0 0] 3" "vaxis" "[0 0 -1 0] 3" }
        side { "id" "1090" "plane" "(50.49610398338425 -124.45402565027105 195.4364950386774) (44.89961088252561 -167.5676290536579 148.46952295671846) (-17.600813310652967 -173.16412215451655 161.05428519701445)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 3" "vaxis" "[0 0 -1 0] 3" }
        side { "id" "1091" "plane" "(36.25216027546995 -71.29490403218392 61.8192528825004) (20.252160275469944 -75.58209111108191 0.00000000000002606193214334677) (-39.460652645632095 -91.58209111108191 16.564418886561363)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 3" "vaxis" "[0 0 -1 0] 3" }
        side { "id" "1092" "plane" "(63.3802929681205 0.6197070318794928 331.08190978587936) (62.228287169493136 -62.22828716949314 319.04851843081406) (-0.619707031879507 -63.38029296812051 331.08190978587936)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 3" "vaxis" "[0 -1 0 0] 3" }
        side { "id" "1093" "plane" "(178.19333615040586 -242.193336150406 194.8388809802324) (189.38632235212313 -189.3863223521233 229.2210908218954) (136.5793085538404 -200.57930855384058 263.60330066355834)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 3" "vaxis" "[0 -1 0 0] 3" }
        side { "id" "1423" "plane" "(155.43827202076292 -9.403912719979957 356.7829597521494) (153.55204595610203 53.93356986061436 347.79377449508047) (94.92224251666595 55.81979592527524 373.386513697559)" "material" "CS_ITALY/YELLOW" "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 -1 0 0] 0.25" }
    }
}
"#,
    ];
    for fixture in fixtures {
        let fragment = source_formats::vmf::Vmf::parse(fixture).unwrap();
        let solid = fragment.entities[0].solids[0].id;
        let mut source = common::room();
        for brush in &mut source.world.solids {
            for side in &mut brush.sides {
                for point in &mut side.plane.0 {
                    *point *= 2.0;
                }
            }
        }
        source.entities.extend(fragment.entities);
        let before = source.write(&Default::default());
        let mut game = common::empty_game();
        game.base_dir = root.clone();
        let output = compiler::compile_with_options(
            &source,
            &game,
            Options {
                physics: PhysicsMode::Engine,
                ..Options::default()
            },
        );
        assert!(
            !output.report.has_errors(),
            "solid {solid}: {:?}",
            output.report
        );
        assert_eq!(output.stats.physics_convexes, 7, "solid {solid}");
        assert_eq!(output.stats.physics_solids, 1, "solid {solid}");
        let bsp = output.bsp.as_ref().unwrap();
        let bytes = bsp.to_bytes();
        let reopened = Bsp::parse(&bytes).unwrap();
        let physics = reopened.physics().unwrap().unwrap();
        assert_eq!(physics.models.len(), 1, "solid {solid}");
        assert_eq!(physics.models[0].solids.len(), 1, "solid {solid}");
        assert!(matches!(
            physics.models[0].solids[0].header(),
            CollisionHeader::Convex { .. }
        ));
        assert_eq!(
            physics.to_bytes(1).unwrap(),
            bsp.lump(lump::PHYSCOLLIDE).data,
            "solid {solid}"
        );
        assert_eq!(source.write(&Default::default()), before, "solid {solid}");
    }
}

#[test]
fn engine_worker_preserves_tiny_carved_fragment_with_explicit_topology() {
    let Some(root) = installed_game() else {
        eprintln!("CS:S not installed; skipping native carved-fragment fixture");
        return;
    };
    let fragment = source_formats::vmf::Vmf::parse(
        r#"
world { "id" "1" "classname" "worldspawn" }
entity
{
    "id" "285"
    "classname" "func_detail"
    "targetname" "detail_misc"
    solid
    {
        "id" "202"
        side
        {
            "id" "1424"
            "plane" "(50.49610398338425 -124.45402565027105 195.4364950386774) (44.89961088252561 -167.5676290536579 148.46952295671846) (-17.600813310652967 -173.16412215451655 161.05428519701445)"
            "material" "DEV/DEV_MEASUREGENERIC01"
            "uaxis" "[1 0 0 0] 3"
            "vaxis" "[0 0 -1 0] 3"
        }
        side
        {
            "id" "1425"
            "plane" "(178.19333615040586 -242.193336150406 194.8388809802324) (189.38632235212313 -189.3863223521233 229.2210908218954) (136.5793085538404 -200.57930855384058 263.60330066355834)"
            "material" "DEV/DEV_MEASUREGENERIC01"
            "uaxis" "[1 0 0 0] 3"
            "vaxis" "[0 -1 0 0] 3"
        }
        side
        {
            "id" "1426"
            "plane" "(151.66581989144115 117.27105244120868 338.80458923801154) (153.55204595610203 53.93356986061436 347.79377449508047) (94.92224251666595 55.81979592527524 373.386513697559)"
            "material" "CS_ITALY/YELLOW"
            "uaxis" "[1 0 0 0] 0.25"
            "vaxis" "[0 -1 0 0] 0.25"
        }
        side
        {
            "id" "1427"
            "plane" "(329.5547724251598 -121.87635466144334 11.070249698994754) (340.4581561282255 -59.79120510516356 0) (338.5433056845053 -70.69458880822923 -63.03530416839365)"
            "material" "CS_ITALY/YELLOW"
            "uaxis" "[0 1 0 0] 0.25"
            "vaxis" "[0 0 -1 0] 0.25"
        }
    }
}
"#,
    )
    .unwrap();
    let geometry = BrushGeometry::from_solid(&fragment.entities[0].solids[0]).unwrap();
    assert!(geometry.mins.abs_diff_eq(
        DVec3::new(354.05544963974535, 18.171157652242833, 264.57813850585126),
        1e-6,
    ));
    assert!(geometry.maxs.abs_diff_eq(
        DVec3::new(354.1877291737219, 18.386599059779208, 264.791323203739),
        1e-6,
    ));
    let mut source = common::room();
    for solid in &mut source.world.solids {
        for side in &mut solid.sides {
            for point in &mut side.plane.0 {
                *point *= 2.0;
            }
        }
    }
    source.entities.extend(fragment.entities);
    let before = source.write(&Default::default());
    let mut game = common::empty_game();
    game.base_dir = root;
    let output = compiler::compile_with_options(
        &source,
        &game,
        Options {
            physics: PhysicsMode::Engine,
            ..Options::default()
        },
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.physics_convexes, 7);
    assert_eq!(output.stats.physics_solids, 1);
    let bsp = output.bsp.as_ref().unwrap();
    let reopened = Bsp::parse(&bsp.to_bytes()).unwrap();
    let physics = reopened.physics().unwrap().unwrap();
    assert_eq!(physics.models.len(), 1);
    assert_eq!(physics.models[0].solids.len(), 1);
    assert!(matches!(
        physics.models[0].solids[0].header(),
        CollisionHeader::Convex { .. }
    ));
    assert_eq!(
        physics.to_bytes(1).unwrap(),
        bsp.lump(lump::PHYSCOLLIDE).data
    );
    let directory = tempfile::tempdir().unwrap();
    let previous = directory.path().join("previous.bsp");
    std::fs::write(&previous, b"previous BSP").unwrap();
    assert!(output.write(&previous).unwrap());
    assert_eq!(std::fs::read(previous).unwrap(), bsp.to_bytes());
    assert_eq!(source.write(&Default::default()), before);
}

#[test]
fn engine_worker_emits_checked_world_clip_and_local_brush_bodies() {
    let Some(root) = installed_game() else {
        eprintln!("CS:S not installed; skipping native worker fixture");
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let materials = directory.path().join("materials");
    std::fs::create_dir(&materials).unwrap();
    std::fs::create_dir(materials.join("dev")).unwrap();
    std::fs::write(
        materials.join("dev/dev_measuregeneric01.vmt"),
        "LightmappedGeneric { }",
    )
    .unwrap();
    std::fs::write(
        materials.join("clip.vmt"),
        "UnlitGeneric { %compileclip 1 $surfaceprop metal }",
    )
    .unwrap();
    std::fs::write(
        materials.join("ice.vmt"),
        "LightmappedGeneric { $surfaceprop ice }",
    )
    .unwrap();
    let scripts = directory.path().join("scripts");
    std::fs::create_dir(&scripts).unwrap();
    std::fs::write(
        scripts.join("surfaceproperties_manifest.txt"),
        "surfaceproperties_manifest { file scripts/mass_surfaces.txt }",
    )
    .unwrap();
    std::fs::write(
        scripts.join("mass_surfaces.txt"),
        "default { density 2000 } ice { density 917 } metal { density 2700 }",
    )
    .unwrap();
    let mut fs = common::empty_game();
    fs.base_dir = root;
    fs.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().into()),
    });
    let mut source = common::room();
    let top = BrushGeometry::from_solid(&source.world.solids[0])
        .unwrap()
        .faces
        .into_iter()
        .find(|face| face.plane.normal.z > 0.99)
        .unwrap()
        .side;
    source.world.solids[0].sides[top].material = "ice".into();
    let mut document = Document::new(source);
    document
        .edit("physics fixtures", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-32.0, -32.0, 0.0),
                DVec3::new(32.0, 32.0, 48.0),
                "clip",
            )?;
            let ObjectId::Solid(body) = edit::create_block(
                tx,
                DVec3::new(80.0, 80.0, 16.0),
                DVec3::new(112.0, 112.0, 48.0),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            edit::tie_to_entity(
                tx,
                &[body],
                "func_brush",
                &[("origin".into(), "96 96 32".into())],
            )?;
            Ok(())
        })
        .unwrap();
    let before = document.vmf().write(&Default::default());
    let options = Options {
        physics: PhysicsMode::Engine,
        ..Options::default()
    };
    let output = compiler::compile_with_options(document.vmf(), &fs, options);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.physics_mode, PhysicsMode::Engine);
    assert_eq!(output.stats.physics_models, 2);
    assert_eq!(output.stats.physics_solids, 4);
    assert_eq!(output.stats.physics_convexes, 9);
    let bsp = output.bsp.unwrap();
    let physics = bsp.physics().unwrap().unwrap();
    assert_eq!(physics.models.len(), 2);
    assert_eq!(physics.models[0].solids.len(), 3);
    assert_eq!(physics.models[1].solids.len(), 1);
    assert!(
        physics
            .models
            .iter()
            .flat_map(|model| &model.solids)
            .all(|solid| matches!(solid.header(), CollisionHeader::Convex { .. }))
    );
    let world_keys = std::str::from_utf8(physics.models[0].key_data).unwrap();
    assert!(
        world_keys.contains("\"contents\" \"65536\"")
            && world_keys.contains("\"contents\" \"131072\"")
    );
    assert!(world_keys.contains("materialtable"));
    assert!(world_keys.contains("\"ice\" \"2\"") && world_keys.contains("\"metal\" \"3\""));
    assert_eq!(output.stats.physics_surface_properties, 3);
    let body_keys = std::str::from_utf8(physics.models[1].key_data).unwrap();
    assert!(body_keys.contains("solid {"));
    let keys = source_formats::keyvalues::parse(body_keys.trim_end_matches('\0')).unwrap();
    let mass = keys[0]
        .children()
        .unwrap()
        .iter()
        .find(|field| field.key == "mass")
        .unwrap()
        .as_str()
        .unwrap()
        .parse::<f64>()
        .unwrap();
    assert!((mass - 1073.942626304).abs() < 0.001, "{mass}");
    let model = bsp.records::<Model>().unwrap()[1];
    assert_eq!(model.mins, [-16.0; 3]);
    assert_eq!(model.maxs, [16.0; 3]);
    assert_eq!(
        output.stats.physics_bytes,
        bsp.lump(lump::PHYSCOLLIDE).data.len()
    );
    assert_eq!(
        Bsp::parse(&bsp.to_bytes())
            .unwrap()
            .physics()
            .unwrap()
            .unwrap()
            .to_bytes(2)
            .unwrap(),
        bsp.lump(lump::PHYSCOLLIDE).data
    );
    assert_eq!(document.vmf().write(&Default::default()), before);
    let repeated = compiler::compile_with_options(document.vmf(), &fs, options)
        .bsp
        .unwrap();
    assert_eq!(
        repeated.lump(lump::PHYSCOLLIDE).data,
        bsp.lump(lump::PHYSCOLLIDE).data
    );
    let mut uniform = document.vmf().clone();
    let body = uniform
        .entities
        .iter_mut()
        .find(|entity| entity.classname() == Some("func_brush"))
        .unwrap();
    let model_id = body.id;
    for side in &mut body.solids[0].sides {
        side.material = "ice".into();
    }
    let uniform_output = compiler::compile_with_options(&uniform, &fs, options);
    assert!(
        !uniform_output.report.has_errors(),
        "{:?}",
        uniform_output.report
    );
    let uniform_bsp = uniform_output.bsp.unwrap();
    let uniform_physics = uniform_bsp.physics().unwrap().unwrap();
    assert!(
        std::str::from_utf8(uniform_physics.models[1].key_data)
            .unwrap()
            .contains("\"surfaceprop\" \"ice\"")
    );
    assert_ne!(uniform_physics.models[1].solids, physics.models[1].solids);
    assert_eq!(uniform_output.stats.physics_surface_properties, 3);
    uniform
        .entities
        .iter_mut()
        .find(|entity| entity.id == model_id)
        .unwrap()
        .solids[0]
        .sides[0]
        .material = "dev/dev_measuregeneric01".into();
    let mixed = compiler::compile_with_options(&uniform, &fs, options);
    assert!(!mixed.report.has_errors(), "{:?}", mixed.report);
    assert!(
        !mixed
            .report
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.object.as_deref() == Some(&format!("entity:{model_id}"))
                    && diagnostic.message.contains("using default")
            )
    );
    let mixed_bsp = mixed.bsp.unwrap();
    let mixed_physics = mixed_bsp.physics().unwrap().unwrap();
    assert_eq!(
        mixed_physics.models[1].key_data,
        uniform_physics.models[1].key_data
    );
    assert_ne!(mixed_physics.models[1].solids, physics.models[1].solids);
    assert_ne!(
        mixed_physics.models[1].solids,
        uniform_physics.models[1].solids
    );
    std::fs::write(
        materials.join("ice.vmt"),
        "LightmappedGeneric { $surfaceprop \"bad/name\" }",
    )
    .unwrap();
    let guarded = directory.path().join("guarded.bsp");
    std::fs::write(&guarded, b"previous BSP").unwrap();
    let invalid = compiler::compile_with_options(document.vmf(), &fs, options);
    assert!(invalid.report.has_errors() && !invalid.write(&guarded).unwrap());
    assert!(
        invalid
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("surface property"))
    );
    assert_eq!(std::fs::read(guarded).unwrap(), b"previous BSP");
}

#[test]
fn absent_and_unverified_backends_preserve_previous_output() {
    let directory = tempfile::tempdir().unwrap();
    let mut fs = common::empty_game();
    fs.base_dir = directory.path().into();
    let before = compiler::compile(&common::room(), &fs)
        .bsp
        .unwrap()
        .to_bytes();
    let path = directory.path().join("previous.bsp");
    std::fs::write(&path, &before).unwrap();
    let options = Options {
        physics: PhysicsMode::Engine,
        ..Options::default()
    };
    let output = compiler::compile_with_options(&common::room(), &fs, options);
    assert!(output.report.has_errors() && output.bsp.is_none());
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let libraries = directory.path().join("bin/linux64");
    std::fs::create_dir_all(&libraries).unwrap();
    std::fs::write(libraries.join("vphysics.so"), "unverified").unwrap();
    let output = compiler::compile_with_options(&common::room(), &fs, options);
    assert!(output.report.has_errors() && output.bsp.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|message| message.message.contains("Unverified CS:S vphysics")),
        "{:?}",
        output.report
    );
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let request = directory.path().join("bad.json");
    std::fs::write(
        &request,
        r#"{"model_count":1,"models":[{"index":0,"bodies":[{"contents":33570827,"shapes":[{"brush":0,"corners":[[0,0,0],[1,0,0],[0,1,0],[1,1,0]],"planes":[[0,0,1,0],[0,0,-1,0],[1,0,0,1],[-1,0,0,0]],"faces":[[0,1,3,2],[2,3,1,0],[0,2,3,1],[1,3,2,0]]}]}]}]}"#,
    )
    .unwrap();
    let failure = directory.path().join("failure.json");
    let result = Command::new(env!("CARGO_BIN_EXE_hammerr-physics"))
        .arg(&libraries)
        .arg(request)
        .arg(directory.path().join("rejected.bin"))
        .arg(&failure)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("topology")
            && String::from_utf8_lossy(&result.stderr).contains("escapes its plane")
    );
    assert!(!directory.path().join("rejected.bin").exists());
    let failure: serde_json::Value =
        serde_json::from_slice(&std::fs::read(failure).unwrap()).unwrap();
    assert_eq!(failure["version"], 1);
    assert_eq!(failure["stage"], "request");
    assert!(failure["tag"].is_null());
}
