use super::*;

const KEY: &str = "_detail_lighting_tint";

#[test]
fn generated_detail_tint_preserves_skewed_sculpted_terrain_alpha_and_samples() {
    let mut fixture = fixture();
    let solid = add_scatter_terrain(&mut fixture, 3, 1);
    skew_terrain(&mut fixture.source, solid);
    std::fs::write(fixture.directory.path().join("detail.vbsp"), "detail { tufts { density 8192 low { alpha 0 a { model models/props/test.mdl upright 0 } } high { alpha 1 b { model models/props/other.mdl upright 0 } } } }").unwrap();
    let reference = compiled(&fixture.source, &fixture.game).bsp.unwrap();
    let original = reference.detail_props().unwrap().unwrap();
    assert!(original.props.iter().any(|prop| prop.model == 0));
    assert!(original.props.iter().any(|prop| prop.model == 1));
    fixture
        .source
        .world
        .properties
        .set(compiler::GENERATED_DETAIL_LIGHTING_TINT_KEY, "128 32 0");
    let before = fixture.source.write(&Default::default());
    let tinted = compiled(&fixture.source, &fixture.game).bsp.unwrap();
    assert_native_terrain_contacts(&tinted);
    let mut actual = tinted.detail_props().unwrap().unwrap();
    assert_eq!(actual.props.len(), original.props.len());
    for (new, old) in actual.props.iter_mut().zip(&original.props) {
        assert_eq!(new.lighting.rgb, [128, 32, 0]);
        assert_eq!(new.lighting.exponent, 0);
        new.lighting = old.lighting;
    }
    assert_eq!(actual, original);
    for index in 0..bsp::LUMP_COUNT {
        assert_eq!(tinted.lump(index), reference.lump(index));
    }
    assert_eq!(fixture.source.write(&Default::default()), before);
}

#[test]
fn dictionary_entry_tints_follow_alpha_groups_when_terrain_models_share_one_asset() {
    let mut fixture = fixture();
    let solid = add_scatter_terrain(&mut fixture, 3, 1);
    skew_terrain(&mut fixture.source, solid);
    let dictionary_path = fixture.directory.path().join("detail.vbsp");
    std::fs::write(&dictionary_path, "detail { tufts { density 8192 low { alpha 0 a { model models/props/test.mdl upright 0 } } high { alpha 1 b { model models/props/other.mdl upright 0 } } } }").unwrap();
    let reference = compiled(&fixture.source, &fixture.game).bsp.unwrap();
    let original = reference.detail_props().unwrap().unwrap();
    assert_eq!(original.models.len(), 2);
    fixture
        .source
        .world
        .properties
        .set(compiler::GENERATED_DETAIL_LIGHTING_TINT_KEY, "128 64 7");
    std::fs::write(&dictionary_path, "detail { tufts { density 8192 low { alpha 0 a { model models/props/test.mdl upright 0 _detail_lighting_tint \"129 127 64\" } } high { alpha 1 b { model models/props/test.mdl upright 0 _detail_lighting_tint \"64 128 255\" } } } }").unwrap();
    let source_before = fixture.source.write(&Default::default());
    let dictionary_before = std::fs::read(&dictionary_path).unwrap();
    let tinted = compiled(&fixture.source, &fixture.game).bsp.unwrap();
    assert_native_terrain_contacts(&tinted);
    let actual = tinted.detail_props().unwrap().unwrap();
    assert_eq!(actual.models.len(), 1);
    assert_eq!(actual.sprites, original.sprites);
    assert_eq!(actual.props.len(), original.props.len());
    let mut seen = [false; 2];
    for (&new, &old) in actual.props.iter().zip(&original.props) {
        let (group, rgb, exponent) = match original.model_name(usize::from(old.model)).unwrap() {
            "models/props/test.mdl" => (0, [129, 63, 3], -1),
            "models/props/other.mdl" => (1, [128, 128, 28], -2),
            path => panic!("Unexpected reference asset {path}"),
        };
        seen[group] = true;
        assert_eq!(new.model, 0);
        assert_eq!(new.lighting, bsp::RgbExp32 { rgb, exponent });
        let mut normalized = new;
        normalized.model = old.model;
        normalized.lighting = old.lighting;
        assert_eq!(normalized, old);
    }
    assert_eq!(seen, [true; 2]);
    for index in 0..bsp::LUMP_COUNT {
        assert_eq!(tinted.lump(index), reference.lump(index));
    }
    assert_eq!(fixture.source.write(&Default::default()), source_before);
    assert_eq!(std::fs::read(&dictionary_path).unwrap(), dictionary_before);
}

