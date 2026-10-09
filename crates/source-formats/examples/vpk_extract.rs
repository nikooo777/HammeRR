//! Extracts files with a given extension from a VPK.
//!
//! cargo run --release -p source-formats --example vpk_extract -- pak_dir.vpk vmt out/

use std::path::Path;

use source_formats::vpk::Vpk;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [vpk, ext, out] = args.as_slice() else {
        eprintln!("usage: vpk_extract <pak_dir.vpk> <extension> <output dir>");
        std::process::exit(2);
    };
    let vpk = Vpk::open(vpk).unwrap_or_else(|e| panic!("{vpk}: {e}"));
    let suffix = format!(".{}", ext.to_ascii_lowercase());
    let mut count = 0;
    for path in vpk.paths().filter(|p| p.ends_with(&suffix)) {
        let data = vpk.read(path).unwrap().unwrap();
        let target = Path::new(out).join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, data).unwrap();
        count += 1;
    }
    eprintln!("extracted {count} files");
}
