use map_core::displacement::{smoothing, triangles};
use source_formats::keyvalues::Node;
use source_formats::vmf::DispInfo;

use super::*;

#[derive(Default)]
pub(super) struct Budget {
    source_vertices: usize,
    pub patches: usize,
    pub vertices: usize,
    work: usize,
}

struct Grid {
    vertices: usize,
    work: usize,
}

fn grid(power: u32) -> Result<Grid, String> {
    if !(2..=4).contains(&power) {
        return Err("Instance terrain supports powers 2 through 4".into());
    }
    let cells = 1_usize << power;
    let vertices = (cells + 1) * (cells + 1);
    Ok(Grid {
        vertices,
        work: vertices + cells * cells * 2,
    })
}

impl Budget {
    fn source(&mut self, grid: &Grid) -> Result<(), String> {
        self.source_vertices = self
            .source_vertices
            .checked_add(grid.vertices)
            .filter(|vertices| *vertices <= MAX_TERRAIN_SOURCE_VERTICES)
            .ok_or_else(|| {
                format!(
                    "Instance terrain exceeds {MAX_TERRAIN_SOURCE_VERTICES} cached-source grid vertices"
                )
            })?;
        self.charge_work(grid.work)
    }

    fn charge_work(&mut self, work: usize) -> Result<(), String> {
        self.work = self
            .work
            .checked_add(work)
            .filter(|work| *work <= MAX_TERRAIN_GRID_WORK)
            .ok_or_else(|| format!("Instance terrain exceeds {MAX_TERRAIN_GRID_WORK} grid work"))?;
        Ok(())
    }

    pub fn expanded(
        &mut self,
        solid: &Solid,
        transforms: usize,
        control: &Control,
    ) -> Result<(), String> {
        for side in &solid.sides {
            let Some(info) = &side.displacement else {
                continue;
            };
            control.checkpoint()?;
            let grid = grid(info.power)?;
            self.vertices = self
                .vertices
                .checked_add(grid.vertices)
                .filter(|vertices| *vertices <= MAX_TERRAIN_EXPANDED_VERTICES)
                .ok_or_else(|| {
                    format!(
                        "Instance terrain exceeds {MAX_TERRAIN_EXPANDED_VERTICES} expanded grid vertices"
                    )
                })?;
            self.patches += 1;
            let work = grid
                .work
                .checked_mul(transforms + 1)
                .ok_or("Instance terrain grid-work overflow")?;
            self.charge_work(work)?;
            checked_metadata(info)?;
        }
        Ok(())
    }
}

pub(super) fn source_grid(
    nodes: &[Node],
    budget: &mut Budget,
    control: &Control,
) -> Result<(), String> {
    let power = nodes
        .iter()
        .find(|node| node.key == "power")
        .and_then(Node::as_str)
        .and_then(|value| value.parse().ok())
        .ok_or("Instance terrain requires a supported power")?;
    let grid = grid(power)?;
    budget.source(&grid)?;
    let size = (1_usize << power) + 1;
    let mut fields = BTreeSet::new();
    for node in nodes {
        control.checkpoint()?;
        if !fields.insert(node.key.as_str()) {
            return Err(format!("Instance terrain repeats field {}", node.key));
        }
        match node.key.as_str() {
            "normals" | "offsets" | "offset_normals" => raw_rows(node, size, size * 3)?,
            "distances" | "alphas" => raw_rows(node, size, size)?,
            "triangle_tags" => raw_rows(node, size - 1, (size - 1) * 2)?,
            "allowed_verts" => {
                let words = source_formats::bsp::DisplacementInfo::default()
                    .allowed_vertices
                    .len();
                let entries = node
                    .children()
                    .ok_or("Instance terrain allowed_verts requires a block")?;
                let [entry] = entries else {
                    return Err("Instance terrain allowed_verts requires one native mask".into());
                };
                if entry.key.parse::<usize>() != Ok(words)
                    || entry
                        .as_str()
                        .is_none_or(|value| integer_count(value, words) != words)
                {
                    return Err("Instance terrain allowed_verts requires one native mask".into());
                }
            }
            "startposition" => raw_position(node)?,
            "power" | "flags" | "elevation" | "subdiv" if node.as_str().is_some() => {}
            _ if node.key.eq_ignore_ascii_case("smooth") && node.as_str().is_some() => {}
            _ => return Err(format!("Unsupported instance terrain field {}", node.key)),
        }
    }
    Ok(())
}

