use std::collections::BTreeSet;

use map_core::surface::FaceId;
use map_core::texture_atlas::regions::MAX_TEXTURE_AXIS;
pub use map_core::texture_atlas::{FitMode, FitOptions, FitRegion, Rectangle};
use render::materials::{material_identity, resolved_material_limited};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use source_formats::gamefs::asset_relative;
use source_formats::vmt::{DuplicateOperations, Material, texture_path, texture_transform};
use source_formats::vtf::{RgbaImage, Vtf};

use super::{Actor, Session, material_search};

pub const MAX_FACES: usize = 256;
pub const MAX_PREVIEW_SIDE: u32 = 256;
pub const MAX_TEXTURE_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_MATERIAL_BYTES: usize = 1024 * 1024;
pub const MAX_DECODE_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_ASSET_PATH_BYTES: usize = 512;
const DYNAMIC_TEXTURE_FLAGS: u32 = 0x0001_8800;

mod regions;

#[derive(Debug)]
pub struct Atlas {
    pub report: Value,
    pub image: RgbaImage,
}

#[derive(Clone)]
pub(super) struct PreviewGuard {
    owner: Actor,
    material: String,
    snapshot: String,
    options: FitOptions,
    dimensions: [u32; 2],
}

fn texture_reference(value: &str) -> Result<String, String> {
    if value.len() > MAX_ASSET_PATH_BYTES {
        return Err("Atlas texture path exceeds its byte limit".into());
    }
    let value = asset_relative(value).map_err(|error| error.to_string())?;
    asset_relative(&texture_path(&value)).map_err(|error| error.to_string())
}

fn atlas_texture(material: &Material) -> Result<String, String> {
    if !matches!(
        material.shader.as_str(),
        "lightmappedgeneric" | "unlitgeneric"
    ) || material.is_tool()
        || ["$basetexture2", "$detail"]
            .iter()
            .any(|key| material.get(key).is_some())
        || !material.blocks.is_empty()
    {
        return Err("Atlas fit supports static single-layer LightmappedGeneric and UnlitGeneric materials without proxies or fallback blocks".into());
    }
    let transform = texture_transform::<f64>(
        material.get("$basetexturetransform"),
        DuplicateOperations::Reject,
    )
    .map_err(|_| "Atlas fit requires an identity base texture transform")?;
    if transform != [1.0, 1.0, 0.0, 0.0] {
        return Err("Atlas fit requires an identity base texture transform".into());
    }
    let base = material
        .primary_base_texture()?
        .filter(|value| !value.trim().is_empty())
        .ok_or("Atlas fit needs the material's effective primary base texture")?;
    let base = texture_reference(base)?;
    if let Some(display) = material
        .get("%tooltexture")
        .filter(|value| !value.trim().is_empty())
        && texture_reference(display)? != base
    {
        return Err("The material's %tooltexture differs from its effective primary base; atlas fit cannot use that substituted editor image".into());
    }
    Ok(base)
}

fn rectangle_json(rectangle: Rectangle) -> Value {
    json!({"x":rectangle.x,"y":rectangle.y,"width":rectangle.width,"height":rectangle.height})
}

fn thumbnail(texture: &Vtf) -> Result<RgbaImage, String> {
    let mip = texture.mip_for_size(MAX_PREVIEW_SIDE);
    let (width, height) = texture.mip_size(mip);
    if u64::from(width) * u64::from(height) > MAX_DECODE_PIXELS {
        return Err("The atlas has no mip within the 16-megapixel thumbnail decode budget".into());
    }
    let image = texture
        .decode_rgba8(mip, 0, 0)
        .map_err(|error| error.to_string())?;
    let largest = image.width.max(image.height);
    if largest <= MAX_PREVIEW_SIDE {
        return Ok(image);
    }
    let width = (image.width * MAX_PREVIEW_SIDE / largest).max(1);
    let height = (image.height * MAX_PREVIEW_SIDE / largest).max(1);
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let offset = ((y * image.height / height) as usize * image.width as usize
                + (x * image.width / width) as usize)
                * 4;
            pixels.extend_from_slice(&image.pixels[offset..offset + 4]);
        }
    }
    Ok(RgbaImage {
        width,
        height,
        pixels,
    })
}

impl Session {
    pub fn has_atlas_face_preview(&self) -> bool {
        self.geometry
            .face_preview
            .as_ref()
            .is_some_and(|preview| preview.atlas.is_some())
    }

