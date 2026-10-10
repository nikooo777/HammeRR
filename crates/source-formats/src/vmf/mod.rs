//! Typed model of Hammer's VMF map format.
//!
//! Keys and blocks this model doesn't understand are kept in `extra` fields
//! and written back after the known ones, so files from other Hammer versions
//! (including Hammer++) survive a load/save cycle.

mod values;

pub use values::{Color, Plane, UvAxis, format_number, format_vec3, parse_numbers, parse_vec3};

use glam::DVec3;

use crate::keyvalues::{self, Node, Value, WriteOptions};
use values::{
    format_int_list, format_number_list, format_vec3_bracketed, format_vec3_list, parse_int_list,
    parse_vec3_list,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Syntax(#[from] keyvalues::ParseError),
    #[error("{path}: {message}")]
    Invalid { path: String, message: String },
}

fn invalid(path: &str, message: impl Into<String>) -> Error {
    Error::Invalid {
        path: path.to_owned(),
        message: message.into(),
    }
}

/// Ordered string key/values. Lookups are ASCII case-insensitive, matching
/// how the engine treats entity keys.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Properties(pub Vec<(String, String)>);

impl Properties {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    pub fn get_all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.0
            .iter()
            .filter(move |(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    /// Replaces the first value stored under `key`, or appends a new pair.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = key.into();
        let value = value.into();
        match self
            .0
            .iter_mut()
            .find(|(k, _)| k.eq_ignore_ascii_case(&key))
        {
            Some((_, v)) => *v = value,
            None => self.0.push((key, value)),
        }
    }

    pub fn remove(&mut self, key: &str) -> Option<String> {
        let index = self
            .0
            .iter()
            .position(|(k, _)| k.eq_ignore_ascii_case(key))?;
        Some(self.0.remove(index).1)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    fn parse(children: &[Node], path: &str) -> Result<Self, Error> {
        children
            .iter()
            .map(|node| match &node.value {
                Value::String(v) => Ok((node.key.clone(), v.clone())),
                Value::Block(_) => Err(invalid(path, format!("unexpected block '{}'", node.key))),
            })
            .collect::<Result<_, _>>()
            .map(Self)
    }

    fn push_nodes(&self, out: &mut Vec<Node>) {
        out.extend(
            self.0
                .iter()
                .map(|(k, v)| Node::string(k.clone(), v.clone())),
        );
    }

    fn to_block(&self, key: &str) -> Node {
        let mut children = Vec::with_capacity(self.0.len());
        self.push_nodes(&mut children);
        Node::block(key, children)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Vmf {
    pub version_info: Option<Properties>,
    pub visgroups: Option<Vec<VisGroup>>,
    pub view_settings: Option<Properties>,
    pub world: World,
    /// Point and brush entities, in file order. Hidden ones carry `hidden`.
    pub entities: Vec<Entity>,
    pub cameras: Option<Node>,
    pub cordon: Option<Node>,
    pub extra: Vec<Node>,
}

impl Vmf {
    pub fn parse(src: &str) -> Result<Self, Error> {
        Self::from_nodes(&keyvalues::parse(src)?)
    }

    pub fn from_nodes(nodes: &[Node]) -> Result<Self, Error> {
        let mut version_info = None;
        let mut visgroups = None;
        let mut view_settings = None;
        let mut world = None;
        let mut entities = Vec::new();
        let mut cameras = None;
        let mut cordon = None;
        let mut extra = Vec::new();

        for node in nodes {
            let Some(children) = node.children() else {
                extra.push(node.clone());
                continue;
            };
            match node.key.as_str() {
                "versioninfo" if version_info.is_none() => {
                    version_info = Some(Properties::parse(children, "versioninfo")?);
                }
                "visgroups" if visgroups.is_none() => {
                    visgroups = Some(VisGroup::parse_list(children, "visgroups")?);
                }
                "viewsettings" if view_settings.is_none() => {
                    view_settings = Some(Properties::parse(children, "viewsettings")?);
                }
                "world" => {
                    if world.is_some() {
                        return Err(invalid("world", "more than one world block"));
                    }
                    world = Some(World::parse(children)?);
                }
                "entity" => entities.push(Entity::parse(children, false)?),
                "hidden" if all_blocks_named(children, "entity") => {
                    for child in children {
                        entities.push(Entity::parse(child.children().unwrap_or_default(), true)?);
                    }
                }
                "cameras" if cameras.is_none() => cameras = Some(node.clone()),
                "cordon" if cordon.is_none() => cordon = Some(node.clone()),
                _ => extra.push(node.clone()),
            }
        }

        Ok(Self {
            version_info,
            visgroups,
            view_settings,
            world: world.ok_or_else(|| invalid("(root)", "missing world block"))?,
            entities,
            cameras,
            cordon,
            extra,
        })
    }

    pub fn to_nodes(&self) -> Vec<Node> {
        let mut out = Vec::with_capacity(self.entities.len() + 8);
        if let Some(info) = &self.version_info {
            out.push(info.to_block("versioninfo"));
        }
        if let Some(groups) = &self.visgroups {
            out.push(Node::block(
                "visgroups",
                groups.iter().map(VisGroup::to_node).collect(),
            ));
        }
        if let Some(settings) = &self.view_settings {
            out.push(settings.to_block("viewsettings"));
        }
        out.push(self.world.to_node());
        out.extend(self.entities.iter().map(Entity::to_node));
        out.extend(self.cameras.iter().cloned());
        out.extend(self.cordon.iter().cloned());
        out.extend(self.extra.iter().cloned());
        out
    }

    pub fn write(&self, options: &WriteOptions) -> String {
        keyvalues::write(&self.to_nodes(), options)
    }

    /// All solids in the map: world brushes first, then brush entities.
    pub fn solids(&self) -> impl Iterator<Item = &Solid> {
        self.world
            .solids
            .iter()
            .chain(self.entities.iter().flat_map(|e| &e.solids))
    }
}

fn all_blocks_named(children: &[Node], key: &str) -> bool {
    children
        .iter()
        .all(|c| c.key == key && c.children().is_some())
}

fn id_label(children: &[Node]) -> &str {
    children
        .iter()
        .find(|n| n.key == "id")
        .and_then(Node::as_str)
        .unwrap_or("?")
}

fn parse_with<T>(
    value: &str,
    path: &str,
    key: &str,
    parse: impl FnOnce(&str) -> Option<T>,
) -> Result<T, Error> {
    parse(value).ok_or_else(|| invalid(path, format!("invalid {key} {value:?}")))
}

fn parse_from_str<T: std::str::FromStr>(value: &str, path: &str, key: &str) -> Result<T, Error> {
    parse_with(value, path, key, |s| s.parse().ok())
}

fn required<T>(value: Option<T>, path: &str, key: &str) -> Result<T, Error> {
    value.ok_or_else(|| invalid(path, format!("missing '{key}'")))
}

/// Pushes `solid` blocks, wrapping hidden ones in `hidden { }` like Hammer.
fn push_solids(out: &mut Vec<Node>, solids: &[Solid]) {
    for solid in solids {
        let node = solid.to_node();
        out.push(if solid.hidden {
            Node::block("hidden", vec![node])
        } else {
            node
        });
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct VisGroup {
    pub name: String,
    pub id: u32,
    pub color: Color,
    pub children: Vec<VisGroup>,
    pub extra_properties: Properties,
    pub extra: Vec<Node>,
}

impl VisGroup {
    fn parse_list(children: &[Node], path: &str) -> Result<Vec<Self>, Error> {
        children
            .iter()
            .filter_map(|node| match (&node.value, node.key.as_str()) {
                (Value::Block(c), "visgroup") => Some(Self::parse(c, path)),
                _ => None,
            })
            .collect()
    }

    fn parse(children: &[Node], parent: &str) -> Result<Self, Error> {
        let path = format!("{parent}/visgroup {}", id_label(children));
        let mut name = None;
        let mut id = None;
        let mut color = None;
        let mut groups = Vec::new();
        let mut extra_properties = Properties::default();
        let mut extra = Vec::new();
        for node in children {
            match (&node.value, node.key.as_str()) {
                (Value::String(v), "name") => name = Some(v.clone()),
                (Value::String(v), "visgroupid") => {
                    id = Some(parse_from_str(v, &path, "visgroupid")?)
                }
                (Value::String(v), "color") => {
                    color = Some(parse_with(v, &path, "color", Color::parse)?)
                }
                (Value::String(v), _) => extra_properties.0.push((node.key.clone(), v.clone())),
                (Value::Block(c), "visgroup") => groups.push(Self::parse(c, &path)?),
                (Value::Block(_), _) => extra.push(node.clone()),
            }
        }
        Ok(Self {
            name: required(name, &path, "name")?,
            id: required(id, &path, "visgroupid")?,
            color: required(color, &path, "color")?,
            children: groups,
            extra_properties,
            extra,
        })
    }

    fn to_node(&self) -> Node {
        let mut out = vec![
            Node::string("name", self.name.clone()),
            Node::string("visgroupid", self.id.to_string()),
            Node::string("color", self.color.to_string()),
        ];
        self.extra_properties.push_nodes(&mut out);
        out.extend(self.children.iter().map(Self::to_node));
        out.extend(self.extra.iter().cloned());
        Node::block("visgroup", out)
    }
}

/// The `world` block: worldspawn key/values, world brushes and groups.
#[derive(Debug, Clone, PartialEq)]
pub struct World {
    pub id: u32,
    pub properties: Properties,
    pub solids: Vec<Solid>,
    pub groups: Vec<Group>,
    pub extra: Vec<Node>,
}

impl World {
    fn parse(children: &[Node]) -> Result<Self, Error> {
        let path = "world";
        let mut id = None;
        let mut properties = Properties::default();
        let mut solids = Vec::new();
        let mut groups = Vec::new();
        let mut extra = Vec::new();
        for node in children {
            match (&node.value, node.key.as_str()) {
                (Value::String(v), "id") if id.is_none() => {
                    id = Some(parse_from_str(v, path, "id")?)
                }
                (Value::String(v), _) => properties.0.push((node.key.clone(), v.clone())),
                (Value::Block(c), "solid") => solids.push(Solid::parse(c, path, false)?),
                (Value::Block(c), "hidden") if all_blocks_named(c, "solid") => {
                    for child in c {
                        solids.push(Solid::parse(
                            child.children().unwrap_or_default(),
                            path,
                            true,
                        )?);
                    }
                }
                (Value::Block(c), "group") => groups.push(Group::parse(c, path)?),
                (Value::Block(_), _) => extra.push(node.clone()),
            }
        }
        Ok(Self {
            id: required(id, path, "id")?,
            properties,
            solids,
            groups,
            extra,
        })
    }

    fn to_node(&self) -> Node {
        let mut out = Vec::with_capacity(self.properties.0.len() + self.solids.len() + 1);
        out.push(Node::string("id", self.id.to_string()));
        self.properties.push_nodes(&mut out);
        push_solids(&mut out, &self.solids);
        out.extend(self.groups.iter().map(Group::to_node));
        out.extend(self.extra.iter().cloned());
        Node::block("world", out)
    }
}

/// An editor group (Ctrl+G in Hammer). Members point at it via `editor.groupid`.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub id: u32,
    pub editor: Option<Properties>,
    pub extra_properties: Properties,
    pub extra: Vec<Node>,
}

impl Group {
    fn parse(children: &[Node], parent: &str) -> Result<Self, Error> {
        let path = format!("{parent}/group {}", id_label(children));
        let mut id = None;
        let mut editor = None;
        let mut extra_properties = Properties::default();
        let mut extra = Vec::new();
        for node in children {
            match (&node.value, node.key.as_str()) {
                (Value::String(v), "id") if id.is_none() => {
                    id = Some(parse_from_str(v, &path, "id")?)
                }
                (Value::String(v), _) => extra_properties.0.push((node.key.clone(), v.clone())),
                (Value::Block(c), "editor") if editor.is_none() => {
                    editor = Some(Properties::parse(c, &path)?);
                }
                (Value::Block(_), _) => extra.push(node.clone()),
            }
        }
        Ok(Self {
            id: required(id, &path, "id")?,
            editor,
            extra_properties,
            extra,
        })
    }

    fn to_node(&self) -> Node {
        let mut out = vec![Node::string("id", self.id.to_string())];
        self.extra_properties.push_nodes(&mut out);
        out.extend(self.editor.iter().map(|e| e.to_block("editor")));
        out.extend(self.extra.iter().cloned());
        Node::block("group", out)
    }
}

/// An entity I/O connection: `"OnTrigger" "target,input,param,delay,times"`.
/// Newer Hammer versions separate the fields with ESC (0x1B) instead of commas.
#[derive(Debug, Clone, PartialEq)]
pub struct Connection {
    pub output: String,
    pub value: String,
}

impl Connection {
    pub fn fields(&self) -> impl Iterator<Item = &str> {
        let separator = if self.value.contains('\u{1b}') {
            '\u{1b}'
        } else {
            ','
        };
        self.value.split(separator)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Property,
    Connections,
    Solid,
    Editor,
    Extra,
}

/// Child order of an entity as read from the file. Hammer writes some
/// entities with keys after `connections` (point entity `origin`) or with a
/// stale `editor` block before the brushes; replaying the order keeps
/// unmodified entities byte-identical. Ignored by equality.
#[derive(Debug, Clone, Default)]
struct Layout(Vec<Slot>);

impl PartialEq for Layout {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entity {
    pub id: u32,
    /// Entity key/values (`classname`, `origin`, `targetname`, ...).
    pub properties: Properties,
    pub connections: Option<Vec<Connection>>,
    pub solids: Vec<Solid>,
    pub editor: Option<Properties>,
    pub hidden: bool,
    pub extra: Vec<Node>,
    layout: Layout,
}

impl Entity {
    pub fn new(id: u32, classname: impl Into<String>) -> Self {
        Self {
            id,
            properties: Properties(vec![("classname".into(), classname.into())]),
            connections: None,
            solids: Vec::new(),
            editor: None,
            hidden: false,
            extra: Vec::new(),
            layout: Layout::default(),
        }
    }

    pub fn classname(&self) -> Option<&str> {
        self.properties.get("classname")
    }

    pub fn origin(&self) -> Option<DVec3> {
        self.properties.get("origin").and_then(parse_vec3)
    }

    fn parse(children: &[Node], hidden: bool) -> Result<Self, Error> {
        let path = format!("entity {}", id_label(children));
        let mut id = None;
        let mut properties = Properties::default();
        let mut connections = None;
        let mut solids = Vec::new();
        let mut editor = None;
        let mut extra = Vec::new();
        let mut layout = Vec::with_capacity(children.len());
        for node in children {
            match (&node.value, node.key.as_str()) {
                (Value::String(v), "id") if id.is_none() => {
                    id = Some(parse_from_str(v, &path, "id")?)
                }
                (Value::String(v), _) => {
                    properties.0.push((node.key.clone(), v.clone()));
                    layout.push(Slot::Property);
                }
                (Value::Block(c), "connections") if connections.is_none() => {
                    let list = Properties::parse(c, &format!("{path}/connections"))?;
                    connections = Some(
                        list.0
                            .into_iter()
                            .map(|(output, value)| Connection { output, value })
                            .collect(),
                    );
                    layout.push(Slot::Connections);
                }
                (Value::Block(c), "solid") => {
                    solids.push(Solid::parse(c, &path, false)?);
                    layout.push(Slot::Solid);
                }
                (Value::Block(c), "hidden") if all_blocks_named(c, "solid") => {
                    for child in c {
                        solids.push(Solid::parse(
                            child.children().unwrap_or_default(),
                            &path,
                            true,
                        )?);
                        layout.push(Slot::Solid);
                    }
                }
                (Value::Block(c), "editor") if editor.is_none() => {
                    editor = Some(Properties::parse(c, &format!("{path}/editor"))?);
                    layout.push(Slot::Editor);
                }
                (Value::Block(_), _) => {
                    extra.push(node.clone());
                    layout.push(Slot::Extra);
                }
            }
        }
        Ok(Self {
            id: required(id, &path, "id")?,
            properties,
            connections,
            solids,
            editor,
            hidden,
            extra,
            layout: Layout(layout),
        })
    }

    /// The recorded layout if it still describes this entity's children,
    /// otherwise Hammer's standard order.
    fn slots(&self) -> Vec<Slot> {
        let count = |slot| self.layout.0.iter().filter(|&&s| s == slot).count();
        let matches = count(Slot::Property) == self.properties.0.len()
            && count(Slot::Connections) == usize::from(self.connections.is_some())
            && count(Slot::Solid) == self.solids.len()
            && count(Slot::Editor) == usize::from(self.editor.is_some())
            && count(Slot::Extra) == self.extra.len();
        if matches {
            return self.layout.0.clone();
        }
        let mut slots = vec![Slot::Property; self.properties.0.len()];
        slots.extend(self.connections.as_ref().map(|_| Slot::Connections));
        slots.extend(std::iter::repeat_n(Slot::Solid, self.solids.len()));
        slots.extend(self.editor.as_ref().map(|_| Slot::Editor));
        slots.extend(std::iter::repeat_n(Slot::Extra, self.extra.len()));
        slots
    }

    fn to_node(&self) -> Node {
        let mut out = Vec::with_capacity(self.properties.0.len() + self.solids.len() + 3);
        out.push(Node::string("id", self.id.to_string()));
        let mut properties = self.properties.0.iter();
        let mut solids = self.solids.iter();
        let mut extra = self.extra.iter();
        for slot in self.slots() {
            match slot {
                Slot::Property => {
                    let (k, v) = properties.next().expect("slot counts match properties");
                    out.push(Node::string(k.clone(), v.clone()));
                }
                Slot::Connections => {
                    let connections = self.connections.as_deref().unwrap_or_default();
                    out.push(Node::block(
                        "connections",
                        connections
                            .iter()
                            .map(|c| Node::string(c.output.clone(), c.value.clone()))
                            .collect(),
                    ));
                }
                Slot::Solid => {
                    let solid = solids.next().expect("slot counts match solids");
                    push_solids(&mut out, std::slice::from_ref(solid));
                }
                Slot::Editor => out.extend(self.editor.iter().map(|e| e.to_block("editor"))),
                Slot::Extra => out.extend(extra.next().cloned()),
            }
        }
        let node = Node::block("entity", out);
        if self.hidden {
            Node::block("hidden", vec![node])
        } else {
            node
        }
    }
}

/// A convex brush, defined by the intersection of its sides' half-spaces.
#[derive(Debug, Clone, PartialEq)]
pub struct Solid {
    pub id: u32,
    pub sides: Vec<Side>,
    pub editor: Option<Properties>,
    pub hidden: bool,
    pub extra_properties: Properties,
    pub extra: Vec<Node>,
}

impl Solid {
    fn parse(children: &[Node], parent: &str, hidden: bool) -> Result<Self, Error> {
        let path = format!("{parent}/solid {}", id_label(children));
        let mut id = None;
        let mut sides = Vec::new();
        let mut editor = None;
        let mut extra_properties = Properties::default();
        let mut extra = Vec::new();
        for node in children {
            match (&node.value, node.key.as_str()) {
                (Value::String(v), "id") if id.is_none() => {
                    id = Some(parse_from_str(v, &path, "id")?)
                }
                (Value::String(v), _) => extra_properties.0.push((node.key.clone(), v.clone())),
                (Value::Block(c), "side") => sides.push(Side::parse(c, &path)?),
                (Value::Block(c), "editor") if editor.is_none() => {
                    editor = Some(Properties::parse(c, &format!("{path}/editor"))?);
                }
                (Value::Block(_), _) => extra.push(node.clone()),
            }
        }
        Ok(Self {
            id: required(id, &path, "id")?,
            sides,
            editor,
            hidden,
            extra_properties,
            extra,
        })
    }

    fn to_node(&self) -> Node {
        let mut out = Vec::with_capacity(self.sides.len() + 2);
        out.push(Node::string("id", self.id.to_string()));
        self.extra_properties.push_nodes(&mut out);
        out.extend(self.sides.iter().map(Side::to_node));
        out.extend(self.editor.iter().map(|e| e.to_block("editor")));
        out.extend(self.extra.iter().cloned());
        Node::block("solid", out)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Side {
    pub id: u32,
    pub plane: Plane,
    pub material: String,
    pub u_axis: UvAxis,
    pub v_axis: UvAxis,
    pub rotation: f64,
    pub lightmap_scale: u32,
    /// Bitmask of smoothing groups. Absent in files from Hammer versions
    /// that predate the feature, which compile as if it were 0.
    pub smoothing_groups: Option<u32>,
    pub displacement: Option<DispInfo>,
    pub extra_properties: Properties,
    pub extra: Vec<Node>,
}

impl Side {
    fn parse(children: &[Node], parent: &str) -> Result<Self, Error> {
        let path = format!("{parent}/side {}", id_label(children));
        let mut id = None;
        let mut plane = None;
        let mut material = None;
        let mut u_axis = None;
        let mut v_axis = None;
        let mut rotation = 0.0;
        let mut lightmap_scale = 16;
        let mut smoothing_groups = None;
        let mut displacement = None;
        let mut extra_properties = Properties::default();
        let mut extra = Vec::new();
        for node in children {
            match (&node.value, node.key.as_str()) {
                (Value::String(v), "id") => id = Some(parse_from_str(v, &path, "id")?),
                (Value::String(v), "plane") => {
                    plane = Some(parse_with(v, &path, "plane", Plane::parse)?)
                }
                (Value::String(v), "material") => material = Some(v.clone()),
                (Value::String(v), "uaxis") => {
                    u_axis = Some(parse_with(v, &path, "uaxis", UvAxis::parse)?)
                }
                (Value::String(v), "vaxis") => {
                    v_axis = Some(parse_with(v, &path, "vaxis", UvAxis::parse)?)
                }
                (Value::String(v), "rotation") => rotation = parse_from_str(v, &path, "rotation")?,
                (Value::String(v), "lightmapscale") => {
                    lightmap_scale = parse_from_str(v, &path, "lightmapscale")?;
                }
                (Value::String(v), "smoothing_groups") => {
                    smoothing_groups = Some(parse_from_str(v, &path, "smoothing_groups")?);
                }
                (Value::String(v), _) => extra_properties.0.push((node.key.clone(), v.clone())),
                (Value::Block(c), "dispinfo") if displacement.is_none() => {
                    displacement = Some(DispInfo::parse(c, &format!("{path}/dispinfo"))?);
                }
                (Value::Block(_), _) => extra.push(node.clone()),
            }
        }
        Ok(Self {
            id: required(id, &path, "id")?,
            plane: required(plane, &path, "plane")?,
            material: required(material, &path, "material")?,
            u_axis: required(u_axis, &path, "uaxis")?,
            v_axis: required(v_axis, &path, "vaxis")?,
            rotation,
            lightmap_scale,
            smoothing_groups,
            displacement,
            extra_properties,
            extra,
        })
    }

    fn to_node(&self) -> Node {
        let mut out = Vec::with_capacity(9 + self.extra_properties.0.len());
        out.push(Node::string("id", self.id.to_string()));
        out.push(Node::string("plane", self.plane.to_string()));
        out.push(Node::string("material", self.material.clone()));
        out.push(Node::string("uaxis", self.u_axis.to_string()));
        out.push(Node::string("vaxis", self.v_axis.to_string()));
        out.push(Node::string("rotation", format_number(self.rotation)));
        out.push(Node::string(
            "lightmapscale",
            self.lightmap_scale.to_string(),
        ));
        out.extend(
            self.smoothing_groups
                .map(|g| Node::string("smoothing_groups", g.to_string())),
        );
        self.extra_properties.push_nodes(&mut out);
        out.extend(self.displacement.iter().map(DispInfo::to_node));
        out.extend(self.extra.iter().cloned());
        Node::block("side", out)
    }
}

/// Displacement data attached to a four-sided brush face.
///
/// Per-vertex fields are stored row by row as in the file: a power-`p`
/// displacement has `2^p + 1` rows of `2^p + 1` vertices, and
/// `triangle_tags` has `2^p` rows of `2 * 2^p` triangles.
#[derive(Debug, Clone, PartialEq)]
pub struct DispInfo {
    pub power: u32,
    pub start_position: DVec3,
    pub flags: Option<u32>,
    pub elevation: f64,
    pub subdivided: bool,
    pub normals: Option<Vec<Vec<DVec3>>>,
    pub distances: Option<Vec<Vec<f64>>>,
    pub offsets: Option<Vec<Vec<DVec3>>>,
    pub offset_normals: Option<Vec<Vec<DVec3>>>,
    pub alphas: Option<Vec<Vec<f64>>>,
    pub triangle_tags: Option<Vec<Vec<i32>>>,
    pub allowed_verts: Option<Vec<i32>>,
    pub extra_properties: Properties,
    pub extra: Vec<Node>,
}

fn parse_rows<T>(
    children: &[Node],
    path: &str,
    parse: impl Fn(&str) -> Option<T>,
) -> Result<Vec<T>, Error> {
    children
        .iter()
        .enumerate()
        .map(|(i, node)| {
            let index = node
                .key
                .strip_prefix("row")
                .and_then(|n| n.parse::<usize>().ok());
            match (node.as_str(), index) {
                (Some(value), Some(index)) if index == i => {
                    parse_with(value, path, &node.key, &parse)
                }
                _ => Err(invalid(
                    path,
                    format!("expected row{i}, found '{}'", node.key),
                )),
            }
        })
        .collect()
}

fn rows_node<T>(key: &str, rows: &[T], format: impl Fn(&T) -> String) -> Node {
    Node::block(
        key,
        rows.iter()
            .enumerate()
            .map(|(i, row)| Node::string(format!("row{i}"), format(row)))
            .collect(),
    )
}

impl DispInfo {
    fn parse(children: &[Node], path: &str) -> Result<Self, Error> {
        let mut power = None;
        let mut start_position = None;
        let mut flags = None;
        let mut elevation = 0.0;
        let mut subdivided = false;
        let mut normals = None;
        let mut distances = None;
        let mut offsets = None;
        let mut offset_normals = None;
        let mut alphas = None;
        let mut triangle_tags = None;
        let mut allowed_verts = None;
        let mut extra_properties = Properties::default();
        let mut extra = Vec::new();
        let floats = |s: &str| parse_numbers(s);
        for node in children {
            let block_path = || format!("{path}/{}", node.key);
            match (&node.value, node.key.as_str()) {
                (Value::String(v), "power") => power = Some(parse_from_str(v, path, "power")?),
                (Value::String(v), "startposition") => {
                    start_position = Some(parse_with(v, path, "startposition", parse_vec3)?);
                }
                (Value::String(v), "flags") => flags = Some(parse_from_str(v, path, "flags")?),
                (Value::String(v), "elevation") => {
                    elevation = parse_from_str(v, path, "elevation")?
                }
                (Value::String(v), "subdiv") => {
                    subdivided = parse_with(v, path, "subdiv", |s| match s {
                        "0" => Some(false),
                        "1" => Some(true),
                        _ => None,
                    })?;
                }
                (Value::String(v), _) => extra_properties.0.push((node.key.clone(), v.clone())),
                (Value::Block(c), "normals") => {
                    normals = Some(parse_rows(c, &block_path(), parse_vec3_list)?)
                }
                (Value::Block(c), "distances") => {
                    distances = Some(parse_rows(c, &block_path(), floats)?)
                }
                (Value::Block(c), "offsets") => {
                    offsets = Some(parse_rows(c, &block_path(), parse_vec3_list)?)
                }
                (Value::Block(c), "offset_normals") => {
                    offset_normals = Some(parse_rows(c, &block_path(), parse_vec3_list)?);
                }
                (Value::Block(c), "alphas") => alphas = Some(parse_rows(c, &block_path(), floats)?),
                (Value::Block(c), "triangle_tags") => {
                    triangle_tags = Some(parse_rows(c, &block_path(), parse_int_list)?);
                }
                (Value::Block(c), "allowed_verts") => {
                    let block_path = block_path();
                    let [entry] = c.as_slice() else {
                        return Err(invalid(&block_path, "expected exactly one entry"));
                    };
                    let values = entry
                        .as_str()
                        .and_then(parse_int_list)
                        .filter(|v| entry.key.parse() == Ok(v.len()))
                        .ok_or_else(|| {
                            invalid(&block_path, format!("invalid entry '{}'", entry.key))
                        })?;
                    allowed_verts = Some(values);
                }
                (Value::Block(_), _) => extra.push(node.clone()),
            }
        }
        Ok(Self {
            power: required(power, path, "power")?,
            start_position: required(start_position, path, "startposition")?,
            flags,
            elevation,
            subdivided,
            normals,
            distances,
            offsets,
            offset_normals,
            alphas,
            triangle_tags,
            allowed_verts,
            extra_properties,
            extra,
        })
    }

    fn to_node(&self) -> Node {
        let mut out = vec![
            Node::string("power", self.power.to_string()),
            Node::string("startposition", format_vec3_bracketed(self.start_position)),
        ];
        out.extend(self.flags.map(|f| Node::string("flags", f.to_string())));
        out.push(Node::string("elevation", format_number(self.elevation)));
        out.push(Node::string(
            "subdiv",
            if self.subdivided { "1" } else { "0" },
        ));
        self.extra_properties.push_nodes(&mut out);
        let vec3_rows = |row: &Vec<DVec3>| format_vec3_list(row);
        let float_rows = |row: &Vec<f64>| format_number_list(row);
        out.extend(
            self.normals
                .as_deref()
                .map(|r| rows_node("normals", r, vec3_rows)),
        );
        out.extend(
            self.distances
                .as_deref()
                .map(|r| rows_node("distances", r, float_rows)),
        );
        out.extend(
            self.offsets
                .as_deref()
                .map(|r| rows_node("offsets", r, vec3_rows)),
        );
        out.extend(
            self.offset_normals
                .as_deref()
                .map(|r| rows_node("offset_normals", r, vec3_rows)),
        );
        out.extend(
            self.alphas
                .as_deref()
                .map(|r| rows_node("alphas", r, float_rows)),
        );
        out.extend(
            self.triangle_tags
                .as_deref()
                .map(|r| rows_node("triangle_tags", r, |row| format_int_list(row))),
        );
        out.extend(self.allowed_verts.as_ref().map(|v| {
            Node::block(
                "allowed_verts",
                vec![Node::string(v.len().to_string(), format_int_list(v))],
            )
        }));
        out.extend(self.extra.iter().cloned());
        Node::block("dispinfo", out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POINT_ENTITY_MAP: &str = "\
world
{
\t\"id\" \"1\"
\t\"classname\" \"worldspawn\"
}
entity
{
\t\"id\" \"2\"
\t\"classname\" \"logic_relay\"
\tconnections
\t{
\t\t\"OnTrigger\" \"door,Open,,0,-1\"
\t}
\t\"origin\" \"0 0 64\"
\teditor
\t{
\t\t\"color\" \"220 30 220\"
\t}
}
";

    fn write_lf(vmf: &Vmf) -> String {
        vmf.write(&WriteOptions {
            line_ending: keyvalues::LineEnding::Lf,
        })
    }

    #[test]
    fn unedited_entity_keeps_file_order() {
        let vmf = Vmf::parse(POINT_ENTITY_MAP).unwrap();
        assert_eq!(write_lf(&vmf), POINT_ENTITY_MAP);

        let entity = &vmf.entities[0];
        assert_eq!(entity.origin(), Some(DVec3::new(0.0, 0.0, 64.0)));
        let fields: Vec<_> = entity.connections.as_ref().unwrap()[0].fields().collect();
        assert_eq!(fields, ["door", "Open", "", "0", "-1"]);
    }

    #[test]
    fn edited_entity_falls_back_to_standard_order() {
        let mut vmf = Vmf::parse(POINT_ENTITY_MAP).unwrap();
        vmf.entities[0].properties.set("targetname", "relay");

        let reparsed = Vmf::parse(&write_lf(&vmf)).unwrap();
        assert_eq!(reparsed, vmf);
        let nodes = reparsed.to_nodes();
        let keys: Vec<_> = nodes[1]
            .children()
            .unwrap()
            .iter()
            .map(|n| n.key.as_str())
            .collect();
        assert_eq!(
            keys,
            [
                "id",
                "classname",
                "origin",
                "targetname",
                "connections",
                "editor"
            ]
        );
    }

    #[test]
    fn new_entities_can_be_built_and_written() {
        let mut vmf = Vmf::parse(POINT_ENTITY_MAP).unwrap();
        let mut light = Entity::new(3, "light");
        light.properties.set("origin", "0 0 128");
        vmf.entities.push(light);

        let reparsed = Vmf::parse(&write_lf(&vmf)).unwrap();
        assert_eq!(reparsed.entities[1].classname(), Some("light"));
        assert_eq!(
            reparsed.entities[1].origin(),
            Some(DVec3::new(0.0, 0.0, 128.0))
        );
    }

    #[test]
    fn reports_where_a_value_is_invalid() {
        let src = POINT_ENTITY_MAP.replace(
            "\t\"classname\" \"worldspawn\"\n",
            "\t\"classname\" \"worldspawn\"\n\tsolid\n\t{\n\t\t\"id\" \"7\"\n\t\tside\n\t\t{\n\t\t\t\"id\" \"9\"\n\t\t\t\"plane\" \"(0 0 0) (1 1)\"\n\t\t}\n\t}\n",
        );
        let err = Vmf::parse(&src).unwrap_err().to_string();
        assert_eq!(err, "world/solid 7/side 9: invalid plane \"(0 0 0) (1 1)\"");
    }
}
