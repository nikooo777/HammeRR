use glam::DVec3;
use map_core::{Document, ObjectId, subobject, texel_uv};
use source_formats::vmf::Vmf;

pub fn repeated_translation(mut document: Document, id: u32, cycles: usize) -> (f64, f64) {
    let original = subobject::solid(&document, id).unwrap().1.clone();
    let owner = document.owner_of_solid(id).unwrap();
    let before = subobject::topology(&original).unwrap();
    let components = before
        .faces
        .iter()
        .map(|face| subobject::Component {
            solid: id,
            sides: vec![face.side],
        })
        .collect::<Vec<_>>();
    let owner_properties = match owner {
        ObjectId::Entity(id) => Some(document.entity(id).unwrap().properties.clone()),
        _ => None,
    };
    let mut geometry_drift = 0f64;
    let mut texture_drift = 0f64;
    for cycle in 0..cycles {
        let offset = [
            DVec3::new(0.125, -0.25, 0.375),
            DVec3::new(-0.375, 0.125, -0.25),
            DVec3::new(0.25, 0.375, -0.125),
        ][cycle % 3];
        for displacement in [offset, -offset] {
            let source = document.vmf().clone();
            let plan =
                subobject::plan_move_with_texture_lock(&document, &components, displacement, true)
                    .unwrap();
            document
                .edit("fractional translation", |tx| subobject::apply(tx, &plan))
                .unwrap();
            let changed = document.vmf().clone();
            document.undo().unwrap();
            assert_eq!(document.vmf(), &source);
            document.redo().unwrap();
            assert_eq!(document.vmf(), &changed);
            let saved = document.vmf().write(&Default::default());
            let reopened = Vmf::parse(&saved).unwrap();
            reopened.validate_ids().unwrap();
            document = Document::try_new(reopened).unwrap();
        }
        assert_eq!(document.owner_of_solid(id), Some(owner));
        if let ObjectId::Entity(id) = owner {
            assert_eq!(
                &document.entity(id).unwrap().properties,
                owner_properties.as_ref().unwrap()
            );
        }
        let changed = subobject::solid(&document, id).unwrap().1;
        let mut expected_brush = original.clone();
        expected_brush.sides = changed.sides.clone();
        assert_eq!(changed, &expected_brush);
        let after = subobject::topology(changed).unwrap();
        assert_eq!(before.vertices.len(), after.vertices.len());
        assert_eq!(before.edges.len(), after.edges.len());
        assert_eq!(before.faces.len(), after.faces.len());
        for vertex in &before.vertices {
            let point = after.vertices[after.resolve(&vertex.sides).unwrap()[0]].position;
            geometry_drift = geometry_drift.max((point - vertex.position).abs().max_element());
            for side in &vertex.sides {
                let old = original
                    .sides
                    .iter()
                    .find(|candidate| candidate.id == *side)
                    .unwrap();
                let new = changed
                    .sides
                    .iter()
                    .find(|candidate| candidate.id == *side)
                    .unwrap();
                texture_drift = texture_drift.max(
                    (texel_uv(old, vertex.position) - texel_uv(new, point))
                        .abs()
                        .max_element(),
                );
            }
        }
        for old in &original.sides {
            let new = changed.sides.iter().find(|side| side.id == old.id).unwrap();
            let mut expected = old.clone();
            expected.plane = new.plane;
            expected.u_axis = new.u_axis;
            expected.v_axis = new.v_axis;
            assert_eq!(new, &expected);
            for (old, new) in old.plane.0.iter().zip(new.plane.0) {
                assert!(old.abs_diff_eq(new, 1e-8));
            }
        }
        assert!(
            geometry_drift < 1e-5,
            "cycle {cycle}, brush {id}: geometry drift {geometry_drift}"
        );
        assert!(
            texture_drift < 0.0001,
            "cycle {cycle}, brush {id}: texture drift {texture_drift}"
        );
    }
    (geometry_drift, texture_drift)
}
