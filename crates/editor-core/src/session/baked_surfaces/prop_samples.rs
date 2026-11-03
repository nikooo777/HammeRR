use glam::Vec3;
use render::compiled_faces::props::{self as prop_scene, VertexSample};

use super::*;

pub const MAX_SAMPLE_ROWS: usize = MAX_ROWS;
const MARKER_HALF_EXTENT: f32 = 4.0;
const NORMAL_LENGTH: f32 = 16.0;
const GUIDE_SEGMENTS: usize = 4;
const MAX_GUIDES: usize = 1;
const MARKER_COLOR: [u8; 4] = [255, 220, 90, 255];
const NORMAL_COLOR: [u8; 4] = [80, 220, 255, 255];
const POLICY: &str = "Exact prepared native-pose hardware occurrences in native section order for the current Draw LOD and lighting channel. Duplicate and unused hardware vertices remain distinct. Hardware indices address the prop's VHV concatenation across all sections/LODs, not root VVD indices. Colors retain stored BGRA and the existing decoded/pre-EV/exposed lighting values; material coverage is independent. Framing uses a fixed point box, not an inferred authored identity. One explicit world-axis point cross and retained-normal segment; guide admission requires displayed baked props and current source/material guards. Whole-prop working-region and authored-model display rules also govern the guide. Closing the sample inspector retains the guide until Clear guide or preparation retirement; Hide and context/region suppression retain its request. No source selection, map/history edits, runtime lighting or native appearance claim.";

#[derive(Clone, Copy)]
pub(super) struct Guide {
    prop: u32,
    section: usize,
    vertex: usize,
    lod: u8,
}

impl Session {
    pub(super) fn prepared_prop_sample(
        &self,
        prop: u32,
        section: usize,
        vertex: usize,
    ) -> Result<VertexSample, String> {
        let ready = self
            .baked_surfaces
            .ready
            .as_ref()
            .ok_or("Prepare baked surfaces first")?;
        let props = ready
            .props
            .as_ref()
            .ok_or("Prepare with Compiled static props enabled first")?;
        let sample =
            props
                .scene
                .vertex_sample(prop, section, vertex, &self.baked_surfaces.options)?;
        if sample.lod != ready.preparation.draw_lod {
            return Err("Hardware sample belongs to a different Draw LOD; prepare and inspect the exact requested LOD".into());
        }
        Ok(sample)
    }

