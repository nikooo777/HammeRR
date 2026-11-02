use super::*;
use map_core::linked_path::{self, Outcome, Request as Operation};
use source_formats::fgd::Catalog;

#[derive(Clone, Debug)]
pub(super) struct Draft {
    pub(super) request: Operation,
    pub(super) points: Option<Vec<DVec3>>,
}

impl Draft {
    pub(super) fn seed(&self) -> u32 {
        match self.request {
            Operation::Topology { seed, .. }
            | Operation::Train { seed, .. }
            | Operation::Duplicate { seed, .. } => seed,
        }
    }

    pub(super) fn label(&self) -> &'static str {
        match self.request {
            Operation::Topology { .. } => "topology",
            Operation::Train { .. } => "train",
            Operation::Duplicate { .. } => "duplicate",
        }
    }

    pub(super) fn history_label(&self) -> &'static str {
        match self.request {
            Operation::Topology { .. } => "convert linked path",
            Operation::Train { .. } => "configure linked train",
            Operation::Duplicate { .. } => "duplicate linked path",
        }
    }

    pub(super) fn report(&self) -> Value {
        match &self.request {
            Operation::Topology {
                classname, closed, ..
            } => json!({"classname":classname,"closed":closed}),
            Operation::Train {
                train,
                speed,
                start_on,
                ..
            } => {
                json!({"train":object_name(ObjectId::Entity(*train)),"speed":speed,"start_on":start_on})
            }
            Operation::Duplicate {
                train,
                offset,
                policies,
                ..
            } => {
                json!({"train":train.map(|id|object_name(ObjectId::Entity(id))),"offset":offset.to_array(),"naming":{"style":match policies.naming.style {map_core::repeat::NameStyle::SourceSuffix=>"source_suffix",map_core::repeat::NameStyle::IndexedPrefix=>"indexed_prefix"},"prefix":policies.naming.prefix,"suffix":policies.naming.suffix,"start_index":policies.naming.start_index},"external":match policies.external {map_core::repeat::ExternalReferences::Keep=>"keep",map_core::repeat::ExternalReferences::Reject=>"reject"}})
            }
        }
    }
}

pub(super) fn apply(
    tx: &mut Transaction<'_>,
    plan: &Plan,
    catalog: &Catalog,
) -> Result<(Vec<ObjectId>, Option<Outcome>), EditError> {
    let Some(draft) = &plan.core else {
        return apply_nodes(tx, plan).map(|objects| (objects, None));
    };
    let outcome = linked_path::apply(tx, &draft.request, catalog)?;
    if let Some(points) = &draft.points {
        linked_path::set_positions(tx, &outcome.graph, points, catalog)?;
    }
    let nodes = outcome
        .graph
        .nodes
        .iter()
        .copied()
        .map(ObjectId::Entity)
        .collect();
    Ok((nodes, Some(outcome)))
}

impl Session {
    pub(super) fn linked_path_train_report(&self, document: &Document, id: u32) -> Value {
        let entity = document.entity(id);
        let helpers = linked_path::startup_helpers(document, id);
        let helper_error = helpers.as_ref().err().map(ToString::to_string);
        let helpers = helpers.ok().map(|helpers| {
            helpers
                .into_iter()
                .map(|id| object_name(ObjectId::Entity(id)))
                .collect::<Vec<_>>()
        });
        json!({"id":object_name(ObjectId::Entity(id)),"classname":entity.and_then(|entity|entity.classname()),"targetname":entity.and_then(|entity|entity.properties.get("targetname")),"target":entity.and_then(|entity|entity.properties.get("target")),"speed":entity.and_then(|entity|entity.properties.get("speed")),"startspeed":entity.and_then(|entity|entity.properties.get("startspeed")),"startup_helpers":helpers,"startup_helper_error":helper_error})
    }

