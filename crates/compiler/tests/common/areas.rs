use glam::DVec3;
use map_core::{Document, ObjectId, edit};
use source_formats::vmf::Vmf;

use crate::common;

pub fn room(divide: bool) -> (Vmf, u32) {
    let mut document = Document::new(common::room());
    if divide {
        for (low, high) in [
            ([-8.0, -256.0, 0.0], [8.0, -64.0, 256.0]),
            ([-8.0, 64.0, 0.0], [8.0, 256.0, 256.0]),
            ([-8.0, -64.0, 192.0], [8.0, 64.0, 256.0]),
        ] {
            document
                .edit("divider", |tx| {
                    edit::create_block(
                        tx,
                        DVec3::from_array(low),
                        DVec3::from_array(high),
                        "tools/toolsnodraw",
                    )
                })
                .unwrap();
        }
    }
    let mut portal = 0;
    document
        .edit("area portal", |tx| {
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::new(-4.0, -64.0, 0.0),
                DVec3::new(4.0, 64.0, 192.0),
                "tools/toolsareaportal",
            )?
            else {
                unreachable!()
            };
            let ObjectId::Entity(id) = edit::tie_to_entity(
                tx,
                &[id],
                "func_areaportal",
                &[
                    ("targetname".into(), "partition_gate".into()),
                    ("StartOpen".into(), "1".into()),
                ],
            )?
            else {
                unreachable!()
            };
            portal = id;
            Ok(())
        })
        .unwrap();
    (document.vmf().clone(), portal)
}

pub fn window_room(foreground: bool) -> (Vmf, u32, u32, Option<u32>) {
    let (vmf, portal) = room(true);
    let mut document = Document::new(vmf);
    let mut models = Vec::new();
    for name in ["rendered_window", "window_foreground"]
        .into_iter()
        .take(if foreground { 2 } else { 1 })
    {
        document
            .edit("window model", |tx| {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::new(-8.0, -64.0, 0.0),
                    DVec3::new(8.0, 64.0, 192.0),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                let ObjectId::Entity(id) = edit::tie_to_entity(
                    tx,
                    &[id],
                    "func_brush",
                    &[
                        ("targetname".into(), name.into()),
                        ("origin".into(), "0.5 -0.25 96.25".into()),
                        ("Solidity".into(), "1".into()),
                    ],
                )?
                else {
                    unreachable!()
                };
                models.push(id);
                Ok(())
            })
            .unwrap();
    }
    document
        .edit("window portal", |tx| {
            let entity = tx.entity_mut(portal)?;
            entity.properties.set("classname", "func_areaportalwindow");
            entity.properties.set("target", "rendered_window");
            if foreground {
                entity
                    .properties
                    .set("BackgroundBModel", "window_foreground");
            }
            Ok(())
        })
        .unwrap();
    (
        document.vmf().clone(),
        portal,
        models[0],
        models.get(1).copied(),
    )
}
