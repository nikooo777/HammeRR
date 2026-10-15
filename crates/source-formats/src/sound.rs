use std::io;

use crate::{
    gamefs::asset_relative,
    keyvalues::{self, Node},
};

pub const MAX_SCRIPT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_CUES: usize = 8192;
pub const MAX_VARIANTS: usize = 128;

#[derive(Debug, Clone)]
pub struct Cue {
    pub name: String,
    pub waves: Vec<String>,
    pub channel: Option<String>,
    pub volume: Option<String>,
    pub pitch: Option<String>,
    pub sound_level: Option<String>,
}

#[derive(Debug)]
pub struct AudioInfo {
    pub codec: &'static str,
    pub channels: Option<u16>,
    pub sample_rate: Option<u32>,
    pub duration: Option<f64>,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub fn document(text: &str) -> io::Result<Vec<Node>> {
    if text.len() > MAX_SCRIPT_BYTES {
        return Err(invalid("Sound script exceeds its byte limit"));
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut depth = 0usize;
    let mut quoted = false;
    while index < bytes.len() {
        if !quoted && bytes[index..].starts_with(b"//") {
            index += bytes[index..]
                .iter()
                .position(|byte| *byte == b'\n')
                .unwrap_or(bytes.len() - index);
            continue;
        }
        if !quoted && bytes[index..].starts_with(b"/*") {
            index += 2;
            index += bytes[index..]
                .windows(2)
                .position(|part| part == b"*/")
                .map_or(bytes.len() - index, |end| end + 2);
            continue;
        }
        match bytes[index] {
            b'"' => quoted = !quoted,
            b'{' if !quoted => {
                depth += 1;
                if depth > 32 {
                    return Err(invalid("Sound script nesting exceeds 32 blocks"));
                }
            }
            b'}' if !quoted => depth = depth.saturating_sub(1),
            _ => {}
        }
        index += 1;
    }
    keyvalues::parse_lenient(text).map_err(|error| invalid(&error.to_string()))
}

pub fn parse_script(text: &str) -> io::Result<Vec<Cue>> {
    let nodes = document(text)?;
    if nodes.len() > MAX_CUES {
        return Err(invalid("Sound script has too many entries"));
    }
    let mut result = Vec::with_capacity(nodes.len());
    for node in nodes {
        if node.key.eq_ignore_ascii_case("#include") || node.key.eq_ignore_ascii_case("#base") {
            return Err(invalid(
                "Sound-script include/base directives are not resolved; choose a sound file or a flattened script",
            ));
        }
        let Some(fields) = node.children() else {
            continue;
        };
        if node.key.is_empty() || node.key.len() > 255 || node.key.contains(['\0', '\n', '\r']) {
            return Err(invalid("Sound script entry name is invalid"));
        }
        let field = |key: &str| {
            fields
                .iter()
                .find(|node| node.key.eq_ignore_ascii_case(key))
                .and_then(Node::as_str)
                .map(str::to_owned)
        };
        let mut waves = Vec::new();
        for field in fields {
            if field.key.eq_ignore_ascii_case("wave") {
                if let Some(value) = field.as_str() {
                    waves.push(value.to_owned());
                }
            } else if field.key.eq_ignore_ascii_case("rndwave") {
                waves.extend(
                    field
                        .children()
                        .unwrap_or_default()
                        .iter()
                        .filter(|node| node.key.eq_ignore_ascii_case("wave"))
                        .filter_map(Node::as_str)
                        .map(str::to_owned),
                );
            }
            if waves.len() > MAX_VARIANTS {
                return Err(invalid("Sound entry has too many wave variants"));
            }
        }
        result.push(Cue {
            name: node.key.clone(),
            waves,
            channel: field("channel"),
            volume: field("volume"),
            pitch: field("pitch"),
            sound_level: field("soundlevel"),
        });
    }
    Ok(result)
}

pub fn audio_path(value: &str) -> io::Result<String> {
    let value = value.trim();
    let flags = value
        .chars()
        .take_while(|character| "*?!#><^@)}".contains(*character))
        .collect::<String>();
    if flags.contains(['?', '!']) {
        return Err(invalid(
            "Voice and sentence sounds need the game; choose a WAV/MP3 file for audition",
        ));
    }
    let path = asset_relative(&value[flags.len()..])?;
    let path = path.strip_prefix("sound/").unwrap_or(&path);
    if ![".wav", ".mp3"].iter().any(|suffix| path.ends_with(suffix)) || path.len() > 1024 {
        return Err(invalid("Choose a relative WAV or MP3 sound path"));
    }
    Ok(path.to_owned())
}

pub fn inspect_audio(path: &str, data: &[u8]) -> io::Result<AudioInfo> {
    if path.ends_with(".mp3") {
        if !data.starts_with(b"ID3")
            && !(data.len() >= 2 && data[0] == 0xff && data[1] & 0xe0 == 0xe0)
        {
            return Err(invalid("MP3 header is missing"));
        }
        return Ok(AudioInfo {
            codec: "MP3",
            channels: None,
            sample_rate: None,
            duration: None,
        });
    }
    if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err(invalid("A little-endian RIFF/WAVE file is required"));
    }
    let integer = |offset: usize| u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
    let end = 8usize
        .checked_add(integer(4) as usize)
        .filter(|&end| end >= 12 && end <= data.len())
        .ok_or_else(|| invalid("WAV RIFF size exceeds its data"))?;
    let mut offset = 12;
    let mut format = None;
    let mut samples = None;
    let mut audio_bytes = 0usize;
    while offset < end {
        if end - offset < 8 {
            return Err(invalid("WAV chunk header is incomplete"));
        }
        let size = integer(offset + 4) as usize;
        let start = offset + 8;
        let stop = start
            .checked_add(size)
            .filter(|&stop| stop <= end)
            .ok_or_else(|| invalid("WAV chunk exceeds its RIFF data"))?;
        match &data[offset..offset + 4] {
            b"fmt " => {
                if size < 16 {
                    return Err(invalid("WAV format chunk is incomplete"));
                }
                let short =
                    |index: usize| u16::from_le_bytes(data[index..index + 2].try_into().unwrap());
                let (codec, channels, rate, bytes) = (
                    short(start),
                    short(start + 2),
                    integer(start + 4),
                    integer(start + 8),
                );
                if !(1..=8).contains(&channels) || !(1..=384000).contains(&rate) || bytes == 0 {
                    return Err(invalid("WAV channels or sample rate exceed preview limits"));
                }
                format = Some((codec, channels, rate, bytes));
            }
            b"fact" if size >= 4 => samples = Some(integer(start)),
            b"data" => {
                audio_bytes = audio_bytes
                    .checked_add(size)
                    .ok_or_else(|| invalid("WAV data length overflow"))?
            }
            _ => {}
        }
        offset = stop
            .checked_add(size % 2)
            .ok_or_else(|| invalid("WAV padding overflow"))?;
        if offset > end {
            return Err(invalid("WAV chunk padding is missing"));
        }
    }
    let (codec, channels, rate, bytes) =
        format.ok_or_else(|| invalid("WAV format chunk is missing"))?;
    Ok(AudioInfo {
        codec: match codec {
            1 => "PCM WAV",
            3 => "Float WAV",
            2 => "MS ADPCM WAV",
            17 => "IMA ADPCM WAV",
            _ => "Compressed WAV",
        },
        channels: Some(channels),
        sample_rate: Some(rate),
        duration: if matches!(codec, 1 | 3) {
            Some(audio_bytes as f64 / f64::from(bytes))
        } else {
            samples.map(|samples| f64::from(samples) / f64::from(rate))
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cues_preserve_metadata_variants_and_native_path_prefixes() {
        assert!(parse_script("#include scripts/extra.txt").is_err());
        let cues=parse_script("Rain { volume \"0.5, 1\" pitch PITCH_NORM rndwave { wave *#Ambient\\rain.wav wave ambient/two.mp3 } }").unwrap();
        assert_eq!(cues[0].name, "Rain");
        assert_eq!(cues[0].waves.len(), 2);
        assert_eq!(cues[0].volume.as_deref(), Some("0.5, 1"));
        assert_eq!(audio_path(&cues[0].waves[0]).unwrap(), "ambient/rain.wav");
        for path in [
            "../outside.wav",
            "/absolute.wav",
            "sound/../../outside.wav",
            "!*voice.wav",
            "?voice.wav",
            "C:\\sound.wav",
        ] {
            assert!(audio_path(path).is_err(), "{path}");
        }
        assert!(parse_script(&format!("{}{}", "a {".repeat(33), "}".repeat(33))).is_err());
    }

    #[test]
    fn wav_metadata_checks_chunks_padding_and_real_duration() {
        let mut data = b"RIFF".to_vec();
        data.extend(38u32.to_le_bytes());
        data.extend(b"WAVEfmt ");
        data.extend(16u32.to_le_bytes());
        data.extend(1u16.to_le_bytes());
        data.extend(1u16.to_le_bytes());
        data.extend(8000u32.to_le_bytes());
        data.extend(8000u32.to_le_bytes());
        data.extend(1u16.to_le_bytes());
        data.extend(8u16.to_le_bytes());
        data.extend(b"data");
        data.extend(2u32.to_le_bytes());
        data.extend([128, 128]);
        let info = inspect_audio("one.wav", &data).unwrap();
        assert_eq!(info.duration, Some(0.00025));
        assert_eq!(info.channels, Some(1));
        data[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(inspect_audio("one.wav", &data).is_err());
        assert!(inspect_audio("one.mp3", b"invalid").is_err());
    }
}
