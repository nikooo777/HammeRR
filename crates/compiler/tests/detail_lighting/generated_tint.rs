use super::*;

const KEY: &str = compiler::GENERATED_DETAIL_LIGHTING_TINT_KEY;
const ENTRY_KEY: &str = "_detail_lighting_tint";

fn entry_fields(fixture: &Fixture, dictionary: &str, fields: &str) {
    assert_eq!(dictionary.matches(" amount ").count(), 6);
    std::fs::write(
        fixture.directory.path().join("detail.vbsp"),
        dictionary.replace(" amount ", &format!(" {fields} amount ")),
    )
    .unwrap();
}

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
    let old_ldr = reference.detail_prop_lighting(false).unwrap();
    for (&new, &old) in new.props.iter().zip(&old.props) {
        let factors = if old.origin[2] == 16.0 {
            [255; 3]
        } else {
            factors
        };
        let old_start = old.light_styles as usize;
        let old_end = old_start + usize::from(old.light_style_count);
        let new_start = new.light_styles as usize;
        let new_end = new_start + usize::from(new.light_style_count);
        let drops_static = factors == [0; 3]
            && old_ldr.as_ref().is_some_and(|records| {
                records[old_start..old_end]
                    .first()
                    .is_some_and(|record| record.style == 0)
            });
        assert_eq!(
            usize::from(new.light_style_count) + usize::from(drops_static),
            usize::from(old.light_style_count)
        );
        let mut comparable = new;
        comparable.light_styles = old.light_styles;
        comparable.light_style_count = old.light_style_count;
        tint::assert_record(comparable, old, factors);
        for hdr in [false, true] {
            let old_styles = reference.detail_prop_lighting(hdr).unwrap();
            let new_styles = actual.detail_prop_lighting(hdr).unwrap();
            assert_eq!(new_styles.is_some(), old_styles.is_some());
            if let Some(old_styles) = old_styles {
                let new_styles = new_styles.unwrap();
                let old_active = &old_styles[old_start..old_end];
                let new_active = &new_styles[new_start..new_end];
                let old_base = old_active
                    .first()
                    .filter(|record| record.style == 0)
                    .map_or(old.lighting, |record| record.lighting);
                let new_base = new_active
                    .first()
                    .filter(|record| record.style == 0)
                    .map_or(new.lighting, |record| record.lighting);
                tint::assert_color(new_base, old_base, factors);
                let expected = old_active
                    .iter()
                    .filter(|record| record.style != 0 || !drops_static)
                    .collect::<Vec<_>>();
                assert_eq!(new_active.len(), expected.len());
                for (actual, old) in new_active.iter().zip(expected) {
                    assert_eq!(actual.style, old.style);
                    tint::assert_color(actual.lighting, old.lighting, factors);
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
    let path = fixture.directory.path().join("detail.vbsp");
    let dictionary = std::fs::read_to_string(&path).unwrap();
    for mode in [
        LightingMode::Fullbright,
        LightingMode::DirectLdr,
        LightingMode::DirectLdrHdr,
    ] {
        std::fs::write(&path, &dictionary).unwrap();
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
        entry_fields(
            &fixture,
            &dictionary,
            "_DETAIL_LIGHTING_TINT \"255 255 255\"",
        );
        assert_eq!(compiled(&fixture, mode, 0).to_bytes(), reference.to_bytes());
        std::fs::write(&path, &dictionary).unwrap();
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
        entry_fields(
            &fixture,
            &dictionary,
            "_detail_lighting_tint \"255 255 255\"",
        );
        assert_eq!(compiled(&fixture, mode, 0).to_bytes(), tinted.to_bytes());
        fixture.source.world.properties.remove(KEY);
        entry_fields(&fixture, &dictionary, "_detail_lighting_tint \"128 64 0\"");
        let dictionary_before = std::fs::read(&path).unwrap();
        assert_eq!(compiled(&fixture, mode, 0).to_bytes(), tinted.to_bytes());
        assert_eq!(std::fs::read(&path).unwrap(), dictionary_before);
        std::fs::write(&path, &dictionary).unwrap();
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
                    .all(
                        |prop| prop.light_style_count == if prop.origin[2] == 16.0 { 3 } else { 2 }
                    )
            );
        }
    }
}

fn assert_product_color(actual: source_formats::bsp::RgbExp32, old: source_formats::bsp::RgbExp32) {
    let expected =
        DVec3::from_array(old.linear()) * (DVec3::new(16_512.0, 8_128.0, 448.0) / 65_025.0);
    let tolerance =
        2.0_f64.powi(i32::from(actual.exponent)) + 2.0_f64.powi(i32::from(old.exponent));
    let actual = DVec3::from_array(actual.linear());
    assert!(
        (actual - expected).abs().max_element() <= tolerance,
        "{actual} != {expected}"
    );
}

fn assert_product(actual: &Bsp, reference: &Bsp) {
    let old = reference.detail_props().unwrap().unwrap();
    let new = actual.detail_props().unwrap().unwrap();
    assert_eq!(new.models, old.models);
    assert_eq!(new.sprites, old.sprites);
    assert_eq!(new.props.len(), old.props.len());
    for (&new, &old) in new.props.iter().zip(&old.props) {
        let mut normalized = new;
        if old.origin[2] == 16.0 {
            assert_eq!(new.lighting, old.lighting);
        } else {
            assert_product_color(new.lighting, old.lighting);
        }
        normalized.lighting = old.lighting;
        assert_eq!(normalized, old);
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
                    if old.origin[2] == 16.0 {
                        assert_eq!(new_styles[index].lighting, old_styles[index].lighting);
                    } else {
                        assert_product_color(
                            new_styles[index].lighting,
                            old_styles[index].lighting,
                        );
                        if old_styles[index].lighting.rgb == [0; 3] {
                            assert_eq!(new_styles[index].lighting.rgb, [0; 3]);
                        }
                    }
                }
            }
        }
    }
    for index in 0..source_formats::bsp::LUMP_COUNT {
        assert_eq!(actual.lump(index), reference.lump(index));
    }
}

