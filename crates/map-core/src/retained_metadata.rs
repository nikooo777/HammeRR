use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use source_formats::vmf::{Properties, Vmf};

const ENVELOPE_VERSION: &str = "1";
const CHUNK_BYTES: usize = 480;
const MAX_WORLD_PROPERTIES: usize = 65_534;

pub(crate) fn is_key(key: &str, prefix: &str) -> bool {
    key.trim()
        .get(..prefix.len())
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix))
}

pub(crate) fn misplaced(vmf: &Vmf, prefix: &str) -> bool {
    vmf.extra.iter().any(|node| is_key(&node.key, prefix))
        || vmf.world.extra.iter().any(|node| is_key(&node.key, prefix))
        || vmf.entities.iter().any(|entity| {
            entity.properties.iter().any(|(key, _)| is_key(key, prefix))
                || entity.extra.iter().any(|node| is_key(&node.key, prefix))
                || entity
                    .editor
                    .as_ref()
                    .is_some_and(|editor| editor.iter().any(|(key, _)| is_key(key, prefix)))
        })
}

fn header<'a>(
    value: &'a str,
    label: &str,
    max_bytes: usize,
) -> Result<(usize, usize, &'a str), String> {
    let fields = value.split(' ').collect::<Vec<_>>();
    let [version, count, bytes, checksum] = fields.as_slice() else {
        return Err(format!("Invalid HammeRR {label} transport header"));
    };
    if *version != ENVELOPE_VERSION {
        return Err(format!("Unsupported HammeRR {label} transport version"));
    }
    let count = count
        .parse::<usize>()
        .map_err(|_| format!("Invalid {label} transport chunk count"))?;
    let bytes = bytes
        .parse::<usize>()
        .map_err(|_| format!("Invalid {label} transport byte count"))?;
    if count == 0
        || count > (max_bytes.div_ceil(3) * 4).div_ceil(CHUNK_BYTES)
        || bytes == 0
        || bytes > max_bytes
        || checksum.len() != 16
        || !checksum.bytes().all(|byte| byte.is_ascii_hexdigit())
        || value != format!("{ENVELOPE_VERSION} {count} {bytes} {checksum}")
    {
        return Err(format!(
            "{label} transport header exceeds its bounds or is not canonical"
        ));
    }
    let encoded = bytes.div_ceil(3) * 4;
    if count != encoded.div_ceil(CHUNK_BYTES) {
        return Err(format!(
            "{label} transport length does not match its chunk count"
        ));
    }
    Ok((count, bytes, checksum))
}

pub(crate) fn read(
    vmf: &Vmf,
    prefix: &str,
    label: &str,
    max_bytes: usize,
) -> Result<Option<String>, String> {
    let mut fields = vmf
        .world
        .properties
        .iter()
        .filter(|(key, _)| is_key(key, prefix));
    let Some(first) = fields.next() else {
        return Ok(None);
    };
    if vmf.world.properties.0.len() > MAX_WORLD_PROPERTIES {
        return Err(format!(
            "{label} transport exceeds the 65534 world-property limit"
        ));
    }
    let fields = std::iter::once(first).chain(fields).collect::<Vec<_>>();
    let mut headers = fields
        .iter()
        .copied()
        .filter(|(key, _)| key.trim().eq_ignore_ascii_case(prefix));
    let Some((key, value)) = headers.next() else {
        return Err(format!("{label} transport is missing its header"));
    };
    if headers.next().is_some() {
        return Err(format!(
            "{label} transport has duplicate or case-aliased headers"
        ));
    }
    if key != prefix || value.len() > CHUNK_BYTES {
        return Err(format!(
            "{label} transport header spelling or size is not canonical"
        ));
    }
    let (count, bytes, checksum) = header(value, label, max_bytes)?;
    if fields.len() != count + 1 {
        return Err(format!(
            "{label} transport has missing, extra or duplicate reserved properties"
        ));
    }
    let encoded_bytes = bytes.div_ceil(3) * 4;
    let mut chunks = vec![None; count];
    for (key, value) in fields {
        if key == prefix {
            continue;
        }
        let index = key
            .strip_prefix(prefix)
            .and_then(|suffix| suffix.strip_prefix('_'))
            .and_then(|index| index.parse::<usize>().ok())
            .filter(|&index| index < count)
            .ok_or_else(|| {
                format!("{label} transport has a malformed or case-aliased chunk key")
            })?;
        let expected = format!("{prefix}_{index:05}");
        let expected_bytes = CHUNK_BYTES.min(encoded_bytes - index * CHUNK_BYTES);
        if key != expected || value.len() != expected_bytes || !value.is_ascii() {
            return Err(format!(
                "{label} transport has noncanonical or truncated chunks"
            ));
        }
        if chunks[index].replace(value).is_some() {
            return Err(format!("{label} transport repeats a chunk number"));
        }
    }
    let mut encoded = String::with_capacity(encoded_bytes);
    for chunk in chunks {
        encoded
            .push_str(chunk.ok_or_else(|| format!("{label} transport is missing a chunk number"))?);
    }
    let decoded = STANDARD
        .decode(encoded)
        .map_err(|_| format!("Invalid base64 {label} transport"))?;
    if decoded.len() != bytes {
        return Err(format!(
            "{label} transport decoded length does not match its header"
        ));
    }
    let decoded =
        String::from_utf8(decoded).map_err(|_| format!("{label} transport is not UTF-8"))?;
    if crate::context::text_fingerprint(&decoded) != checksum {
        return Err(format!(
            "{label} transport checksum does not match its contents"
        ));
    }
    Ok(Some(decoded))
}

pub(crate) fn encode(
    prefix: &str,
    label: &str,
    value: &str,
    max_bytes: usize,
) -> Result<Vec<(String, String)>, String> {
    if value.len() > max_bytes {
        return Err(format!(
            "{label} transport exceeds its {max_bytes}-byte serialized metadata limit"
        ));
    }
    let checksum = crate::context::text_fingerprint(value);
    let bytes = value.len();
    let encoded = STANDARD.encode(value);
    let count = encoded.len().div_ceil(CHUNK_BYTES);
    let mut properties = Vec::with_capacity(count + 1);
    properties.push((
        prefix.into(),
        format!("{ENVELOPE_VERSION} {count} {bytes} {checksum}"),
    ));
    for (index, chunk) in encoded.as_bytes().chunks(CHUNK_BYTES).enumerate() {
        let value =
            std::str::from_utf8(chunk).map_err(|_| format!("{label} transport must be ASCII"))?;
        properties.push((format!("{prefix}_{index:05}"), value.to_owned()));
    }
    Ok(properties)
}

pub(crate) fn store(
    vmf: &mut Vmf,
    prefix: &str,
    label: &str,
    carrier: Vec<(String, String)>,
) -> Result<(), String> {
    let ordinary = vmf
        .world
        .properties
        .iter()
        .filter(|(key, _)| !is_key(key, prefix))
        .count();
    if ordinary.saturating_add(carrier.len()) > MAX_WORLD_PROPERTIES {
        return Err(format!(
            "{label} transport and ordinary world properties exceed the shared 65534-property limit"
        ));
    }
    let mut properties = Vec::with_capacity(ordinary + carrier.len());
    let mut carrier = Some(carrier);
    for (key, value) in &vmf.world.properties.0 {
        if is_key(key, prefix) {
            if let Some(carrier) = carrier.take() {
                properties.extend(carrier);
            }
        } else {
            properties.push((key.clone(), value.clone()));
        }
    }
    if let Some(carrier) = carrier {
        properties.extend(carrier);
    }
    vmf.world.properties = Properties(properties);
    Ok(())
}
