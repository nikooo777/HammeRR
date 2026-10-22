use std::io;

use source_formats::bsp::{self, Bsp};

use crate::Control;

#[derive(Debug)]
pub(crate) enum SizeError {
    LumpCount,
    AlignmentOverflow,
    GameLumpOverflow,
    PayloadOverflow,
    Exceeded,
    Cancelled(String),
}

impl SizeError {
    pub(crate) fn into_io(self, label: &str) -> io::Error {
        let message = match self {
            Self::LumpCount => "BSP must have exactly 64 lump slots".into(),
            Self::AlignmentOverflow | Self::PayloadOverflow => {
                format!("{label} BSP size overflow")
            }
            Self::GameLumpOverflow => format!("{label} game-lump size overflow"),
            Self::Exceeded => format!("{label} BSP exceeds its byte limit"),
            Self::Cancelled(message) => return io::Error::other(message),
        };
        io::Error::new(io::ErrorKind::InvalidData, message)
    }
}

pub(crate) fn verify(
    bsp: &Bsp,
    pak_bytes: usize,
    map_flags_bytes: Option<usize>,
    maximum: usize,
    control: &Control,
) -> Result<(), SizeError> {
    if bsp.lumps.len() != bsp::LUMP_COUNT {
        return Err(SizeError::LumpCount);
    }
    let mut size = 8usize + bsp::LUMP_COUNT * 16 + 4;
    for (index, lump) in bsp.lumps.iter().enumerate() {
        control.checkpoint().map_err(SizeError::Cancelled)?;
        size = size
            .checked_add(3)
            .map(|value| value & !3)
            .ok_or(SizeError::AlignmentOverflow)?;
        let bytes = if index == bsp::lump::PAKFILE {
            pak_bytes
        } else if index == bsp::lump::MAP_FLAGS {
            map_flags_bytes.unwrap_or(lump.data.len())
        } else if index == bsp::lump::GAME_LUMP {
            let directory = bsp
                .game_lumps
                .len()
                .checked_mul(16)
                .and_then(|value| value.checked_add(4))
                .ok_or(SizeError::GameLumpOverflow)?;
            bsp.game_lumps
                .iter()
                .try_fold(directory, |size, lump| size.checked_add(lump.data.len()))
                .ok_or(SizeError::GameLumpOverflow)?
        } else {
            lump.data.len()
        };
        size = size.checked_add(bytes).ok_or(SizeError::PayloadOverflow)?;
        if size > maximum {
            return Err(SizeError::Exceeded);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use source_formats::bsp::GameLump;

    use super::*;

    fn fixture() -> Bsp {
        let mut bsp = Bsp::new();
        for (index, lump) in bsp.lumps.iter_mut().enumerate() {
            lump.data = vec![0; index % 7 + 1];
        }
        bsp.lumps[bsp::lump::MAP_FLAGS].data = vec![0; 9];
        bsp.game_lumps = vec![
            GameLump {
                id: *b"odd1",
                flags: 0,
                version: 1,
                data: vec![1; 3],
            },
            GameLump {
                id: *b"odd2",
                flags: 0,
                version: 1,
                data: vec![2; 5],
            },
        ];
        bsp
    }

    #[test]
    fn exact_limits_match_serialization_with_odd_lumps_and_prospective_replacements() {
        let control = Control::default();
        for (game_lumps, pak_bytes, map_flags_bytes) in
            [(false, 0, None), (true, 7, None), (true, 13, Some(4))]
        {
            let mut original = fixture();
            if !game_lumps {
                original.game_lumps.clear();
            }
            let mut serialized = original.clone();
            serialized.lumps[bsp::lump::PAKFILE].data = vec![0; pak_bytes];
            if let Some(bytes) = map_flags_bytes {
                serialized.lumps[bsp::lump::MAP_FLAGS].data = vec![0; bytes];
            }
            let maximum = serialized.to_bytes().len();
            verify(&original, pak_bytes, map_flags_bytes, maximum, &control).unwrap();
            let error =
                verify(&original, pak_bytes, map_flags_bytes, maximum - 1, &control).unwrap_err();
            assert!(matches!(error, SizeError::Exceeded));
        }
    }
}
