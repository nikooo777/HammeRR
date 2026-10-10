#[path = "../tests/common/mod.rs"]
#[allow(dead_code)]
mod common;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os().nth(1).ok_or("usage: room <map.vmf>")?;
    std::fs::write(path, common::room().write(&Default::default()))?;
    Ok(())
}
