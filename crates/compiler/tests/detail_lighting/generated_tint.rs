use super::*;

const KEY: &str = compiler::GENERATED_DETAIL_LIGHTING_TINT_KEY;

fn generated_fixture() -> Fixture {
    let mut fixture = fixture();
    tint::atlas(&mut fixture);
    fixture
        .source
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("_detail_lighting_tint", "64 255 128");
    std::fs::write(
        fixture.directory.path().join("materials/test/ground.vmt"),
        "UnlitGeneric { %detailtype tufts }",
    )
    .unwrap();
    fixture.source.world.solids[0]
        .sides
        .iter_mut()
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .material = "test/ground".into();
    let mut entries = String::from("model { model models/props/test.mdl amount 0.2 upright 1 }");
    for (name, orientation, extra, amount) in [
        ("fixed", 0, "", "0.2"),
        ("full", 1, "", "0.2"),
        ("vertical", 2, "", "0.2"),
        ("cross", 0, "sprite_shape cross", "0.1"),
        (
            "tri",
            0,
            "sprite_shape tri shape_angle 35 shape_size 0.5",
            "0.1",
        ),
    ] {
        entries.push_str(&format!(" {name} {{ sprite \"0 0 8 8 16\" spritesize \"0.5 0 8 16\" spriterandomscale 0.2 detailOrientation {orientation} amount {amount} {extra} }}"));
    }
    std::fs::write(
        fixture.directory.path().join("detail.vbsp"),
        format!("detail {{ tufts {{ density 512 group {{ alpha 1 {entries} }} }} }}"),
    )
    .unwrap();
    for (style, ldr, hdr) in [
        ("0", "255 255 255 128", "255 255 255 64"),
        ("1", "255 255 255 120", "0 0 0 0"),
        ("2", "0 0 0 0", "255 255 255 80"),
    ] {
        entity(
            &mut fixture.source,
            "light",
            "0.5 0.5 180",
            &[
                ("style", style),
                ("_light", ldr),
                ("_lightHDR", hdr),
                ("_constant_attn", "1"),
                ("_quadratic_attn", "0"),
            ],
        );
    }
    fixture
}

fn assert_tinted(actual: &Bsp, reference: &Bsp, factors: [u8; 3]) {
    let old = reference.detail_props().unwrap().unwrap();
    let new = actual.detail_props().unwrap().unwrap();
    assert_eq!(new.models, old.models);
    assert_eq!(new.sprites, old.sprites);
    assert_eq!(new.props.len(), old.props.len());
    for (&new, &old) in new.props.iter().zip(&old.props) {
        let factors = if old.origin[2] == 16.0 {
            [255; 3]
        } else {
            factors
        };
        tint::assert_record(new, old, factors);
        for hdr in [false, true] {
            let old_styles = reference.detail_prop_lighting(hdr).unwrap();
            let new_styles = actual.detail_prop_lighting(hdr).unwrap();
            assert_eq!(new_styles.is_some(), old_styles.is_some());
            if let Some(old_styles) = old_styles {
                let new_styles = new_styles.unwrap();
                assert_eq!(new_styles.len(), old_styles.len());
                let start = old.light_styles as usize;
                for index in start..start + usize::from(old.light_style_count) {
                    assert_eq!(new_styles[index].style, old_styles[index].style);
                    tint::assert_color(
                        new_styles[index].lighting,
                        old_styles[index].lighting,
                        factors,
                    );
                }
            }
        }
    }
    for index in 0..source_formats::bsp::LUMP_COUNT {
        assert_eq!(actual.lump(index), reference.lump(index));
    }
}

#[test]
fn generated_detail_tint_keeps_white_bytes_rng_and_explicit_lighting_independent() {
    let mut fixture = generated_fixture();
    for mode in [
        LightingMode::Fullbright,
        LightingMode::DirectLdr,
        LightingMode::DirectLdrHdr,
    ] {
        fixture.source.world.properties.remove(KEY);
        let reference = compiled(&fixture, mode, 0);
        let details = reference.detail_props().unwrap().unwrap();
        let kinds = details
            .props
            .iter()
            .filter(|prop| prop.origin[2] == 0.0)
            .map(|prop| (prop.kind, prop.orientation))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            kinds,
            [(0, 0), (1, 0), (1, 1), (1, 2), (2, 0), (3, 0)]
                .into_iter()
                .collect()
        );
        assert!(details.props.iter().any(|prop| prop.scale != 1.0));
        fixture
            .source
            .world
            .properties
            .set(KEY.to_ascii_uppercase(), "255 255 255");
        assert_eq!(compiled(&fixture, mode, 0).to_bytes(), reference.to_bytes());
        fixture.source.world.properties.set(KEY, "128 64 0");
        let before = fixture.source.write(&Default::default());
        let tinted = compiled(&fixture, mode, 0);
        assert_tinted(&tinted, &reference, [128, 64, 0]);
        assert!(
            !tinted
                .entities()
                .unwrap()
                .to_ascii_lowercase()
                .contains(KEY)
        );
        assert_eq!(fixture.source.write(&Default::default()), before);
        if mode == LightingMode::Fullbright {
            assert!(
                tinted
                    .detail_props()
                    .unwrap()
                    .unwrap()
                    .props
                    .iter()
                    .filter(|prop| prop.origin[2] == 0.0)
                    .all(|prop| prop.lighting.rgb == [128, 64, 0] && prop.lighting.exponent == 0)
            );
        }
        if mode == LightingMode::DirectLdrHdr {
            fixture.source.world.properties.set(KEY, "0 0 0");
            let black = compiled(&fixture, mode, 0);
            assert_tinted(&black, &reference, [0; 3]);
            assert!(
                black
                    .detail_props()
                    .unwrap()
                    .unwrap()
                    .props
                    .iter()
                    .all(|prop| prop.light_style_count == 2)
            );
        }
    }
}

#[test]
fn generated_detail_tint_rejects_invalid_and_duplicate_world_values_even_without_details() {
    let mut fixture = fixture();
    fixture
        .source
        .entities
        .retain(|entity| entity.classname() != Some("prop_detail"));
    let previous = compiled(&fixture, LightingMode::Fullbright, 0).to_bytes();
    let destination = fixture.directory.path().join("previous.bsp");
    std::fs::write(&destination, &previous).unwrap();
    for invalid in [
        "", "1 2", "1 2 3 4", "-1 0 0", "256 0 0", "1.5 0 0", "NaN 0 0",
    ] {
        fixture.source.world.properties.set(KEY, invalid);
        let before = fixture.source.write(&Default::default());
        let output = compiler::compile(&fixture.source, &fixture.game);
        assert!(output.bsp.is_none() && output.report.has_errors());
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|row| row.message.contains(KEY)
                    && row.object == Some(format!("entity:{}", fixture.source.world.id)))
        );
        assert!(!output.write(&destination).unwrap());
        assert_eq!(std::fs::read(&destination).unwrap(), previous);
        assert_eq!(fixture.source.write(&Default::default()), before);
    }
    fixture.source.world.properties.set(KEY, "128 64 0");
    fixture
        .source
        .world
        .properties
        .0
        .push((KEY.to_ascii_uppercase(), "255 255 255".into()));
    let before = fixture.source.write(&Default::default());
    let imported = Vmf::parse(&before).unwrap();
    let output = compiler::compile(&imported, &fixture.game);
    assert!(output.bsp.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|row| row.message.contains("World repeats"))
    );
    assert!(!output.write(&destination).unwrap());
    assert_eq!(std::fs::read(&destination).unwrap(), previous);
    assert_eq!(imported.write(&Default::default()), before);
}