    pub fn inspect_material_atlas(&mut self, material: &str) -> Result<Atlas, String> {
        if material.len() > MAX_ASSET_PATH_BYTES {
            return Err("Atlas material path exceeds its byte limit".into());
        }
        let name = material_search::material_name(material)?;
        let material = resolved_material_limited(&self.game, &name, MAX_MATERIAL_BYTES)?
            .ok_or("Atlas material was not found in mounted content")?;
        let texture = atlas_texture(&material)?;
        let id = self.materials.load_limited(
            &self.game,
            &name,
            [MAX_MATERIAL_BYTES, MAX_TEXTURE_BYTES],
        )?;
        let cached = self.materials.get(id);
        if !cached.matches_material(&material) {
            return Err("Material declarations changed since the cached base image was loaded. Reload material assets, then inspect the atlas again.".into());
        }
        if let Some(error) = &cached.error {
            return Err(error.clone());
        }
        if cached.display_texture.as_deref() != Some(texture.as_str()) {
            return Err("The cached editor image differs from the effective atlas texture. Reload material assets, then inspect the atlas again.".into());
        }
        let base = cached
            .base
            .as_ref()
            .ok_or("Atlas base texture is unavailable")?;
        if base.authored_frames != 1
            || base.frames != 1
            || base.faces != 1
            || base.depth != 1
            || base.flags & DYNAMIC_TEXTURE_FLAGS != 0
            || base.authored_depth.is_some_and(|depth| depth != 1)
            || base.width > MAX_TEXTURE_AXIS
            || base.height > MAX_TEXTURE_AXIS
            || cached.width != base.width
            || cached.height != base.height
        {
            return Err("Atlas fit requires one static 2D base texture, at most 8192 pixels per axis, matching the editor's UV dimensions".into());
        }
        let image = thumbnail(base)?;
        let mut identity = Sha256::new();
        identity.update(b"hammerr-atlas-image-v1\0");
        identity.update((material.params.len() as u64).to_le_bytes());
        for value in std::iter::once(material.shader.as_str()).chain(
            material
                .params
                .iter()
                .flat_map(|(key, value)| [key.as_str(), value.as_str()]),
        ) {
            identity.update((value.len() as u64).to_le_bytes());
            identity.update(value.as_bytes());
        }
        identity.update(texture.as_bytes());
        identity.update(base.width.to_le_bytes());
        identity.update(base.height.to_le_bytes());
        identity.update(base.flags.to_le_bytes());
        identity.update(format!("{:?}", base.format).as_bytes());
        identity.update([base.mip_count]);
        for mip in 0..base.mip_count {
            identity.update(
                base.image_data(mip, 0, 0)
                    .map_err(|error| error.to_string())?,
            );
        }
        let image_identity = format!("{:x}", identity.finalize());
        let snapshot = format!(
            "{}:{}:{}:{}x{}",
            self.content_epoch,
            material_identity(&material),
            name,
            base.width,
            base.height
        );
        Ok(Atlas {
            report: json!({"material":name,"texture":texture,"width":base.width,"height":base.height,"image_identity":image_identity,
                "asset_generation":self.materials.generation().to_string(),"content_epoch":self.content_epoch.to_string(),"snapshot":snapshot,
                "preview_width":image.width,"preview_height":image.height,
                "coordinate_system":"Integer texture edges, origin at top left; X grows right and Y grows down.",
                "cache_policy":"Thumbnail and UV dimensions use the same cached base image as the viewport. Reload material assets to refresh external texture-file changes.",
                "limits":{"faces":MAX_FACES,"texture_bytes":MAX_TEXTURE_BYTES,"material_bytes_per_file":MAX_MATERIAL_BYTES,"preview_side":MAX_PREVIEW_SIDE,"decode_pixels":MAX_DECODE_PIXELS}}),
            image,
        })
    }

    pub fn preview_atlas_fit(
        &mut self,
        actor: Actor,
        faces: &[FaceId],
        material: &str,
        rectangle: Rectangle,
        snapshot: &str,
        token: Option<u64>,
    ) -> Result<Value, String> {
        self.preview_atlas_fit_region(
            actor,
            faces,
            material,
            FitRegion {
                rectangle,
                padding: 0,
            },
            snapshot,
            token,
        )
    }

    pub fn preview_atlas_fit_region(
        &mut self,
        actor: Actor,
        faces: &[FaceId],
        material: &str,
        region: FitRegion,
        snapshot: &str,
        token: Option<u64>,
    ) -> Result<Value, String> {
        self.preview_atlas_fit_options(
            actor,
            faces,
            material,
            FitOptions {
                region,
                mode: FitMode::PerFace,
            },
            snapshot,
            token,
        )
    }

