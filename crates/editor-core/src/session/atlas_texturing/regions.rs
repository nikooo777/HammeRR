use map_core::texture_atlas::regions::{self, Region, Regions};

use super::*;

fn region_json(region: &Region) -> Value {
    json!({"id":region.id,"name":region.name,"material":region.material,
        "image_identity":region.image_identity,"dimensions":region.dimensions,
        "rectangle":rectangle_json(region.region.rectangle),"padding":region.region.padding})
}

impl Session {
    pub fn atlas_regions_snapshot(&self) -> String {
        format!(
            "{}:{}",
            self.document_generation(),
            self.document().revision()
        )
    }

    fn check_atlas_regions_snapshot(&self, snapshot: &str) -> Result<(), String> {
        if snapshot != self.atlas_regions_snapshot() {
            return Err(
                "Map changed. List the saved atlas regions again before using this action.".into(),
            );
        }
        Ok(())
    }

    fn atlas_region_edit_admission(&self, snapshot: &str) -> Result<(), String> {
        self.check_atlas_regions_snapshot(snapshot)?;
        if self.closing {
            return Err("The editor is closing".into());
        }
        if self.has_face_preview()
            || !std::ptr::eq(self.render_document(), &self.document)
            || self.block_preview.is_some()
        {
            return Err(
                "Apply or cancel the active geometry preview before changing saved atlas regions."
                    .into(),
            );
        }
        Ok(())
    }

    pub fn atlas_regions_state(&self) -> Result<Value, String> {
        let catalog = Regions::parse(self.document.vmf())?;
        Ok(
            json!({"snapshot":self.atlas_regions_snapshot(),"max_regions":regions::MAX_REGIONS,
            "max_name_characters":regions::MAX_NAME_CHARS,
            "regions":catalog.regions.iter().map(region_json).collect::<Vec<_>>()}),
        )
    }

    pub fn save_atlas_region(
        &mut self,
        actor: Actor,
        name: &str,
        material: &str,
        region: FitRegion,
        atlas_snapshot: &str,
        catalog_snapshot: &str,
    ) -> Result<Value, String> {
        self.atlas_region_edit_admission(catalog_snapshot)?;
        let mut catalog = Regions::parse(self.document.vmf())?;
        let atlas = self.inspect_material_atlas(material)?;
        if atlas.report["snapshot"].as_str() != Some(atlas_snapshot) {
            return Err(
                "Atlas assets changed. Inspect the material again before saving a region.".into(),
            );
        }
        let id = catalog.next_id()?;
        catalog.regions.push(Region {
            id,
            name: name.trim().into(),
            material: atlas.report["material"].as_str().unwrap().into(),
            image_identity: atlas.report["image_identity"].as_str().unwrap().into(),
            dimensions: [
                atlas.report["width"].as_u64().unwrap() as u32,
                atlas.report["height"].as_u64().unwrap() as u32,
            ],
            region,
            extra: Vec::new(),
        });
        catalog.validate()?;
        self.edit(actor, "save atlas region", |tx| catalog.write(tx))?;
        let mut state = self.atlas_regions_state()?;
        state["saved_id"] = json!(id);
        Ok(state)
    }

    pub fn rename_atlas_region(
        &mut self,
        actor: Actor,
        id: u32,
        name: &str,
        catalog_snapshot: &str,
    ) -> Result<Value, String> {
        self.atlas_region_edit_admission(catalog_snapshot)?;
        let mut catalog = Regions::parse(self.document.vmf())?;
        catalog
            .regions
            .iter_mut()
            .find(|region| region.id == id)
            .ok_or("Saved atlas region no longer exists")?
            .name = name.trim().into();
        catalog.validate()?;
        self.edit(actor, "rename atlas region", |tx| catalog.write(tx))?;
        self.atlas_regions_state()
    }

    pub fn delete_atlas_region(
        &mut self,
        actor: Actor,
        id: u32,
        catalog_snapshot: &str,
    ) -> Result<Value, String> {
        self.atlas_region_edit_admission(catalog_snapshot)?;
        let mut catalog = Regions::parse(self.document.vmf())?;
        if !catalog.regions.iter().any(|region| region.id == id) {
            return Err("Saved atlas region no longer exists".into());
        }
        catalog.regions.retain(|region| region.id != id);
        self.edit(actor, "delete atlas region", |tx| catalog.write(tx))?;
        self.atlas_regions_state()
    }

    pub fn choose_atlas_region(
        &mut self,
        id: u32,
        material: &str,
        atlas_snapshot: &str,
        catalog_snapshot: &str,
    ) -> Result<Value, String> {
        self.check_atlas_regions_snapshot(catalog_snapshot)?;
        let catalog = Regions::parse(self.document.vmf())?;
        let region = catalog
            .regions
            .iter()
            .find(|region| region.id == id)
            .ok_or("Saved atlas region no longer exists")?;
        let atlas = self.inspect_material_atlas(material)?;
        if atlas.report["snapshot"].as_str() != Some(atlas_snapshot) {
            return Err(
                "Atlas assets changed. Inspect the material again before choosing a region.".into(),
            );
        }
        if atlas.report["material"].as_str() != Some(region.material.as_str()) {
            return Err("The saved atlas region belongs to a different material. Inspect that material first.".into());
        }
        let dimensions = [
            atlas.report["width"].as_u64().unwrap() as u32,
            atlas.report["height"].as_u64().unwrap() as u32,
        ];
        if dimensions != region.dimensions
            || atlas.report["image_identity"].as_str() != Some(region.image_identity.as_str())
        {
            return Err("The saved atlas region's material or image changed. Its original pixel region cannot be reused; inspect the image and save a new region.".into());
        }
        let mut result = region_json(region);
        result["snapshot"] = atlas.report["snapshot"].clone();
        result["catalog_snapshot"] = json!(self.atlas_regions_snapshot());
        Ok(result)
    }
}
