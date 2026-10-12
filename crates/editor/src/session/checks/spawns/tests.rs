use glam::{DMat3, EulerRot};
use map_core::checks::Issue;
use map_core::{Document, Plane};
use source_formats::gamefs::{Mount, MountedPath};
use source_formats::vmf::Entity;

use crate::session::{Actor, Session};

use super::*;

fn fixture() -> (Session, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let materials = directory.path().join("materials/checks");
    std::fs::create_dir_all(&materials).unwrap();
    for (name, text) in [
        ("solid", "LightmappedGeneric {}"),
        ("nodraw", "LightmappedGeneric { %compilenodraw true }"),
        ("playerclip", "LightmappedGeneric { %playerclip 1 }"),
        ("clip", "LightmappedGeneric { %compileclip 1 }"),
        ("npcclip", "LightmappedGeneric { %compilenpcclip 1 }"),
        ("water", "Water {}"),
        ("nonsolid", "LightmappedGeneric { %compilenonsolid 1 }"),
        ("glass", "LightmappedGeneric { $translucent 1 }"),
        ("skip", "LightmappedGeneric { %compileskip 1 }"),
        ("hint", "LightmappedGeneric { %compilehint 1 }"),
        ("trigger", "LightmappedGeneric { %compiletrigger 1 }"),
        ("origin", "LightmappedGeneric { %compileorigin 1 }"),
        ("areaportal", "LightmappedGeneric { %compileareaportal 1 }"),
        ("ladder", "LightmappedGeneric { %compileladder 1 }"),
        ("detail", "LightmappedGeneric { %compiledetail 1 }"),
        ("unsupported", "LightmappedGeneric { %compileunknown 1 }"),
        ("malformed", ""),
    ] {
        std::fs::write(materials.join(format!("{name}.vmt")), text).unwrap();
    }
    let game = GameFs {
        game_dir: directory.path().to_owned(),
        base_dir: directory.path().to_owned(),
        mounts: vec![MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(directory.path().to_owned()),
        }],
    };
    (Session::new(Ok(game)), directory)
}

fn block(session: &mut Session, mins: DVec3, maxs: DVec3, material: &str) -> u32 {
    let ObjectId::Solid(id) = session
        .create_block(Actor::Agent, mins, maxs, material, false)
        .unwrap()
    else {
        panic!("Expected a world brush");
    };
    id
}

fn spawn(session: &mut Session, classname: &str, origin: DVec3) -> u32 {
    let ObjectId::Entity(id) = session
        .create_entity(Actor::Agent, classname, Some(origin), BTreeMap::new())
        .unwrap()
    else {
        panic!("Expected a point spawn");
    };
    id
}

fn issues<'a>(session: &'a Session, code: &str) -> Vec<&'a Issue> {
    session
        .map_check_report()
        .unwrap()
        .result
        .issues
        .iter()
        .filter(|issue| issue.code == code)
        .collect()
}

#[test]
fn clear_room_and_floor_contact_preserve_map_and_history_for_both_actors() {
    let (mut session, _directory) = fixture();
    for (mins, maxs) in [
        ([-144.0, -144.0, -16.0], [144.0, 144.0, 0.0]),
        ([-144.0, -144.0, 72.0], [144.0, 144.0, 88.0]),
        ([-144.0, -144.0, 0.0], [-128.0, 144.0, 72.0]),
        ([128.0, -144.0, 0.0], [144.0, 144.0, 72.0]),
        ([-128.0, -144.0, 0.0], [128.0, -128.0, 72.0]),
        ([-128.0, 128.0, 0.0], [128.0, 144.0, 72.0]),
    ] {
        block(&mut session, mins.into(), maxs.into(), "checks/solid");
    }
    spawn(&mut session, TEAMS[0].0, DVec3::new(-32.0, 0.0, 0.0));
    spawn(&mut session, TEAMS[1].0, DVec3::new(32.0, 0.0, 0.0));
    let before = session.document.vmf().clone();
    let revision = session.document.revision();
    let history = session
        .document
        .undo_labels()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for actor in [Actor::User, Actor::Agent] {
        session.run_map_checks(actor);
        assert!(issues(&session, "spawn_clearance").is_empty());
        assert!(issues(&session, "spawn_origin").is_empty());
        assert!(issues(&session, "missing_team_spawn").is_empty());
        assert_eq!(issues(&session, "spawn_clearance_scope").len(), 1);
        assert_eq!(session.document.vmf(), &before);
        assert_eq!(session.document.revision(), revision);
        assert_eq!(
            session
                .document
                .undo_labels()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
            history
        );
    }
}