    fn prop_sample_suppression(
        &self,
        ordinal: u32,
        lod: u8,
        authored_models: bool,
    ) -> Option<&'static str> {
        if !self.baked_surfaces_visible() {
            return Some(
                self.baked_surface_suppression_reason()
                    .unwrap_or("Show the prepared Baked surfaces layer first"),
            );
        }
        if !authored_models {
            return Some("Enable Authored models to display compiled prop samples");
        }
        let Some(ready) = &self.baked_surfaces.ready else {
            return Some("The prepared baked surfaces were retired");
        };
        if lod != ready.preparation.draw_lod {
            return Some("The sample guide belongs to a different prepared Draw LOD");
        }
        let Some(prop) = ready.props.as_ref().and_then(|props| {
            props
                .scene
                .props()
                .iter()
                .find(|prop| prop.ordinal == ordinal)
        }) else {
            return Some("The prepared compiled prop is unavailable");
        };
        if !self.baked_prop_in_region(prop) {
            return Some("The compiled prop is outside the current working region");
        }
        None
    }

    pub(super) fn prop_sample_guide_report(&self, authored_models: bool) -> Value {
        let Some(guide) = self.baked_surfaces.sample_guide else {
            return json!({"requested":false,"displayed":false,"status":"not_requested","summary":"No prepared prop sample guide is requested","policy":POLICY,"guide_cap":MAX_GUIDES,"segments":GUIDE_SEGMENTS});
        };
        let sample = self.prepared_prop_sample(guide.prop, guide.section, guide.vertex);
        let reason = self.prop_sample_suppression(guide.prop, guide.lod, authored_models);
        let error = sample.as_ref().err();
        let displayed = reason.is_none() && error.is_none();
        let summary = if displayed {
            format!(
                "Guide shown for prop {}, section {}, vertex {}",
                guide.prop, guide.section, guide.vertex
            )
        } else {
            format!(
                "Prop {} guide hidden: {}",
                guide.prop,
                error
                    .map(String::as_str)
                    .or(reason)
                    .unwrap_or("sample unavailable")
            )
        };
        json!({
            "requested":true,"displayed":displayed,"status":if displayed {"displayed"} else {"suppressed"},"summary":summary,
            "token":self.baked_surfaces.serial.to_string(),"prop":guide.prop,"section":guide.section,"vertex":guide.vertex,"draw_lod":guide.lod,
            "channel":self.baked_surfaces.options.channel,"exposure_ev":self.baked_surfaces.options.exposure_ev,
            "sample":sample.as_ref().ok(),"error":error,"suppression_reason":reason,
            "point_marker_half_extent":MARKER_HALF_EXTENT,"normal_length":NORMAL_LENGTH,"segments":GUIDE_SEGMENTS,"guide_cap":MAX_GUIDES,"policy":POLICY,
        })
    }

    pub fn inspect_baked_prop_samples(
        &mut self,
        token: u64,
        prop: u32,
        offset: usize,
        limit: usize,
    ) -> Result<Value, String> {
        if !(1..=MAX_SAMPLE_ROWS).contains(&limit) || offset.checked_add(limit).is_none() {
            return Err("Prepared prop sample pages require 1–64 rows without overflow".into());
        }
        self.check_baked_surface_token(token)?;
        self.preflight_baked_surfaces();
        let control = Control::default();
        let (_, _, current) = self.baked_surface_current(&control)?;
        if !current {
            return Err("Prepared prop samples require the current owned build and unchanged preview assets; prepare again".into());
        }
        let ready = self
            .baked_surfaces
            .ready
            .as_ref()
            .ok_or("Prepare baked surfaces first")?;
        let props = ready
            .props
            .as_ref()
            .ok_or("Prepare with Compiled static props enabled first")?;
        let info = props
            .scene
            .props()
            .iter()
            .find(|info| info.ordinal == prop)
            .ok_or("Compiled prop ordinal is not present in this preparation")?;
        let sections = props.scene.sections(prop)?;
        let prop_info = json!(info);
        let page = props.scene.sample_page(
            prop,
            ready.preparation.draw_lod,
            offset,
            limit,
            &self.baked_surfaces.options,
        )?;
        let frame_blocker = if self.closing {
            Some("The editor is closing".into())
        } else {
            self.inspection_viewport_ready("framing a prepared prop sample")
                .err()
        };
        let frame_available = frame_blocker.is_none();
        let guide_reason = self
            .prop_sample_suppression(prop, ready.preparation.draw_lod, self.view.authored_models)
            .or(frame_blocker.as_deref());
        let guide_available = frame_available && guide_reason.is_none();
        let in_region = self.baked_prop_in_region(info);
        let rows = page
            .rows
            .into_iter()
            .map(|sample| {
                let mut row = json!(sample);
                row["frame_available"] = json!(frame_available);
                row["guide_available"] = json!(guide_available);
                row["in_working_region"] = json!(in_region);
                row
            })
            .collect::<Vec<_>>();
        ready.build.verify(&control, MAX_METADATA_BYTES)?;
        ready.verify_assets(&control)?;
        let (build, assets, current) = self.baked_surface_current(&control)?;
        if !current {
            self.preflight_baked_surfaces();
            return Err(
                "Prepared prop sample context changed during inspection; inspect again".into(),
            );
        }
        let summary = format!(
            "Prop {prop} · LOD {} · {} baked vertices · {}",
            page.draw_lod,
            page.total,
            self.baked_surfaces
                .options
                .channel
                .name()
                .to_ascii_uppercase(),
        );
        Ok(json!({
            "token":token.to_string(),"prop":prop,"prop_info":prop_info,"status":"ready","summary":summary,
            "draw_lod":page.draw_lod,"channel":page.channel,"exposure_ev":page.exposure_ev,"sections":sections,
            "page":{"offset":offset,"limit":limit,"total":page.total,"returned":rows.len(),"next_offset":page.next_offset,"rows":rows},
            "frame_available":frame_available,"frame_unavailable_reason":frame_blocker,"guide_available":guide_available,"guide_unavailable_reason":guide_reason,
            "in_working_region":in_region,"guide":self.prop_sample_guide_report(self.view.authored_models),
            "build":build,"preview_assets":assets,"authored_identity":null,
            "limits":{"rows":MAX_SAMPLE_ROWS,"sections":prop_scene::MAX_SECTIONS,"guides":MAX_GUIDES,"guide_segments":GUIDE_SEGMENTS},"policy":POLICY,
        }))
    }

    pub fn set_baked_prop_sample_guide(
        &mut self,
        actor: Actor,
        token: u64,
        prop: u32,
        section: usize,
        vertex: usize,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        self.check_baked_surface_token(token)?;
        self.preflight_baked_surfaces();
        self.inspection_viewport_ready("showing a prepared prop sample guide")?;
        let control = Control::default();
        if !self.baked_surface_current(&control)?.2 {
            return Err(
                "Prepared prop guides require the current owned build and unchanged preview assets"
                    .into(),
            );
        }
        let sample = self.prepared_prop_sample(prop, section, vertex)?;
        let guide = Guide {
            prop,
            section,
            vertex,
            lod: sample.lod,
        };
        if let Some(reason) =
            self.prop_sample_suppression(guide.prop, guide.lod, self.view.authored_models)
        {
            return Err(reason.into());
        }
        let ready = self
            .baked_surfaces
            .ready
            .as_ref()
            .ok_or("Prepared baked surfaces were retired")?;
        ready.build.verify(&control, MAX_METADATA_BYTES)?;
        ready.verify_assets(&control)?;
        self.baked_surfaces.sample_guide = Some(guide);
        self.scene_serial += 1;
        self.log(actor, format!("Show compiled prop {prop} sample guide"));
        Ok(self.prop_sample_guide_report(self.view.authored_models))
    }

    pub fn clear_baked_prop_sample_guide(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<Value, String> {
        self.check_baked_surface_token(token)?;
        self.baked_surfaces.sample_guide = None;
        self.scene_serial += 1;
        self.log(actor, "Cleared prepared prop sample guide");
        Ok(self.prop_sample_guide_report(self.view.authored_models))
    }

    pub(super) fn append_baked_prop_sample_guide(
        &self,
        mesh: &mut SceneMesh,
        authored_models: bool,
    ) {
        let Some(guide) = self.baked_surfaces.sample_guide else {
            return;
        };
        if self
            .prop_sample_suppression(guide.prop, guide.lod, authored_models)
            .is_some()
        {
            return;
        }
        let Ok(sample) = self.prepared_prop_sample(guide.prop, guide.section, guide.vertex) else {
            return;
        };
        let point = Vec3::from_array(sample.position);
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            for position in [
                point - axis * MARKER_HALF_EXTENT,
                point + axis * MARKER_HALF_EXTENT,
            ] {
                mesh.lines.push(render::scene::LineVertex {
                    position: position.to_array(),
                    color: MARKER_COLOR,
                });
            }
        }
        for position in [
            point,
            point + Vec3::from_array(sample.normal) * NORMAL_LENGTH,
        ] {
            mesh.lines.push(render::scene::LineVertex {
                position: position.to_array(),
                color: NORMAL_COLOR,
            });
        }
    }
}
