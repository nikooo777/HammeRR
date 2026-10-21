fn main() {
    println!("cargo:rerun-if-changed=src/physics/native_polyhedron.cpp");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").expect("Cargo must set the target OS");
    let pointer_width = std::env::var("CARGO_CFG_TARGET_POINTER_WIDTH")
        .expect("Cargo must set the target pointer width");
    if target_os == "linux" && pointer_width == "64" {
        cc::Build::new()
            .cpp(true)
            .std("c++20")
            .file("src/physics/native_polyhedron.cpp")
            .compile("hammerr_polyhedron");
    }
}
