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

#[cfg(test)]
mod loading_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_bounded_finite_paths() {
        assert_eq!(parse("0 0 0\n\n64 32 16\r\n").unwrap().len(), 2);
        for text in [
            "",
            "0 0 0",
            "0 0 0\nNaN 0 0",
            "0 0 0\n1 2 3 4",
            "0 0 0\n1000001 0 0",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
        assert!(parse(&"0 0 0\n".repeat(MAX_POINTS + 1)).is_err());
    }

    #[test]
    fn tiny_segments_keep_direction_far_from_origin() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tiny.lin");
        std::fs::write(&path, "999999 999999 0\n999999 999999.001 0\n").unwrap();
        let mut session = Session::new(Err("no assets".into()));
        session.load_pointfile(Actor::Agent, &path).unwrap();
        session.navigate_pointfile(0, None).unwrap();
        assert!((session.camera.yaw_degrees - 90.0).abs() < 0.001);
        session.navigate_pointfile(1, None).unwrap();
        assert!((session.camera.yaw_degrees - 90.0).abs() < 0.001);
    }

    #[test]
    fn stale_compile_does_not_replace_a_newer_loaded_path() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("current.lin");
        std::fs::write(&path, "0 0 32\n64 0 32\n").unwrap();
        let mut session = Session::new(Ok(GameFs {
            game_dir: directory.path().to_path_buf(),
            base_dir: directory.path().to_path_buf(),
            mounts: Vec::new(),
        }));
        let request = session
            .begin_compile(Actor::Agent, Some(&directory.path().join("old.bsp")))
            .unwrap();
        session
            .create_block(Actor::User, DVec3::ZERO, DVec3::splat(64.0), "test", false)
            .unwrap();
        session.load_pointfile(Actor::User, &path).unwrap();
        let token = session.pointfile_report(0, 1).unwrap().token;
        let result = request.run();
        assert!(result.is_ok());
        session.finish_compile(Actor::Agent, &result);
        let report = session.pointfile_report(0, 1).unwrap();
        assert_eq!(report.token, token);
        assert!(report.current);
        assert_eq!(report.total, 2);
        assert_eq!(report.source.as_deref(), Some("file"));
    }

    #[test]
    fn framing_rejects_unrepresentable_planar_extents_without_moving_camera() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("large.lin");
        std::fs::write(&path, "-1000000 0 0\n1000000 0 0\n").unwrap();
        let mut session = Session::new(Err("no assets".into()));
        session.load_pointfile(Actor::Agent, &path).unwrap();
        session.set_projection(Actor::User, render::Projection::Top);
        let before = session.camera;
        assert!(session.frame_pointfile(1.0, None).is_err());
        assert_eq!(session.camera, before);
        assert!(session.frame_pointfile(0.001, None).is_err());
        session.set_projection(Actor::User, render::Projection::Perspective);
        session.frame_pointfile(0.01, None).unwrap();
        assert!(session.camera.position.is_finite());
    }

    #[test]
    fn load_navigate_frame_clear_and_stale_guards_preserve_the_map() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("test.lin");
        std::fs::write(&path, "0 0 32\n64 0 32\n64 0 32\n64 64 64\n").unwrap();
        let mut session = Session::new(Err("no assets".into()));
        let before = session.document.vmf().clone();
        session.load_pointfile(Actor::User, &path).unwrap();
        let report = session.pointfile_report(1, 2).unwrap();
        assert_eq!(report.total, 4);
        assert_eq!(report.points, [[64.0, 0.0, 32.0]; 2]);
        assert_eq!(report.next_offset, Some(3));
        assert_eq!(session.build_scene().leak_lines.len(), 6);
        session.navigate_pointfile(1, Some(&report.token)).unwrap();
        assert_eq!(session.camera.position, glam::Vec3::new(64.0, 0.0, 32.0));
        assert!((session.camera.yaw_degrees - 90.0).abs() < 0.001);
        session.frame_pointfile(0.5, Some(&report.token)).unwrap();
        assert!(session.navigate_pointfile(4, None).is_err());
        std::fs::write(&path, "invalid").unwrap();
        assert!(session.load_pointfile(Actor::Agent, &path).is_err());
        assert_eq!(session.pointfile_report(0, 1).unwrap().token, report.token);
        assert_eq!(session.document.vmf(), &before);
        assert_eq!(session.document.undo_labels().count(), 0);
        session.clear_pointfile(Some(&report.token)).unwrap();
        assert!(session.pointfile_points().is_none());
        assert!(session.navigate_pointfile(0, Some(&report.token)).is_err());
        std::fs::write(&path, "0 0 32\n64 0 32\n").unwrap();
        session.load_pointfile(Actor::Agent, &path).unwrap();
        session
            .create_block(Actor::Agent, DVec3::ZERO, DVec3::splat(64.0), "test", false)
            .unwrap();
        assert!(!session.pointfile_report(0, 1).unwrap().current);
        assert!(session.pointfile_points().is_none());
        assert!(session.navigate_pointfile(0, None).is_err());
        session.new_map(Actor::User, true).unwrap();
        assert_eq!(session.pointfile_report(0, 1).unwrap().total, 0);
    }
}
