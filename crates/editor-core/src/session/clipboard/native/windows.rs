use clipboard_win::{Clipboard, formats::CF_UNICODETEXT, raw};

const MAX_UTF16_BYTES: usize = map_core::transfer::MAX_BYTES * 2 + 2;

pub(super) fn read() -> Result<String, String> {
    let _clipboard = Clipboard::new_attempts(10)
        .map_err(|error| format!("Cannot open Windows clipboard: {error}"))?;
    let size = raw::size(CF_UNICODETEXT)
        .ok_or("Windows clipboard has no readable Unicode text")?
        .get();
    if size > MAX_UTF16_BYTES {
        return Err("Windows clipboard Unicode text exceeds the 8 MiB text limit".into());
    }
    if size % 2 != 0 {
        return Err("Windows clipboard Unicode text has an odd byte count".into());
    }
    let mut bytes = vec![0; size];
    let copied = raw::get(CF_UNICODETEXT, &mut bytes)
        .map_err(|error| format!("Cannot read Windows clipboard Unicode text: {error}"))?;
    if copied != size || raw::size(CF_UNICODETEXT).map(|size| size.get()) != Some(size) {
        return Err("Windows clipboard Unicode text changed while reading".into());
    }
    let terminator = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .position(|unit| *unit == [0, 0])
        .ok_or("Windows clipboard Unicode text has no terminator")?;
    let units = bytes[..terminator * 2]
        .as_chunks::<2>()
        .0
        .iter()
        .copied()
        .map(u16::from_le_bytes);
    let mut text = String::new();
    for character in char::decode_utf16(units) {
        let character = character.map_err(|_| "Windows clipboard contains invalid UTF-16")?;
        if text.len() + character.len_utf8() > map_core::transfer::MAX_BYTES {
            return Err("Windows clipboard text exceeds 8 MiB after UTF-8 conversion".into());
        }
        text.push(character);
    }
    Ok(text)
}
