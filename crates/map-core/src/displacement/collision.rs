use source_formats::vmf::DispInfo;

#[derive(Debug, Clone, Copy, Default)]
pub struct Settings {
    pub physics: Option<bool>,
    pub player: Option<bool>,
    pub ray: Option<bool>,
}

#[derive(Debug, Clone, Copy)]
pub struct State {
    pub physics: bool,
    pub player: bool,
    pub ray: bool,
}

pub fn state(flags: u32) -> State {
    State {
        physics: flags & 2 == 0,
        player: flags & 4 == 0,
        ray: flags & 8 == 0,
    }
}

pub fn apply(info: &DispInfo, settings: Settings) -> DispInfo {
    let mut result = info.clone();
    let before = info.flags.unwrap_or(0);
    let mut flags = before;
    for (enabled, bit) in [
        (settings.physics, 2),
        (settings.player, 4),
        (settings.ray, 8),
    ] {
        if let Some(enabled) = enabled {
            if enabled {
                flags &= !bit;
            } else {
                flags |= bit;
            }
        }
    }
    if flags != before {
        result.flags = Some(flags);
    }
    result
}
