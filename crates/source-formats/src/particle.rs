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
const MAX_PROGRAM_BYTES: usize = 1024 * 1024;

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

#[derive(Clone, Debug)]
pub enum Property {
    Text(String),
    Int(i32),
    Float(f32),
    Bool(bool),
    Color([u8; 4]),
    Vector([f32; 3]),
    Unsupported,
}

#[derive(Debug)]
pub struct Operator {
    pub name: String,
    pub properties: BTreeMap<String, Property>,
}

#[derive(Debug)]
pub struct Program {
    pub name: String,
    pub properties: BTreeMap<String, Property>,
    pub functions: BTreeMap<String, Vec<Operator>>,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub fn manifest(data: &[u8]) -> io::Result<Vec<ManifestFile>> {
    if data.len() > MAX_MANIFEST_BYTES {
        return Err(invalid("Particle manifest exceeds 1 MiB"));
    }
    let text = std::str::from_utf8(data).map_err(|_| invalid("Particle manifest is not UTF-8"))?;
    let nodes =
        keyvalues::parse_bounded(text, 1, 129).map_err(|error| invalid(&error.to_string()))?;
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
    Value(Property),
    Reference(Option<usize>),
    References(Vec<Option<usize>>),
    Other,
}

#[derive(Clone, Copy)]
#[repr(usize)]
enum Field {
    Definitions,
    Material,
    MaxParticles,
    FunctionName,
    Children,
    Child,
    Renderers,
    Operators,
    Initializers,
    Emitters,
    Forces,
    Constraints,
}

const FIELD_NAMES: [&str; 12] = [
    "particlesystemdefinitions",
    "material",
    "max_particles",
    "functionname",
    "children",
    "child",
    "renderers",
    "operators",
    "initializers",
    "emitters",
    "forces",
    "constraints",
];
type FieldKeys = [Option<usize>; FIELD_NAMES.len()];

const FUNCTION_FIELDS: [(Field, &str); 6] = [
    (Field::Renderers, "renderers"),
    (Field::Operators, "operators"),
    (Field::Initializers, "initializers"),
    (Field::Emitters, "emitters"),
    (Field::Forces, "forces"),
    (Field::Constraints, "constraints"),
];

struct Element {
    kind: usize,
    name: String,
    attributes: BTreeMap<usize, Attribute>,
}

impl Element {
    fn field(&self, keys: &FieldKeys, field: Field) -> Option<&Attribute> {
        self.attributes.get(&keys[field as usize]?)
    }
    fn references(&self, keys: &FieldKeys, field: Field) -> io::Result<&[Option<usize>]> {
        match self.field(keys, field) {
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

struct Decoded {
    strings: Vec<String>,
    elements: Vec<Element>,
    fields: FieldKeys,
}

fn decode(data: &[u8]) -> io::Result<Decoded> {
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
    let mut attribute_keys = BTreeMap::new();
    let canonical_keys = folded_strings
        .iter()
        .enumerate()
        .map(|(index, name)| *attribute_keys.entry(name.as_str()).or_insert(index))
        .collect::<Vec<_>>();
    let field_keys: FieldKeys = FIELD_NAMES.map(|name| attribute_keys.get(name).copied());
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
            let name = canonical_keys[reader.index(&strings)?];
            if !names.insert(name) {
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
                    Attribute::Value(Property::Text(text.unwrap()))
                }
            } else if kind == 6 {
                for _ in 0..count {
                    let size = reader.count(MAX_FILE_BYTES)?;
                    reader.take(size)?;
                }
                Attribute::Other
            } else if !array && matches!(kind, 2 | 3 | 4 | 8 | 10) {
                let property = match kind {
                    2 => Property::Int(reader.int()?),
                    3 => Property::Float(f32::from_le_bytes(reader.take(4)?.try_into().unwrap())),
                    4 => match reader.take(1)?[0] {
                        0 => Property::Bool(false),
                        1 => Property::Bool(true),
                        _ => Property::Unsupported,
                    },
                    8 => Property::Color(reader.take(4)?.try_into().unwrap()),
                    10 => {
                        let mut vector = [0.0; 3];
                        for axis in &mut vector {
                            *axis = f32::from_le_bytes(reader.take(4)?.try_into().unwrap());
                        }
                        Property::Vector(vector)
                    }
                    _ => unreachable!(),
                };
                Attribute::Value(property)
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
    if elements[0].field(&field_keys, Field::Definitions).is_none() {
        return Err(invalid(
            "Particle PCF root has no particleSystemDefinitions",
        ));
    }
    Ok(Decoded {
        strings,
        elements,
        fields: field_keys,
    })
}

fn definition_name(element: &Element, strings: &[String]) -> io::Result<()> {
    if strings[element.kind] != "DmeParticleSystemDefinition"
        || element.name.is_empty()
        || element.name.len() > 255
        || element.name.contains(['\"', '\r', '\n'])
    {
        return Err(invalid("Particle definition type or name is invalid"));
    }
    Ok(())
}

fn function_name<'a>(
    function: &'a Element,
    strings: &[String],
    field_keys: &FieldKeys,
) -> io::Result<&'a str> {
    if strings[function.kind] != "DmeParticleOperator" {
        return Err(invalid("Particle function has an unexpected element type"));
    }
    let name = match function.field(field_keys, Field::FunctionName) {
        Some(Attribute::Value(Property::Text(value))) => value.as_str(),
        None => &function.name,
        _ => return Err(invalid("Particle functionName must be a string")),
    };
    if name.is_empty() {
        return Err(invalid("Particle function name is empty"));
    }
    Ok(name)
}

fn properties(
    element: &Element,
    strings: &[String],
    omit: &[Option<usize>],
    remaining: &mut usize,
) -> io::Result<BTreeMap<String, Property>> {
    let mut result = BTreeMap::new();
    for (&key, value) in &element.attributes {
        if omit.contains(&Some(key)) {
            continue;
        }
        let name = &strings[key];
        let text_bytes = match value {
            Attribute::Value(Property::Text(text)) => text.len(),
            _ => 0,
        };
        *remaining = remaining
            .checked_sub(name.len() + text_bytes + size_of::<(String, Property)>())
            .ok_or_else(|| invalid("Particle preview program exceeds 1 MiB"))?;
        let property = match value {
            Attribute::Value(property) => property.clone(),
            _ => Property::Unsupported,
        };
        result.insert(name.to_ascii_lowercase(), property);
    }
    Ok(result)
}

pub fn program(data: &[u8], name: &str) -> io::Result<Program> {
    let Decoded {
        strings,
        elements,
        fields,
    } = decode(data)?;
    let mut selected = None;
    for index in elements[0].references(&fields, Field::Definitions)? {
        let index = index.ok_or_else(|| invalid("Particle definition root is null"))?;
        let root = &elements[index];
        definition_name(root, &strings)?;
        if root.name.eq_ignore_ascii_case(name) && selected.replace(root).is_some() {
            return Err(invalid("Particle preview definition is ambiguous"));
        }
    }
    let root = selected.ok_or_else(|| invalid("Particle preview definition was not found"))?;
    for field in [Field::Children, Field::Forces, Field::Constraints] {
        if !root.references(&fields, field)?.is_empty() {
            return Err(invalid(
                "Particle preview excludes child systems, forces and constraints",
            ));
        }
    }
    let mut functions = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut remaining = MAX_PROGRAM_BYTES;
    for (field, category) in FUNCTION_FIELDS {
        let mut operators = Vec::new();
        let refs = root.references(&fields, field)?;
        if refs.len() > 32 {
            return Err(invalid(
                "Particle preview permits up to 32 operations per category",
            ));
        }
        for index in refs {
            let index = index.ok_or_else(|| invalid("Particle function reference is null"))?;
            if !seen.insert(index) {
                return Err(invalid("Particle preview repeats an operator element"));
            }
            let function = &elements[index];
            if function.field(&fields, Field::FunctionName).is_none() {
                return Err(invalid(
                    "Particle preview operator requires an explicit functionName",
                ));
            }
            let name = function_name(function, &strings, &fields)?;
            remaining = remaining
                .checked_sub(name.len())
                .ok_or_else(|| invalid("Particle preview program exceeds 1 MiB"))?;
            operators.push(Operator {
                name: name.to_owned(),
                properties: properties(
                    function,
                    &strings,
                    &[fields[Field::FunctionName as usize]],
                    &mut remaining,
                )?,
            });
        }
        functions.insert(category.to_owned(), operators);
    }
    let omitted = [
        Field::Children,
        Field::Renderers,
        Field::Operators,
        Field::Initializers,
        Field::Emitters,
        Field::Forces,
        Field::Constraints,
    ]
    .map(|field| fields[field as usize]);
    Ok(Program {
        name: root.name.clone(),
        properties: properties(root, &strings, &omitted, &mut remaining)?,
        functions,
    })
}

pub fn parse(data: &[u8]) -> io::Result<File> {
    let Decoded {
        strings,
        elements,
        fields: field_keys,
    } = decode(data)?;
    let mut metadata_bytes = 0usize;
    let mut definitions = Vec::new();
    let mut roots = BTreeSet::new();
    for index in elements[0].references(&field_keys, Field::Definitions)? {
        let index = index.ok_or_else(|| invalid("Particle definition root is null"))?;
        if !roots.insert(index) {
            return Err(invalid("Particle definition root is repeated"));
        }
        let element = &elements[index];
        definition_name(element, &strings)?;
        let mut functions = BTreeMap::new();
        for (field, category) in FUNCTION_FIELDS {
            let mut names = Vec::new();
            for index in element.references(&field_keys, field)? {
                let index = index.ok_or_else(|| invalid("Particle function reference is null"))?;
                let function = &elements[index];
                let name = function_name(function, &strings, &field_keys)?;
                metadata_bytes += name.len();
                if metadata_bytes > MAX_METADATA_BYTES {
                    return Err(invalid("Particle definition metadata exceeds 8 MiB"));
                }
                names.push(name.to_owned());
            }
            functions.insert(category.into(), names);
        }
        let mut children = Vec::new();
        for index in element.references(&field_keys, Field::Children)? {
            let index = index.ok_or_else(|| invalid("Particle child reference is null"))?;
            let child = &elements[index];
            if strings[child.kind] != "DmeParticleChild" {
                return Err(invalid("Particle child has an unexpected element type"));
            }
            let name = match child.field(&field_keys, Field::Child) {
                Some(Attribute::Reference(Some(index)))
                    if strings[elements[*index].kind] == "DmeParticleSystemDefinition" =>
                {
                    &elements[*index].name
                }
                _ => return Err(invalid("Particle child has no internal child definition")),
            };
            if name.is_empty() || name.len() > 255 || name.contains(['\"', '\r', '\n']) {
                return Err(invalid("Particle child system name is invalid"));
            }
            metadata_bytes += name.len();
            if metadata_bytes > MAX_METADATA_BYTES {
                return Err(invalid("Particle definition metadata exceeds 8 MiB"));
            }
            children.push(name.clone());
        }
        let material = match element.field(&field_keys, Field::Material) {
            Some(Attribute::Value(Property::Text(value))) => Some(value.clone()),
            None => None,
            _ => return Err(invalid("Particle material must be a string")),
        };
        let max_particles = match element.field(&field_keys, Field::MaxParticles) {
            Some(Attribute::Value(Property::Int(value))) => Some(*value),
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
        elements: elements.len(),
    })
}