    pub fn preview_atlas_fit_options(
        &mut self,
        actor: Actor,
        faces: &[FaceId],
        material: &str,
        options: FitOptions,
        snapshot: &str,
        token: Option<u64>,
    ) -> Result<Value, String> {
        if let Some(preview) = &self.geometry.face_preview {
            if token != Some(preview.token)
                || preview
                    .atlas
                    .as_ref()
                    .is_none_or(|guard| guard.owner != actor)
            {
                return Err(
                    "Another face preview is active; finish it before fitting an atlas region"
                        .into(),
                );
            }
        } else if token.is_some() {
            return Err("Atlas preview was closed or replaced; reopen atlas fitting".into());
        } else if !std::ptr::eq(self.render_document(), &self.document) {
            return Err("Finish the active geometry preview before fitting an atlas region".into());
        }
        let faces: BTreeSet<_> = faces.iter().copied().collect();
        if faces.is_empty() || faces.len() > MAX_FACES {
            return Err(format!("Choose between 1 and {MAX_FACES} flat brush faces"));
        }
        let atlas = self.inspect_material_atlas(material)?;
        if atlas.report["snapshot"].as_str() != Some(snapshot) {
            return Err(
                "Atlas assets changed. Inspect the material again before previewing.".into(),
            );
        }
        let dimensions = [
            atlas.report["width"].as_u64().unwrap() as u32,
            atlas.report["height"].as_u64().unwrap() as u32,
        ];
        let material = atlas.report["material"].as_str().unwrap().to_owned();
        let faces: Vec<_> = faces.into_iter().collect();
        let mut edits =
            map_core::texture_atlas::plan_fit(&self.document, &faces, options, dimensions)
                .map_err(|error| error.to_string())?;
        for (_, edit) in &mut edits {
            edit.material = Some(material.clone());
        }
        self.preview_or_apply_faces(actor, edits, true)?;
        self.geometry
            .face_preview
            .as_mut()
            .expect("created face preview")
            .atlas = Some(PreviewGuard {
            owner: actor,
            material,
            snapshot: snapshot.to_owned(),
            options,
            dimensions,
        });
        Ok(self.atlas_fit_state())
    }

    pub fn atlas_fit_state(&self) -> Value {
        let Some(preview) = &self.geometry.face_preview else {
            return json!({"preview":null});
        };
        let Some(guard) = &preview.atlas else {
            return json!({"preview":null});
        };
        let usable = guard
            .options
            .region
            .usable_rectangle(guard.dimensions)
            .expect("validated atlas region");
        let mut state = json!({"preview":{"token":preview.token.to_string(),"owner":guard.owner.label(),"material":guard.material,
            "snapshot":guard.snapshot,"rectangle":rectangle_json(guard.options.region.rectangle),"dimensions":guard.dimensions,
            "uv_bounds":usable.uv_bounds(guard.dimensions).expect("validated rectangle"),
            "faces":preview.edits.iter().map(|(face,_)|json!({"solid":face.solid,"side":face.side})).collect::<Vec<_>>()}});
        if guard.options.region.padding != 0 {
            state["preview"]["padding"] = json!(guard.options.region.padding);
            state["preview"]["usable_rectangle"] = rectangle_json(usable);
        }
        if let FitMode::Together { reference } = guard.options.mode {
            state["preview"]["mode"] = json!("together");
            state["preview"]["reference"] = json!({"solid":reference.solid,"side":reference.side});
        }
        state
    }

    pub(super) fn check_atlas_face_commit(&mut self, actor: Actor) -> Result<(), String> {
        let guard = self
            .geometry
            .face_preview
            .as_ref()
            .and_then(|preview| preview.atlas.clone());
        let Some(guard) = guard else {
            return Ok(());
        };
        if guard.owner != actor {
            return Err("Atlas preview belongs to another actor".into());
        }
        let atlas = self.inspect_material_atlas(&guard.material)?;
        if atlas.report["snapshot"].as_str() != Some(guard.snapshot.as_str()) {
            return Err(
                "Atlas assets changed after Preview. Preview the atlas fit again before applying."
                    .into(),
            );
        }
        Ok(())
    }

    pub fn cancel_owned_face_preview(
        &mut self,
        actor: Actor,
        token: Option<u64>,
    ) -> Result<(), String> {
        self.check_face_preview_token(token)?;
        if self
            .geometry
            .face_preview
            .as_ref()
            .and_then(|preview| preview.atlas.as_ref())
            .is_some_and(|guard| guard.owner != actor)
        {
            return Err("Atlas preview belongs to another actor".into());
        }
        self.clear_face_preview();
        Ok(())
    }
}
