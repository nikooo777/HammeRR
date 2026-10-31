use map_core::texture_atlas::regions::{KEY, Regions};
use source_formats::vmf::Vmf;

pub(super) fn admit(child: &Vmf) -> Result<(), String> {
    if child.extra.iter().any(|node| {
        node.key != KEY
            && node.key != map_core::color_palette::KEY
            && node.key != map_core::reference_images::KEY
            && !node.key.eq_ignore_ascii_case("cordons")
    }) {
        return Err("Instance child has unknown or compiler-owned map metadata".into());
    }
    Regions::parse(child)
        .map_err(|error| format!("Instance child atlas region metadata: {error}"))?;
    map_core::reference_images::Metadata::parse(child)
        .map_err(|error| format!("Instance child reference image metadata: {error}"))?;
    map_core::color_palette::Metadata::parse(child)
        .map_err(|error| format!("Instance child color palette metadata: {error}"))?;
    Ok(())
}
