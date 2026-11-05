use std::collections::BTreeSet;
use std::path::PathBuf;

use render::{Camera, Projection};
use serde::{Deserialize, Serialize};

use crate::preference_file::{CoordinatedFile, ObservedSource, Publication, SaveError, Source};

pub const FILE_NAME: &str = "saved-workspace.json";
pub const MAX_FILE_BYTES: u64 = 128 * 1024;
const FORMAT_VERSION: u32 = 1;
const LOCK_FILE_NAME: &str = ".saved-workspace.lock";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraSnapshot {
    pub position: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub fov: f32,
    pub projection: String,
    pub ortho_height: f32,
}

impl CameraSnapshot {
    pub fn from_camera(camera: Camera) -> Self {
        Self {
            position: camera.position.to_array(),
            yaw: camera.yaw_degrees,
            pitch: camera.pitch_degrees,
            fov: camera.fov_y_degrees,
            projection: camera.projection.name().into(),
            ortho_height: camera.ortho_height,
        }
    }

    pub fn camera(&self) -> Result<Camera, String> {
        let camera = Camera {
            position: glam::Vec3::from_array(self.position),
            yaw_degrees: self.yaw,
            pitch_degrees: self.pitch,
            fov_y_degrees: self.fov,
            projection: Projection::parse(&self.projection)
                .ok_or("Saved workspace has an unknown camera projection")?,
            ortho_height: self.ortho_height,
        };
        camera.validate(1.0)?;
        Ok(camera)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewSnapshot {
    pub layout: crate::session::view_layout::Layout,
    pub active_pane: usize,
    pub linked_orthographic: bool,
    pub cameras: [CameraSnapshot; crate::session::view_layout::MAX_PANES],
}

impl ViewSnapshot {
    pub fn validate(&self) -> Result<(), String> {
        if self.active_pane >= self.layout.pane_count() {
            return Err("Saved workspace active pane must belong to its view layout".into());
        }
        for camera in &self.cameras {
            camera.camera()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentSnapshot {
    pub entry: crate::recent_maps::Entry,
    pub views: ViewSnapshot,
    pub layout: crate::workspace_preferences::Layout,
}

impl DocumentSnapshot {
    pub fn validate(&self) -> Result<(), String> {
        self.entry.validate()?;
        let content = self
            .entry
            .content
            .as_ref()
            .ok_or("Saved workspace documents require a retained content configuration")?;
        if content.fgd.is_none() || self.entry.game_directory.is_none() {
            return Err("Saved workspace documents require explicit content and FGD paths".into());
        }
        self.views.validate()?;
        self.layout.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub documents: Vec<DocumentSnapshot>,
}

impl Snapshot {
    pub fn validate(&self) -> Result<(), String> {
        if self.documents.is_empty() || self.documents.len() > crate::startup::DOCUMENT_LIMIT {
            return Err(format!(
                "Save a workspace with 1–{} named documents",
                crate::startup::DOCUMENT_LIMIT
            ));
        }
        let mut paths = BTreeSet::new();
        for document in &self.documents {
            document.validate()?;
            if !paths.insert(&document.entry.path) {
                return Err("Saved workspace documents must have distinct map paths".into());
            }
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    version: u32,
    snapshot: Snapshot,
}

pub struct Store {
    pub directory: PathBuf,
}

impl Store {
    pub fn path(&self) -> PathBuf {
        self.directory.join(FILE_NAME)
    }

    fn target(&self) -> CoordinatedFile<'_> {
        CoordinatedFile {
            directory: &self.directory,
            file_name: FILE_NAME,
            lock_name: LOCK_FILE_NAME,
            label: "Saved workspace",
            reload: "Reload saved workspace",
            saved_description: "workspace",
            max_bytes: MAX_FILE_BYTES,
        }
    }

    pub(crate) fn load_snapshot(&self) -> Result<(Option<Snapshot>, ObservedSource), String> {
        let source = self.target().observe()?;
        let snapshot = self.parse_source(&source)?;
        Ok((snapshot, source))
    }

    pub(crate) fn read_current(&self, expected: &ObservedSource) -> Result<ObservedSource, String> {
        let current = self.target().observe_from(expected)?;
        if current.source != expected.source {
            return Err("Saved workspace changed; reload it before saving or restoring".into());
        }
        Ok(current)
    }

    pub(crate) fn parse_source(&self, source: &ObservedSource) -> Result<Option<Snapshot>, String> {
        let Source::File { bytes, length, .. } = &source.source else {
            return Ok(None);
        };
        if *length > MAX_FILE_BYTES {
            return Err(format!(
                "{} exceeds {MAX_FILE_BYTES} bytes; file is preserved",
                self.path().display()
            ));
        }
        let file: File = serde_json::from_slice(bytes)
            .map_err(|error| format!("{}: {error}", self.path().display()))?;
        if file.version != FORMAT_VERSION {
            return Err(format!(
                "Unsupported saved workspace version {}; file is preserved",
                file.version
            ));
        }
        file.snapshot.validate()?;
        Ok(Some(file.snapshot))
    }

    pub(crate) fn save_checked(
        &self,
        snapshot: &Snapshot,
        expected: &ObservedSource,
    ) -> Result<Publication, SaveError> {
        snapshot.validate().map_err(SaveError::failure)?;
        self.parse_source(expected).map_err(SaveError::failure)?;
        let bytes = serde_json::to_vec_pretty(&File {
            version: FORMAT_VERSION,
            snapshot: snapshot.clone(),
        })
        .map_err(|error| SaveError::failure(error.to_string()))?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(SaveError::failure(format!(
                "Saved workspace must fit in {MAX_FILE_BYTES} bytes"
            )));
        }
        self.target().publish(&bytes, expected)
    }
}