fn assert_dictionary_error(fixture: &Fixture, text: &str, expected: &str, previous: &[u8]) {
    let path = fixture.directory.path().join("detail.vbsp");
    let destination = fixture.directory.path().join("previous.bsp");
    std::fs::write(&path, text).unwrap();
    std::fs::write(&destination, previous).unwrap();
    let source = fixture.source.write(&Default::default());
    let output = compiler::compile(&fixture.source, &fixture.game);
    assert!(output.bsp.is_none() && output.report.has_errors(), "{text}");
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|row| row.message.contains(expected)),
        "{text}: {:?}",
        output.report
    );
    assert!(!output.write(&destination).unwrap());
    assert_eq!(std::fs::read(&destination).unwrap(), previous);
    assert_eq!(fixture.source.write(&Default::default()), source);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
}

#[test]
fn selected_dictionary_entry_tint_errors_preserve_previous_output_and_source() {
    let fixture = fixture();
    let previous = compiled(&fixture.source, &fixture.game)
        .bsp
        .unwrap()
        .to_bytes();
    for invalid in [
        "", "1 2", "1 2 3 4", "-1 0 0", "256 0 0", "1.5 0 0", "NaN 0 0",
    ] {
        let text = format!(
            "detail {{ tufts {{ density 64 group {{ alpha 1 item {{ model {MODEL} amount 0 {KEY} \"{invalid}\" }} }} }} }}"
        );
        assert_dictionary_error(&fixture, &text, "three integer linear RGB", &previous);
    }
    for (fields, expected) in [
        (
            "_detail_lighting_tint \"128 64 0\" _DETAIL_LIGHTING_TINT \"255 255 255\"",
            "repeats _detail_lighting_tint",
        ),
        (
            "_detail_lighting_tint { value \"128 64 0\" }",
            "_detail_lighting_tint must be a value",
        ),
        (
            "rendercolor \"128 64 0\"",
            "field rendercolor is unsupported",
        ),
    ] {
        let text = format!(
            "detail {{ tufts {{ density 64 group {{ alpha 1 item {{ model {MODEL} {fields} }} }} }} }}"
        );
        assert_dictionary_error(&fixture, &text, expected, &previous);
    }
    for (text, expected) in [
        (
            format!(
                "detail {{ {KEY} \"128 64 0\" tufts {{ density 64 group {{ alpha 1 item {{ model {MODEL} }} }} }} }}"
            ),
            "_detail_lighting_tint is only supported inside an entry",
        ),
        (
            format!(
                "detail {{ tufts {{ density 64 {KEY} \"128 64 0\" group {{ alpha 1 item {{ model {MODEL} }} }} }} }}"
            ),
            "Unsupported detail type field _detail_lighting_tint",
        ),
        (
            format!(
                "detail {{ tufts {{ density 64 group {{ alpha 1 {KEY} \"128 64 0\" item {{ model {MODEL} }} }} }} }}"
            ),
            "Unsupported detail group field _detail_lighting_tint",
        ),
    ] {
        assert_dictionary_error(&fixture, &text, expected, &previous);
    }
}

