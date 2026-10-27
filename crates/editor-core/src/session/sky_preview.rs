use std::sync::Arc;

use render::{
    Camera, Projection, SurfaceView, ViewOptions,
    sky::{Channel, Face, Skybox},
};
use serde_json::{Value, json};

use super::Session;

fn source(source: compiler::sky_assets::SkyAssetSource) -> render::icons::Source {
    match source {
        compiler::sky_assets::SkyAssetSource::Directory { mount, file } => {
            render::icons::Source::Directory {
                mount: mount.display().to_string(),
                file: file.display().to_string(),
            }
        }
        compiler::sky_assets::SkyAssetSource::Vpk { index, files } => render::icons::Source::Vpk {
            index: index.display().to_string(),
            files: files
                .iter()
                .map(|file| file.display().to_string())
                .collect(),
        },
    }
}
impl Session {
    pub fn sky_preview_options(
        &self,
        mut view: ViewOptions,
        channel: Option<&str>,
        exposure_ev: Option<f64>,
    ) -> Result<ViewOptions, String> {
        if let Some(channel) = channel {
            view.sky_channel = Channel::parse(channel)?;
        }
        if let Some(exposure) = exposure_ev {
            if !exposure.is_finite() || !(-8.0..=8.0).contains(&exposure) {
                return Err("HDR sky exposure must be finite and between -8 and 8 EV".into());
            }
            view.sky_exposure_ev = exposure as f32;
        }
        Ok(view)
    }
    pub fn skybox_preview_for(&mut self, view: ViewOptions) -> Option<Arc<Skybox>> {
        if view.sky_channel == Channel::Ldr {
            return self.skybox_preview();
        }
        let authored = self.document.vmf().world.properties.get("skyname")?;
        if authored.is_empty() {
            return None;
        }
        let duplicate = self
            .document
            .vmf()
            .world
            .properties
            .get_all("skyname")
            .count()
            != 1;
        let mut end = authored.len().min(256);
        while !authored.is_char_boundary(end) {
            end -= 1;
        }
        let family = authored[..end].to_owned();
        let error = if authored.len() > 256 {
            Some("HDR sky family exceeds 256 bytes; no assets were read".to_owned())
        } else if duplicate {
            Some(
                "Duplicate world skyname keys; HDR preview is unavailable; no assets were read"
                    .to_owned(),
            )
        } else {
            self.game_error.as_ref().map(|error| {
                format!(
                    "Game content is unavailable: {}; no assets were read",
                    error.chars().take(512).collect::<String>()
                )
            })
        };
        if let Some(error) = error {
            let faces = render::sky::SUFFIXES.map(|suffix| {
                Face::hdr(
                    None,
                    format!("skybox/{family}{suffix}"),
                    None,
                    None,
                    None,
                    None,
                    Some(error.clone()),
                )
            });
            return Some(Arc::new(Skybox::hdr(family, faces)));
        }
        if let Some(cached) = self.materials.cached_hdr_skybox(&family) {
            return Some(cached);
        }
        self.materials.clear_hdr_skybox();
        let faces = compiler::sky_assets::load_hdr_sky_preview(&self.game, &family).map(|face| {
            Face::hdr(
                face.texture,
                face.material,
                face.transform.as_deref(),
                face.source.map(source),
                face.texture_path,
                face.texture_source.map(source),
                face.error,
            )
        });
        Some(self.materials.cache_hdr_skybox(Skybox::hdr(family, faces)))
    }

    pub fn sky_preview_state(&mut self, view: ViewOptions, camera: Camera) -> Value {
        let sky = self.skybox_preview_for(view);
        let reason = if !view.sky_preview {
            Some("disabled")
        } else if camera.projection != Projection::Perspective
            || view.surface != SurfaceView::Textured
        {
            Some("unsupported_view")
        } else if !sky.as_ref().is_some_and(|sky| sky.ready()) {
            Some("unavailable")
        } else {
            None
        };
        let mut report = match sky {
            None => {
                json!({"name":null,"ready":false,"summary":"Set world skyname to select a 2D sky","faces":[]})
            }
            Some(sky) => {
                json!({"name":sky.name,"ready":sky.ready(),"summary":sky.summary(),"faces":sky.faces.iter().zip(render::sky::SUFFIXES).map(|(face,suffix)| json!({"suffix":suffix,"material":face.material,"issue":face.issue,"size":face.image().map(|image| [image.width,image.height]),"texture":face.texture_path,"source":face.source,"texture_source":face.texture_source,"format":face.image().map(|image| format!("{:?}",image.format))})).collect::<Vec<_>>()})
            }
        };
        report["name_truncated"] = json!(
            view.sky_channel == Channel::Hdr
                && self
                    .document
                    .vmf()
                    .world
                    .properties
                    .get("skyname")
                    .is_some_and(|name| name.len() > 256)
        );
        report["requested_channel"] = json!(view.sky_channel.name());
        report["exposure_ev"] = json!(view.sky_exposure_ev);
        report["tonemap"] = json!(if view.sky_channel == Channel::Hdr {
            "reinhard_per_channel"
        } else {
            "none"
        });
        report["color_policy"] = json!(if view.sky_channel == Channel::Hdr {
            render::sky::HDR_POLICY
        } else {
            "Existing LDR sRGB texture sampling and sRGB target; HDR exposure is inactive"
        });
        if view.sky_channel == Channel::Hdr {
            report["limits"] = json!({"asset_lookups":128,"total_bytes":64*1024*1024,"material_bytes":1024*1024,"texture_bytes":compiler::sky_assets::MAX_HDR_PREVIEW_TEXTURE_BYTES,"texture_axis":4096,"cached_families":1,"gpu_bytes_per_set":64*1024*1024,"exposure_ev":[-8,8]});
        }
        report["eligible"] = json!(reason.is_none());
        report["suppression_reason"] = json!(reason);
        report
    }
}

#[cfg(test)]
mod tests;
