use std::io;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{Value, json};

use crate::isolation::Identity;
use crate::util::{self, invalid};

use super::profile::{HARDWARE_VERSION, MAX_HARDWARE, PROBE, SAMPLE_COUNT};

pub(super) fn hardware_records(path: &Path) -> io::Result<Vec<Value>> {
    let bytes = match util::read(path, MAX_HARDWARE) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(&[][..], |end| &bytes[..=end]);
    let mut records = Vec::new();
    for line in complete
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        if records.len() >= 64 {
            return Err(invalid("Hardware observer exceeded 64 events"));
        }
        let record: Value = serde_json::from_slice(line).map_err(io::Error::other)?;
        let event = record["event"]
            .as_str()
            .ok_or_else(|| invalid("Hardware observer event is missing"))?;
        if !matches!(
            event,
            "start"
                | "factory"
                | "hardware_factory"
                | "hardware_implementation"
                | "sample"
                | "complete"
        ) {
            return Err(invalid(format!("Hardware observer failure: {event}")));
        }
        records.push(record);
    }
    if complete.len() != bytes.len() && records.iter().any(|row| row["event"] == "complete") {
        return Err(invalid(
            "Hardware observer has trailing bytes after completion",
        ));
    }
    Ok(records)
}

pub(super) fn verify_hardware(records: &[Value], native: &Identity) -> io::Result<Option<Value>> {
    for row in records {
        if !matches!(
            row["event"].as_str(),
            Some(
                "start"
                    | "factory"
                    | "hardware_factory"
                    | "hardware_implementation"
                    | "sample"
                    | "complete"
            )
        ) {
            return Err(invalid(
                "Hardware observer reported an unknown or failed event",
            ));
        }
    }
    let starts: Vec<_> = records
        .iter()
        .filter(|row| row["event"] == "start")
        .collect();
    if starts.len() > 1
        || starts.first().is_some_and(|row| {
            row["pid"].as_u64() != Some(u64::from(native.pid))
                || row["hardware_version"] != HARDWARE_VERSION
        })
    {
        return Err(invalid(
            "Hardware observer differs from the isolated native process",
        ));
    }
    if records.last().is_none_or(|row| row["event"] != "complete") {
        if records.iter().any(|row| row["event"] == "complete") {
            return Err(invalid("Hardware observer emitted events after completion"));
        }
        return Ok(None);
    }
    let complete = records
        .iter()
        .filter(|row| row["event"] == "complete")
        .count();
    let samples: Vec<_> = records
        .iter()
        .filter(|row| row["event"] == "sample")
        .collect();
    let acquired = records.iter().find(|row| {
        row["event"] == "hardware_factory"
            && row["present"] == true
            && row["code"] == 0
            && row["version"] == HARDWARE_VERSION
            && matches!(
                row["module"].as_str(),
                Some("materialsystem.so" | "shaderapivk.so" | "shaderapidx9.so")
            )
    });
    let implementations: Vec<_> = records
        .iter()
        .filter(|row| row["event"] == "hardware_implementation")
        .collect();
    if starts.len() != 1
        || records.first().is_none_or(|row| row["event"] != "start")
        || complete != 1
        || samples.len() != SAMPLE_COUNT
        || acquired.is_none()
        || implementations.len() != 1
        || !matches!(
            implementations[0]["module"].as_str(),
            Some("shaderapivk.so" | "shaderapidx9.so")
        )
    {
        return Err(invalid(
            "Hardware observer requires acquisition, one start, completion and exactly eight samples",
        ));
    }
    let first_sample = records
        .iter()
        .position(|row| row["event"] == "sample")
        .ok_or_else(|| invalid("Missing hardware samples"))?;
    if !records[..first_sample]
        .iter()
        .any(|row| Some(row) == acquired)
        || !records[..first_sample]
            .iter()
            .any(|row| row["event"] == "hardware_implementation")
    {
        return Err(invalid(
            "Hardware samples preceded native interface acquisition",
        ));
    }
    let mut previous_time = None;
    for (index, row) in samples.iter().enumerate() {
        let time = row["monotonic_ns"]
            .as_u64()
            .ok_or_else(|| invalid("Invalid hardware sample timestamp"))?;
        if row["sequence"].as_u64() != Some(index as u64)
            || previous_time.is_some_and(|previous| time <= previous)
            || row["dx_support_level"]
                .as_u64()
                .is_none_or(|level| level < 90)
            || !matches!(row["hardware_hdr_type"].as_u64(), Some(1 | 2))
            || row["hdr_type"].as_u64() != Some(0)
            || row["hdr_enabled"].as_bool() != Some(false)
        {
            return Err(invalid(
                "Hardware observer did not measure stable capable LDR hardware state",
            ));
        }
        if row["dx_support_level"] != samples[0]["dx_support_level"]
            || row["hardware_hdr_type"] != samples[0]["hardware_hdr_type"]
        {
            return Err(invalid("Hardware capability changed during sampling"));
        }
        previous_time = Some(time);
    }
    Ok(Some(
        json!({"native_pid":native.pid,"native_start":native.start,"acquisition":acquired,"samples":samples,"records":records}),
    ))
}

pub(super) fn loaded_observer(directory: &Path, root: &Path, native: &Identity) -> io::Result<()> {
    let maps = util::text(
        &PathBuf::from(format!("/proc/{}/maps", native.pid)),
        4 * 1024 * 1024,
    )?;
    let pattern = Regex::new(r"^\S+\s+\S+\s+\S+\s+\S+\s+\S+\s+(.+)$").map_err(io::Error::other)?;
    for path in [
        directory.join(PROBE),
        root.join("bin/linux64/materialsystem.so"),
        root.join("bin/linux64/libdxvk_d3d9.so"),
    ] {
        let canonical = path.canonicalize()?;
        if !maps.lines().any(|line| {
            pattern
                .captures(line)
                .is_some_and(|capture| Path::new(&capture[1]) == canonical)
        }) {
            return Err(invalid(format!(
                "Did not observe pinned diagnostic module: {}",
                canonical.display()
            )));
        }
    }
    Ok(())
}