    pub(super) fn linked_path_operation_plan(
        &self,
        draft: Draft,
    ) -> Result<(Request, Plan), String> {
        if self.closing {
            return Err("The editor is closing".into());
        }
        self.ensure_partitions_resolved()?;
        let mut document = self.document.preview_snapshot();
        let outcome = document
            .edit("plan linked path", |tx| {
                let outcome = linked_path::apply(tx, &draft.request, &self.definitions)?;
                if let Some(points) = &draft.points {
                    linked_path::set_positions(tx, &outcome.graph, points, &self.definitions)?;
                }
                Ok(outcome)
            })
            .map_err(|error| error.to_string())?;
        let points = outcome
            .graph
            .nodes
            .iter()
            .map(|&id| {
                document
                    .entity(id)
                    .and_then(|entity| entity.origin())
                    .ok_or("Planned path node has no origin")
            })
            .collect::<Result<Vec<_>, _>>()?;
        let request = Request {
            classname: outcome.graph.classname.clone(),
            prefix: "path".into(),
            points: points.clone(),
            closed: outcome.graph.closed,
        };
        let plan = Plan {
            core: Some(draft),
            classname: outcome.graph.classname,
            points,
            properties: Vec::new(),
            existing: None,
            removed: Vec::new(),
        };
        Ok((request, plan))
    }

    fn preview_linked_path_operation(
        &mut self,
        actor: Actor,
        draft: Draft,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.check_linked_path_owner(actor, snapshot)?;
        let (request, plan) = self.linked_path_operation_plan(draft)?;
        self.install_linked_path(actor, request, plan)
    }

    pub fn preview_linked_path_topology(
        &mut self,
        actor: Actor,
        seed: u32,
        classname: String,
        closed: bool,
        points: Option<Vec<DVec3>>,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.preview_linked_path_operation(
            actor,
            Draft {
                request: Operation::Topology {
                    seed,
                    classname,
                    closed,
                },
                points,
            },
            snapshot,
        )
    }

    pub fn preview_linked_path_train(
        &mut self,
        actor: Actor,
        seed: u32,
        train: u32,
        speed: f64,
        start_on: bool,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.preview_linked_path_operation(
            actor,
            Draft {
                request: Operation::Train {
                    seed,
                    train,
                    speed,
                    start_on,
                },
                points: None,
            },
            snapshot,
        )
    }

    pub fn preview_linked_path_duplicate(
        &mut self,
        actor: Actor,
        seed: u32,
        train: Option<u32>,
        offset: DVec3,
        policies: map_core::repeat::Policies,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.preview_linked_path_operation(
            actor,
            Draft {
                request: Operation::Duplicate {
                    seed,
                    train,
                    offset,
                    policies,
                },
                points: None,
            },
            snapshot,
        )
    }

    pub(super) fn linked_path_point_plan(
        &self,
        token: u64,
        row: usize,
        origin: DVec3,
    ) -> Result<(Request, Plan), String> {
        let preview = self.checked_linked_path(token)?;
        let mut request = preview.request.clone();
        let point = request
            .points
            .get_mut(row)
            .ok_or("Choose an existing zero-based path row")?;
        *point = origin;
        if let Some(mut draft) = preview.core.clone() {
            draft.points = Some(request.points);
            self.linked_path_operation_plan(draft)
        } else if let Some(mut existing) = preview.existing.clone() {
            existing.nodes[row].origin = origin;
            self.linked_path_change_plan(&existing)
        } else {
            let plan = self.linked_path_plan(&request)?;
            Ok((request, plan))
        }
    }

    pub fn update_linked_path_point(
        &mut self,
        actor: Actor,
        token: u64,
        row: usize,
        origin: DVec3,
    ) -> Result<Value, String> {
        let preview = self.checked_linked_path(token)?;
        if preview.owner != actor {
            return Err("Review the other actor's linked path before positioning it".into());
        }
        if preview.picking.is_some() || preview.handles.is_some() {
            return Err("Finish or cancel viewport positioning before editing a path row".into());
        }
        let (request, plan) = self.linked_path_point_plan(token, row, origin)?;
        self.install_linked_path(actor, request, plan)
    }
}
