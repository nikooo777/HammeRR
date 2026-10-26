use compiler::{Control, visibility_inspection::MAX_LEAVES};
use glam::DVec3;
use serde_json::{Value, json};
use source_formats::bsp::{Bsp, Leaf, StaticProps, lump};

use super::{Query, Session};
use crate::session::{Actor, baked_build, compiled_visibility};

pub const MAX_GUIDE_LEAVES: usize = compiled_visibility::MAX_GUIDE_LEAVES;
pub const MAX_GUIDE_BYTES: usize = compiled_visibility::MAX_GUIDE_BYTES;
const MAX_NATIVE_LEAF_BYTES: usize = MAX_LEAVES * 56;
pub(super) const MAX_PREPARATION_BYTES: usize = MAX_NATIVE_LEAF_BYTES + MAX_GUIDE_BYTES;
const LINE_VERTICES_PER_BOX: usize = 24;
const SOURCE_MODE: baked_build::SourceVerification =
    baked_build::SourceVerification::OriginalMetadata {
        maximum_bytes: 32 * 1024 * 1024,
    };

#[derive(Default)]
pub(crate) struct State {
    guide: Option<Guide>,
    displayed: bool,
}

impl State {
    pub(super) fn retained_bytes(&self) -> usize {
        self.guide.as_ref().map_or(0, |guide| guide.retained_bytes)
    }
}

pub(super) struct Guide {
    token: u64,
    prop: u32,
    first_leaf: u16,
    boxes: Vec<LeafBounds>,
    retained_bytes: usize,
    line_vertices: usize,
}

struct LeafBounds {
    ordinal: u16,
    bounds: [[f64; 3]; 2],
}

impl Session {
    fn compiled_prop_leaf_bounds_current(&self, guide: &Guide) -> bool {
        self.compiled
            .as_ref()
            .is_some_and(|build| build.lightmap_identity.token == guide.token)
            && baked_build::build_state_controlled(self, SOURCE_MODE, &Control::default())
                .is_ok_and(|state| state["current"] == true)
    }

    pub fn compiled_prop_leaf_bounds(&self) -> Value {
        let Some(guide) = &self.compiled_prop_leaf_bounds.guide else {
            return json!({"requested":false,"displayed":false});
        };
        let current = self.compiled_prop_leaf_bounds_current(guide);
        json!({
            "requested":true,"displayed":current,
            "build_token":guide.token.to_string(),"prop":guide.prop,
            "first_leaf":guide.first_leaf,"leaf_count":guide.boxes.len(),
            "native_leaves":guide.boxes.iter().map(|leaf|json!({"ordinal":leaf.ordinal,"bounds":leaf.bounds})).collect::<Vec<_>>(),
            "retained_bytes":guide.retained_bytes,"line_vertices":guide.line_vertices,
            "guide_leaf_cap":MAX_GUIDE_LEAVES,"guide_byte_cap":MAX_GUIDE_BYTES,
            "reason":if current {Value::Null} else {json!("The compiled prop leaf bounds guide is suppressed until its owned build is current")},
            "meaning":"Complete native static-prop leaf membership bounds; conservative boxes, not an exact prop mesh, convex leaf cells or runtime visibility"
        })
    }

    pub fn set_compiled_prop_leaf_bounds(
        &mut self,
        actor: Actor,
        build_token: Option<String>,
        prop: Option<u32>,
    ) -> Result<Value, String> {
        self.set_compiled_prop_leaf_bounds_controlled(actor, build_token, prop, &Control::default())
    }

    pub fn set_compiled_prop_leaf_bounds_controlled(
        &mut self,
        actor: Actor,
        build_token: Option<String>,
        prop: Option<u32>,
        control: &Control,
    ) -> Result<Value, String> {
        control.checkpoint()?;
        self.ensure_not_closing()?;
        let Some(prop) = prop else {
            if build_token.is_some() {
                return Err("Clearing prop leaf bounds accepts no build token".into());
            }
            self.compiled_prop_leaf_bounds = State::default();
            self.scene_serial += 1;
            self.log(actor, "Cleared compiled prop leaf bounds");
            return Ok(self.compiled_prop_leaf_bounds());
        };
        self.inspection_viewport_ready("showing compiled prop leaf bounds")?;
        let build_token =
            build_token.ok_or("Compiled prop leaf bounds require the returned build token")?;
        let (inspection, guide) = self.inspect_baked_prop_vertices_prepared(
            &Query {
                build_token: Some(build_token),
                prop: Some(prop),
                prop_offset: usize::try_from(prop)
                    .map_err(|_| "Native prop ordinal is too large")?,
                prop_limit: 1,
                section_limit: 1,
                vertex_limit: 1,
                ..Query::default()
            },
            control,
            true,
        )?;
        if inspection.report["build"]["current"] != true
            || inspection.report["build"]["status"] != "current"
        {
            return Err("Prop leaf bounds require a current successful owned build; refresh inspection or compile again. Previous guide is unchanged".into());
        }
        let guide = guide.ok_or("Native prop leaf bounds are unavailable")?;
        control.checkpoint()?;
        self.compiled_prop_leaf_bounds.guide = Some(guide);
        self.compiled_prop_leaf_bounds.displayed = true;
        self.scene_serial += 1;
        self.log(actor, format!("Show compiled prop {prop} leaf bounds"));
        Ok(self.compiled_prop_leaf_bounds())
    }

