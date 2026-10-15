use source_formats::{keyvalues::Node, vmf::Vmf};

pub(super) fn validate(vmf: &Vmf) -> Result<(), String> {
    for node in vmf.cordon.iter().chain(&vmf.extra) {
        if !node.key.eq_ignore_ascii_case("cordon") && !node.key.eq_ignore_ascii_case("cordons") {
            continue;
        }
        let children = node.children().ok_or(
            "Imported cordon metadata must be a block; compile regions are not implemented",
        )?;
        let flags = children
            .iter()
            .filter(|child| child.key.eq_ignore_ascii_case("active"))
            .collect::<Vec<&Node>>();
        let active = match flags.as_slice() {
            [] => false,
            [flag] => match flag.as_str().map(str::trim) {
                Some(value) if value == "0" || value.eq_ignore_ascii_case("false") => false,
                Some(value) if value == "1" || value.eq_ignore_ascii_case("true") => true,
                _ => return Err("Imported cordon has an invalid active flag; use 0/1 or false/true before building. Compile regions are not implemented".into()),
            },
            _ => return Err("Imported cordon has repeated active flags; resolve them before building. Compile regions are not implemented".into()),
        };
        if active {
            return Err("Compile regions are not implemented; active imported cordon metadata cannot be built safely. Disable the cordon in the source editor and save the VMF to build the full map".into());
        }
    }
    Ok(())
}
