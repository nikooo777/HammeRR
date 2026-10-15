use glam::DVec3;
use source_formats::keyvalues::Node;

use super::*;
use crate::{BrushGeometry, clip, edit, transform};

fn block(document: &mut Document, mins: DVec3, maxs: DVec3) -> u32 {
    let id = document.edit("block", |tx|edit::create_block(tx,mins,maxs,"brick/test")).unwrap();
    let ObjectId::Solid(id) = id else { unreachable!() };
    id
}

fn adjacent() -> (Document, u32, u32) {
    let mut document = Document::empty();
    let first = block(&mut document,DVec3::ZERO,DVec3::splat(64.0));
    let second = block(&mut document,DVec3::new(64.0,0.0,0.0),DVec3::new(128.0,64.0,64.0));
    (document,first,second)
}

#[test]
fn adjacent_union_retains_exact_outer_faces_metadata_and_history() {
    let (mut document,first,second) = adjacent();
    document.edit("metadata",|tx| {
        for id in [first,second] {
            let solid = tx.solid_mut(id)?;
            solid.extra.push(Node::string("vendor",format!("brush-{id}")));
            solid.extra_properties.set("custom",format!("solid-{id}"));
            for side in &mut solid.sides {
                side.material = format!("CUSTOM/FACE_{}",side.id);
                side.u_axis.offset = f64::from(side.id)+0.25;
                side.v_axis.scale = -0.75;
                side.rotation = 17.0;
                side.lightmap_scale = 32;
                side.smoothing_groups = Some(7);
                side.extra.push(Node::string("vendor","face"));
                side.extra_properties.set("custom","side");
            }
        }
        Ok(())
    }).unwrap();
    let original = document.vmf().clone();
    let history = document.undo_labels().count();
    let result = document.edit("merge",|tx|merge(tx,&[second,first])).unwrap();
    assert_eq!(result.sources,vec![first,second]);
    assert_eq!(result.object,ObjectId::Solid(first));
    assert_eq!(result.removed,vec![second]);
    assert_eq!(result.faces.len(),6);
    assert_eq!(document.undo_labels().count(),history+1);
    let merged = document.solid(first).unwrap();
    assert_eq!(merged.extra,original.world.solids[0].extra);
    assert_eq!(merged.extra_properties,original.world.solids[0].extra_properties);
    assert_eq!(merged.editor,original.world.solids[0].editor);
    assert_eq!(merged.hidden,original.world.solids[0].hidden);
    for face in &result.faces {
        let expected = original.solids().find(|solid|solid.id==face.source_solid).unwrap().sides.iter().find(|side|side.id==face.source_side).unwrap();
        let actual = merged.sides.iter().find(|side|side.id==face.side).unwrap();
        assert_eq!(actual,expected);
        let normal = Plane::from_points(actual.plane.0).unwrap().normal;
        if normal != DVec3::X {
            assert_eq!(face.source_solid,first);
        }
    }
    let shape = BrushGeometry::from_solid(merged).unwrap();
    assert!(shape.mins.abs_diff_eq(DVec3::ZERO,1e-7));
    assert!(shape.maxs.abs_diff_eq(DVec3::new(128.0,64.0,64.0),1e-7));
    document.vmf().validate_ids().unwrap();
    let applied = document.vmf().clone();
    let reopened = Document::new(source_formats::vmf::Vmf::parse(&applied.write(&Default::default())).unwrap());
    assert_eq!(reopened.vmf(),&applied);
    document.undo().unwrap();
    assert_eq!(document.vmf(),&original);
    let repeated = document.edit("merge",|tx|merge(tx,&[first,second])).unwrap();
    assert_eq!(repeated,result);
    assert_eq!(document.vmf(),&applied);
    document.undo().unwrap();
    document.redo().unwrap();
    assert_eq!(document.vmf(),&applied);
}

#[test]
fn overlapping_contained_and_coincident_inputs_merge_without_filling_extra_volume() {
    for (mins,maxs,expected) in [
        (DVec3::new(32.0,0.0,0.0),DVec3::new(96.0,64.0,64.0),DVec3::new(96.0,64.0,64.0)),
        (DVec3::splat(16.0),DVec3::splat(48.0),DVec3::splat(64.0)),
        (DVec3::ZERO,DVec3::splat(64.0),DVec3::splat(64.0)),
    ] {
        let mut document = Document::empty();
        let first = block(&mut document,DVec3::ZERO,DVec3::splat(64.0));
        let second = block(&mut document,mins,maxs);
        let result = document.edit("merge",|tx|merge(tx,&[first,second])).unwrap();
        let shape = BrushGeometry::from_solid(document.solid(result.solid).unwrap()).unwrap();
        assert!(shape.mins.abs_diff_eq(DVec3::ZERO,1e-7));
        assert!(shape.maxs.abs_diff_eq(expected,1e-7));
        assert_eq!(shape.faces.len(),6);
    }
}

