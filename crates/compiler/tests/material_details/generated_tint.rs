use super::*;

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
