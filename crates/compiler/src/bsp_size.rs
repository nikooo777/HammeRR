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
