use serde::Serialize;

use super::*;

pub const MAX_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_POINTS: usize = 65_536;
pub const MAX_PAGE: usize = 200;
const MAX_COORDINATE: f64 = 1_000_000.0;

#[derive(Default)]
pub struct State {
    serial: u64,
    loaded: Option<Loaded>,
    index: usize,
}

struct Loaded {
    path: PathBuf,
    points: Vec<DVec3>,
    snapshot: Snapshot,
}

#[derive(Serialize)]
pub struct Report {
    pub token: String,
    pub source: Option<String>,
    pub path: Option<String>,
    pub current: bool,
    pub total: usize,
    pub index: usize,
    pub position: Option<[f64; 3]>,
    pub length: f64,
    pub bounds: Option<[[f64; 3]; 2]>,
    pub offset: usize,
    pub next_offset: Option<usize>,
    pub points: Vec<[f64; 3]>,
}

fn parse(text: &str) -> Result<Vec<DVec3>, String> {
    let mut points = Vec::new();
    for (line, value) in text.lines().enumerate() {
        if value.trim().is_empty() {
            continue;
        }
        let point = source_formats::vmf::parse_vec3(value)
            .filter(|point| point.is_finite() && point.abs().max_element() <= MAX_COORDINATE)
            .ok_or_else(|| format!("Pointfile line {} must contain three finite coordinates within ±{MAX_COORDINATE}", line + 1))?;
        points.push(point);
        if points.len() > MAX_POINTS {
            return Err(format!("Pointfiles support at most {MAX_POINTS} points"));
        }
    }
    if points.len() < 2 {
        return Err("Pointfile needs at least two points".into());
    }
    Ok(points)
}

impl Session {
    pub(super) fn reset_pointfile(&mut self) {
        self.pointfile.loaded = None;
        self.pointfile.index = 0;
        self.pointfile.serial += 1;
    }