#[test]
fn dictionary_entry_tint_composes_fractional_fullbright_and_preserves_dual_styles() {
    let mut fixture = generated_fixture();
    let path = fixture.directory.path().join("detail.vbsp");
    let dictionary = std::fs::read_to_string(&path).unwrap();
    let fullbright = compiled(&fixture, LightingMode::Fullbright, 0);
    let dual = compiled(&fixture, LightingMode::DirectLdrHdr, 0);
    fixture.source.world.properties.set(KEY, "128 64 7");
    entry_fields(
        &fixture,
        &dictionary,
        "_detail_lighting_tint \"129 127 64\"",
    );
    let composed = compiled(&fixture, LightingMode::Fullbright, 0);
    assert_product(&composed, &fullbright);
    for prop in composed.detail_props().unwrap().unwrap().props {
        let expected = if prop.origin[2] == 16.0 {
            source_formats::bsp::RgbExp32 {
                rgb: [64, 255, 128],
                exponent: 0,
            }
        } else {
            source_formats::bsp::RgbExp32 {
                rgb: [129, 63, 3],
                exponent: -1,
            }
        };
        assert_eq!(prop.lighting, expected);
    }
    let composed = compiled(&fixture, LightingMode::DirectLdrHdr, 0);
    assert_product(&composed, &dual);
    let details = composed.detail_props().unwrap().unwrap();
    assert!(details.props.iter().all(|prop| prop.light_style_count == 3));
    for (hdr, absent_style) in [(false, 2), (true, 1)] {
        let styles = composed.detail_prop_lighting(hdr).unwrap().unwrap();
        assert!(styles.iter().any(|style| style.style == absent_style));
        assert!(
            styles
                .iter()
                .filter(|style| style.style == absent_style)
                .all(|style| style.lighting.rgb == [0; 3])
        );
    }
    entry_fields(&fixture, &dictionary, "_detail_lighting_tint \"0 0 0\"");
    let black = compiled(&fixture, LightingMode::DirectLdrHdr, 0);
    assert_tinted(&black, &dual, [0; 3]);
    assert!(
        black
            .detail_props()
            .unwrap()
            .unwrap()
            .props
            .iter()
            .all(|prop| prop.light_style_count == if prop.origin[2] == 16.0 { 3 } else { 2 })
    );
    assert!(!composed.entities().unwrap().contains(ENTRY_KEY));
}

#[test]
fn dictionary_entry_tint_is_applied_once_after_reflected_lighting() {
    let mut fixture = generated_fixture();
    let dictionary = std::fs::read_to_string(fixture.directory.path().join("detail.vbsp")).unwrap();
    std::fs::write(
        fixture.directory.path().join("materials/test/lit.vmt"),
        "LightmappedGeneric { }",
    )
    .unwrap();
    std::fs::write(
        fixture.directory.path().join("materials/test/ground.vmt"),
        "LightmappedGeneric { %detailtype tufts }",
    )
    .unwrap();
    for side in fixture
        .source
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        if side.material != "test/ground" {
            side.material = "test/lit".into();
        }
    }
    let direct = compiled(&fixture, LightingMode::DirectLdr, 0);
    let bounced = compiled(&fixture, LightingMode::DirectLdr, 1);
    let direct_details = direct.detail_props().unwrap().unwrap();
    let bounced_details = bounced.detail_props().unwrap().unwrap();
    assert!(
        bounced_details
            .props
            .iter()
            .zip(&direct_details.props)
            .any(|(new, old)| old.origin[2] == 0.0
                && new.lighting.linear()[0] > old.lighting.linear()[0])
    );
    fixture.source.world.properties.set(KEY, "128 64 7");
    let world = compiled(&fixture, LightingMode::DirectLdr, 1);
    fixture.source.world.properties.remove(KEY);
    entry_fields(&fixture, &dictionary, "_detail_lighting_tint \"128 64 7\"");
    assert_eq!(
        compiled(&fixture, LightingMode::DirectLdr, 1).to_bytes(),
        world.to_bytes()
    );
    fixture.source.world.properties.set(KEY, "128 64 7");
    entry_fields(
        &fixture,
        &dictionary,
        "_detail_lighting_tint \"129 127 64\"",
    );
    assert_product(&compiled(&fixture, LightingMode::DirectLdr, 1), &bounced);
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
