use std::path::PathBuf;

use source_formats::fgd::{Catalog, ClassKind};

#[test]
fn installed_css_entities_include_lights_and_inherited_properties() {
    let path = std::env::var_os("CSS_FGD").map(PathBuf::from).or_else(|| {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home).join(
                ".local/share/Steam/steamapps/common/SourceSDK/bin/source2009/bin/cstrike.fgd",
            )
        })
    });
    let Some(path) = path.filter(|path| path.is_file()) else {
        return;
    };
    let catalog = Catalog::load(&path).unwrap();
    assert!(catalog.classes().count() > 100);
    assert!(catalog.sources.len() >= 2);
    let light = catalog.class("light").unwrap();
    assert_eq!(light.kind, ClassKind::Point);
    assert!(light.defaults().contains_key("_light"));
    assert!(light.property("targetname").is_some());
    assert!(light.inputs.iter().any(|input| input.name == "TurnOn"));
    assert_eq!(catalog.class("func_door").unwrap().kind, ClassKind::Solid);
    assert!(
        catalog
            .class("info_player_terrorist")
            .unwrap()
            .property("angles")
            .is_some()
    );
}

#[test]
fn includes_resolve_relative_paths_and_reject_cycles() {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().join("base.fgd");
    let root = directory.path().join("root.fgd");
    std::fs::write(
        &base,
        "@BaseClass = Base [ value(integer) : \"Value\" : 3 ]",
    )
    .unwrap();
    std::fs::write(
        &root,
        "@include \"base.fgd\" @PointClass base(Base) = point []",
    )
    .unwrap();
    assert_eq!(
        Catalog::load(&root)
            .unwrap()
            .class("point")
            .unwrap()
            .defaults()["value"],
        "3"
    );
    std::fs::write(&base, "@include \"root.fgd\"").unwrap();
    assert!(Catalog::load(&root).unwrap_err().contains("cycle"));
}

#[test]
fn extensions_apply_after_includes_and_repeated_sources_are_not_replayed() {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().join("base.fgd");
    let extension = directory.path().join("extension.fgd");
    let root = directory.path().join("root.fgd");
    std::fs::write(
        &base,
        r#"
        @BaseClass = Common [ value(integer) : "Value" : 1 ]
        @PointClass base(Common) = point [ output OnReady(void) : "Ready" ]
        "#,
    )
    .unwrap();
    std::fs::write(
        &extension,
        r#"
        @ExtendClass = Common [ inherited(string) : "Inherited" : "Extended base" ]
        @ExtendClass = point [ value(integer) : "Value" : 2 input Enable(void) ]
        @ExtendClass = missing [ ignored(string) ]
        "#,
    )
    .unwrap();
    std::fs::write(
        &root,
        r#"
        @include "extension.fgd"
        @ExtendClass = point [ value(integer) : "Value" : 3 ]
        @include "base.fgd"
        @include "extension.fgd"
        "#,
    )
    .unwrap();
    let catalog = Catalog::load(&root).unwrap();
    assert_eq!(catalog.sources.len(), 3);
    assert_eq!(catalog.classes().count(), 1);
    let class = catalog.class("point").unwrap();
    assert_eq!(class.defaults()["value"], "3");
    assert_eq!(class.defaults()["inherited"], "Extended base");
    assert_eq!(class.inputs[0].name, "Enable");
    assert_eq!(class.outputs[0].name, "OnReady");
}
