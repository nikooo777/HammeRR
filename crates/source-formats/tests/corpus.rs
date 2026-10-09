//! Round-trip tests against real Hammer maps.
//!
//! `VMF_CORPUS` selects the corpus directories (a `:`-separated list). By
//! default the Source SDK content folders of a local Steam install are used,
//! and the test is skipped when no VMF files are found.

use std::path::{Path, PathBuf};

use source_formats::keyvalues::{self, LineEnding, Node, Value, WriteOptions};
use source_formats::vmf::{Vmf, parse_numbers};

fn corpus_dirs() -> Vec<PathBuf> {
    if let Some(dirs) = std::env::var_os("VMF_CORPUS") {
        return std::env::split_paths(&dirs).collect();
    }
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let common = Path::new(&home).join(".local/share/Steam/steamapps/common");
    vec![common.join("sourcesdk_content"), common.join("SourceSDK")]
}

fn collect_vmfs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_vmfs(&path, out);
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("vmf"))
        {
            out.push(path);
        }
    }
}

#[test]
fn corpus_round_trip() {
    let mut files = Vec::new();
    for dir in corpus_dirs() {
        collect_vmfs(&dir, &mut files);
    }
    files.sort();
    if files.is_empty() {
        eprintln!("no VMF corpus found; skipping");
        return;
    }

    let failures: Vec<String> = files
        .iter()
        .filter_map(|path| {
            check(path)
                .err()
                .map(|e| format!("{}: {e}", path.display()))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} files failed:\n{}",
        failures.len(),
        files.len(),
        failures.join("\n")
    );
    eprintln!("{} VMF files round-tripped", files.len());
}

fn check(path: &Path) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let text = std::str::from_utf8(&bytes).map_err(|e| format!("not UTF-8: {e}"))?;

    let nodes = keyvalues::parse(text).map_err(|e| e.to_string())?;
    let line_ending = if text.contains("\r\n") {
        LineEnding::CrLf
    } else {
        LineEnding::Lf
    };
    let rewritten = keyvalues::write(&nodes, &WriteOptions { line_ending });
    if rewritten != text {
        let at = rewritten
            .bytes()
            .zip(text.bytes())
            .position(|(a, b)| a != b)
            .unwrap_or(rewritten.len().min(text.len()));
        return Err(format!("KeyValues rewrite differs at byte {at}"));
    }

    let vmf = Vmf::from_nodes(&nodes).map_err(|e| e.to_string())?;
    let written = vmf.to_nodes();
    compare(&nodes, &written, "")?;

    let reparsed = Vmf::from_nodes(&written).map_err(|e| format!("re-parse: {e}"))?;
    if reparsed != vmf {
        return Err("typed model changed after write and re-parse".into());
    }
    Ok(())
}

fn compare(original: &[Node], written: &[Node], path: &str) -> Result<(), String> {
    for (i, (a, b)) in original.iter().zip(written).enumerate() {
        let here = format!("{path}/{}[{i}]", a.key);
        if a.key != b.key {
            return Err(format!("{here}: key became '{}'", b.key));
        }
        match (&a.value, &b.value) {
            (Value::String(x), Value::String(y)) if equivalent(x, y) => {}
            (Value::Block(x), Value::Block(y)) => compare(x, y, &here)?,
            _ => return Err(format!("{here}: {:?} became {:?}", a.value, b.value)),
        }
    }
    if original.len() != written.len() {
        return Err(format!(
            "{path}: {} children became {}",
            original.len(),
            written.len()
        ));
    }
    Ok(())
}

/// Equal text, or the same numbers after reformatting (`1e-008` vs `0.00000001`).
fn equivalent(a: &str, b: &str) -> bool {
    a == b || matches!((parse_numbers(a), parse_numbers(b)), (Some(x), Some(y)) if x == y)
}
