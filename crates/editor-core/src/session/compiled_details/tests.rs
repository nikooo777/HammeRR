use std::path::PathBuf;

use map_core::{ObjectId, query::Ray};
use source_formats::vmf::Entity;

use super::*;
use crate::session::{SelectMode, compilation::BuildSettingsUpdate};

const ATLAS: &str = "UnlitGeneric { $basetexture test/atlas $translucent 1 $nocull 1 $vertexcolor 1 $vertexalpha 1 }";

fn fixture() -> (tempfile::TempDir, Session, PathBuf) {
    fixture_with_atlas(Some("test/atlas"))
}

fn fixture_with_atlas(declaration: Option<&str>) -> (tempfile::TempDir, Session, PathBuf) {
    let (directory, game) = crate::test_assets::model_lighting::game();
    std::fs::create_dir_all(directory.path().join("maps")).unwrap();
    for name in ["one", "two"] {
        std::fs::write(
            directory
                .path()
                .join(format!("materials/models/test/{name}.vmt")),
            "UnlitGeneric { $basetexture test/checker }",
        )
        .unwrap();
    }
    std::fs::write(directory.path().join("materials/test/atlas.vmt"), ATLAS).unwrap();
    let mut atlas = std::fs::read(directory.path().join("materials/test/checker.vtf")).unwrap();
    atlas[16..18].copy_from_slice(&32u16.to_le_bytes());
    atlas.truncate(80 + 32 * 32 * 4);
    std::fs::create_dir_all(directory.path().join("materials/detail")).unwrap();
    std::fs::write(
        directory.path().join("materials/detail/detailsprites.vmt"),
        ATLAS.replace("test/atlas", "detail/detailsprites"),
    )
    .unwrap();
    std::fs::write(
        directory.path().join("materials/detail/detailsprites.vtf"),
        &atlas,
    )
    .unwrap();
    std::fs::write(directory.path().join("materials/test/atlas.vtf"), atlas).unwrap();
    std::fs::write(
        directory.path().join("materials/test/ground.vmt"),
        "UnlitGeneric { %detailtype tufts }",
    )
    .unwrap();
    std::fs::write(directory.path().join("detail.vbsp"),"detail { tufts { density 128 group { alpha 1 item { sprite \"0 0 4 4 8\" spritesize \"0.25 0.5 16 32\" detailOrientation 2 upright 1 sway 0.5 } } } }").unwrap();
    let mut vmf = crate::test_assets::room();
    for side in vmf
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.material = "test/ground".into();
    }
    match declaration {
        Some(name) => vmf.world.properties.set("detailmaterial", name),
        None => {
            vmf.world.properties.remove("detailmaterial");
        }
    }
    for (id, kind, origin) in [
        (9000, "", "0 0 64"),
        (9001, "cross", "64 0 64"),
        (9002, "tri", "-64 0 64"),
    ] {
        let mut entity = Entity::new(id, "prop_detail_sprite");
        for (key, value) in [
            ("origin", origin),
            ("position_ul", "-4 16"),
            ("position_lr", "12 -16"),
            ("tex_ul", "0 0"),
            ("tex_size", "4 4"),
            ("tex_total_size", "8"),
            ("scale", "2"),
            ("detailOrientation", "0"),
        ] {
            entity.properties.set(key, value);
        }
        if !kind.is_empty() {
            entity.properties.set("sprite_shape", kind);
        }
        vmf.entities.push(entity);
    }
    let mut model = Entity::new(9003, "prop_detail");
    for (key, value) in [
        ("origin", "0 64 64"),
        ("angles", "0 90 0"),
        ("model", "models/test/receiver.mdl"),
    ] {
        model.properties.set(key, value);
    }
    vmf.entities.push(model);
    let source = directory.path().join("maps/details.vmf");
    std::fs::write(&source, vmf.write(&Default::default())).unwrap();
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &source, false).unwrap();
    session
        .update_build_settings(
            Actor::User,
            BuildSettingsUpdate {
                lighting: Some("fullbright"),
                physics: Some("none"),
                generate_reflections: Some(false),
                pack_sky_assets: Some(false),
                pack_water_assets: Some(false),
                pack_static_prop_assets: Some(false),
                bake_static_prop_vertices: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    let completed = session.begin_compile(Actor::User, None).unwrap().run();
    session.finish_compile(Actor::User, &completed);
    assert!(
        completed.as_ref().unwrap().published(),
        "{:?}",
        session.compile_status()
    );
    assert_eq!(
        baked_build::build_state_controlled(&session, SOURCE_MODE, &Control::default()).unwrap()["status"],
        "current"
    );
    (directory, session, source)
}

fn prepare(session: &mut Session) -> u64 {
    let request = session.begin_compiled_details(Actor::User).unwrap();
    let token = request.token();
    let report = session
        .finish_compiled_details(Actor::User, request.run())
        .unwrap();
    assert_eq!(report["status"], "prepared", "{report}");
    token
}

#[test]
fn final_record_layer_preserves_authored_selection_picking_and_outline_controls() {
    let (_directory, mut session, _source) = fixture();
    let bsp = Bsp::read(&session.compiled.as_ref().unwrap().path).unwrap();
    let native = bsp.detail_props().unwrap().unwrap();
    assert!(
        native.props.len() > 4,
        "fixture must include actual generated records"
    );
    let token = prepare(&mut session);
    let report = session
        .inspect_compiled_details(Some(token), 0, 64)
        .unwrap();
    assert_eq!(report["page"]["total"], native.props.len());
    assert!(
        report["page"]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["source_association"].is_null())
    );
    for (row, prop) in report["page"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .zip(&native.props)
    {
        assert_eq!(row["origin"], json!(prop.origin));
        assert_eq!(row["leaf"], prop.leaf);
        assert_eq!(row["kind"], prop.kind);
    }
    assert!(session.inspect_compiled_details(None, 1, 1).is_err());
    assert!(
        session
            .inspect_compiled_details(Some(token + 1), 0, 1)
            .is_err()
    );
    assert!(
        session
            .inspect_compiled_details(Some(token), native.props.len() + 1, 1)
            .is_err()
    );
    assert_eq!(
        session
            .inspect_compiled_details(Some(token), native.props.len(), 1)
            .unwrap()["page"]["returned"],
        0
    );
    let selected = ObjectId::Solid(session.document.vmf().world.solids[0].id);
    session.select(&[selected], SelectMode::Replace).unwrap();
    let source_bytes = session.document.vmf().write(&Default::default());
    let revision = session.document.revision();
    let camera = session.camera();
    let ray = Ray {
        origin: glam::DVec3::new(0.0, 0.0, 128.0),
        direction: glam::DVec3::NEG_Z,
    };
    let hit = session.pick(&ray).map(|hit| hit.object);
    let ordinary = session.build_scene();
    let outline = ordinary.selection_outline_ranges.clone();
    let ordinary_vertices = ordinary.vertices.clone();
    let shown = session
        .set_compiled_details_visible(Actor::User, token, true)
        .unwrap();
    assert_eq!(shown["complete"], true);
    assert_eq!(shown["displayed"], true);
    let displayed = session.build_scene();
    assert!(displayed.vertices.len() > ordinary_vertices.len());
    assert_eq!(
        &displayed.vertices[..ordinary_vertices.len()],
        ordinary_vertices.as_slice()
    );
    assert_eq!(displayed.selection_outline_ranges, outline);
    assert_eq!(session.pick(&ray).map(|hit| hit.object), hit);
    assert_eq!(
        session.selection().iter().copied().collect::<Vec<_>>(),
        [selected]
    );
    assert_eq!(
        session.document.vmf().write(&Default::default()),
        source_bytes
    );
    assert_eq!(session.document.revision(), revision);
    assert_eq!(session.camera(), camera);
    let stamp = session.working_region_stamp();
    session
        .set_working_region(
            Actor::User,
            crate::session::working_region::Action::Set {
                enabled: true,
                bounds: [glam::DVec3::splat(10_000.0), glam::DVec3::splat(10_016.0)],
            },
            &stamp,
        )
        .unwrap();
    assert!(session.build_scene().indices.is_empty());
    let report = session.inspect_compiled_details(Some(token), 0, 1).unwrap();
    assert_eq!(report["displayed"], true);
    assert_eq!(report["view_policy"]["region_records"], 0);
    let stamp = session.working_region_stamp();
    session
        .set_working_region(
            Actor::User,
            crate::session::working_region::Action::Set {
                enabled: true,
                bounds: [
                    glam::DVec3::new(-2.0, -2.0, 63.0),
                    glam::DVec3::new(2.0, 2.0, 65.0),
                ],
            },
            &stamp,
        )
        .unwrap();
    assert!(!session.build_scene().indices.is_empty());
    let report = session.inspect_compiled_details(Some(token), 0, 1).unwrap();
    assert!(report["view_policy"]["region_records"].as_u64().unwrap() > 0);
    assert!(report["view_policy"]["region_records"].as_u64().unwrap() < native.props.len() as u64);
    let stamp = session.working_region_stamp();
    session
        .set_working_region(
            Actor::User,
            crate::session::working_region::Action::Clear,
            &stamp,
        )
        .unwrap();
    let stamp = session.working_region_stamp();
    session
        .set_editor_visibility(
            Actor::User,
            crate::session::editor_visibility::Action::Hide(vec![selected]),
            &stamp,
        )
        .unwrap();
    session.preflight_compiled_details();
    assert_eq!(
        session.inspect_compiled_details(Some(token), 0, 1).unwrap()["displayed"],
        true
    );
    let stamp = session.working_region_stamp();
    session
        .set_editor_visibility(
            Actor::User,
            crate::session::editor_visibility::Action::ShowAll,
            &stamp,
        )
        .unwrap();
    assert_eq!(session.document.revision(), revision);
    session.set_face_mode(true);
    session.preflight_compiled_details();
    assert_eq!(
        session.inspect_compiled_details(Some(token), 0, 1).unwrap()["status"],
        "context_suppressed"
    );
    session.set_face_mode(false);
    session.preflight_compiled_details();
    assert_eq!(
        session.inspect_compiled_details(Some(token), 0, 1).unwrap()["displayed"],
        true
    );
    session
        .set_compiled_details_visible(Actor::User, token, false)
        .unwrap();
    assert_eq!(
        session.build_scene().vertices.len(),
        ordinary_vertices.len()
    );
}

#[test]
fn cancelled_expired_and_stale_requests_cannot_restore_a_complete_layer() {
    let (_directory, mut session, _source) = fixture();
    let request = session.begin_compiled_details(Actor::Agent).unwrap();
    let token = request.token();
    assert!(session.begin_compiled_details(Actor::User).is_err());
    session
        .cancel_compiled_details(Actor::Agent, token)
        .unwrap();
    let cancelled = session
        .finish_compiled_details(Actor::Agent, request.run())
        .unwrap();
    assert_eq!(cancelled["status"], "cancelled");
    assert_eq!(cancelled["complete"], false);
    let request = session.begin_compiled_details(Actor::Agent).unwrap();
    session
        .clear_compiled_details(Actor::User, request.token())
        .unwrap();
    assert!(
        session
            .finish_compiled_details(Actor::Agent, request.run())
            .is_err()
    );
    let token = prepare(&mut session);
    session
        .set_compiled_details_visible(Actor::User, token, true)
        .unwrap();
    session
        .edit(Actor::User, "Change committed detail source", |tx| {
            tx.world_properties_mut()
                .set("detailmaterial", "test/other");
            Ok(())
        })
        .unwrap();
    let stale = session
        .inspect_compiled_details(Some(token), 0, 64)
        .unwrap();
    assert_eq!(stale["status"], "stale");
    assert_eq!(stale["displayed"], false);
    assert!(stale["page"]["total"].as_u64().unwrap() > 0);
    assert!(
        session
            .set_compiled_details_visible(Actor::Agent, token, true)
            .is_err()
    );
    assert!(session.begin_compiled_details(Actor::Agent).is_err());
}

#[test]
fn preview_texture_observation_is_separate_and_output_replacement_retires_rows() {
    let (directory, mut session, _source) = fixture();
    let token = prepare(&mut session);
    session
        .set_compiled_details_visible(Actor::User, token, true)
        .unwrap();
    let texture = directory.path().join("materials/test/checker.vtf");
    let mut bytes = std::fs::read(&texture).unwrap();
    bytes[80] ^= 0xff;
    std::fs::write(&texture, bytes).unwrap();
    let build =
        baked_build::build_state_controlled(&session, SOURCE_MODE, &Control::default()).unwrap();
    assert_eq!(
        build["current"], true,
        "Model preview textures are not compiler-consumed in this fixture: {build}"
    );
    let stale = session.inspect_compiled_details(Some(token), 0, 1).unwrap();
    assert_eq!(stale["status"], "stale");
    assert_eq!(stale["displayed"], false);
    assert_eq!(stale["build"]["current"], true);
    assert_eq!(stale["preview_assets"]["status"], "stale");
    assert!(
        stale["preview_assets"]["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic
                .as_str()
                .unwrap()
                .contains("Preview asset observations"))
    );
    let path = session.compiled.as_ref().unwrap().path.clone();
    std::fs::write(path, b"replacement output").unwrap();
    let report = session.inspect_compiled_details(Some(token), 0, 1).unwrap();
    assert_eq!(report["status"], "unavailable");
    assert!(report["page"]["total"].is_null());
    assert_eq!(report["complete"], false);
}

#[test]
fn unreadable_preview_only_texture_refuses_the_whole_geometry_set() {
    let (directory, mut session, _source) = fixture();
    let token = prepare(&mut session);
    let source = session.document.vmf().write(&Default::default());
    let ordinary = session.build_scene().vertices.len();
    std::fs::write(
        directory.path().join("materials/test/checker.vtf"),
        b"unsupported preview texture",
    )
    .unwrap();
    assert_eq!(
        baked_build::build_state_controlled(&session, SOURCE_MODE, &Control::default()).unwrap()["current"],
        true
    );
    assert!(
        session
            .set_compiled_details_visible(Actor::Agent, token, true)
            .is_err()
    );
    let report = session.inspect_compiled_details(Some(token), 0, 1).unwrap();
    assert_eq!(report["status"], "unavailable");
    assert_eq!(report["complete"], false);
    assert_eq!(report["displayed"], false);
    assert!(report["error"].as_str().unwrap().contains("VTF"));
    assert_eq!(session.build_scene().vertices.len(), ordinary);
    assert_eq!(session.document.vmf().write(&Default::default()), source);
}

#[test]
fn absent_and_empty_native_atlas_declarations_compile_prepare_and_show_default_assets() {
    assert_eq!(DetailProps::sprite_material_name(Some(" ")), " ");
    assert_eq!(
        DetailProps::sprite_material_name(Some(" test/atlas ")),
        " test/atlas "
    );
    for declaration in [None, Some("")] {
        let (_directory, mut session, source) = fixture_with_atlas(declaration);
        let original = std::fs::read(&source).unwrap();
        let bsp = Bsp::read(&session.compiled.as_ref().unwrap().path).unwrap();
        let blocks = source_formats::keyvalues::parse_flat_blocks_bounded(
            bsp.entities().unwrap(),
            FlatBlockLimits {
                max_bytes: 1024 * 1024,
                max_blocks: 8192,
                max_pairs: 65_536,
            },
        )
        .unwrap();
        let world = blocks
            .iter()
            .find(|block| {
                block
                    .iter()
                    .any(|(key, value)| key == "classname" && value == "worldspawn")
            })
            .unwrap();
        let emitted = world
            .iter()
            .find(|(key, _)| key == "detailmaterial")
            .map(|(_, value)| value.as_str());
        assert_eq!(emitted, declaration);
        let native = bsp.detail_props().unwrap().unwrap();
        assert!(native.props.len() > 4);
        let token = prepare(&mut session);
        let report = session
            .inspect_compiled_details(Some(token), 0, 64)
            .unwrap();
        assert_eq!(report["sprite_material"], "detail/detailsprites");
        assert_eq!(report["page"]["total"], native.props.len());
        let shown = session
            .set_compiled_details_visible(Actor::User, token, true)
            .unwrap();
        assert_eq!(shown["complete"], true, "{shown}");
        assert_eq!(shown["displayed"], true, "{shown}");
        let atlas = session.materials.find("detail/detailsprites").unwrap();
        assert_eq!(
            atlas.display_texture.as_deref(),
            Some("materials/detail/detailsprites.vtf")
        );
        assert_eq!(atlas.base.as_ref().unwrap().width, 32);
        assert!(!session.build_scene().indices.is_empty());
        assert_eq!(std::fs::read(source).unwrap(), original);
    }
}