    pub fn preflight_compiled_prop_leaf_bounds(&mut self) {
        let displayed = self
            .compiled_prop_leaf_bounds
            .guide
            .as_ref()
            .is_some_and(|guide| self.compiled_prop_leaf_bounds_current(guide));
        if displayed != self.compiled_prop_leaf_bounds.displayed {
            self.compiled_prop_leaf_bounds.displayed = displayed;
            self.scene_serial += 1;
        }
    }

    pub(crate) fn append_compiled_prop_leaf_bounds(&self, mesh: &mut render::SceneMesh) {
        let Some(guide) = self
            .compiled_prop_leaf_bounds
            .guide
            .as_ref()
            .filter(|guide| self.compiled_prop_leaf_bounds_current(guide))
        else {
            return;
        };
        mesh.lines.reserve_exact(guide.line_vertices);
        for leaf in &guide.boxes {
            let [mins, maxs] = leaf.bounds.map(DVec3::from_array);
            mesh.add_bounds_lines(mins, maxs, [220, 140, 255, 255]);
        }
    }
}

fn storage(capacity: usize) -> Result<(usize, usize), String> {
    let retained = capacity
        .checked_mul(size_of::<LeafBounds>())
        .and_then(|bytes| bytes.checked_add(size_of::<Guide>()))
        .ok_or("Prop leaf bounds storage overflow")?;
    let line_vertices = capacity
        .checked_mul(LINE_VERTICES_PER_BOX)
        .ok_or("Prop leaf bounds line count overflow")?;
    line_vertices
        .checked_mul(size_of::<render::scene::LineVertex>())
        .and_then(|bytes| bytes.checked_add(retained))
        .filter(|&bytes| bytes <= MAX_GUIDE_BYTES)
        .ok_or("Complete prop leaf bounds exceed the 256 KiB bounds/renderer budget; previous guide is unchanged")?;
    Ok((retained, line_vertices))
}

pub(super) fn prepare(
    bsp: &Bsp,
    props: Option<&StaticProps>,
    token: u64,
    ordinal: u32,
    control: &Control,
) -> Result<Guide, String> {
    control.checkpoint()?;
    let props = props.ok_or("The owned build has no native static-prop table")?;
    let prop = props
        .props
        .get(ordinal as usize)
        .ok_or("Compiled prop ordinal is outside the native table")?;
    let start = usize::from(prop.first_leaf);
    let count = usize::from(prop.leaf_count);
    let end = start
        .checked_add(count)
        .ok_or("Native prop leaf membership range overflow")?;
    let membership = props
        .leaves
        .get(start..end)
        .ok_or("Native prop leaf membership range is invalid; previous guide is unchanged")?;
    if count > MAX_GUIDE_LEAVES {
        return Err(format!(
            "Complete prop {ordinal} guide has {count} native leaf memberships; cap is {MAX_GUIDE_LEAVES} leaves. Previous guide is unchanged. Ordinary inspection remains available"
        ));
    }
    storage(count)?;
    if bsp.lumps[lump::LEAFS].data.len() > MAX_NATIVE_LEAF_BYTES {
        return Err("Native BSP leaves exceed the prop leaf bounds inspection limit".into());
    }
    let leaves = bsp.records::<Leaf>().map_err(|error| error.to_string())?;
    if leaves.len() > MAX_LEAVES {
        return Err("Native BSP leaf count exceeds the prop leaf bounds inspection limit".into());
    }
    let mut boxes = Vec::new();
    boxes
        .try_reserve_exact(count)
        .map_err(|_| "Cannot reserve complete prop leaf bounds; previous guide is unchanged")?;
    let (retained_bytes, _) = storage(boxes.capacity())?;
    for &ordinal in membership {
        control.checkpoint()?;
        let leaf = leaves.get(usize::from(ordinal)).ok_or(
            "Native prop leaf ordinal is outside the BSP leaf table; previous guide is unchanged",
        )?;
        if leaf.mins.iter().zip(leaf.maxs).any(|(&min, max)| min > max) {
            return Err("Native prop leaf bounds are invalid; previous guide is unchanged".into());
        }
        boxes.push(LeafBounds {
            ordinal,
            bounds: [leaf.mins.map(f64::from), leaf.maxs.map(f64::from)],
        });
    }
    control.checkpoint()?;
    Ok(Guide {
        token,
        prop: ordinal,
        first_leaf: prop.first_leaf,
        boxes,
        retained_bytes,
        line_vertices: count * LINE_VERTICES_PER_BOX,
    })
}

#[cfg(test)]
mod tests;
