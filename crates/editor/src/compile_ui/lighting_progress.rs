use serde_json::Value;

fn channel(value: &Value) -> Option<&'static str> {
    match value.as_str()? {
        "preparation" => Some("Shared preparation"),
        "ldr" => Some("LDR"),
        "hdr" => Some("HDR"),
        _ => None,
    }
}

fn phase(value: &Value) -> Option<&'static str> {
    match value.as_str()? {
        "preparation" => Some("Preparation"),
        "direct" => Some("Direct lighting"),
        "point_spot" => Some("Point and spot lights"),
        "sun" => Some("Sunlight"),
        "sky_ambient" => Some("Sky ambient"),
        "bounce" => Some("Diffuse bounce"),
        "prop_shadows" => Some("Prop shadows"),
        "prop_vertices" => Some("Prop vertex lighting"),
        "ambient_probes" => Some("Ambient probes"),
        "detail_props" => Some("Detail props"),
        "finalization" => Some("Finalization"),
        _ => None,
    }
}

pub(super) fn details(stats: &Value) -> String {
    let progress = &stats["lighting_progress"];
    if progress.is_null() {
        return String::new();
    }
    render(progress).unwrap_or_else(|| "\nLighting progress data is unavailable.".into())
}

fn render(progress: &Value) -> Option<String> {
    let complete = progress["completed"].as_bool()?;
    let planned = progress["lightmap_samples_planned"].as_u64()?;
    let completed = progress["lightmap_samples_completed"].as_u64()?;
    let planning_complete = progress["lightmap_samples_planned_complete"].as_bool()?;
    let mut text = format!(
        "\nLighting work charged: {} / {} units · {}\nLighting stage: {} · {}\nLightmap samples completed: {completed} / {planned} planned{}\nWork by channel: shared {} · LDR {} · HDR {}",
        progress["work"].as_u64()?,
        progress["limit"].as_u64()?,
        if complete {
            "complete"
        } else {
            "stopped before completion"
        },
        channel(&progress["current_channel"])?,
        phase(&progress["current_phase"])?,
        if planning_complete {
            ""
        } else {
            " (planning incomplete)"
        },
        progress["preparation_work"].as_u64()?,
        progress["ldr_work"].as_u64()?,
        progress["hdr_work"].as_u64()?,
    );
    if !complete {
        text.push_str("\nCharged work includes a rejected budget request before that work runs.");
    }
    for entry in progress["phases"].as_array()? {
        let work = entry["work"].as_u64()?;
        if work > 0 {
            text.push_str(&format!(
                "\n  {} · {}: {work} work units",
                channel(&entry["channel"])?,
                phase(&entry["phase"])?,
            ));
        }
    }
    let mut contributors = progress["contributors"]
        .as_array()?
        .iter()
        .collect::<Vec<_>>();
    contributors.sort_by_key(|entry| std::cmp::Reverse(entry["work"].as_u64()));
    for entry in contributors {
        let work = entry["work"].as_u64()?;
        if work == 0 {
            continue;
        }
        text.push_str(&format!(
            "\n  Light entity:{} · {} · {}: {work} work units",
            entry["entity_id"].as_u64()?,
            channel(&entry["channel"])?,
            phase(&entry["phase"])?,
        ));
        if !entry["instance"].is_null() {
            text.push_str(&format!(
                " ({} · child entity:{})",
                entry["instance"]["file"].as_str()?,
                entry["instance"]["original_entity_id"].as_u64()?,
            ));
            if entry["instance"]["file_truncated"].as_bool()? {
                text.push_str(" [source path truncated]");
            }
        }
    }
    let other = progress["other_contributors_work"].as_u64()?;
    let unattributed = progress["unattributed_work"].as_u64()?;
    text.push_str(&format!(
        "\nOther contributor work: {other} · Shared or unattributed work: {unattributed}"
    ));
    if progress["contributors_truncated"].as_bool()? {
        text.push_str("\nPer-light detail is limited to 64 entries; additional work remains in Other contributor work.");
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn failed_lighting_is_reported_with_real_partial_counts_and_source() {
        let stats = json!({"lighting_progress":{
            "completed":false,"work":101,"limit":100,
            "current_channel":"hdr","current_phase":"sky_ambient",
            "lightmap_samples_planned":48,"lightmap_samples_completed":29,
            "lightmap_samples_planned_complete":true,
            "preparation_work":5,"ldr_work":60,"hdr_work":36,
            "phases":[{"channel":"hdr","phase":"sky_ambient","work":36}],
            "contributors":[{"entity_id":17,"work":90,"channel":"hdr","phase":"sky_ambient",
                "instance":{"file":"courtyard.vmf","original_entity_id":23,"file_truncated":false}}],
            "other_contributors_work":0,"unattributed_work":11,"contributors_truncated":false
        }});
        let text = details(&stats);
        assert!(text.contains("101 / 100 units · stopped before completion"));
        assert!(text.contains("HDR · Sky ambient"));
        assert!(text.contains("29 / 48 planned"));
        assert!(text.contains("shared 5 · LDR 60 · HDR 36"));
        assert!(text.contains("Light entity:17"));
        assert!(text.contains("courtyard.vmf · child entity:23"));
        assert!(!text.contains("planning incomplete"));
        assert!(!text.contains("complete\n"));
    }

    #[test]
    fn absent_progress_is_optional_but_malformed_progress_is_explicit() {
        assert!(details(&json!({})).is_empty());
        assert!(details(&json!({"lighting_progress":{"completed":false}})).contains("unavailable"));
    }
}