#[test]
fn dictionary_entry_tint_keeps_unused_groups_and_block_labels_legal() {
    let mut fixture = fixture();
    let path = fixture.directory.path().join("detail.vbsp");
    let reference = compiled(&fixture.source, &fixture.game)
        .bsp
        .unwrap()
        .to_bytes();
    std::fs::write(&path, format!("detail {{ tufts {{ density 64 low {{ alpha 0 item {{ model {MODEL} {KEY} \"1.5 0 0\" }} }} high {{ alpha 1 item {{ model {MODEL} upright 1 minAngle 0 maxAngle 0 }} }} }} }}")).unwrap();
    assert_eq!(
        compiled(&fixture.source, &fixture.game)
            .bsp
            .unwrap()
            .to_bytes(),
        reference
    );

    std::fs::write(
        fixture.directory.path().join("materials/test/scatter.vmt"),
        format!("UnlitGeneric {{ %detailtype {KEY} }}"),
    )
    .unwrap();
    std::fs::write(&path, format!("detail {{ {KEY} {{ density 64 {KEY} {{ alpha 1 {KEY} {{ model {MODEL} upright 1 minAngle 0 maxAngle 0 {KEY} \"255 255 255\" }} }} }} }}")).unwrap();
    assert_eq!(
        compiled(&fixture.source, &fixture.game)
            .bsp
            .unwrap()
            .to_bytes(),
        reference
    );

    std::fs::write(
        fixture.directory.path().join("materials/test/scatter.vmt"),
        "UnlitGeneric { %detailtype tufts }",
    )
    .unwrap();
    let solid = add_scatter_terrain(&mut fixture, 3, 1);
    let text = format!(
        "detail {{ tufts {{ density 8192 low {{ alpha 0 item {{ model {MODEL} {KEY} \"1.5 0 0\" }} }} high {{ alpha 1 item {{ model {MODEL} upright 1 }} }} }} }}"
    );
    std::fs::write(&path, &text).unwrap();
    for row in terrain_info_mut(&mut fixture.source, solid)
        .alphas
        .as_mut()
        .unwrap()
    {
        row.fill(255.0);
    }
    let high = compiled(&fixture.source, &fixture.game).bsp.unwrap();
    assert!(!high.detail_props().unwrap().unwrap().props.is_empty());
    for row in terrain_info_mut(&mut fixture.source, solid)
        .alphas
        .as_mut()
        .unwrap()
    {
        row.fill(0.0);
    }
    assert_dictionary_error(
        &fixture,
        &text,
        "three integer linear RGB",
        &high.to_bytes(),
    );

    std::fs::write(
        fixture.directory.path().join("materials/test/scatter.vmt"),
        "UnlitGeneric { }",
    )
    .unwrap();
    let no_request = compiled(&fixture.source, &fixture.game)
        .bsp
        .unwrap()
        .to_bytes();
    std::fs::write(&path, format!("detail {{ {KEY} \"128 64 0\" }}")).unwrap();
    assert_eq!(
        compiled(&fixture.source, &fixture.game)
            .bsp
            .unwrap()
            .to_bytes(),
        no_request
    );
}

#[test]
fn dictionary_entry_tint_keeps_func_detail_surface_geometry_and_world_tint_bytes() {
    let mut fixture = fixture();
    fixture.source.world.solids[0]
        .sides
        .iter_mut()
        .find(|side| side.id == fixture.floor_side)
        .unwrap()
        .material = "dev/dev_measuregeneric01".into();
    let mut document = Document::new(fixture.source);
    document
        .edit("detail scatter surface", |tx| {
            let ObjectId::Solid(solid) = edit::create_block(
                tx,
                DVec3::new(-64.0, -64.0, 0.0),
                DVec3::new(64.0, 64.0, 16.0),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            tx.solid_mut(solid)?
                .sides
                .iter_mut()
                .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                .unwrap()
                .material = MATERIAL.into();
            edit::tie_to_entity(tx, &[solid], "func_detail", &[])?;
            Ok(())
        })
        .unwrap();
    fixture.source = document.vmf().clone();
    dictionary(&fixture, 1024, &format!("model {MODEL} upright 1"));
    fixture
        .source
        .world
        .properties
        .set(compiler::GENERATED_DETAIL_LIGHTING_TINT_KEY, "128 64 7");
    let reference = compiled(&fixture.source, &fixture.game).bsp.unwrap();
    assert!(!reference.detail_props().unwrap().unwrap().props.is_empty());
    fixture
        .source
        .world
        .properties
        .remove(compiler::GENERATED_DETAIL_LIGHTING_TINT_KEY);
    dictionary(
        &fixture,
        1024,
        &format!("model {MODEL} upright 1 {KEY} \"128 64 7\""),
    );
    assert_eq!(
        compiled(&fixture.source, &fixture.game)
            .bsp
            .unwrap()
            .to_bytes(),
        reference.to_bytes()
    );
}
