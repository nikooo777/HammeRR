use std::mem::size_of;

use source_formats::{
    keyvalues::{Node, Value},
    vmf::{DispInfo, Entity, Properties, Solid, VisGroup, Vmf},
};

pub(super) const MAX_BYTES: usize = 32 * 1024 * 1024;
pub(super) const MAX_ITEMS: usize = 131_072;
pub(super) const MAX_DEPTH: usize = 64;

#[derive(Default)]
struct Budget {
    bytes: usize,
    items: usize,
}

impl Budget {
    fn bytes(&mut self, bytes: usize) -> Result<(), String> {
        self.bytes = self.bytes.saturating_add(bytes);
        if self.bytes > MAX_BYTES {
            return Err("3D sky preview source snapshot exceeds 32 MiB".into());
        }
        Ok(())
    }

    fn list<T>(&mut self, values: &[T]) -> Result<(), String> {
        self.items = self.items.saturating_add(values.len());
        if self.items > MAX_ITEMS {
            return Err("3D sky preview source snapshot exceeds 131072 items".into());
        }
        self.bytes(
            values
                .len()
                .saturating_mul(size_of::<T>().saturating_add(16)),
        )
    }

    fn text(&mut self, text: &str) -> Result<(), String> {
        self.bytes(text.len())
    }

    fn properties(&mut self, properties: &Properties) -> Result<(), String> {
        self.list(&properties.0)?;
        for (key, value) in &properties.0 {
            self.text(key)?;
            self.text(value)?;
        }
        Ok(())
    }

    fn optional_properties(&mut self, properties: Option<&Properties>) -> Result<(), String> {
        if let Some(properties) = properties {
            self.properties(properties)?;
        }
        Ok(())
    }

    fn nodes(&mut self, nodes: &[Node], depth: usize) -> Result<(), String> {
        self.list(nodes)?;
        for node in nodes {
            self.node(node, depth)?;
        }
        Ok(())
    }

    fn node(&mut self, node: &Node, depth: usize) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err("3D sky preview source snapshot exceeds 64 nested levels".into());
        }
        self.text(&node.key)?;
        match &node.value {
            Value::String(value) => self.text(value),
            Value::Block(children) => self.nodes(children, depth + 1),
        }
    }

    fn visgroups(&mut self, groups: &[VisGroup], depth: usize) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err("3D sky preview source snapshot exceeds 64 nested levels".into());
        }
        self.list(groups)?;
        for group in groups {
            self.text(&group.name)?;
            self.properties(&group.extra_properties)?;
            self.nodes(&group.extra, 1)?;
            self.visgroups(&group.children, depth + 1)?;
        }
        Ok(())
    }

    fn rows<T>(&mut self, rows: Option<&[Vec<T>]>) -> Result<(), String> {
        if let Some(rows) = rows {
            self.list(rows)?;
            for row in rows {
                self.list(row)?;
            }
        }
        Ok(())
    }

    fn displacement(&mut self, info: &DispInfo) -> Result<(), String> {
        self.rows(info.normals.as_deref())?;
        self.rows(info.distances.as_deref())?;
        self.rows(info.offsets.as_deref())?;
        self.rows(info.offset_normals.as_deref())?;
        self.rows(info.alphas.as_deref())?;
        self.rows(info.triangle_tags.as_deref())?;
        if let Some(vertices) = &info.allowed_verts {
            self.list(vertices)?;
        }
        self.properties(&info.extra_properties)?;
        self.nodes(&info.extra, 1)
    }

    fn solids(&mut self, solids: &[Solid]) -> Result<(), String> {
        self.list(solids)?;
        for solid in solids {
            self.optional_properties(solid.editor.as_ref())?;
            self.properties(&solid.extra_properties)?;
            self.nodes(&solid.extra, 1)?;
            self.list(&solid.sides)?;
            for side in &solid.sides {
                self.text(&side.material)?;
                self.properties(&side.extra_properties)?;
                self.nodes(&side.extra, 1)?;
                if let Some(info) = &side.displacement {
                    self.displacement(info)?;
                }
            }
        }
        Ok(())
    }
}

pub(super) fn admit(vmf: &Vmf) -> Result<usize, String> {
    let mut budget = Budget::default();
    budget.bytes(size_of::<Vmf>())?;
    budget.optional_properties(vmf.version_info.as_ref())?;
    budget.optional_properties(vmf.view_settings.as_ref())?;
    if let Some(groups) = &vmf.visgroups {
        budget.visgroups(groups, 1)?;
    }
    budget.nodes(&vmf.visgroups_extra, 1)?;
    budget.properties(&vmf.world.properties)?;
    budget.solids(&vmf.world.solids)?;
    budget.list(&vmf.world.groups)?;
    for group in &vmf.world.groups {
        budget.optional_properties(group.editor.as_ref())?;
        budget.properties(&group.extra_properties)?;
        budget.nodes(&group.extra, 1)?;
    }
    budget.nodes(&vmf.world.extra, 1)?;
    budget.list(&vmf.entities)?;
    for entity in &vmf.entities {
        budget.properties(&entity.properties)?;
        budget.optional_properties(entity.editor.as_ref())?;
        budget.solids(&entity.solids)?;
        budget.nodes(&entity.extra, 1)?;
        if let Some(connections) = &entity.connections {
            budget.list(connections)?;
            for connection in connections {
                budget.text(&connection.output)?;
                budget.text(&connection.value)?;
            }
        }
    }
    for node in [&vmf.cameras, &vmf.cordon].into_iter().flatten() {
        budget.node(node, 1)?;
    }
    budget.nodes(&vmf.extra, 1)?;
    Ok(budget.bytes)
}

pub(super) fn capture(vmf: &Vmf) -> Result<(Vmf, usize), String> {
    let bytes = admit(vmf)?;
    let mut snapshot = Vmf {
        version_info: vmf.version_info.clone(),
        visgroups: vmf.visgroups.clone(),
        visgroups_extra: vmf.visgroups_extra.clone(),
        view_settings: vmf.view_settings.clone(),
        world: vmf.world.clone(),
        entities: Vec::with_capacity(vmf.entities.len()),
        cameras: vmf.cameras.clone(),
        cordon: vmf.cordon.clone(),
        extra: vmf.extra.clone(),
    };
    for entity in &vmf.entities {
        let mut copy = Entity::new(entity.id, String::new());
        copy.properties = entity.properties.clone();
        copy.connections = entity.connections.clone();
        copy.solids = entity.solids.clone();
        copy.editor = entity.editor.clone();
        copy.hidden = entity.hidden;
        copy.extra = entity.extra.clone();
        snapshot.entities.push(copy);
    }
    Ok((snapshot, bytes))
}
