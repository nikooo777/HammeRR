use glam::{DVec2, DVec3};
use render::Projection;
use serde::{Deserialize, Serialize};

pub const MIN_GRID: f64 = 0.125;
pub const MAX_GRID: f64 = 4096.0;
pub const MAX_EXTENT: f64 = 65_536.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Settings {
    pub grid_size: f64,
    pub snap: bool,
    pub grid_visible: bool,
    pub elevation: f64,
    pub depth: f64,
    pub draw_block: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            grid_size: 16.0,
            snap: true,
            grid_visible: true,
            elevation: 0.0,
            depth: 128.0,
            draw_block: false,
        }
    }
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct Update {
    /// Grid spacing in Hammer units, from 0.125 to 4096; fractional and custom sizes are supported.
    pub grid_size: Option<f64>,
    pub snap: Option<bool>,
    pub grid_visible: Option<bool>,
    /// Starting coordinate along the workplane's normal axis: Z for top, Y for front, X for side.
    pub elevation: Option<f64>,
    /// Positive brush thickness along the workplane's normal axis.
    pub depth: Option<f64>,
    /// Enable the planar brush-drawing tool in the human viewport.
    pub draw_block: Option<bool>,
}

impl Settings {
    pub fn updated(self, update: Update) -> Result<Self, String> {
        let next = Self {
            grid_size: update.grid_size.unwrap_or(self.grid_size),
            snap: update.snap.unwrap_or(self.snap),
            grid_visible: update.grid_visible.unwrap_or(self.grid_visible),
            elevation: update.elevation.unwrap_or(self.elevation),
            depth: update.depth.unwrap_or(self.depth),
            draw_block: update.draw_block.unwrap_or(self.draw_block),
        };
        if !next.grid_size.is_finite() || !(MIN_GRID..=MAX_GRID).contains(&next.grid_size) {
            return Err(format!(
                "Grid spacing must be between {MIN_GRID} and {MAX_GRID} units"
            ));
        }
        if !next.elevation.is_finite() || next.elevation.abs() > MAX_EXTENT {
            return Err(format!(
                "Elevation must be finite and within ±{MAX_EXTENT} units"
            ));
        }
        if !next.depth.is_finite() || !(MIN_GRID..=MAX_EXTENT).contains(&next.depth) {
            return Err(format!(
                "Brush depth must be between {MIN_GRID} and {MAX_EXTENT} units"
            ));
        }
        Ok(next)
    }

    pub fn snap_point(self, point: DVec3) -> DVec3 {
        if self.snap {
            (point / self.grid_size).round() * self.grid_size
        } else {
            point
        }
    }

    pub fn block_bounds(
        self,
        projection: Projection,
        start: DVec2,
        end: DVec2,
    ) -> Result<(DVec3, DVec3), String> {
        let (horizontal, vertical, normal) = projection
            .axes()
            .ok_or("Choose Top, Front or Side to draw a brush")?;
        if !start.is_finite() || !end.is_finite() {
            return Err("Brush corners must be finite".into());
        }
        let mut mins = DVec3::ZERO;
        let mut maxs = DVec3::ZERO;
        mins[horizontal] = start.x.min(end.x);
        maxs[horizontal] = start.x.max(end.x);
        mins[vertical] = start.y.min(end.y);
        maxs[vertical] = start.y.max(end.y);
        mins[normal] = self.elevation;
        maxs[normal] = self.elevation + self.depth;
        mins = self.snap_point(mins);
        maxs = self.snap_point(maxs);
        if self.snap && maxs[normal] <= mins[normal] {
            maxs[normal] = mins[normal] + self.grid_size;
        }
        validate_bounds(mins, maxs)?;
        Ok((mins, maxs))
    }
}

pub fn validate_bounds(mins: DVec3, maxs: DVec3) -> Result<(), String> {
    if !mins.is_finite() || !maxs.is_finite() || !mins.cmplt(maxs).all() {
        return Err("Brush must have finite corners and positive width, height and depth".into());
    }
    Ok(())
}
