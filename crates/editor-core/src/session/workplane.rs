use glam::DVec3;
use map_core::query::Ray;
use map_core::surface::{self, FaceId};
use map_core::workplane::{self, Frame, Grid};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Actor, Session};

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Update {
    pub origin: Option<[f64; 3]>,
    pub normal: Option<[f64; 3]>,
    pub tangent: Option<[f64; 3]>,
    pub enabled: Option<bool>,
    pub show: Option<bool>,
    pub grid_size: Option<f64>,
    pub half_extent: Option<f64>,
    pub snap: Option<bool>,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Intersection {
    pub point: [f64; 3],
    pub coordinates: [f64; 2],
    pub distance: f64,
    pub snapped: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct State {
    grid: Grid,
    enabled: bool,
    show: bool,
    snap: bool,
    serial: u64,
    source: Option<Value>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            grid: Grid::default(),
            enabled: false,
            show: true,
            snap: true,
            serial: 0,
            source: None,
        }
    }
}

impl State {
    pub(super) fn guide(&self) -> Option<Grid> {
        (self.enabled && self.show).then_some(self.grid)
    }

    pub(super) fn reset_document(&mut self) {
        *self = Self {
            serial: self.serial + 1,
            ..Self::default()
        };
    }
}

impl Session {
    pub fn workplane_stamp(&self) -> String {
        format!(
            "workplane-v1:{}:{}:{}",
            self.buffer_review_scope, self.document_generation, self.workplane.serial
        )
    }

    pub fn workplane_state(&self) -> Value {
        let state = &self.workplane;
        let frame = state.grid.frame();
        json!({
            "snapshot":self.workplane_stamp(),"serial":state.serial,
            "enabled":state.enabled,"show":state.show,"shown":state.enabled&&state.show,
            "ready":state.enabled,"snap":state.snap,
            "origin":frame.origin().to_array(),"normal":frame.normal().to_array(),
            "tangent":frame.tangent().to_array(),"bitangent":frame.bitangent().to_array(),
            "grid_size":state.grid.spacing(),"half_extent":state.grid.half_extent(),
            "display_spacing":state.grid.display_spacing(),"source":state.source,
            "scope":"session","persisted":false,
            "render_policy":"reference_overlay","occlusion":"drawn_over_geometry",
            "limits":{"world_coordinate":workplane::MAX_COORDINATE,"grid_min":workplane::MIN_GRID,
                "grid_max":workplane::MAX_GRID,"extent_min":workplane::MIN_GRID,
                "extent_max":workplane::MAX_COORDINATE,"display_half_cells":workplane::MAX_HALF_CELLS},
        })
    }

    fn check_workplane_snapshot(&self, expected: Option<&str>) -> Result<(), String> {
        self.ensure_not_closing()?;
        if expected.is_some_and(|expected| expected != self.workplane_stamp()) {
            return Err(
                "Workplane changed; inspect Workplane again before applying settings".into(),
            );
        }
        Ok(())
    }

    fn adopt_workplane(&mut self, actor: Actor, mut state: State) -> Value {
        if state != self.workplane {
            state.serial = self.workplane.serial + 1;
            self.workplane = state;
            self.view_serial += 1;
            self.log(actor, "Changed workplane");
        }
        self.workplane_state()
    }

    pub fn set_workplane(
        &mut self,
        actor: Actor,
        update: Update,
        expected: Option<&str>,
    ) -> Result<Value, String> {
        self.check_workplane_snapshot(expected)?;
        let previous = &self.workplane;
        let current = previous.grid.frame();
        let origin = update
            .origin
            .map(DVec3::from_array)
            .unwrap_or(current.origin());
        let frame = if update.normal.is_some() || update.tangent.is_some() {
            Frame::new(
                origin,
                update
                    .normal
                    .map(DVec3::from_array)
                    .unwrap_or(current.normal()),
                update.tangent.map(DVec3::from_array),
            )?
        } else {
            current.with_origin(origin)?
        };
        let grid = Grid::new(
            frame,
            update.grid_size.unwrap_or(previous.grid.spacing()),
            update.half_extent.unwrap_or(previous.grid.half_extent()),
        )?;
        let geometry_changed =
            update.origin.is_some() || update.normal.is_some() || update.tangent.is_some();
        let state = State {
            grid,
            enabled: update.enabled.unwrap_or(previous.enabled),
            show: update.show.unwrap_or(previous.show),
            snap: update.snap.unwrap_or(previous.snap),
            serial: previous.serial,
            source: if geometry_changed {
                None
            } else {
                previous.source.clone()
            },
        };
        Ok(self.adopt_workplane(actor, state))
    }

