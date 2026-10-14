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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Document, ObjectId, edit, surface::FaceId};
    use glam::DVec3;

    #[test]
    fn named_collision_flags_preserve_other_bits_and_no_op_representation() {
        let mut doc = Document::empty();
        let ObjectId::Solid(id) = doc
            .edit("block", |tx| {
                edit::create_block(tx, DVec3::ZERO, DVec3::splat(64.0), "brick/test")
            })
            .unwrap()
        else {
            unreachable!()
        };
        let target = FaceId {
            solid: id,
            side: doc.solid(id).unwrap().sides[0].id,
        };
        doc.edit("terrain", |tx| {
            super::super::edit::create(tx, &[target], 2, 0)
        })
        .unwrap();
        let mut info = doc.solid(id).unwrap().sides[0]
            .displacement
            .clone()
            .unwrap();
        info.flags = None;
        assert_eq!(apply(&info, Settings::default()), info);
        assert_eq!(
            apply(
                &info,
                Settings {
                    physics: Some(true),
                    ..Default::default()
                }
            ),
            info
        );
        info.flags = Some(1 | 8);
        let result = apply(
            &info,
            Settings {
                physics: Some(false),
                player: Some(false),
                ray: Some(true),
            },
        );
        assert_eq!(result.flags, Some(1 | 2 | 4));
        let mut unchanged = result.clone();
        unchanged.flags = info.flags;
        assert_eq!(unchanged, info);
        assert_eq!(
            apply(
                &result,
                Settings {
                    physics: Some(true),
                    player: Some(true),
                    ray: None
                }
            )
            .flags,
            Some(1)
        );
    }
}
