const ICON: &str = "../../assets/branding/hammerr-icon-impact.ico";

fn main() {
    println!("cargo:rerun-if-changed={ICON}");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").expect("Cargo must set the target OS");
    if target_os != "windows" {
        return;
    }
    winresource::WindowsResource::new()
        .set_icon(ICON)
        .set("ProductName", "HammeRR")
        .compile()
        .expect("compile Windows application icon");
}