pub(super) fn hidden_source_grid(
    nodes: &[Node],
    budget: &mut Budget,
    control: &Control,
) -> Result<(), String> {
    let mut grid = Grid {
        vertices: 0,
        work: 0,
    };
    for node in nodes {
        control.checkpoint()?;
        if node.key == "startposition" && node.as_str().is_some() {
            raw_position(node)?;
            continue;
        }
        let stride = match node.key.as_str() {
            "normals" | "offsets" | "offset_normals" => 3,
            "distances" | "alphas" | "triangle_tags" | "allowed_verts" => 1,
            _ => continue,
        };
        let Some(rows) = node.children() else {
            continue;
        };
        let mut entries = 0_usize;
        for row in rows {
            control.checkpoint()?;
            let Some(value) = row.as_str() else { continue };
            let remaining = MAX_TERRAIN_GRID_WORK.saturating_sub(grid.work);
            let words = if matches!(node.key.as_str(), "triangle_tags" | "allowed_verts") {
                integer_count(value, remaining)
            } else {
                numeric_count(value, remaining)
            };
            grid.work = grid.work.saturating_add(words);
            if grid.work > MAX_TERRAIN_GRID_WORK {
                return Err(format!(
                    "Instance terrain exceeds {MAX_TERRAIN_GRID_WORK} grid work"
                ));
            }
            entries = entries.saturating_add(words.div_ceil(stride));
            if entries > MAX_TERRAIN_SOURCE_VERTICES {
                return Err(format!(
                    "Instance terrain exceeds {MAX_TERRAIN_SOURCE_VERTICES} cached-source grid vertices"
                ));
            }
        }
        grid.vertices = grid.vertices.max(entries);
    }
    budget.source(&grid)
}

fn numeric_count(value: &str, maximum: usize) -> usize {
    source_formats::vmf::numeric_tokens(value)
        .take(maximum.saturating_add(1))
        .count()
}

fn integer_count(value: &str, maximum: usize) -> usize {
    value
        .split_ascii_whitespace()
        .take(maximum.saturating_add(1))
        .count()
}

fn raw_position(node: &Node) -> Result<(), String> {
    if node
        .as_str()
        .is_none_or(|value| numeric_count(value, 3) != 3)
    {
        return Err("Instance terrain startposition requires three numeric coordinates".into());
    }
    Ok(())
}

fn raw_rows(node: &Node, rows: usize, words: usize) -> Result<(), String> {
    let valid = node.children().is_some_and(|entries| {
        entries.len() == rows
            && entries.iter().enumerate().all(|(index, entry)| {
                entry.key == format!("row{index}")
                    && entry.as_str().is_some_and(|value| {
                        if node.key == "triangle_tags" {
                            integer_count(value, words) == words
                        } else {
                            numeric_count(value, words) == words
                        }
                    })
            })
    });
    if !valid {
        return Err(format!(
            "Instance terrain {} rows do not match its grid",
            node.key
        ));
    }
    Ok(())
}

fn checked_metadata(info: &DispInfo) -> Result<(), String> {
    grid(info.power)?;
    let size = (1_usize << info.power) + 1;
    if info.offset_normals.as_ref().is_some_and(|rows| {
        rows.len() != size
            || rows.iter().any(|row| row.len() != size)
            || rows.iter().flatten().any(|normal| !normal.is_finite())
    }) {
        return Err("Instance terrain offset normals need finite matching grid rows".into());
    }
    if !info.extra.is_empty()
        || info
            .extra_properties
            .iter()
            .any(|(key, _)| !key.eq_ignore_ascii_case("smooth"))
        || info.allowed_verts.as_ref().is_some_and(|words| {
            words.len()
                != source_formats::bsp::DisplacementInfo::default()
                    .allowed_vertices
                    .len()
        })
    {
        return Err("Instance terrain has unsupported opaque metadata or native mask".into());
    }
    Ok(())
}

pub(super) fn check_solid(
    solid: &Solid,
    geometry: &BrushGeometry,
    control: &Control,
) -> Result<(), String> {
    for (index, side) in solid.sides.iter().enumerate() {
        let Some(info) = &side.displacement else {
            continue;
        };
        control.checkpoint()?;
        checked_metadata(info)?;
        let face = geometry
            .faces
            .iter()
            .find(|face| face.side == index)
            .ok_or("Instance terrain must have an effective quad cage face")?;
        let (_, mesh) = crate::map::terrain::checked_mesh(face, info)?;
        smoothing::angle(info)?;
        triangles::native_tags(info, &mesh).map_err(|error| error.to_string())?;
    }
    Ok(())
}
