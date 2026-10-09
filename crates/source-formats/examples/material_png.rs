//! Writes a material's editor texture to a PNG.
//!
//! cargo run --release -p source-formats --example material_png -- \
//!     <gameinfo.txt> de_cbble/cobbleroad01 out.png

use std::fs::File;
use std::io::BufWriter;

use source_formats::gamefs::GameFs;
use source_formats::vmt::Material;
use source_formats::vtf::Vtf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [gameinfo, material, out] = args.as_slice() else {
        eprintln!("usage: material_png <gameinfo.txt> <material> <out.png>");
        std::process::exit(2);
    };
    let fs = GameFs::from_gameinfo(gameinfo).expect("mount game");
    let material = Material::load(&fs, material)
        .expect("parse material")
        .expect("material exists");
    let (texture, bytes) = material
        .editor_textures()
        .into_iter()
        .find_map(|t| Some((t.clone(), fs.read(&t).unwrap()?)))
        .expect("material has an existing texture");
    let vtf = Vtf::parse(bytes).expect("parse vtf");
    let image = vtf.decode_rgba8(0, 0, 0).expect("decode");
    eprintln!(
        "{texture}: {}x{} {:?}, {} mips, shader {}",
        image.width, image.height, vtf.format, vtf.mip_count, material.shader
    );

    let mut encoder = png::Encoder::new(
        BufWriter::new(File::create(out).unwrap()),
        image.width,
        image.height,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&image.pixels)
        .unwrap();
}
