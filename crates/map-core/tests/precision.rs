mod common;

use glam::DVec3;
use map_core::{Document, ObjectId, edit, transform};
use source_formats::keyvalues::Node;

fn fixture(origin: DVec3, owner: bool, sloped: bool) -> (Document, u32) {
    let mut document = Document::empty();
    let ObjectId::Solid(id) = document
        .edit("block", |tx| {
            edit::create_block(
                tx,
                origin,
                origin + DVec3::new(64.125, 31.75, 16.5),
                "brick/test",
            )
        })
        .unwrap()
    else {
        unreachable!()
    };
    document
        .edit("metadata", |tx| {
            let brush = tx.solid_mut(id)?;
            brush.extra.push(Node::block(
                "vendor_data",
                vec![Node::string("resource", "test/brush")],
            ));
            for side in &mut brush.sides {
                side.extra_properties.set("vendor_side", "preserve");
                side.u_axis.offset = 13.375;
                side.v_axis.offset = -7.125;
                side.lightmap_scale = 8;
                side.smoothing_groups = Some(5);
            }
            Ok(())
        })
        .unwrap();
    if sloped {
        document
            .edit("oblique", |tx| {
                transform::transform(
                    tx,
                    &[ObjectId::Solid(id)],
                    transform::Transform {
                        rotation: DVec3::new(17.0, -23.0, 31.0),
                        pivot: origin,
                        texture_lock: true,
                        ..Default::default()
                    },
                )
            })
            .unwrap();
    }
    if owner {
        document
            .edit("entity", |tx| {
                edit::tie_to_entity(
                    tx,
                    &[id],
                    "func_detail",
                    &[("targetname".into(), "precise_fixture".into())],
                )
            })
            .unwrap();
    }
    (document, id)
}

#[test]
fn repeated_fractional_edits_and_vmf_round_trips_preserve_world_and_entity_brushes() {
    for origin in [
        DVec3::new(0.125, -0.25, 0.375),
        DVec3::new(8192.125, -4096.25, 1024.375),
        DVec3::new(-32768.125, 16384.25, -8192.375),
    ] {
        for owner in [false, true] {
            for sloped in [false, true] {
                let (document, id) = fixture(origin, owner, sloped);
                let (geometry, texture) = common::repeated_translation(document, id, 16);
                eprintln!(
                    "origin {origin}, entity {owner}, sloped {sloped}: drift {geometry} units / {texture} texels"
                );
            }
        }
    }
}

#[test]
fn repeated_thin_slope_edits_preserve_authored_planes_and_incidence() {
    let (mut document, id) = fixture(DVec3::ZERO, false, false);
    let p = DVec3::new;
    let planes = [
        [
            p(3928.0, 5888.0, 57.0981),
            p(4396.0, 5888.0, 57.0981),
            p(4396.0, 5784.0, -0.051_668_4),
        ],
        [
            p(3928.0, 5792.0, -0.051_668_5),
            p(4396.0, 5792.0, -0.051_668_5),
            p(4396.0, 5888.0, 52.9497),
        ],
        [
            p(3928.0, 5784.0, -0.051_668_4),
            p(4396.0, 5784.0, -0.051_668_4),
            p(4396.0, 5792.0, -0.051_668_5),
        ],
        [
            p(3928.0, 5888.0, 57.0981),
            p(3928.0, 5784.0, -0.051_668_4),
            p(3928.0, 5792.0, -0.051_668_5),
        ],
        [
            p(4396.0, 5888.0, 57.0981),
            p(3928.0, 5888.0, 57.0981),
            p(3928.0, 5888.0, 52.9497),
        ],
        [
            p(4396.0, 5888.0, 52.9497),
            p(4396.0, 5792.0, -0.051_668_5),
            p(4396.0, 5784.0, -0.051_668_4),
        ],
    ];
    document
        .edit("thin slope", |tx| {
            for (side, plane) in tx.solid_mut(id)?.sides.iter_mut().zip(planes) {
                side.plane.0 = plane;
                let normal = map_core::Plane::from_points(plane).unwrap().normal;
                let horizontal = normal.cross(DVec3::X);
                side.u_axis.axis = if horizontal.length() > 0.001 {
                    horizontal.normalize()
                } else {
                    DVec3::Y
                };
                side.v_axis.axis = normal.cross(side.u_axis.axis).normalize();
            }
            Ok(())
        })
        .unwrap();
    let (geometry, texture) = common::repeated_affine(document.preview_snapshot(), id, 32);
    eprintln!("thin affine slope: drift {geometry} units / {texture} texels");
    let (geometry, texture) = common::repeated_translation(document, id, 32);
    eprintln!("thin slope: drift {geometry} units / {texture} texels");
}

#[test]
fn repeated_scale_and_compound_rotation_round_trips_preserve_precise_brushes() {
    for origin in [
        DVec3::new(0.125, -0.25, 0.375),
        DVec3::new(8192.125, -4096.25, 1024.375),
        DVec3::new(-32768.125, 16384.25, -8192.375),
    ] {
        for owner in [false, true] {
            for sloped in [false, true] {
                let (document, id) = fixture(origin, owner, sloped);
                let (geometry, texture) = common::repeated_affine(document, id, 16);
                eprintln!(
                    "affine origin {origin}, entity {owner}, sloped {sloped}: drift {geometry} units / {texture} texels"
                );
            }
        }
    }
}
