use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const MAX_DEPTH: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassKind {
    Base,
    Point,
    Solid,
}

impl ClassKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Point => "point",
            Self::Solid => "solid",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub value: String,
    pub label: String,
    pub selected: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Property {
    pub name: String,
    pub kind: String,
    pub label: String,
    pub description: String,
    pub default: Option<String>,
    pub choices: Vec<Choice>,
    pub read_only: bool,
}

impl Property {
    pub fn default_value(&self) -> Option<String> {
        if self.kind == "flags" && self.default.is_none() {
            let mask = self
                .choices
                .iter()
                .filter(|choice| choice.selected)
                .fold(0, |mask, choice| {
                    mask | choice.value.parse::<u64>().unwrap_or_default()
                });
            Some(mask.to_string())
        } else {
            self.default.clone()
        }
    }

    pub fn validate(&self, value: &str) -> Result<(), String> {
        let valid = match self.kind.as_str() {
            "integer" => value.parse::<i64>().is_ok(),
            "flags" => value.parse::<u64>().is_ok(),
            "angle" if self.name.eq_ignore_ascii_case("angles") => vector(value, 3).is_some(),
            "float" | "angle" => number(value).is_some(),
            "bool" | "boolean" => matches!(value, "0" | "1"),
            "vector" | "origin" => vector(value, 3).is_some(),
            "color255" => vector(value, 3)
                .or_else(|| vector(value, 4))
                .is_some_and(|values| {
                    values[..3]
                        .iter()
                        .all(|v| v.fract() == 0.0 && (-1.0..=255.0).contains(v))
                        && values.get(3).is_none_or(|v| *v >= 0.0)
                }),
            "color1" => vector(value, 3)
                .is_some_and(|values| values.iter().all(|v| (0.0..=1.0).contains(v))),
            "choices" => {
                self.choices.is_empty() || self.choices.iter().any(|choice| choice.value == value)
            }
            _ => true,
        };
        if valid {
            Ok(())
        } else {
            Err(format!(
                "{} expects {}, got {value:?}",
                self.name, self.kind
            ))
        }
    }
}

fn number(value: &str) -> Option<f64> {
    value.parse::<f64>().ok().filter(|v| v.is_finite())
}

