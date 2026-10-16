use super::*;
use source_formats::fgd::Catalog;

fn appearance(definition: &str) -> Appearance {
    let catalog = Catalog::parse(definition).unwrap();
    Appearance::resolve(catalog.class("sample").unwrap())
}

#[test]
fn size_forms_inheritance_extensions_and_color_preserve_offset_units() {
    let centered = appearance("@PointClass size(16 32 72) color(12 34 56) = sample []");
    assert_eq!(
        centered.local_bounds,
        Some([DVec3::new(-8.0, -16.0, -36.0), DVec3::new(8.0, 16.0, 36.0)])
    );
    assert_eq!(centered.color, Some([12, 34, 56]));
    let inherited = appearance(
        "@BaseClass size(-16 -12 0, 24 20 72) color(30 60 90) = shared [] @PointClass base(shared) sphere(radius) = sample []",
    );
    assert_eq!(
        inherited.local_bounds,
        Some([DVec3::new(-16.0, -12.0, 0.0), DVec3::new(24.0, 20.0, 72.0)])
    );
    assert_eq!(inherited.unsupported, ["sphere"]);
    let extended = appearance(
        "@PointClass size(16 16 16) = sample [] @ExtendClass size(-4 -8 -12, 20 24 28) = sample []",
    );
    assert_eq!(
        extended.local_bounds,
        Some([DVec3::new(-4.0, -8.0, -12.0), DVec3::new(20.0, 24.0, 28.0)])
    );
}

#[test]
fn malformed_boxes_and_colors_report_errors_without_successful_class_bounds() {
    for declaration in [
        "size()",
        "size(1 2)",
        "size(0 16 16)",
        "size(-16 16 16)",
        "size(1.5 2 3)",
        "size(NaN 2 3)",
        "size(-16 -16 0, 16 16 0)",
        "size(-65537 -1 -1, 1 1 1)",
        "size(2147483648 2 3)",
        "size(16 16 16) size(32 32 32)",
    ] {
        let helper = appearance(&format!("@PointClass {declaration} = sample []"));
        assert!(helper.invalid_size, "{declaration}");
        assert!(helper.local_bounds.is_none(), "{declaration}");
        assert!(!helper.errors.is_empty(), "{declaration}");
    }
    for declaration in [
        "color(256 0 0)",
        "color(-1 0 0)",
        "color(1 2)",
        "color(1.5 2 3)",
    ] {
        let helper = appearance(&format!(
            "@PointClass size(16 16 16) {declaration} = sample []"
        ));
        assert!(helper.color.is_none());
        assert!(helper.local_bounds.is_some());
        assert!(!helper.invalid_size);
        assert!(!helper.errors.is_empty());
    }
}

#[test]
fn translated_boxes_are_world_aligned_and_use_checked_parallel_boundary_rays() {
    let helper = appearance("@PointClass size(-24 -12 0, 40 20 72) = sample []");
    let mut document = crate::Document::empty();
    let id = document
        .edit("sample", |tx| {
            crate::edit::create_entity(
                tx,
                "sample",
                Some(DVec3::new(100.0, 200.0, 300.0)),
                &[
                    ("angles".into(), "30 45 60".into()),
                    ("modelscale".into(), "9".into()),
                ],
                Vec::new(),
            )
        })
        .unwrap();
    let ObjectId::Entity(id) = id else {
        unreachable!()
    };
    let entity = document.entity(id).unwrap();
    assert_eq!(
        helper.world_bounds(entity),
        Some((
            DVec3::new(76.0, 188.0, 300.0),
            DVec3::new(140.0, 220.0, 372.0)
        ))
    );
    let ray = Ray {
        origin: DVec3::new(76.0, 100.0, 350.0),
        direction: DVec3::Y * 4.0,
    };
    let hit = helper.pick(entity, &ray).unwrap();
    assert_eq!(hit.point, DVec3::new(76.0, 188.0, 350.0));
    assert_eq!(hit.distance, 88.0);
    assert!(
        helper
            .pick(
                entity,
                &Ray {
                    origin: DVec3::new(75.0, 100.0, 350.0),
                    ..ray
                }
            )
            .is_none()
    );
    assert!(
        helper
            .pick(
                entity,
                &Ray {
                    direction: DVec3::splat(f64::NAN),
                    ..ray
                }
            )
            .is_none()
    );
    assert!(
        helper
            .pick(
                entity,
                &Ray {
                    direction: DVec3::ZERO,
                    ..ray
                }
            )
            .is_none()
    );
    let picker = crate::picking::Picker::with_entity_bounds(
        &document,
        |_| true,
        true,
        |_| true,
        |entity| helper.world_bounds(entity),
    );
    assert_eq!(picker.pick(&ray).unwrap().point, hit.point);
}
