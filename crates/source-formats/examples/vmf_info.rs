//! Prints a short summary of one or more VMF files.
//!
//! cargo run --release -p source-formats --example vmf_info -- map.vmf

use std::collections::BTreeMap;
use std::time::Instant;

use source_formats::vmf::Vmf;

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: vmf_info <file.vmf>...");
        std::process::exit(2);
    }
    for path in paths {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                eprintln!("{path}: {e}");
                continue;
            }
        };
        let start = Instant::now();
        let vmf = match Vmf::parse(&text) {
            Ok(vmf) => vmf,
            Err(e) => {
                eprintln!("{path}: {e}");
                continue;
            }
        };
        let elapsed = start.elapsed();

        let solids = vmf.solids().count();
        let sides: usize = vmf.solids().map(|s| s.sides.len()).sum();
        let displacements = vmf
            .solids()
            .flat_map(|s| &s.sides)
            .filter(|s| s.displacement.is_some())
            .count();
        let mut classes: BTreeMap<&str, usize> = BTreeMap::new();
        for entity in &vmf.entities {
            *classes
                .entry(entity.classname().unwrap_or("?"))
                .or_default() += 1;
        }
        let mut top: Vec<_> = classes.into_iter().collect();
        top.sort_by_key(|&(_, count)| std::cmp::Reverse(count));

        println!("{path}");
        println!(
            "  {:.1} MB parsed in {elapsed:.1?}",
            text.len() as f64 / 1_048_576.0
        );
        println!(
            "  {solids} solids, {sides} sides, {displacements} displacements, {} entities",
            vmf.entities.len()
        );
        if let Some(sky) = vmf.world.properties.get("skyname") {
            println!("  skybox: {sky}");
        }
        let summary: Vec<String> = top
            .iter()
            .take(8)
            .map(|(c, n)| format!("{c} x{n}"))
            .collect();
        println!("  top classes: {}", summary.join(", "));
    }
}
