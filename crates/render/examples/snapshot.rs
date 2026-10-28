//! Renders a map to a PNG without opening a window.
//!
//! cargo run --release -p render --example snapshot -- \
//!     <gameinfo.txt> <map.vmf> <out.png> [x,y,z,yaw,pitch | overview] [--no-tools]
//!
//! Without a camera argument, Hammer's saved camera is used, falling back to
//! an overview of the whole map.

use std::collections::HashSet;
use std::fs::File;
use std::io::BufWriter;
use std::time::Instant;

use glam::Vec3;
use render::{Camera, GpuContext, MaterialLibrary, Renderer, SceneMesh, ViewOptions};
use source_formats::gamefs::GameFs;
use source_formats::vmf::Vmf;

const WIDTH: u32 = 1600;
const HEIGHT: u32 = 900;

fn parse_camera(arg: &str) -> Camera {
    let v: Vec<f32> = arg
        .split(',')
        .map(|n| n.trim().parse().expect("number"))
        .collect();
    let [x, y, z, yaw, pitch] = v[..] else {
        panic!("camera must be x,y,z,yaw,pitch");
    };
    Camera {
        position: Vec3::new(x, y, z),
        yaw_degrees: yaw,
        pitch_degrees: pitch,
        fov_y_degrees: 75.0,
        ..Camera::looking_at(Vec3::ZERO, Vec3::X)
    }
}

fn main() {
    let (flags, args): (Vec<String>, Vec<String>) =
        std::env::args().skip(1).partition(|a| a.starts_with("--"));
    let options = ViewOptions {
        tool_textures: !flags.iter().any(|f| f == "--no-tools"),
        ..ViewOptions::default()
    };
    let (gameinfo, map, out) = match &args[..] {
        [g, m, o, ..] => (g, m, o),
        _ => {
            eprintln!(
                "usage: snapshot <gameinfo.txt> <map.vmf> <out.png> [x,y,z,yaw,pitch | overview]"
            );
            std::process::exit(2);
        }
    };

    let start = Instant::now();
    let fs = GameFs::from_gameinfo(gameinfo).expect("mount game");
    let vmf = Vmf::parse(&std::fs::read_to_string(map).expect("read map")).expect("parse map");
    let parsed = start.elapsed();
    let mut library = MaterialLibrary::default();
    let mesh = SceneMesh::build(&vmf, &fs, &mut library, &HashSet::new());
    let built = start.elapsed();
    eprintln!(
        "parsed in {parsed:.1?}, scene built in {:.1?}: {} vertices, {} triangles, {} materials, {:?}",
        built - parsed,
        mesh.vertices.len(),
        mesh.indices.len() / 3,
        mesh.batches.len(),
        mesh.stats
    );

    let camera = match args.get(3).map(String::as_str) {
        Some("overview") => Camera::overview(mesh.mins, mesh.maxs),
        Some(arg) => parse_camera(arg),
        None => Camera::from_vmf(&vmf)
            .map(|c| Camera {
                fov_y_degrees: 75.0,
                ..c
            })
            .unwrap_or_else(|| Camera::overview(mesh.mins, mesh.maxs)),
    };
    eprintln!("camera: {camera:?}");

    let ctx = GpuContext::new().expect("GPU");
    eprintln!("adapter: {:?}", ctx.adapter.get_info().name);
    let mut renderer = Renderer::new(&ctx);
    let scene = renderer
        .upload(&ctx, &mesh, &library)
        .expect("scene upload");
    let uploaded = start.elapsed();
    let pixels = renderer.render_to_rgba(&ctx, &scene, &camera, &options, WIDTH, HEIGHT);
    eprintln!(
        "uploaded in {:.1?}, rendered in {:.1?}",
        uploaded - built,
        start.elapsed() - uploaded
    );

    let mut encoder = png::Encoder::new(
        BufWriter::new(File::create(out).expect("create png")),
        WIDTH,
        HEIGHT,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&pixels)
        .unwrap();
}
