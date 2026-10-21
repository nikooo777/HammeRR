use source_formats::vmf::Entity;

use super::*;

const MAX_CUBEMAP_SIZE: u8 = 9;

impl Map<'_> {
    pub(super) fn add_cubemap(&mut self, entity: &Entity, report: &mut Report) {
        let object = Some(format!("entity:{}", entity.id));
        let origin = entity.origin().unwrap_or(DVec3::ZERO);
        if entity.solids.iter().any(|solid| !solid.hidden) {
            report.diagnostics.push(Diagnostic::error(
                "env_cubemap is a point helper; remove its brush geometry",
                object,
                Some(origin),
            ));
            return;
        }
        let size = entity
            .properties
            .get("cubemapsize")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("0")
            .parse::<u8>();
        let Ok(size) = size else {
            report.diagnostics.push(cubemap_size_error(object, origin));
            return;
        };
        if size > MAX_CUBEMAP_SIZE {
            report.diagnostics.push(cubemap_size_error(object, origin));
            return;
        }
        if self.cubemaps.len() >= bsp::MAX_CUBEMAP_SAMPLES {
            report.diagnostics.push(Diagnostic::error(
                format!(
                    "BSP cubemap sample limit is {}; remove excess visible env_cubemap helpers",
                    bsp::MAX_CUBEMAP_SAMPLES
                ),
                object,
                Some(origin),
            ));
            return;
        }
        if entity
            .properties
            .get("sides")
            .is_some_and(|value| !value.trim().is_empty())
        {
            report.diagnostics.push(Diagnostic::warning(
                "Cubemap face bindings require Generate reflections; the sample is compiled but its sides list is unused in this build",
                object,
                Some(origin),
            ));
        }
        self.cubemaps.push(bsp::Cubemap {
            origin: origin.to_array().map(|value| value as i32),
            size,
            padding: [0; 3],
        });
    }
}

fn cubemap_size_error(object: Option<String>, origin: DVec3) -> Diagnostic {
    Diagnostic::error(
        format!(
            "Cubemap Size must be an integer from 0 to {MAX_CUBEMAP_SIZE} (0 uses the game default; 1–9 select 1–256 pixels)"
        ),
        object,
        Some(origin),
    )
}