pub fn vector(value: &str, size: usize) -> Option<Vec<f64>> {
    let values = value
        .split_whitespace()
        .map(number)
        .collect::<Option<Vec<_>>>()?;
    (values.len() == size).then_some(values)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Io {
    pub name: String,
    pub kind: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Helper {
    pub name: String,
    pub arguments: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Class {
    pub name: String,
    pub kind: ClassKind,
    pub description: String,
    pub bases: Vec<String>,
    pub helpers: Vec<Helper>,
    pub properties: Vec<Property>,
    pub inputs: Vec<Io>,
    pub outputs: Vec<Io>,
}

impl Class {
    pub fn property(&self, name: &str) -> Option<&Property> {
        self.properties
            .iter()
            .find(|property| property.name.eq_ignore_ascii_case(name))
    }

    pub fn defaults(&self) -> BTreeMap<String, String> {
        self.properties
            .iter()
            .filter(|property| !property.read_only)
            .filter_map(|property| {
                property
                    .default_value()
                    .map(|value| (property.name.clone(), value))
            })
            .collect()
    }
}

#[derive(Default, Debug)]
pub struct Catalog {
    classes: BTreeMap<String, Class>,
    pub sources: Vec<PathBuf>,
}

impl Catalog {
    pub fn parse(text: &str) -> Result<Self, String> {
        let declarations = Parser::new(text)?.parse()?;
        let mut raw = BTreeMap::new();
        for declaration in declarations {
            match declaration {
                Declaration::Class(class) => {
                    raw.insert(class.name.clone(), class);
                }
                Declaration::Include(_) => {
                    return Err("use Catalog::load to resolve FGD includes".into());
                }
            }
        }
        Self::resolve(raw, Vec::new())
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let mut raw = BTreeMap::new();
        let mut sources = Vec::new();
        load(path, &mut raw, &mut sources, &mut Vec::new())?;
        Self::resolve(raw, sources)
    }

    fn resolve(raw: BTreeMap<String, Class>, sources: Vec<PathBuf>) -> Result<Self, String> {
        let mut classes = BTreeMap::new();
        for name in raw.keys() {
            resolve(name, &raw, &mut classes, &mut Vec::new())?;
        }
        Ok(Self { classes, sources })
    }

    pub fn classes(&self) -> impl Iterator<Item = &Class> {
        self.classes
            .values()
            .filter(|class| class.kind != ClassKind::Base)
    }

    pub fn class(&self, name: &str) -> Option<&Class> {
        self.classes()
            .find(|class| class.name.eq_ignore_ascii_case(name))
    }
}

fn load(
    path: &Path,
    raw: &mut BTreeMap<String, Class>,
    sources: &mut Vec<PathBuf>,
    active: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let path = path
        .canonicalize()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if active.contains(&path) || active.len() >= MAX_DEPTH {
        return Err(format!(
            "FGD include cycle or excessive depth at {}",
            path.display()
        ));
    }
    if sources.contains(&path) {
        return Ok(());
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let declarations = Parser::new(&text)
        .and_then(Parser::parse)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    active.push(path.clone());
    for declaration in declarations {
        match declaration {
            Declaration::Class(class) => {
                raw.insert(class.name.clone(), class);
            }
            Declaration::Include(include) => load(
                &path
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join(include.replace('\\', "/")),
                raw,
                sources,
                active,
            )?,
        }
    }
    active.pop();
    sources.push(path);
    Ok(())
}

fn resolve(
    name: &str,
    raw: &BTreeMap<String, Class>,
    resolved: &mut BTreeMap<String, Class>,
    active: &mut Vec<String>,
) -> Result<Class, String> {
    if let Some(class) = resolved.get(name) {
        return Ok(class.clone());
    }
    if active.iter().any(|item| item == name) || active.len() >= MAX_DEPTH {
        return Err(format!(
            "FGD inheritance cycle or excessive depth at {name}"
        ));
    }
    let own = raw
        .get(name)
        .ok_or_else(|| format!("unknown FGD base class {name}"))?;
    let mut class = own.clone();
    class.properties.clear();
    class.inputs.clear();
    class.outputs.clear();
    class.helpers.clear();
    active.push(name.into());
    for base in &own.bases {
        let parent = resolve(base, raw, resolved, active)?;
        merge(&mut class, &parent);
    }
    merge(&mut class, own);
    active.pop();
    resolved.insert(name.into(), class.clone());
    Ok(class)
}

fn merge(class: &mut Class, other: &Class) {
    for property in &other.properties {
        let mut property = property.clone();
        if property.kind == "flags"
            && let Some(previous) = class.property(&property.name)
        {
            let mut choices = previous.choices.clone();
            for choice in &property.choices {
                upsert(&mut choices, choice.clone(), |item| &item.value);
            }
            property.choices = choices;
        }
        upsert(&mut class.properties, property, |item| &item.name);
    }
    for input in &other.inputs {
        upsert(&mut class.inputs, input.clone(), |item| &item.name);
    }
    for output in &other.outputs {
        upsert(&mut class.outputs, output.clone(), |item| &item.name);
    }
    for helper in &other.helpers {
        upsert(&mut class.helpers, helper.clone(), |item| &item.name);
    }
}

fn upsert<T>(items: &mut Vec<T>, value: T, name: impl Fn(&T) -> &str) {
    if let Some(index) = items
        .iter()
        .position(|item| name(item).eq_ignore_ascii_case(name(&value)))
    {
        items[index] = value;
    } else {
        items.push(value);
    }
}

enum Declaration {
    Include(String),
    Class(Class),
}

struct Token {
    text: String,
    offset: usize,
    quoted: bool,
}

struct Parser<'a> {
    source: &'a str,
    tokens: Vec<Token>,
    index: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Result<Self, String> {
        let mut tokens = Vec::new();
        let mut chars = source.char_indices().peekable();
        while let Some((offset, character)) = chars.next() {
            if character.is_whitespace() || character == '\u{feff}' {
                continue;
            }
            if character == '/' && chars.peek().is_some_and(|(_, c)| *c == '/') {
                for (_, c) in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
                continue;
            }
            if character == '/' && chars.peek().is_some_and(|(_, c)| *c == '*') {
                chars.next();
                let mut closed = false;
                while let Some((_, c)) = chars.next() {
                    if c == '*' && chars.peek().is_some_and(|(_, c)| *c == '/') {
                        chars.next();
                        closed = true;
                        break;
                    }
                }
                if !closed {
                    return Err("unterminated FGD comment".into());
                }
                continue;
            }
            let mut text = String::new();
            let quoted = character == '"';
            if quoted {
                let mut closed = false;
                while let Some((_, c)) = chars.next() {
                    if c == '"' {
                        closed = true;
                        break;
                    }
                    if c == '\\' && chars.peek().is_some_and(|(_, c)| *c == '"') {
                        chars.next();
                        text.push('"');
                    } else {
                        text.push(c);
                    }
                }
                if !closed {
                    return Err(format!("unterminated FGD string at byte {offset}"));
                }
            } else {
                text.push(character);
                if !"()[]=:,+".contains(character) {
                    while let Some(&(_, c)) = chars.peek() {
                        if c.is_whitespace()
                            || "()[]=:,+\"".contains(c)
                            || c == '/' && chars.clone().nth(1).is_some_and(|(_, c)| c == '/')
                        {
                            break;
                        }
                        text.push(c);
                        chars.next();
                    }
                }
            }
            tokens.push(Token {
                text,
                offset,
                quoted,
            });
        }
        Ok(Self {
            source,
            tokens,
            index: 0,
        })
    }

    fn peek(&self) -> &str {
        self.tokens
            .get(self.index)
            .map_or("", |token| token.text.as_str())
    }

    fn take(&mut self, token: &str) -> bool {
        if self
            .tokens
            .get(self.index)
            .is_some_and(|item| !item.quoted && item.text == token)
        {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, token: &str) -> Result<(), String> {
        if self.take(token) {
            Ok(())
        } else {
            Err(self.error(&format!("expected {token:?}, got {:?}", self.peek())))
        }
    }

    fn error(&self, message: &str) -> String {
        let offset = self
            .tokens
            .get(self.index)
            .map_or(self.source.len(), |token| token.offset);
        let line = self.source[..offset]
            .bytes()
            .filter(|b| *b == b'\n')
            .count()
            + 1;
        format!("line {line}: {message}")
    }

    fn atom(&mut self) -> Result<String, String> {
        let token = self
            .tokens
            .get(self.index)
            .ok_or_else(|| self.error("unexpected end of FGD"))?;
        if !token.quoted
            && ("()[]=:,+".contains(token.text.as_str()) || token.text.starts_with('@'))
        {
            return Err(self.error("expected a value"));
        }
        self.index += 1;
        Ok(token.text.clone())
    }

    fn text(&mut self) -> Result<String, String> {
        let mut text = self.atom()?;
        while self.take("+") {
            text += &self.atom()?;
        }
        Ok(text)
    }

    fn optional_text(&mut self) -> Result<Option<String>, String> {
        if self
            .tokens
            .get(self.index)
            .is_none_or(|token| !token.quoted && matches!(token.text.as_str(), ":" | "=" | "]"))
        {
            Ok(None)
        } else {
            self.text().map(Some)
        }
    }

    fn parse(mut self) -> Result<Vec<Declaration>, String> {
        let mut declarations = Vec::new();
        while self.index < self.tokens.len() {
            let directive = self.peek().to_ascii_lowercase();
            self.index += 1;
            match directive.as_str() {
                "@include" => declarations.push(Declaration::Include(self.text()?)),
                "@baseclass" | "@pointclass" | "@solidclass" | "@npcclass" | "@filterclass"
                | "@keyframeclass" | "@moveclass" => {
                    let kind = match directive.as_str() {
                        "@baseclass" => ClassKind::Base,
                        "@solidclass" => ClassKind::Solid,
                        _ => ClassKind::Point,
                    };
                    declarations.push(Declaration::Class(self.class(kind)?));
                }
                _ if directive.starts_with('@') => {
                    let mut depth = 0_i32;
                    while self.index < self.tokens.len() {
                        let token = &self.tokens[self.index];
                        if !token.quoted {
                            if depth == 0 && token.text.starts_with('@') {
                                break;
                            }
                            if matches!(token.text.as_str(), "(" | "[") {
                                depth += 1;
                            }
                            if matches!(token.text.as_str(), ")" | "]") {
                                depth -= 1;
                            }
                        }
                        self.index += 1;
                    }
                    if depth != 0 {
                        return Err(self.error("unbalanced FGD directive"));
                    }
                }
                _ => return Err(self.error("expected an FGD directive")),
            }
        }
        Ok(declarations)
    }

    fn class(&mut self, kind: ClassKind) -> Result<Class, String> {
        let mut helpers = Vec::new();
        let mut bases = Vec::new();
        while !self.take("=") {
            let name = self.atom()?.to_ascii_lowercase();
            let mut arguments = Vec::new();
            if self.take("(") {
                while !self.take(")") {
                    if !self.take(",") {
                        arguments.push(self.atom()?);
                    }
                }
            } else if name == "base" {
                return Err(self.error("base requires parenthesized class names"));
            }
            if name == "base" {
                bases.extend(arguments);
            } else {
                helpers.push(Helper { name, arguments });
            }
        }
        let name = self.atom()?;
        let description = if self.take(":") {
            self.text()?.replace("\\n", "\n")
        } else {
            String::new()
        };
        self.expect("[")?;
        let mut class = Class {
            name,
            kind,
            description,
            bases,
            helpers,
            properties: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
        };
        while !self.take("]") {
            let mut name = self.atom()?;
            let io = if matches!(name.to_ascii_lowercase().as_str(), "input" | "output") {
                let io = name.to_ascii_lowercase();
                name = self.atom()?;
                Some(io)
            } else {
                None
            };
            self.expect("(")?;
            let kind = self.atom()?.to_ascii_lowercase();
            self.expect(")")?;
            let mut read_only = false;
            while matches!(
                self.peek().to_ascii_lowercase().as_str(),
                "readonly" | "report"
            ) {
                read_only |= self.peek().eq_ignore_ascii_case("readonly");
                self.index += 1;
            }
            let mut fields = Vec::new();
            while self.take(":") {
                fields.push(self.optional_text()?);
            }
            if let Some(io) = io {
                let member = Io {
                    name,
                    kind,
                    description: fields
                        .first()
                        .cloned()
                        .flatten()
                        .unwrap_or_default()
                        .replace("\\n", "\n"),
                };
                if io == "input" {
                    class.inputs.push(member);
                } else {
                    class.outputs.push(member);
                }
                continue;
            }
            let mut choices = Vec::new();
            if self.take("=") {
                self.expect("[")?;
                while !self.take("]") {
                    let value = self.text()?;
                    self.expect(":")?;
                    let label = self.text()?;
                    let selected = if self.take(":") {
                        self.text()? == "1"
                    } else {
                        false
                    };
                    choices.push(Choice {
                        value,
                        label,
                        selected,
                    });
                }
            }
            class.properties.push(Property {
                label: fields
                    .first()
                    .cloned()
                    .flatten()
                    .unwrap_or_else(|| name.clone()),
                default: fields.get(1).cloned().flatten(),
                description: fields
                    .get(2)
                    .cloned()
                    .flatten()
                    .unwrap_or_default()
                    .replace("\\n", "\n"),
                name,
                kind,
                choices,
                read_only,
            });
        }
        Ok(class)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFINITIONS: &str = r#"
        @BaseClass = Shared [ value(integer) : "Count" : 2 input Enable(void) : "Enable" ]
        @BaseClass base(Shared) = Light [
            _light(color255) : "Brightness" : "240 230 220 300"
            spawnflags(flags) = [ 1 : "Enabled" : 1 2 : "Special" : 0 ]
        ]
        @PointClass base(Light) size(-8 -8 -8, 8 8 8) = light : "A light. " + "Place it." [
            value(integer) : "Count" : 4
            angles(angle) : "Orientation" : "0 0 0"
            pitch(angle) : "Pitch" : 0
            spawnflags(flags) = [ 2 : "Special" : 1 ]
            style(choices) : "Style" : 0 = [ 0 : "Steady" 1 : "Pulse" ]
            output OnReady(void) : "Ready"
        ]
    "#;

    #[test]
    fn inheritance_defaults_flags_helpers_and_io() {
        let catalog = Catalog::parse(DEFINITIONS).unwrap();
        let light = catalog.class("LIGHT").unwrap();
        assert_eq!(catalog.classes().count(), 1);
        assert_eq!(light.kind, ClassKind::Point);
        assert_eq!(light.description, "A light. Place it.");
        assert_eq!(light.defaults()["value"], "4");
        assert_eq!(light.defaults()["spawnflags"], "3");
        assert_eq!(light.defaults()["_light"], "240 230 220 300");
        assert_eq!(light.inputs[0].name, "Enable");
        assert_eq!(light.outputs[0].name, "OnReady");
        assert_eq!(light.helpers[0].name, "size");
    }

    #[test]
    fn values_reject_nonfinite_numbers_bad_choices_and_colors() {
        let catalog = Catalog::parse(DEFINITIONS).unwrap();
        let light = catalog.class("light").unwrap();
        assert!(light.property("style").unwrap().validate("5").is_err());
        assert!(light.property("value").unwrap().validate("NaN").is_err());
        let angles = light.property("angles").unwrap();
        assert!(angles.validate("0 90 0").is_ok());
        assert!(angles.validate("90").is_err());
        assert!(angles.validate("0 NaN 0").is_err());
        let pitch = light.property("pitch").unwrap();
        assert!(pitch.validate("-45").is_ok());
        assert!(pitch.validate("0 90 0").is_err());
        let color = light.property("_light").unwrap();
        assert!(color.validate("255 128 64 500.5").is_ok());
        assert!(color.validate("-1 -1 -1 1").is_ok());
        assert!(color.validate("256 0 0 200").is_err());
        assert!(color.validate("255 255 255 NaN").is_err());
        assert!(vector("0 inf 0", 3).is_none());
    }

    #[test]
    fn malformed_and_cyclic_definitions_are_rejected() {
        for source in [
            "@PointClass = broken [ value(integer)",
            "@PointClass = bad [ value(integer) : \"unterminated",
            "@BaseClass base(A) = A []",
            "@PointClass base(Missing) = point []",
        ] {
            assert!(Catalog::parse(source).is_err(), "{source}");
        }
    }

    #[test]
    fn comments_empty_defaults_and_readonly_fields() {
        let catalog = Catalog::parse(
            r#"/* comment */ @PointClass = point [
            text(string) : "Text // not a comment" : "" // comment
            id(integer) readonly : "ID" : 1
            limit(integer) : "Limit" : : "No default"
        ]"#,
        )
        .unwrap();
        let class = catalog.class("point").unwrap();
        assert_eq!(class.defaults()["text"], "");
        assert!(!class.defaults().contains_key("id"));
        assert!(!class.defaults().contains_key("limit"));
        assert!(class.property("id").unwrap().read_only);
    }
}
