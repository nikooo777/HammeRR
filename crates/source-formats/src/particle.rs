use std::{
    collections::{BTreeMap, BTreeSet},
    io,
};

use crate::{gamefs::asset_relative, keyvalues};

pub const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_ELEMENTS: usize = 16384;
const MAX_ATTRIBUTES: usize = 262144;
const MAX_VALUES: usize = 1048576;
const MAX_STRING: usize = 4096;
const MAX_METADATA_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct ManifestFile {
    pub path: String,
    pub precache: bool,
}

#[derive(Clone, Debug)]
pub struct Definition {
    pub name: String,
    pub material: Option<String>,
    pub max_particles: Option<i32>,
    pub functions: BTreeMap<String, Vec<String>>,
    pub children: Vec<String>,
}

#[derive(Debug)]
pub struct File {
    pub definitions: Vec<Definition>,
    pub elements: usize,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub fn manifest(data: &[u8]) -> io::Result<Vec<ManifestFile>> {
    if data.len() > MAX_MANIFEST_BYTES {
        return Err(invalid("Particle manifest exceeds 1 MiB"));
    }
    let text = std::str::from_utf8(data).map_err(|_| invalid("Particle manifest is not UTF-8"))?;
    let nodes = keyvalues::parse_lenient(text).map_err(|error| invalid(&error.to_string()))?;
    if nodes.len() != 1 || !nodes[0].key.eq_ignore_ascii_case("particles_manifest") {
        return Err(invalid("Expected one particles_manifest block"));
    }
    let fields = nodes[0]
        .children()
        .ok_or_else(|| invalid("Particle manifest has no file block"))?;
    if fields.is_empty() || fields.len() > 128 {
        return Err(invalid("Particle manifest must list 1 to 128 files"));
    }
    let mut seen = BTreeSet::new();
    let mut files = Vec::new();
    for field in fields {
        if !field.key.eq_ignore_ascii_case("file") {
            return Err(invalid(
                "Particle manifest includes or unknown directives are unsupported",
            ));
        }
        let value = field
            .as_str()
            .ok_or_else(|| invalid("Particle manifest file must be a path"))?;
        let precache = value.starts_with('!');
        let value = value.strip_prefix('!').unwrap_or(value);
        let path = asset_relative(value)?;
        if !path.starts_with("particles/") || !path.ends_with(".pcf") || path.len() > 1024 {
            return Err(invalid(
                "Particle manifest files must be relative particles/*.pcf paths",
            ));
        }
        if !seen.insert(path.clone()) {
            return Err(invalid("Particle manifest lists a file more than once"));
        }
        files.push(ManifestFile { path, precache });
    }
    Ok(files)
}

struct Reader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> io::Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(count)
            .filter(|end| *end <= self.data.len())
            .ok_or_else(|| invalid("Particle DMX data is truncated"))?;
        let bytes = &self.data[self.offset..end];
        self.offset = end;
        Ok(bytes)
    }
    fn int(&mut self) -> io::Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn count(&mut self, maximum: usize) -> io::Result<usize> {
        let value = self.int()?;
        usize::try_from(value)
            .ok()
            .filter(|count| *count <= maximum)
            .ok_or_else(|| invalid("Particle DMX count exceeds its bounded limit"))
    }
    fn short(&mut self) -> io::Result<usize> {
        let value = i16::from_le_bytes(self.take(2)?.try_into().unwrap());
        usize::try_from(value).map_err(|_| invalid("Particle DMX dictionary index is negative"))
    }
    fn string(&mut self) -> io::Result<String> {
        let remaining = &self.data[self.offset..];
        let count = remaining
            .iter()
            .take(MAX_STRING + 1)
            .position(|byte| *byte == 0)
            .ok_or_else(|| invalid("Particle DMX string is unterminated or exceeds 4096 bytes"))?;
        let bytes = self.take(count + 1)?;
        if !bytes[..count].is_ascii() {
            return Err(invalid("Binary 2 particle DMX strings must be ASCII"));
        }
        Ok(std::str::from_utf8(&bytes[..count]).unwrap().into())
    }
    fn index(&mut self, strings: &[String]) -> io::Result<usize> {
        let index = self.short()?;
        if index >= strings.len() {
            return Err(invalid("Particle DMX dictionary index is out of bounds"));
        }
        Ok(index)
    }
}

#[derive(Debug)]
enum Attribute {
    Text(String),
    Int(i32),
    Reference(Option<usize>),
    References(Vec<Option<usize>>),
    Other,
}