#[test]
fn occupied_hull_and_tight_ceiling_report_each_team_and_support_navigation() {
    let (mut session, _directory) = fixture();
    let terrorist = spawn(&mut session, TEAMS[0].0, DVec3::ZERO);
    let ct = spawn(&mut session, TEAMS[1].0, DVec3::new(128.0, 0.0, 0.0));
    block(
        &mut session,
        DVec3::new(-1.0, -1.0, 35.0),
        DVec3::new(1.0, 1.0, 37.0),
        "checks/solid",
    );
    block(
        &mut session,
        DVec3::new(100.0, -32.0, 71.0),
        DVec3::new(160.0, 32.0, 80.0),
        "checks/solid",
    );
    let before = session.document.vmf().clone();
    let token = session.run_map_checks(Actor::Agent);
    let report = session
        .map_checks_json(Some("warning"), "spawn_clearance", 0, 200)
        .unwrap();
    assert_eq!(report["matching"], 2);
    assert!(
        issues(&session, "spawn_clearance")
            .iter()
            .any(|issue| issue.location.object == Some(ObjectId::Entity(ct)))
    );
    let first = report["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|issue| issue["object"] == format!("entity:{terrorist}"))
        .unwrap();
    let index = first["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(first["position"], serde_json::json!([0.0, 0.0, 0.0]));
    assert_eq!(first["selectable"], true);
    session
        .select_map_issue(Actor::User, token, index, true, 1.6)
        .unwrap();
    assert_eq!(
        session.selection().iter().copied().collect::<Vec<_>>(),
        vec![ObjectId::Entity(terrorist)]
    );
    assert_eq!(session.document.vmf(), &before);
    assert!(!session.map_checks_stale());
    block(
        &mut session,
        DVec3::splat(300.0),
        DVec3::splat(316.0),
        "checks/solid",
    );
    assert!(
        session
            .select_map_issue(Actor::Agent, token, index, true, 1.6)
            .is_err()
    );
    assert_eq!(
        session
            .map_checks_json(None, "spawn_clearance", 0, 200)
            .unwrap()["stale"],
        true
    );
    session.run_map_checks(Actor::Agent);
    assert!(
        session
            .select_map_issue(Actor::Agent, token, index, false, 1.6)
            .is_err()
    );
}

#[test]
fn contact_tolerance_allows_touching_and_subtolerance_penetration() {
    for (mins, maxs, intersects) in [
        ([-32.0, -32.0, -16.0], [32.0, 32.0, 0.0], false),
        ([-32.0, -32.0, -16.0], [32.0, 32.0, 0.005], false),
        ([-32.0, -32.0, -16.0], [32.0, 32.0, 0.02], true),
        ([-32.0, -32.0, 72.0], [32.0, 32.0, 80.0], false),
        ([-32.0, -32.0, 71.995], [32.0, 32.0, 80.0], false),
        ([-32.0, -32.0, 71.98], [32.0, 32.0, 80.0], true),
        ([16.0, -32.0, 0.0], [32.0, 32.0, 80.0], false),
        ([15.98, -32.0, 0.0], [32.0, 32.0, 80.0], true),
    ] {
        let (mut session, _directory) = fixture();
        spawn(&mut session, TEAMS[0].0, DVec3::ZERO);
        block(&mut session, mins.into(), maxs.into(), "checks/solid");
        session.run_map_checks(Actor::Agent);
        assert_eq!(
            !issues(&session, "spawn_clearance").is_empty(),
            intersects,
            "{mins:?}..{maxs:?}"
        );
    }
}

#[test]
fn sloped_wedge_uses_convex_volume_instead_of_aabb_or_corner_sampling() {
    for (distance, intersects) in [(44.0, false), (24.0, true)] {
        let (mut session, _directory) = fixture();
        spawn(&mut session, TEAMS[0].0, DVec3::ZERO);
        block(
            &mut session,
            DVec3::new(0.0, 0.0, 8.0),
            DVec3::new(64.0, 64.0, 64.0),
            "checks/solid",
        );
        let mut vmf = session.document.vmf().clone();
        let normal = DVec3::new(-1.0, -1.0, -0.125).normalize();
        let winding = Plane {
            normal,
            dist: -distance / DVec3::new(1.0, 1.0, 0.125).length(),
        }
        .base_winding();
        let mut side = vmf.world.solids[0].sides[0].clone();
        side.id = 1000;
        side.plane.0 = [winding[0], winding[1], winding[2]];
        vmf.world.solids[0].sides.push(side);
        let geometry = checked_geometry(&vmf.world.solids[0]).unwrap();
        assert!(bounds_overlap(&geometry, HULL_MINS, HULL_MAXS));
        session.replace_document(Document::new(vmf), None);
        session.run_map_checks(Actor::Agent);
        assert_eq!(!issues(&session, "spawn_clearance").is_empty(), intersects);
    }
}

#[test]
fn oblique_brush_edges_supply_separating_axes_missing_from_face_only_checks() {
    let (mut session, _directory) = fixture();
    spawn(&mut session, TEAMS[0].0, DVec3::ZERO);
    let half = DVec3::new(27.088334980664015, 25.411704535849363, 4.64679599801705);
    block(&mut session, -half, half, "checks/solid");
    let mut vmf = session.document.vmf().clone();
    let rotation = DMat3::from_euler(
        EulerRot::ZYX,
        -0.9399245898094697,
        -0.22302486445956582,
        1.024869800337406,
    );
    let center = DVec3::new(
        11.688228968221686,
        0.035328664732951154,
        -25.846811383676304,
    );
    for side in &mut vmf.world.solids[0].sides {
        for point in &mut side.plane.0 {
            *point = rotation * *point + center;
        }
    }
    let geometry = checked_geometry(&vmf.world.solids[0]).unwrap();
    let mut face_axes = vec![DVec3::X, DVec3::Y, DVec3::Z];
    face_axes.extend(geometry.faces.iter().map(|face| face.plane.normal));
    assert!(bounds_overlap(&geometry, HULL_MINS, HULL_MAXS));
    assert!(intersects_box(
        &project_axes(&geometry, &face_axes),
        HULL_MINS,
        HULL_MAXS
    ));
    assert!(!intersects_box(
        &project_axes(&geometry, &separating_axes(&geometry)),
        HULL_MINS,
        HULL_MAXS
    ));
    session.replace_document(Document::new(vmf), None);
    session.run_map_checks(Actor::Agent);
    assert!(issues(&session, "spawn_clearance").is_empty());
}

#[test]
fn compile_contents_distinguish_player_clips_water_nonsolid_tools_and_mixed_faces() {
    for (material, mixed, blocks) in [
        ("solid", false, true),
        ("nodraw", false, true),
        ("playerclip", false, true),
        ("clip", false, true),
        ("npcclip", false, false),
        ("water", false, false),
        ("nonsolid", false, false),
        ("glass", false, true),
        ("skip", false, false),
        ("hint", false, false),
        ("trigger", false, false),
        ("origin", false, false),
        ("areaportal", false, false),
        ("ladder", false, false),
        ("detail", false, true),
        ("playerclip", true, true),
        ("npcclip", true, false),
        ("water", true, false),
        ("nonsolid", true, false),
        ("glass", true, true),
        ("skip", true, true),
        ("detail", true, true),
    ] {
        let (mut session, _directory) = fixture();
        spawn(&mut session, TEAMS[0].0, DVec3::ZERO);
        block(
            &mut session,
            DVec3::new(-8.0, -8.0, 8.0),
            DVec3::new(8.0, 8.0, 64.0),
            &format!("checks/{material}"),
        );
        if mixed {
            let mut vmf = session.document.vmf().clone();
            for side in &mut vmf.world.solids[0].sides[1..] {
                side.material = "checks/nodraw".into();
            }
            session.replace_document(Document::new(vmf), None);
        }
        session.run_map_checks(Actor::Agent);
        assert_eq!(
            !issues(&session, "spawn_clearance").is_empty(),
            blocks,
            "{material}, mixed={mixed}"
        );
    }
}

#[test]
fn hidden_content_and_other_brush_entities_are_excluded_but_func_detail_blocks() {
    let (mut session, _directory) = fixture();
    spawn(&mut session, TEAMS[0].0, DVec3::ZERO);
    block(
        &mut session,
        DVec3::new(-8.0, -8.0, 8.0),
        DVec3::new(8.0, 8.0, 64.0),
        "checks/playerclip",
    );
    let original = session.document.vmf().world.solids[0].clone();
    for (classname, entity_hidden, solid_hidden, blocks) in [
        ("worldspawn", false, true, false),
        ("func_detail", false, false, true),
        ("func_detail", true, false, false),
        ("func_detail", false, true, false),
        ("func_door", false, false, false),
        ("func_brush", false, false, false),
        ("trigger_multiple", false, false, false),
        ("func_buyzone", false, false, false),
        ("func_bomb_target", false, false, false),
        ("func_hostage_rescue", false, false, false),
    ] {
        let mut vmf = session.document.vmf().clone();
        vmf.world.solids.clear();
        vmf.entities.retain(|entity| entity.id != 1000);
        let mut solid = original.clone();
        solid.hidden = solid_hidden;
        if classname == "worldspawn" {
            vmf.world.solids.push(solid);
        } else {
            let mut entity = Entity::new(1000, classname);
            entity.hidden = entity_hidden;
            entity.solids.push(solid);
            vmf.entities.push(entity);
        }
        session.replace_document(Document::new(vmf), None);
        session.run_map_checks(Actor::Agent);
        assert_eq!(
            !issues(&session, "spawn_clearance").is_empty(),
            blocks,
            "{classname}, entity hidden={entity_hidden}, brush hidden={solid_hidden}"
        );
    }
}

#[test]
fn unknown_materials_and_invalid_brushes_are_unchecked_without_guessed_blockers() {
    for material in ["missing", "unsupported", "malformed"] {
        let (mut session, _directory) = fixture();
        spawn(&mut session, TEAMS[0].0, DVec3::ZERO);
        block(
            &mut session,
            DVec3::new(-8.0, -8.0, 8.0),
            DVec3::new(8.0, 8.0, 64.0),
            &format!("checks/{material}"),
        );
        session.run_map_checks(Actor::Agent);
        assert!(issues(&session, "spawn_clearance").is_empty(), "{material}");
        assert_eq!(issues(&session, "spawn_clearance_unchecked").len(), 1);
    }
    let (mut session, _directory) = fixture();
    spawn(&mut session, TEAMS[0].0, DVec3::ZERO);
    block(
        &mut session,
        DVec3::new(-8.0, -8.0, 8.0),
        DVec3::new(8.0, 8.0, 64.0),
        "checks/solid",
    );
    let mut vmf = session.document.vmf().clone();
    vmf.world.solids[0].sides.pop();
    session.replace_document(Document::new(vmf), None);
    session.run_map_checks(Actor::Agent);
    assert!(issues(&session, "spawn_clearance").is_empty());
    assert_eq!(issues(&session, "spawn_clearance_unchecked").len(), 1);
    session.game_error = Some("unavailable".into());
    session.run_map_checks(Actor::Agent);
    assert_eq!(issues(&session, "spawn_clearance_unchecked").len(), 1);
    assert!(
        issues(&session, "spawn_clearance_unchecked")[0]
            .message
            .contains("Game content is unavailable")
    );
}

#[test]
fn missing_invalid_duplicated_origins_and_parented_spawns_remain_unchecked() {
    let (mut session, _directory) = fixture();
    let mut vmf = session.document.vmf().clone();
    for (id, origin) in [
        (100, None),
        (101, Some("0 0")),
        (102, Some("NaN 0 0")),
        (103, Some("16385 0 0")),
    ] {
        let mut entity = Entity::new(id, TEAMS[0].0);
        if let Some(origin) = origin {
            entity.properties.set("origin", origin);
        }
        vmf.entities.push(entity);
    }
    let mut duplicate = Entity::new(104, TEAMS[1].0);
    duplicate.properties.0.extend([
        ("origin".into(), "0 0 0".into()),
        ("ORIGIN".into(), "64 0 0".into()),
    ]);
    vmf.entities.push(duplicate);
    let mut parented = Entity::new(105, TEAMS[1].0);
    parented.properties.set("origin", "0 0 0");
    parented.properties.set("parentname", "moving_parent");
    vmf.entities.push(parented);
    session.replace_document(Document::new(vmf), None);
    session.run_map_checks(Actor::Agent);
    assert_eq!(issues(&session, "spawn_origin").len(), 5);
    assert_eq!(issues(&session, "spawn_clearance_unchecked").len(), 1);
    assert!(issues(&session, "spawn_clearance").is_empty());
    assert!(issues(&session, "missing_team_spawn").is_empty());
}

#[test]
fn duplicate_spawn_ids_keep_clearance_readable_and_reject_ambiguous_navigation() {
    let (mut session, _directory) = fixture();
    let id = spawn(&mut session, TEAMS[0].0, DVec3::ZERO);
    block(
        &mut session,
        DVec3::new(-8.0, -8.0, 8.0),
        DVec3::new(8.0, 8.0, 64.0),
        "checks/solid",
    );
    let mut vmf = session.document.vmf().clone();
    vmf.entities.push(vmf.entities[0].clone());
    session.replace_document(Document::new(vmf), None);
    let token = session.run_map_checks(Actor::Agent);
    let report = session
        .map_checks_json(None, "spawn_clearance", 0, 200)
        .unwrap();
    let blocked = report["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|issue| issue["code"] == "spawn_clearance")
        .collect::<Vec<_>>();
    assert_eq!(blocked.len(), 2);
    assert!(
        blocked
            .iter()
            .all(|issue| issue["object"] == format!("entity:{id}")
                && issue["ambiguous"] == true
                && issue["selectable"] == false)
    );
    let index = blocked[0]["id"].as_str().unwrap().parse().unwrap();
    assert!(
        session
            .select_map_issue(Actor::Agent, token, index, true, 1.6)
            .is_err()
    );
}

#[test]
fn hidden_team_spawns_do_not_satisfy_presence_or_produce_clearance_issues() {
    let (mut session, _directory) = fixture();
    spawn(&mut session, TEAMS[0].0, DVec3::ZERO);
    let mut vmf = session.document.vmf().clone();
    vmf.entities[0].hidden = true;
    session.replace_document(Document::new(vmf), None);
    session.run_map_checks(Actor::Agent);
    assert_eq!(issues(&session, "missing_team_spawn").len(), 2);
    assert!(issues(&session, "spawn_clearance_scope").is_empty());
}

#[test]
fn redundant_materials_do_not_change_collision_and_blocker_details_are_bounded() {
    let (mut session, _directory) = fixture();
    spawn(&mut session, TEAMS[0].0, DVec3::ZERO);
    for _ in 0..6 {
        block(
            &mut session,
            DVec3::new(-8.0, -8.0, 8.0),
            DVec3::new(8.0, 8.0, 64.0),
            "checks/solid",
        );
    }
    let mut vmf = session.document.vmf().clone();
    let mut duplicate = vmf.world.solids[0].sides[0].clone();
    duplicate.id = 1000;
    duplicate.material = "checks/missing".into();
    vmf.world.solids[0].sides.push(duplicate);
    session.replace_document(Document::new(vmf), None);
    session.run_map_checks(Actor::Agent);
    assert!(issues(&session, "spawn_clearance_unchecked").is_empty());
    let blocked = issues(&session, "spawn_clearance");
    assert_eq!(blocked.len(), 1);
    assert_eq!(
        blocked[0].message.matches("brush ").count(),
        MAX_BLOCKER_NAMES
    );
    assert!(blocked[0].message.contains("and 2 more"));
}

#[test]
fn displacement_and_complex_geometry_are_explicitly_outside_collision_scope() {
    let (mut session, _directory) = fixture();
    spawn(&mut session, TEAMS[0].0, DVec3::ZERO);
    block(
        &mut session,
        DVec3::new(-8.0, -8.0, 8.0),
        DVec3::new(8.0, 8.0, 64.0),
        "checks/solid",
    );
    let source = session.document.vmf().clone();
    let mut vmf = source.clone();
    vmf.world.solids[0].sides[0].displacement = Some(source_formats::vmf::DispInfo {
        power: 2,
        start_position: DVec3::new(-8.0, -8.0, 64.0),
        flags: None,
        elevation: 0.0,
        subdivided: false,
        normals: None,
        distances: None,
        offsets: None,
        offset_normals: None,
        alphas: None,
        triangle_tags: None,
        allowed_verts: None,
        extra_properties: Default::default(),
        extra: Vec::new(),
    });
    session.replace_document(Document::new(vmf), None);
    session.run_map_checks(Actor::Agent);
    assert!(issues(&session, "spawn_clearance").is_empty());
    assert_eq!(issues(&session, "compiler_displacement").len(), 1);
    assert!(
        issues(&session, "spawn_clearance_scope")[0]
            .message
            .contains("displacements")
    );
    let mut vmf = source;
    while vmf.world.solids[0].sides.len() <= MAX_BRUSH_SIDES {
        let mut duplicate = vmf.world.solids[0].sides[0].clone();
        duplicate.id = 1000 + vmf.world.solids[0].sides.len() as u32;
        vmf.world.solids[0].sides.push(duplicate);
    }
    session.replace_document(Document::new(vmf), None);
    session.run_map_checks(Actor::Agent);
    assert!(issues(&session, "spawn_clearance").is_empty());
    assert_eq!(issues(&session, "spawn_clearance_unchecked").len(), 1);
    assert!(
        issues(&session, "spawn_clearance_unchecked")[0]
            .message
            .contains("256")
    );
}
