use std::collections::HashMap;

use glam::DVec3;
use map_core::brush::BrushGeometry;
use map_core::displacement::{lod, native, neighbors};
use source_formats::bsp::{self, Displacements};
use source_formats::vmf::{DispInfo, Solid};

use super::Geometry;
use crate::Control;

const MAX_WORK: usize = 16_000_000;

#[derive(Default)]
pub(in crate::map) struct Bounds {
    patches: Vec<Patch>,
    work: usize,
}

struct Patch {
    solid: u32,
    base: [[f32; 3]; 4],
    corners: [DVec3; 4],
    data: Displacements,
    positions: Vec<DVec3>,
    bounds: (DVec3, DVec3),
}

fn extend(bounds: &mut (DVec3, DVec3), point: DVec3) -> Result<(), String> {
    if !point.is_finite() {
        return Err("Terrain membership requires finite geometry".into());
    }
    bounds.0 = bounds.0.min(point);
    bounds.1 = bounds.1.max(point);
    Ok(())
}

fn native_bounds(
    data: &Displacements,
    base: [[f32; 3]; 4],
    bounds: &mut (DVec3, DVec3),
    control: &Control,
) -> Result<(), String> {
    for rounding in bsp::NativeDisplacementRounding::ALL {
        control.checkpoint()?;
        let profile = bsp::NativeDisplacementProfile::CssLinux64Build25338318 { rounding };
        for point in data
            .native_positions(0, base, profile)
            .map_err(|error| error.to_string())?
        {
            extend(bounds, DVec3::from_array(point.map(f64::from)))?;
        }
    }
    Ok(())
}

impl Patch {
    fn new(
        solid: u32,
        face: &map_core::brush::Face,
        disp: &DispInfo,
        control: &Control,
    ) -> Result<Self, String> {
        let geometry = Geometry::new(face, disp)?;
        let base = geometry
            .base
            .map(|point| native::stored_vertex(point).as_vec3().to_array());
        let size = geometry.mesh.size;
        let corners = [0, (size - 1) * size, size * size - 1, size - 1]
            .map(|index| native::stored_vertex(geometry.mesh.base_positions[index]));
        let data = Displacements {
            infos: vec![geometry.info],
            vertices: geometry.vertices,
            triangles: Vec::new(),
        };
        let mut bounds = (DVec3::MAX, DVec3::MIN);
        for point in geometry.mesh.positions {
            extend(&mut bounds, point)?;
        }
        let positions = data
            .positions(0, base)
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|point| DVec3::from_array(point.map(f64::from)))
            .collect::<Vec<_>>();
        for &point in &positions {
            extend(&mut bounds, point)?;
        }
        native_bounds(&data, base, &mut bounds, control)?;
        Ok(Self {
            solid,
            base,
            corners,
            data,
            positions,
            bounds,
        })
    }

    fn lod_bounds(&mut self, lod: lod::Patch, control: &Control) -> Result<(), String> {
        for &point in &lod.positions {
            extend(&mut self.bounds, point)?;
        }
        self.data.vertices = lod::encoded_vertices(&self.data.vertices, &self.positions, &lod)?;
        self.data.infos[0].allowed_vertices = lod.mask;
        for point in self
            .data
            .positions(0, self.base)
            .map_err(|error| error.to_string())?
        {
            extend(&mut self.bounds, DVec3::from_array(point.map(f64::from)))?;
        }
        native_bounds(&self.data, self.base, &mut self.bounds, control)
    }
}

fn consume(work: &mut usize, amount: usize, control: &Control) -> Result<(), String> {
    control.checkpoint()?;
    *work = work
        .checked_add(amount)
        .filter(|&work| work <= MAX_WORK)
        .ok_or("Terrain membership exceeded its geometry work budget")?;
    Ok(())
}

impl Bounds {
    pub(in crate::map) fn add(
        &mut self,
        solid: &Solid,
        geometry: &BrushGeometry,
        control: &Control,
    ) -> Result<(), String> {
        control.checkpoint()?;
        for face in &geometry.faces {
            let Some(disp) = &solid.sides[face.side].displacement else {
                continue;
            };
            if self.patches.len() >= bsp::MAX_DISPLACEMENTS {
                return Err("Terrain membership exceeds the native displacement limit".into());
            }
            let vertices = if (2..=4).contains(&disp.power) {
                ((1usize << disp.power) + 1).pow(2)
            } else {
                map_core::displacement::MAX_GRID_VERTICES
            };
            consume(&mut self.work, 5 * 32 * vertices, control)?;
            self.patches
                .push(Patch::new(solid.id, face, disp, control).map_err(|error| {
                    format!(
                        "solid:{} side:{}: terrain membership bounds: {error}",
                        solid.id, solid.sides[face.side].id
                    )
                })?);
        }
        Ok(())
    }

    pub(in crate::map) fn finish(
        mut self,
        control: &Control,
    ) -> Result<HashMap<u32, (DVec3, DVec3)>, String> {
        control.checkpoint()?;
        self.native_lod_bounds(control)
            .map_err(|error| format!("Terrain native membership bounds: {error}"))?;
        let mut bounds = HashMap::<u32, (DVec3, DVec3)>::new();
        for patch in self.patches {
            let entry = bounds
                .entry(patch.solid)
                .or_insert((DVec3::MAX, DVec3::MIN));
            extend(entry, patch.bounds.0)?;
            extend(entry, patch.bounds.1)?;
        }
        Ok(bounds)
    }

    fn native_lod_bounds(&mut self, control: &Control) -> Result<(), String> {
        let grids = self
            .patches
            .iter()
            .map(|patch| neighbors::SurfaceGrid {
                power: patch.data.infos[0].power as u32,
                corners: patch.corners,
                positions: &patch.positions,
            })
            .collect::<Vec<_>>();
        let found = neighbors::find_spans(&grids, &mut |amount| {
            consume(&mut self.work, amount, control)
        })?;
        neighbors::native_connections(&grids, &found.pairs, &mut |amount| {
            consume(&mut self.work, amount, control)
        })?;
        let (lod, _) = lod::prepare(&grids, &found.pairs, &mut |amount| {
            consume(&mut self.work, amount, control)
        })?;
        drop(grids);
        for (patch, lod) in self.patches.iter_mut().zip(lod) {
            let Some(lod) = lod else {
                continue;
            };
            consume(&mut self.work, 5 * 32 * patch.positions.len(), control)?;
            patch
                .lod_bounds(lod, control)
                .map_err(|error| format!("solid:{}: {error}", patch.solid))?;
        }
        Ok(())
    }
}