struct Element {
    kind: usize,
    name: String,
    attributes: BTreeMap<usize, Attribute>,
}

impl Element {
    fn field<'a>(&'a self, strings: &[String], name: &str) -> Option<&'a Attribute> {
        self.attributes
            .iter()
            .find(|(key, _)| strings[**key].eq_ignore_ascii_case(name))
            .map(|(_, value)| value)
    }
    fn references(&self, strings: &[String], name: &str) -> io::Result<&[Option<usize>]> {
        match self.field(strings, name) {
            Some(Attribute::References(values)) => Ok(values),
            None => Ok(&[]),
            _ => Err(invalid(
                "Particle definition collection must be an element array",
            )),
        }
    }
}

fn reference(reader: &mut Reader<'_>, count: usize) -> io::Result<Option<usize>> {
    match reader.int()? {
        -1 => Ok(None),
        value if value >= 0 && (value as usize) < count => Ok(Some(value as usize)),
        _ => Err(invalid(
            "Particle DMX element reference is out of bounds or external",
        )),
    }
}

pub fn parse(data: &[u8]) -> io::Result<File> {
    if data.len() > MAX_FILE_BYTES {
        return Err(invalid("Particle PCF exceeds 16 MiB"));
    }
    let header = b"<!-- dmx encoding binary 2 format pcf 1 -->\n\0";
    if !data.starts_with(header) {
        return Err(invalid(
            "Unsupported particle format; expected DMX binary 2 / pcf 1",
        ));
    }
    let mut reader = Reader {
        data,
        offset: header.len(),
    };
    let string_count = reader.short()?;
    let mut strings = Vec::with_capacity(string_count);
    for _ in 0..string_count {
        strings.push(reader.string()?);
    }
    let folded_strings = strings
        .iter()
        .map(|value| value.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let element_count = reader.count(MAX_ELEMENTS)?;
    if element_count == 0 {
        return Err(invalid("Particle DMX has no root element"));
    }
    let mut elements = Vec::with_capacity(element_count);
    for _ in 0..element_count {
        let kind = reader.index(&strings)?;
        let name = reader.string()?;
        reader.take(16)?;
        elements.push(Element {
            kind,
            name,
            attributes: BTreeMap::new(),
        });
    }
    let mut total_attributes = 0usize;
    let mut total_values = 0usize;
    for element in &mut elements {
        let count = reader.count(512)?;
        total_attributes += count;
        if total_attributes > MAX_ATTRIBUTES {
            return Err(invalid("Particle DMX has too many attributes"));
        }
        let mut names = BTreeSet::new();
        for _ in 0..count {
            let name = reader.index(&strings)?;
            if !names.insert(&folded_strings[name]) {
                return Err(invalid("Particle DMX has duplicate attributes"));
            }
            let encoded = reader.take(1)?[0];
            if !(1..=28).contains(&encoded) {
                return Err(invalid("Particle DMX attribute type is unsupported"));
            }
            let array = encoded > 14;
            let kind = if array { encoded - 14 } else { encoded };
            let count = if array { reader.count(MAX_VALUES)? } else { 1 };
            total_values += count;
            if total_values > MAX_VALUES {
                return Err(invalid("Particle DMX has too many attribute values"));
            }
            let value = if kind == 1 {
                if array {
                    let mut refs = Vec::with_capacity(count.min(data.len() / 4));
                    for _ in 0..count {
                        refs.push(reference(&mut reader, element_count)?);
                    }
                    Attribute::References(refs)
                } else {
                    Attribute::Reference(reference(&mut reader, element_count)?)
                }
            } else if kind == 5 {
                let mut text = None;
                for _ in 0..count {
                    text = Some(reader.string()?);
                }
                if array {
                    Attribute::Other
                } else {
                    Attribute::Text(text.unwrap())
                }
            } else if kind == 6 {
                for _ in 0..count {
                    let size = reader.count(MAX_FILE_BYTES)?;
                    reader.take(size)?;
                }
                Attribute::Other
            } else if kind == 2 && !array {
                Attribute::Int(reader.int()?)
            } else {
                let width = match kind {
                    2 | 3 | 8 => 4,
                    4 => 1,
                    9 => 8,
                    10 | 12 => 12,
                    11 | 13 => 16,
                    14 => 64,
                    _ => {
                        return Err(invalid(
                            "Particle DMX element-id attributes are unsupported",
                        ));
                    }
                };
                reader.take(
                    count
                        .checked_mul(width)
                        .ok_or_else(|| invalid("Particle DMX array size overflow"))?,
                )?;
                Attribute::Other
            };
            element.attributes.insert(name, value);
        }
    }
    if reader.offset != data.len() {
        return Err(invalid("Particle DMX has trailing data"));
    }
    if !matches!(
        strings[elements[0].kind].as_str(),
        "DmElement" | "DmeElement"
    ) {
        return Err(invalid("Particle PCF root has an unexpected element type"));
    }
    if elements[0]
        .field(&strings, "particleSystemDefinitions")
        .is_none()
    {
        return Err(invalid(
            "Particle PCF root has no particleSystemDefinitions",
        ));
    }
    let mut metadata_bytes = 0usize;
    let mut definitions = Vec::new();
    let mut roots = BTreeSet::new();
    for index in elements[0].references(&strings, "particleSystemDefinitions")? {
        let index = index.ok_or_else(|| invalid("Particle definition root is null"))?;
        if !roots.insert(index) {
            return Err(invalid("Particle definition root is repeated"));
        }
        let element = &elements[index];
        if strings[element.kind] != "DmeParticleSystemDefinition"
            || element.name.is_empty()
            || element.name.len() > 255
            || element.name.contains(['\r', '\n'])
        {
            return Err(invalid("Particle definition type or name is invalid"));
        }
        let mut functions = BTreeMap::new();
        for category in [
            "renderers",
            "operators",
            "initializers",
            "emitters",
            "forces",
            "constraints",
        ] {
            let mut names = Vec::new();
            for index in element.references(&strings, category)? {
                let index = index.ok_or_else(|| invalid("Particle function reference is null"))?;
                let function = &elements[index];
                if strings[function.kind] != "DmeParticleOperator" {
                    return Err(invalid("Particle function has an unexpected element type"));
                }
                let name = match function.field(&strings, "functionName") {
                    Some(Attribute::Text(value)) => value.as_str(),
                    None => &function.name,
                    _ => return Err(invalid("Particle functionName must be a string")),
                };
                if name.is_empty() {
                    return Err(invalid("Particle function name is empty"));
                }
                metadata_bytes += name.len();
                if metadata_bytes > MAX_METADATA_BYTES {
                    return Err(invalid("Particle definition metadata exceeds 8 MiB"));
                }
                names.push(name.to_owned());
            }
            functions.insert(category.into(), names);
        }
        let mut children = Vec::new();
        for index in element.references(&strings, "children")? {
            let index = index.ok_or_else(|| invalid("Particle child reference is null"))?;
            let child = &elements[index];
            if strings[child.kind] != "DmeParticleChild" {
                return Err(invalid("Particle child has an unexpected element type"));
            }
            let name = match child.field(&strings, "child") {
                Some(Attribute::Reference(Some(index)))
                    if strings[elements[*index].kind] == "DmeParticleSystemDefinition" =>
                {
                    &elements[*index].name
                }
                _ => return Err(invalid("Particle child has no internal child definition")),
            };
            if name.is_empty() || name.len() > 255 || name.contains(['\r', '\n']) {
                return Err(invalid("Particle child system name is invalid"));
            }
            metadata_bytes += name.len();
            if metadata_bytes > MAX_METADATA_BYTES {
                return Err(invalid("Particle definition metadata exceeds 8 MiB"));
            }
            children.push(name.clone());
        }
        let material = match element.field(&strings, "material") {
            Some(Attribute::Text(value)) => Some(value.clone()),
            None => None,
            _ => return Err(invalid("Particle material must be a string")),
        };
        let max_particles = match element.field(&strings, "max_particles") {
            Some(Attribute::Int(value)) => Some(*value),
            None => None,
            _ => return Err(invalid("Particle maximum count must be an integer")),
        };
        metadata_bytes += element.name.len() + material.as_ref().map_or(0, String::len);
        if metadata_bytes > MAX_METADATA_BYTES {
            return Err(invalid("Particle definition metadata exceeds 8 MiB"));
        }
        definitions.push(Definition {
            name: element.name.clone(),
            material,
            max_particles,
            functions,
            children,
        });
    }
    if definitions.is_empty() {
        return Err(invalid("Particle PCF lists no named systems"));
    }
    Ok(File {
        definitions,
        elements: element_count,
    })
}

#[cfg(test)]
mod tests;