    pub fn pointfile_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            self.pointfile.serial,
            self.pointfile.index
        )
    }

    fn pointfile_data(&self) -> (&[DVec3], bool) {
        if let Some(loaded) = &self.pointfile.loaded {
            return (&loaded.points, self.current_snapshot(&loaded.snapshot));
        }
        self.leak()
            .map_or((&[], false), |leak| (&leak.points, true))
    }

    pub fn pointfile_points(&self) -> Option<&[DVec3]> {
        let (points, current) = self.pointfile_data();
        (current && !points.is_empty()).then_some(points)
    }

    pub fn pointfile_report(&self, offset: usize, limit: usize) -> Result<Report, String> {
        if !(1..=MAX_PAGE).contains(&limit) {
            return Err(format!("Pointfile page size must be 1–{MAX_PAGE}"));
        }
        let (points, current) = self.pointfile_data();
        let bounds = query::union(points.iter().map(|point| (*point, *point)));
        let total = points.len();
        let rows = points
            .iter()
            .skip(offset)
            .take(limit)
            .map(|point| point.to_array())
            .collect::<Vec<_>>();
        let end = offset.saturating_add(rows.len());
        Ok(Report {
            token: self.pointfile.serial.to_string(),
            source: if self.pointfile.loaded.is_some() {
                Some("file".into())
            } else if total > 0 {
                Some("compile".into())
            } else {
                None
            },
            path: self
                .pointfile
                .loaded
                .as_ref()
                .map(|loaded| loaded.path.display().to_string()),
            current,
            total,
            index: self.pointfile.index.min(total.saturating_sub(1)),
            position: points
                .get(self.pointfile.index.min(total.saturating_sub(1)))
                .map(|point| point.to_array()),
            length: points
                .windows(2)
                .map(|pair| pair[0].distance(pair[1]))
                .sum(),
            bounds: bounds.map(|(min, max)| [min.to_array(), max.to_array()]),
            offset,
            next_offset: (end < total).then_some(end),
            points: rows,
        })
    }

    pub fn load_pointfile(&mut self, actor: Actor, path: &Path) -> Result<(), String> {
        self.ensure_not_closing()?;
        let bytes = crate::bounded_file::Source::capture(path, MAX_BYTES)
            .and_then(crate::bounded_file::Source::read)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let text = String::from_utf8(bytes).map_err(|error| error.to_string())?;
        let points = parse(&text)?;
        let count = points.len();
        self.reset_pointfile();
        self.pointfile.loaded = Some(Loaded {
            path: path.to_owned(),
            points,
            snapshot: Snapshot {
                generation: self.document_generation,
                revision: self.document.revision(),
            },
        });
        self.set_view(ViewOptions {
            leak_path: true,
            ..self.view
        });
        self.scene_serial += 1;
        self.log(actor, format!("loaded pointfile with {count} points"));
        Ok(())
    }

    fn guard_pointfile(&self, token: Option<&str>, require_current: bool) -> Result<(), String> {
        if token.is_some_and(|token| token != self.pointfile.serial.to_string()) {
            return Err(
                "Pointfile changed. Inspect it again before navigating or clearing.".into(),
            );
        }
        let (points, current) = self.pointfile_data();
        if require_current && (!current || points.is_empty()) {
            return Err(
                "No current pointfile. Load a file or compile again after map edits.".into(),
            );
        }
        Ok(())
    }

    pub fn clear_pointfile(&mut self, token: Option<&str>) -> Result<(), String> {
        self.guard_pointfile(token, false)?;
        self.reset_pointfile();
        self.leak = None;
        self.scene_serial += 1;
        Ok(())
    }

    pub fn navigate_pointfile(&mut self, index: usize, token: Option<&str>) -> Result<(), String> {
        self.guard_pointfile(token, true)?;
        let points = self.pointfile_points().unwrap();
        let position = *points
            .get(index)
            .ok_or("Point index is outside the path (indices start at zero)")?;
        let mut camera = self.camera;
        if camera.projection == render::Projection::Perspective {
            let direction = points[index + 1..]
                .iter()
                .find(|point| point.distance_squared(position) > 1e-8)
                .map(|point| *point - position)
                .or_else(|| {
                    points[..index]
                        .iter()
                        .rev()
                        .find(|point| point.distance_squared(position) > 1e-8)
                        .map(|point| position - *point)
                });
            camera.position = position.as_vec3();
            if let Some(direction) = direction {
                let direction = direction.normalize();
                camera.yaw_degrees = direction.y.atan2(direction.x).to_degrees() as f32;
                camera.pitch_degrees = direction.z.clamp(-1.0, 1.0).asin().to_degrees() as f32;
            }
        } else {
            camera = Camera::planar(camera.projection, position.as_vec3(), camera.ortho_height);
        }
        self.pointfile.index = index;
        self.set_camera(camera);
        Ok(())
    }

    pub fn frame_pointfile(&mut self, aspect: f32, token: Option<&str>) -> Result<(), String> {
        self.guard_pointfile(token, true)?;
        if !aspect.is_finite() || !(0.01..=100.0).contains(&aspect) {
            return Err("Viewport aspect must be finite and between 0.01 and 100".into());
        }
        let points = self.pointfile_points().unwrap();
        let (min, max) = query::union(points.iter().map(|point| (*point, *point))).unwrap();
        if self.camera.projection != render::Projection::Perspective {
            let size = (max - min).as_vec3();
            let height = size
                .dot(self.camera.up().abs())
                .max(size.dot(self.camera.right().abs()) / aspect)
                * 1.2;
            if height > Camera::MAX_ORTHO_HEIGHT {
                return Err("Path exceeds the planar zoom limit. Use perspective framing or navigate individual points.".into());
            }
        }
        self.set_camera(
            self.camera
                .frame_bounds(min.as_vec3(), max.as_vec3(), aspect),
        );
        Ok(())
    }
}
