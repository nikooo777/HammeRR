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

#[cfg(test)]
mod tests {
    use super::*;

    fn solid() -> Solid {
        let mut document = Document::new(crate::test_common::room());
        let solid = &document.vmf().world.solids[0];
        let geometry = BrushGeometry::from_solid(solid).unwrap();
        let face = geometry
            .faces
            .iter()
            .find(|face| face.plane.normal.z > 0.99)
            .unwrap();
        let target = map_core::surface::FaceId {
            solid: solid.id,
            side: solid.sides[face.side].id,
        };
        document
            .edit("bounded instance terrain", |tx| {
                map_core::displacement::edit::create(tx, &[target], 2, 0)
            })
            .unwrap();
        document.into_vmf().world.solids.remove(0)
    }

    fn nodes() -> Vec<Node> {
        vec![
            Node::string("power", "2"),
            Node::string("startposition", "[0 0 0]"),
        ]
    }

    #[test]
    fn raw_source_caps_and_shapes_reject_before_typed_grid_allocation() {
        let mut budget = Budget {
            source_vertices: MAX_TERRAIN_SOURCE_VERTICES - 25,
            ..Budget::default()
        };
        source_grid(&nodes(), &mut budget, &Control::default()).unwrap();
        let error = source_grid(&nodes(), &mut budget, &Control::default()).unwrap_err();
        assert!(error.contains("cached-source grid vertices"), "{error}");
        let mut malformed = nodes();
        malformed.push(Node::block("normals", vec![Node::string("row0", "0 0")]));
        let error =
            source_grid(&malformed, &mut Budget::default(), &Control::default()).unwrap_err();
        assert!(error.contains("normals rows do not match"), "{error}");
        let mut duplicate = nodes();
        duplicate.push(Node::string("power", "4"));
        let error =
            source_grid(&duplicate, &mut Budget::default(), &Control::default()).unwrap_err();
        assert!(error.contains("repeats field power"), "{error}");
    }

    #[test]
    fn cumulative_work_counts_source_and_each_pose_without_charging_plain_brushes() {
        let grid = grid(2).unwrap();
        let mut budget = Budget {
            work: MAX_TERRAIN_GRID_WORK - grid.work,
            ..Budget::default()
        };
        source_grid(&nodes(), &mut budget, &Control::default()).unwrap();
        let error = source_grid(&nodes(), &mut budget, &Control::default()).unwrap_err();
        assert!(error.contains("grid work"), "{error}");
        let mut budget = Budget::default();
        let source = crate::test_common::room();
        budget
            .expanded(
                &source.world.solids[0],
                MAX_EXPANSION_DEPTH,
                &Control::default(),
            )
            .unwrap();
        assert_eq!((budget.patches, budget.vertices, budget.work), (0, 0, 0));
        let control = Control::default();
        control.cancel().unwrap();
        assert!(source_grid(&nodes(), &mut budget, &control).is_err());
    }

    #[test]
    fn bracket_packed_rows_and_fixed_vectors_are_bounded_before_typed_allocation() {
        let packed = vec!["0(0)"; 15].join(" ");
        assert_eq!(packed.split_ascii_whitespace().count(), 15);
        let mut malformed = nodes();
        malformed.push(Node::block(
            "normals",
            (0..5)
                .map(|row| Node::string(format!("row{row}"), &packed))
                .collect(),
        ));
        let error =
            source_grid(&malformed, &mut Budget::default(), &Control::default()).unwrap_err();
        assert!(error.contains("normals rows do not match"), "{error}");
        let mut valid = nodes();
        valid.push(Node::block(
            "normals",
            (0..5)
                .map(|row| Node::string(format!("row{row}"), "[0][0][1]".repeat(5)))
                .collect(),
        ));
        source_grid(&valid, &mut Budget::default(), &Control::default()).unwrap();
        let mut position = nodes();
        position[1] = Node::string("startposition", "[0][0] 0 0");
        assert_eq!(
            position[1]
                .as_str()
                .unwrap()
                .split_ascii_whitespace()
                .count(),
            3
        );
        for hidden in [false, true] {
            let result = if hidden {
                hidden_source_grid(&position, &mut Budget::default(), &Control::default())
            } else {
                source_grid(&position, &mut Budget::default(), &Control::default())
            };
            assert!(
                result
                    .unwrap_err()
                    .contains("startposition requires three numeric coordinates")
            );
        }
    }