    pub fn reset_workplane(
        &mut self,
        actor: Actor,
        expected: Option<&str>,
    ) -> Result<Value, String> {
        self.check_workplane_snapshot(expected)?;
        let state = State {
            serial: self.workplane.serial,
            ..State::default()
        };
        Ok(self.adopt_workplane(actor, state))
    }

    fn workplane_from_frame(
        &mut self,
        actor: Actor,
        frame: Frame,
        source: Value,
    ) -> Result<Value, String> {
        let grid = Grid::new(
            frame,
            self.workplane.grid.spacing(),
            self.workplane.grid.half_extent(),
        )?;
        let state = State {
            grid,
            enabled: true,
            show: true,
            source: Some(source),
            ..self.workplane.clone()
        };
        Ok(self.adopt_workplane(actor, state))
    }

    pub fn workplane_from_face(
        &mut self,
        actor: Actor,
        face: FaceId,
        expected: Option<&str>,
    ) -> Result<Value, String> {
        self.check_workplane_snapshot(expected)?;
        let side =
            surface::side(&self.document, face).ok_or("Authored workplane face does not exist")?;
        if side.displacement.is_some() {
            return Err(
                "Terrain has no single face plane; pick a surface point and normal instead".into(),
            );
        }
        let plane = map_core::Plane::from_points(side.plane.0)
            .ok_or("Authored face has an invalid plane")?;
        let vertices =
            surface::face_vertices(&self.document, face).map_err(|error| error.to_string())?;
        if vertices.len() < 3 {
            return Err("Authored face has no bounded polygon".into());
        }
        let origin = vertices.iter().copied().sum::<DVec3>() / vertices.len() as f64;
        let tangent = vertices
            .iter()
            .zip(vertices.iter().cycle().skip(1))
            .take(vertices.len())
            .map(|(a, b)| b - a)
            .max_by(|a, b| a.length_squared().total_cmp(&b.length_squared()))
            .ok_or("Authored face has no usable edge")?;
        let frame = Frame::new(origin, plane.normal, Some(tangent))?;
        self.workplane_from_frame(
            actor,
            frame,
            json!({"kind":"authored_face_snapshot","solid":face.solid,"side":face.side}),
        )
    }

    pub fn workplane_from_ray(
        &mut self,
        actor: Actor,
        ray: Ray,
        expected: Option<&str>,
    ) -> Result<Value, String> {
        self.check_workplane_snapshot(expected)?;
        let surface = self.placement_surface(&[], ray, true)?;
        let frame = Frame::new(surface.hit.point, surface.normal, None)?;
        self.workplane_from_frame(actor, frame, json!({"kind":"surface_snapshot","object":crate::identity::id_string(surface.hit.object),"solid":surface.hit.solid,"side":surface.hit.side}))
    }

    pub fn workplane_intersection(
        &self,
        ray: Ray,
        snap: Option<bool>,
    ) -> Result<Intersection, String> {
        if !self.workplane.enabled {
            return Err("Enable a workplane before intersecting a ray with it".into());
        }
        let hit = self
            .workplane
            .grid
            .intersect(ray, snap.unwrap_or(self.workplane.snap))?;
        Ok(Intersection {
            point: hit.point.to_array(),
            coordinates: hit.coordinates.to_array(),
            distance: hit.distance,
            snapped: hit.snapped,
        })
    }
}
