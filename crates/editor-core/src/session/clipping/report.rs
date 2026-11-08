use map_core::clip::{self, TerrainBlendMode, TerrainCut, TerrainMode};
use serde_json::{Value, json};

pub fn terrain_mode_name(mode: TerrainMode) -> &'static str {
    match mode {
        TerrainMode::Grid => "grid",
        TerrainMode::Flat => "flat",
        TerrainMode::Cells => "cells",
        TerrainMode::Fragments => "fragments",
    }
}

pub fn blend_mode_name(mode: TerrainBlendMode) -> &'static str {
    match mode {
        TerrainBlendMode::Exact => "exact",
        TerrainBlendMode::NativeFloat => "native_float",
        TerrainBlendMode::NearestByte => "nearest_byte",
    }
}

pub fn terrain_limits() -> Value {
    json!({"sources":clip::MAX_TERRAIN_SOURCES,"patches":clip::MAX_TERRAIN_PATCHES,
        "vertices":clip::MAX_TERRAIN_VERTICES,"triangles":clip::MAX_TERRAIN_TRIANGLES,
        "neighbor_faces":clip::MAX_TERRAIN_NEIGHBOR_FACES,
        "native_map_patches":source_formats::bsp::MAX_DISPLACEMENTS})
}

pub fn terrain_report(
    terrain: &[TerrainCut],
    blend_mode: TerrainBlendMode,
    partition_neighbors: bool,
) -> Value {
    let (raw_limit, packed_limit) = blend_mode.error_limits();
    json!({"blend_mode":blend_mode_name(blend_mode),
    "partition_neighbors":partition_neighbors,
    "partitioned_neighbors":terrain.iter().filter(|cut|cut.partitioned)
        .map(|cut|json!({"solid":cut.source.solid,"side":cut.source.side})).collect::<Vec<_>>(),
    "round_blend":blend_mode == TerrainBlendMode::NearestByte,
    "terrain":terrain.iter().map(|cut| {
        let mut value = json!({
            "source":{"solid":cut.source.solid,"side":cut.source.side},
            "operation":if cut.partitioned { "partition" } else { "cut" },
            "faces":cut.faces.iter().map(|face|json!({"solid":face.solid,"side":face.side})).collect::<Vec<_>>(),
            "source_power":cut.source_power,"output_power":cut.output_power,"vertices":cut.vertices,"triangles":cut.triangles,
            "verified_neighbors":cut.verified_neighbors,"preserved_links":cut.preserved_links,
        });
        if cut.output_power.is_none() {
            value["face_powers"] = json!(cut.faces.iter().zip(&cut.face_powers)
                .map(|(face, power)|json!({"solid":face.solid,"side":face.side,"power":power}))
                .collect::<Vec<_>>());
        }
        if let Some(error) = cut.blend_error {
            value["blend_error"] = json!({
                "raw":error.raw,"packed":error.packed,
                "raw_limit":raw_limit,"packed_limit":packed_limit,
            });
        }
        value
    }).collect::<Vec<_>>(),
    "terrain_work":{
        "sources":terrain.len(),"patches":terrain.iter().map(|cut|cut.faces.len()).sum::<usize>(),
        "vertices":terrain.iter().map(|cut|cut.vertices).sum::<usize>(),
        "triangles":terrain.iter().map(|cut|cut.triangles).sum::<usize>(),
        "limits":terrain_limits()
    }})
}
