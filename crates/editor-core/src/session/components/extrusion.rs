use super::*;
use map_core::subobject::{ExtrusionFace, ExtrusionPlan, ExtrusionSpec, ExtrusionSurface};
use map_core::surface::FaceId;

pub(super) struct Draft {
    pub spec: ExtrusionSpec,
    pub selection: BTreeSet<PartitionRef>,
    pub plan: ExtrusionPlan,
}

impl Draft {
    pub fn report(&self) -> Value {
        json!({"height":self.spec.height,"direction":if self.spec.height < 0.0 {"inward"} else {"outward"},"wall_material":self.spec.wall_material,"texture_lock":self.spec.texture_lock,
            "selection":self.selection.iter().map(|part|json!({"solid":part.solid,"partition":part.partition})).collect::<Vec<_>>(),
            "pieces":self.plan.pieces.iter().map(|piece|json!({"source_solid":piece.source.solid,"source_side":piece.source.side,"partition":piece.partition,"solid":(self.spec.height > 0.0).then_some(piece.solid.id),
                "owner":match piece.owner {ObjectId::Solid(_)=>"world".to_string(),ObjectId::Entity(id)=>format!("entity:{id}")},
                "bounds":map_core::query::solid_bounds(&piece.solid).map(|(mins,maxs)|json!({"mins":mins.to_array(),"maxs":maxs.to_array()})),
                "faces":face_report(&piece.solid, &piece.faces) })).collect::<Vec<_>>(),
            "fragments":self.plan.fragments.iter().map(|fragment|json!({"source_solid":fragment.source,"solid":fragment.solid.id,
                "owner":match fragment.owner {ObjectId::Solid(_)=>"world".to_string(),ObjectId::Entity(id)=>format!("entity:{id}")},
                "bounds":map_core::query::solid_bounds(&fragment.solid).map(|(mins,maxs)|json!({"mins":mins.to_array(),"maxs":maxs.to_array()})),
                "faces":face_report(&fragment.solid, &fragment.faces)})).collect::<Vec<_>>()})
    }
}

fn face_report(solid: &source_formats::vmf::Solid, faces: &[ExtrusionFace]) -> Vec<Value> {
    faces.iter().map(|face| {
        let side = solid.sides.iter().find(|side|side.id==face.side).unwrap();
        json!({"side":face.side,"source_side":face.source_side,"partition":face.partition,"surface":match face.surface {ExtrusionSurface::Source=>"source",ExtrusionSurface::Top=>"top",ExtrusionSurface::Base=>"base",ExtrusionSurface::Boundary=>"boundary",ExtrusionSurface::Divider=>"divider"},
            "material":side.material.to_ascii_lowercase(),"plane":side.plane.0.map(|point|point.to_array()),"uaxis":{"axis":side.u_axis.axis.to_array(),"offset":side.u_axis.offset,"scale":side.u_axis.scale},"vaxis":{"axis":side.v_axis.axis.to_array(),"offset":side.v_axis.offset,"scale":side.v_axis.scale},"rotation":side.rotation,"lightmap_scale":side.lightmap_scale,"smoothing_groups":side.smoothing_groups})
    }).collect()
}

impl Session {
    pub fn preview_partition_extrusion(
        &mut self,
        token: u64,
        spec: ExtrusionSpec,
    ) -> Result<u64, String> {
        let draft = self.checked_partition(token)?.clone();
        if draft.rise != 0.0 {
            return Err("Return to flat before extruding highlighted pieces".into());
        }
        let selection = draft
            .selection
            .iter()
            .map(|part| (part.solid, part.partition))
            .collect::<Vec<_>>();
        let mut document = self.document.preview_snapshot();
        let plan = document
            .edit("preview partition extrusion", |tx| {
                subobject::extrude_partitions_with_surfaces(
                    tx,
                    &draft.partitions,
                    &selection,
                    &spec,
                    &draft.material_plan(),
                    &draft.texture_plan(),
                )
            })
            .map_err(|error| error.to_string())?;
        Ok(self.install_component_preview(Preview {
            extrusion: Some(Draft {
                spec,
                selection: draft.selection.clone(),
                plan,
            }),
            partition: Some(draft),
            ..Preview::new(document, self.geometry.components.selection.clone())
        }))
    }

    pub(super) fn commit_partition_extrusion(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<usize, String> {
        let preview = self.checked_component_preview(token)?;
        let draft = preview.partition.as_ref().unwrap();
        let extrusion = preview.extrusion.as_ref().unwrap();
        if draft.selection != extrusion.selection {
            return Err(
                "Highlighted pieces changed; preview the extrusion again before Apply".into(),
            );
        }
        let partitions = draft.partitions.clone();
        let materials = draft.material_plan();
        let textures = draft.texture_plan();
        let selection = extrusion
            .selection
            .iter()
            .map(|part| (part.solid, part.partition))
            .collect::<Vec<_>>();
        let spec = extrusion.spec.clone();
        let expected = extrusion.plan.clone();
        let solids = if spec.height < 0.0 {
            expected
                .fragments
                .iter()
                .map(|fragment| &fragment.solid)
                .collect::<Vec<_>>()
        } else {
            expected
                .pieces
                .iter()
                .map(|piece| &piece.solid)
                .collect::<Vec<_>>()
        };
        let faces = solids
            .iter()
            .flat_map(|solid| {
                solid.sides.iter().map(|side| FaceId {
                    solid: solid.id,
                    side: side.id,
                })
            })
            .collect::<Vec<_>>();
        let count = self.edit(actor, "extrude flat partition pieces", |tx| {
            let actual = subobject::extrude_partitions_with_surfaces(
                tx,
                &partitions,
                &selection,
                &spec,
                &materials,
                &textures,
            )?;
            if actual != expected {
                return Err(EditError::Invalid(
                    "Extrusion allocation changed; preview the pieces again".into(),
                ));
            }
            Ok(if spec.height < 0.0 {
                actual.fragments.len()
            } else {
                actual.pieces.len()
            })
        })?;
        self.select_faces(&faces, SelectMode::Replace)?;
        Ok(count)
    }
}
