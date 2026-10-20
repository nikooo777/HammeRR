use super::*;

#[test]
fn replacing_a_valid_model_after_leaf_emission_rejects_the_old_receiver_snapshot() {
    let (directory, game) = fixtures::game();
    let vmf = fixtures::scene(false);
    let mut bsp = crate::compile_with_options(&vmf, &game, Options::default())
        .bsp
        .unwrap();
    let control = Control::default();
    let mut report = Report::default();
    let mut map = Map::load_with_prop_sources_controlled(
        &vmf,
        &game,
        crate::materials::PropSourceMode::Direct,
        &mut report,
        &control,
    )
    .unwrap();
    let props = crate::static_props::bake(
        &vmf,
        &game,
        &mut bsp,
        &mut report,
        &control,
        map.materials.prop_lighting_sources_mut(),
    )
    .unwrap();
    assert!(!report.has_errors(), "{report:?}");
    assert_eq!(props.props, 1);
    let source_reads = map.materials.take_prop_lighting_sources().unwrap();
    let previous = bsp.to_bytes();

    let path = directory.path().join("models/props/test.mdl");
    let original = std::fs::read(&path).unwrap();
    let mut replacement = original.clone();
    replacement[104..108].copy_from_slice(&(-16.0_f32).to_le_bytes());
    assert_ne!(
        source_formats::mdl::ModelHeader::parse(&original)
            .unwrap()
            .hull_mins,
        source_formats::mdl::ModelHeader::parse(&replacement)
            .unwrap()
            .hull_mins,
    );
    overwrite(&path, 0, &replacement);
    let fresh = compile(&vmf, &game, false);
    assert!(fresh.prop_lighting_assets.is_some());

    let mut budget = Budget::new(&control);
    let tracker =
        super::super::super::asset_tracking::Tracker::new(&game, source_reads, &mut budget)
            .unwrap();
    assert!(
        prepare(
            &map,
            Settings {
                vmf: &vmf,
                prop_sources: &props.sources,
            },
            tracker,
            &bsp,
            &mut report,
            &mut budget,
        )
        .is_err()
    );
    assert!(report.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("models/props/test.mdl changed during baking")
    }));
    assert_eq!(bsp.to_bytes(), previous);
}