    #[test]
    fn hidden_grids_ignore_semantics_but_charge_actual_bracket_packed_allocations() {
        let mut hidden = nodes();
        hidden[0] = Node::string("power", u32::MAX.to_string());
        hidden.push(Node::string("unsupported_hidden", "ignored"));
        hidden.push(Node::block(
            "offsets",
            vec![Node::string("row0", "[1][2][3](4)(5)(6)7 8 9")],
        ));
        let mut budget = Budget::default();
        hidden_source_grid(&hidden, &mut budget, &Control::default()).unwrap();
        assert_eq!((budget.source_vertices, budget.work), (3, 9));
        assert!(source_grid(&hidden, &mut Budget::default(), &Control::default()).is_err());
        let mut vertices = Budget {
            source_vertices: MAX_TERRAIN_SOURCE_VERTICES - 3,
            ..Budget::default()
        };
        hidden_source_grid(&hidden, &mut vertices, &Control::default()).unwrap();
        assert_eq!(vertices.source_vertices, MAX_TERRAIN_SOURCE_VERTICES);
        assert!(
            hidden_source_grid(&hidden, &mut vertices, &Control::default())
                .unwrap_err()
                .contains("cached-source grid vertices")
        );
        let mut work = Budget {
            work: MAX_TERRAIN_GRID_WORK - 9,
            ..Budget::default()
        };
        hidden_source_grid(&hidden, &mut work, &Control::default()).unwrap();
        assert_eq!(work.work, MAX_TERRAIN_GRID_WORK);
        assert!(
            hidden_source_grid(&hidden, &mut work, &Control::default())
                .unwrap_err()
                .contains("grid work")
        );
    }

    #[test]
    fn hidden_dense_rows_still_fail_before_typed_allocation_and_observe_cancellation() {
        let mut hidden = nodes();
        hidden.push(Node::block(
            "alphas",
            vec![Node::string(
                "row0",
                "(0)".repeat(MAX_TERRAIN_SOURCE_VERTICES + 1),
            )],
        ));
        let error =
            hidden_source_grid(&hidden, &mut Budget::default(), &Control::default()).unwrap_err();
        assert!(error.contains("cached-source grid vertices"), "{error}");
        let control = Control::default();
        control.cancel().unwrap();
        assert!(hidden_source_grid(&nodes(), &mut Budget::default(), &control).is_err());
    }

    #[test]
    fn repeated_grid_occurrences_spend_expanded_vertices_and_each_intermediate_pose() {
        let solid = solid();
        let mut vertices = Budget {
            vertices: MAX_TERRAIN_EXPANDED_VERTICES - 25,
            ..Budget::default()
        };
        vertices
            .expanded(&solid, MAX_EXPANSION_DEPTH, &Control::default())
            .unwrap();
        assert_eq!(vertices.vertices, MAX_TERRAIN_EXPANDED_VERTICES);
        let error = vertices
            .expanded(&solid, MAX_EXPANSION_DEPTH, &Control::default())
            .unwrap_err();
        assert!(error.contains("expanded grid vertices"), "{error}");
        let work = grid(2).unwrap().work * (MAX_EXPANSION_DEPTH + 1);
        let mut poses = Budget {
            work: MAX_TERRAIN_GRID_WORK - work,
            ..Budget::default()
        };
        poses
            .expanded(&solid, MAX_EXPANSION_DEPTH, &Control::default())
            .unwrap();
        let error = poses
            .expanded(&solid, MAX_EXPANSION_DEPTH, &Control::default())
            .unwrap_err();
        assert!(error.contains("grid work"), "{error}");
        let control = Control::default();
        control.cancel().unwrap();
        let mut cancelled = Budget::default();
        assert!(cancelled.expanded(&solid, 1, &control).is_err());
        assert_eq!(cancelled.vertices, 0);
        let ordinary = serde_json::to_value(InstanceStats::default()).unwrap();
        assert!(ordinary.get("terrain_patches").is_none());
        assert!(ordinary.get("terrain_vertices").is_none());
    }
}