#[test]
fn oblique_split_and_rotated_neighbors_recombine_into_the_original_convex_brush() {
    for normal in [DVec3::new(1.0,1.0,0.0),DVec3::new(1.0,2.0,3.0)] {
        let mut document = Document::empty();
        let first = block(&mut document,DVec3::splat(-64.0),DVec3::splat(64.0));
        let original = document.solid(first).unwrap().clone();
        let cut = document.edit("split",|tx|clip::clip(tx,&[ObjectId::Solid(first)],Plane{normal,dist:7.0},clip::Keep::Both,None)).unwrap();
        let sources = cut.cuts[0].solids.clone();
        assert_eq!(sources.len(),2);
        let result = document.edit("merge",|tx|merge(tx,&sources)).unwrap();
        assert_eq!(document.solid(result.solid).unwrap(),&original);
    }
    let (mut document,first,second) = adjacent();
    document.edit("rotate",|tx|transform::transform(tx,&[ObjectId::Solid(first),ObjectId::Solid(second)],transform::Transform{rotation:DVec3::new(13.0,27.0,41.0),..Default::default()})).unwrap();
    let result = document.edit("merge",|tx|merge(tx,&[first,second])).unwrap();
    assert_eq!(document.solid(result.solid).unwrap().sides.len(),6);
    subobject::topology(document.solid(result.solid).unwrap()).unwrap();
}

#[test]
fn gaps_concavities_and_tangent_only_connections_reject_atomically() {
    for (mins,maxs) in [
        (DVec3::new(80.0,0.0,0.0),DVec3::new(128.0,64.0,64.0)),
        (DVec3::new(64.0,32.0,0.0),DVec3::new(128.0,96.0,64.0)),
        (DVec3::new(64.0,64.0,0.0),DVec3::new(128.0,128.0,64.0)),
        (DVec3::splat(64.0),DVec3::splat(128.0)),
    ] {
        let mut document = Document::empty();
        let first = block(&mut document,DVec3::ZERO,DVec3::splat(64.0));
        let second = block(&mut document,mins,maxs);
        let original = document.vmf().clone();
        let history = document.undo_labels().count();
        assert!(document.edit("invalid merge",|tx|merge(tx,&[first,second])).is_err());
        assert_eq!(document.vmf(),&original);
        assert_eq!(document.undo_labels().count(),history);
    }
}

#[test]
fn fully_covered_hull_vertices_do_not_hide_an_uncovered_interior_cavity() {
    let mut document = Document::empty();
    let mut sources = Vec::new();
    for (mins,maxs) in [
        ([0.0,0.0,0.0],[128.0,16.0,64.0]),
        ([0.0,112.0,0.0],[128.0,128.0,64.0]),
        ([0.0,16.0,0.0],[16.0,112.0,64.0]),
        ([112.0,16.0,0.0],[128.0,112.0,64.0]),
    ] {
        sources.push(block(&mut document,DVec3::from_array(mins),DVec3::from_array(maxs)));
    }
    let original = document.vmf().clone();
    let error = document.edit("invalid merge",|tx|merge(tx,&sources)).unwrap_err();
    assert!(error.to_string().contains("uncovered volume"),"{error}");
    assert_eq!(document.vmf(),&original);
}

#[test]
fn entity_children_merge_without_changing_owner_siblings_or_properties() {
    let (mut document,first,second) = adjacent();
    let sibling = block(&mut document,DVec3::splat(256.0),DVec3::splat(288.0));
    let unrelated = block(&mut document,DVec3::splat(512.0),DVec3::splat(544.0));
    let owner = document.edit("tie",|tx|edit::tie_to_entity(tx,&[first,second,sibling],"func_detail",&[])).unwrap();
    let original = document.vmf().clone();
    let result = document.edit("merge",|tx|merge(tx,&[second,first])).unwrap();
    assert_eq!(result.object,owner);
    let ObjectId::Entity(entity) = owner else {unreachable!()};
    let merged_owner = document.entity(entity).unwrap();
    assert_eq!(merged_owner.solids.len(),2);
    assert_eq!(merged_owner.properties,original.entities[0].properties);
    assert_eq!(subobject::solid(&document,sibling).unwrap().1,original.solids().find(|solid|solid.id==sibling).unwrap());
    assert_eq!(document.solid(unrelated).unwrap(),original.solids().find(|solid|solid.id==unrelated).unwrap());
    document.vmf().validate_ids().unwrap();
    document.undo().unwrap();
    assert_eq!(document.vmf(),&original);
}

#[test]
fn mixed_owners_displacements_duplicates_and_invalid_sources_reject() {
    let (mut document,first,second) = adjacent();
    for sources in [vec![],vec![first],vec![first,first],vec![first,u32::MAX]] {
        let original = document.vmf().clone();
        assert!(document.edit("invalid merge",|tx|merge(tx,&sources)).is_err());
        assert_eq!(document.vmf(),&original);
    }
    document.edit("tie",|tx|edit::tie_to_entity(tx,&[first],"func_detail",&[])).unwrap();
    let original = document.vmf().clone();
    assert!(document.edit("mixed",|tx|merge(tx,&[first,second])).unwrap_err().to_string().contains("mixed owners"));
    assert_eq!(document.vmf(),&original);
    document.undo().unwrap();
    document.edit("displacement",|tx| {
        tx.solid_mut(second)?.sides[0].displacement = Some(source_formats::vmf::DispInfo {
            power:2,start_position:DVec3::ZERO,flags:None,elevation:0.0,subdivided:false,
            normals:None,distances:None,offsets:None,offset_normals:None,alphas:None,triangle_tags:None,allowed_verts:None,extra_properties:Default::default(),extra:Vec::new(),
        });
        Ok(())
    }).unwrap();
    let original = document.vmf().clone();
    assert!(document.edit("displacement merge",|tx|merge(tx,&[first,second])).unwrap_err().to_string().contains("displacements"));
    assert_eq!(document.vmf(),&original);
}
