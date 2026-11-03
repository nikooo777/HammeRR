use glam::DVec3;
use source_formats::bsp::{Bsp, Leaf};

use crate::map::Map;
use crate::{Control, Diagnostic, Report};

const NO_SKY_AREA: i16 = 255;

pub(crate) fn validate(
    map: &Map<'_>,
    bsp: &Bsp,
    report: &mut Report,
    control: &Control,
) -> Result<(), String> {
    let mut cameras = map
        .entities
        .iter()
        .filter(|entity| entity.classname.eq_ignore_ascii_case("sky_camera"))
        .peekable();
    let Some(first) = cameras.peek() else {
        return Ok(());
    };
    control.checkpoint()?;
    let leaves = bsp.records::<Leaf>().map_err(|error| {
        format!(
            "entity:{}: sky_camera area lookup failed: {error}",
            first.id
        )
    })?;
    for camera in cameras {
        let origin = camera.origin.unwrap_or(DVec3::ZERO);
        let point = bsp
            .world_leaf_at_controlled(origin.to_array(), &mut || {
                control.checkpoint().map_err(std::io::Error::other)
            })
            .map_err(|error| {
                format!(
                    "entity:{}: sky_camera area lookup failed: {error}",
                    camera.id
                )
            })?;
        let leaf = leaves.get(point.ordinal).ok_or_else(|| {
            format!(
                "entity:{}: sky_camera leaf is outside the emitted BSP",
                camera.id
            )
        })?;
        if leaf.area() == NO_SKY_AREA {
            report.diagnostics.push(
                Diagnostic::error(
                    "Native Source treats sky_camera visibility area 255 as no 3D sky; move this camera into another visibility area",
                    Some(format!("entity:{}", camera.id)),
                    Some(origin),
                )
                .with_world_position(),
            );
        }
    }
    Ok(())
}
