use std::collections::HashMap;

use glam::DVec3;
use source_formats::bsp::{self, Bsp, GameLump, Lump, contents, lump};
use source_formats::vmf::format_vec3;

use crate::faces::{self, Face};
use crate::map::Map;
use crate::tree::Tree;

const VERTEX_PRECISION: f64 = 10_000.0;
const OPEN_LEAF_FLAGS: i16 = 3;

pub struct Model {
    pub entity: usize,
    pub tree: Tree,
    pub faces: Vec<Face>,
}

fn index<T: TryFrom<usize>>(value: usize, kind: &str) -> Result<T, String> {
    T::try_from(value).map_err(|_| format!("BSP {kind} limit exceeded"))
}

fn short_bounds(bounds: (DVec3, DVec3)) -> Result<([i16; 3], [i16; 3]), String> {
    let mut mins = [0; 3];
    let mut maxs = [0; 3];
    for axis in 0..3 {
        let lo = bounds.0[axis].floor();
        let hi = bounds.1[axis].ceil();
        if lo < i16::MIN as f64 || hi > i16::MAX as f64 {
            return Err("BSP bounds exceed the coordinate limit".into());
        }
        mins[axis] = lo as i16;
        maxs[axis] = hi as i16;
    }
    Ok((mins, maxs))
}

#[derive(Default)]
struct Geometry {
    vertices: Vec<[f32; 3]>,
    edges: Vec<bsp::Edge>,
    surfedges: Vec<i32>,
    normal_indices: Vec<u16>,
    by_vertex: HashMap<[i64; 3], u16>,
    by_edge: HashMap<[u16; 2], i32>,
}

impl Geometry {
    fn vertex(&mut self, point: DVec3) -> Result<u16, String> {
        let key = (point * VERTEX_PRECISION).round().as_i64vec3().to_array();
        if let Some(&id) = self.by_vertex.get(&key) {
            return Ok(id);
        }
        let id = index(self.vertices.len(), "vertex")?;
        self.vertices
            .push(key.map(|v| (v as f64 / VERTEX_PRECISION) as f32));
        self.by_vertex.insert(key, id);
        Ok(id)
    }

    fn edge(&mut self, edge: [u16; 2]) -> Result<i32, String> {
        if let Some(&id) = self.by_edge.get(&edge) {
            return Ok(id);
        }
        if let Some(&id) = self.by_edge.get(&[edge[1], edge[0]]) {
            return Ok(-id);
        }
        let id = index(self.edges.len(), "edge")?;
        self.edges.push(bsp::Edge(edge));
        self.by_edge.insert(edge, id);
        Ok(id)
    }

    fn face(&mut self, face: &Face, map: &Map, original: usize) -> Result<bsp::Face, String> {
        let vertices = face
            .winding
            .0
            .iter()
            .map(|&point| self.vertex(point))
            .collect::<Result<Vec<_>, _>>()?;
        let first_edge = index(self.surfedges.len(), "surfedge")?;
        let plane = index(face.plane, "plane")?;
        for i in 0..vertices.len() {
            let edge = self.edge([vertices[i], vertices[(i + 1) % vertices.len()]])?;
            self.surfedges.push(edge);
            self.normal_indices.push(plane);
        }
        let info = &map.texinfos.infos[face.texinfo];
        let (lightmap_mins, lightmap_size) = faces::lightmap_bounds(&face.winding, info);
        Ok(bsp::Face {
            plane,
            side: (face.plane & 1) as u8,
            on_node: u8::from(face.node.is_some()),
            first_edge,
            edge_count: index(vertices.len(), "face edge")?,
            texinfo: index(face.texinfo, "texinfo")?,
            dispinfo: -1,
            fog_volume: -1,
            styles: [255; 4],
            light_offset: -1,
            area: face.winding.area() as f32,
            lightmap_mins,
            lightmap_size,
            original_face: index(original, "original face")?,
            ..bsp::Face::default()
        })
    }
}

fn entity_text(map: &Map, models: &[Model]) -> Result<String, String> {
    let mut text = String::new();
    for (entity_index, entity) in map.entities.iter().enumerate() {
        let mut properties = entity.properties.clone();
        if entity_index == 0 {
            let (mins, maxs) = map.entities[0]
                .brushes
                .iter()
                .fold((DVec3::MAX, DVec3::MIN), |(lo, hi), &i| {
                    (lo.min(map.brushes[i].mins), hi.max(map.brushes[i].maxs))
                });
            properties.retain(|(key, _)| !matches!(key.as_str(), "world_mins" | "world_maxs"));
            properties.push(("world_mins".into(), format_vec3(mins)));
            properties.push(("world_maxs".into(), format_vec3(maxs)));
            properties.push(("hammerid".into(), entity.id.to_string()));
        } else if let Some(model) = models.iter().position(|m| m.entity == entity_index) {
            properties.retain(|(key, _)| key != "model");
            properties.push(("model".into(), format!("*{model}")));
        }
        text.push_str("{\n");
        for (key, value) in properties {
            if key.contains(['"', '\0']) || value.contains(['"', '\0']) {
                return Err(format!(
                    "entity:{} contains an invalid key/value",
                    entity.id
                ));
            }
            text.push_str(&format!("\"{key}\" \"{value}\"\n"));
        }
        text.push_str("}\n");
    }
    Ok(text)
}

pub fn build(map: &Map, models: &[Model], revision: i32) -> Result<Bsp, String> {
    index::<u16>(map.planes.len().saturating_sub(1), "plane")?;
    index::<i16>(map.texinfos.infos.len().saturating_sub(1), "texinfo")?;
    let mut bsp = Bsp::new();
    bsp.map_revision = revision;
    let mut nodes = Vec::<bsp::Node>::new();
    let mut leaves = vec![bsp::Leaf {
        contents: contents::SOLID,
        cluster: -1,
        water_data: -1,
        ..bsp::Leaf::default()
    }];
    let mut mappings = Vec::new();
    for model in models {
        let mut mapping = vec![0i32; model.tree.nodes.len()];
        for node_index in model.tree.preorder() {
            if model.tree.nodes[node_index].split.is_some() {
                mapping[node_index] = index(nodes.len(), "node")?;
                nodes.push(bsp::Node::default());
            } else {
                let leaf: i32 = index(leaves.len(), "leaf")?;
                mapping[node_index] = -1 - leaf;
                leaves.push(bsp::Leaf::default());
            }
        }
        mappings.push(mapping);
    }
    let mut geometry = Geometry {
        edges: vec![bsp::Edge::default()],
        ..Geometry::default()
    };
    let mut faces = Vec::new();
    let mut bsp_models = Vec::new();
    let mut leaf_faces = vec![Vec::<u16>::new(); leaves.len()];
    let mut leaf_brushes = vec![Vec::<u16>::new(); leaves.len()];
    for (model, mapping) in models.iter().zip(&mappings) {
        let model_start = faces.len();
        let mut groups: HashMap<Option<usize>, Vec<&Face>> = HashMap::new();
        for face in &model.faces {
            groups.entry(face.node).or_default().push(face);
        }
        for node_index in model.tree.preorder() {
            let node = &model.tree.nodes[node_index];
            let (mins, maxs) = short_bounds(node.volume.bounds())?;
            if let Some((plane, children)) = node.split {
                let start = faces.len();
                for face in groups.remove(&Some(node_index)).unwrap_or_default() {
                    let id = index(faces.len(), "leaf face")?;
                    faces.push(geometry.face(face, map, faces.len())?);
                    for &leaf in &face.leaves {
                        let leaf = (-1 - mapping[leaf]) as usize;
                        leaf_faces[leaf].push(id);
                    }
                }
                nodes[mapping[node_index] as usize] = bsp::Node {
                    plane: index(plane, "plane")?,
                    children: children.map(|c| mapping[c]),
                    mins,
                    maxs,
                    first_face: index(start, "node face")?,
                    face_count: index(faces.len() - start, "node face count")?,
                    area: 1,
                    padding: 0,
                };
            } else {
                let open = !node.solid();
                leaves[(-1 - mapping[node_index]) as usize] = bsp::Leaf {
                    contents: node.contents,
                    cluster: if open { 0 } else { -1 },
                    area_flags: if open { 1 | (OPEN_LEAF_FLAGS << 9) } else { 0 },
                    mins,
                    maxs,
                    water_data: -1,
                    ..bsp::Leaf::default()
                };
            }
        }
        for face in groups.remove(&None).unwrap_or_default() {
            let id = index(faces.len(), "leaf face")?;
            faces.push(geometry.face(face, map, faces.len())?);
            for &leaf in &face.leaves {
                leaf_faces[(-1 - mapping[leaf]) as usize].push(id);
            }
        }
        for &brush in &map.entities[model.entity].brushes {
            let id = index(brush, "leaf brush")?;
            for leaf in model.tree.brush_leaves(&map.brushes[brush], &map.planes) {
                leaf_brushes[(-1 - mapping[leaf]) as usize].push(id);
            }
        }
        let (mins, maxs) = map.entities[model.entity]
            .brushes
            .iter()
            .fold((DVec3::MAX, DVec3::MIN), |(lo, hi), &i| {
                (lo.min(map.brushes[i].mins), hi.max(map.brushes[i].maxs))
            });
        bsp_models.push(bsp::Model {
            mins: mins.as_vec3().to_array(),
            maxs: maxs.as_vec3().to_array(),
            origin: [0.0; 3],
            head_node: mapping[0],
            first_face: index(model_start, "model face")?,
            face_count: index(faces.len() - model_start, "model face count")?,
        });
    }
    let mut all_leaf_faces = Vec::new();
    let mut all_leaf_brushes = Vec::new();
    for ((leaf, mut face_list), mut brush_list) in
        leaves.iter_mut().zip(leaf_faces).zip(leaf_brushes)
    {
        face_list.sort_unstable();
        face_list.dedup();
        brush_list.sort_unstable();
        brush_list.dedup();
        leaf.first_leaf_face = index(all_leaf_faces.len(), "leaf face list")?;
        leaf.leaf_face_count = index(face_list.len(), "leaf face count")?;
        leaf.first_leaf_brush = index(all_leaf_brushes.len(), "leaf brush list")?;
        leaf.leaf_brush_count = index(brush_list.len(), "leaf brush count")?;
        all_leaf_faces.extend(face_list);
        all_leaf_brushes.extend(brush_list);
    }
    let mut brushes = Vec::new();
    let mut sides = Vec::new();
    for brush in &map.brushes {
        brushes.push(bsp::Brush {
            first_side: index(sides.len(), "brush side")?,
            side_count: index(brush.sides.len(), "brush side count")?,
            contents: brush.contents,
        });
        for side in &brush.sides {
            sides.push(bsp::BrushSide {
                plane: index(side.plane, "plane")?,
                texinfo: index(side.texinfo, "texinfo")?,
                dispinfo: -1,
                bevel: u8::from(side.bevel),
                thin: 0,
            });
        }
    }
    let planes: Vec<bsp::Plane> = map
        .planes
        .iter()
        .map(|p| bsp::Plane {
            normal: p.normal.as_vec3().to_array(),
            dist: p.dist as f32,
            kind: p.kind,
        })
        .collect();
    let texdata: Vec<bsp::TexData> = map
        .texdata
        .iter()
        .enumerate()
        .map(|(index, &material)| {
            let info = map.materials.get(material);
            bsp::TexData {
                reflectivity: info.reflectivity,
                name_index: index as i32,
                width: info.width,
                height: info.height,
                view_width: info.width,
                view_height: info.height,
            }
        })
        .collect();
    let names: Vec<&str> = map
        .texdata
        .iter()
        .map(|&i| map.materials.get(i).name.as_str())
        .collect();
    bsp.set_records(&planes);
    bsp.set_records(&texdata);
    bsp.set_records(&map.texinfos.infos);
    bsp.set_records(&nodes);
    bsp.set_records(&leaves);
    bsp.set_records(&faces);
    bsp.set_values(lump::ORIGINALFACES, &faces);
    bsp.set_records(&brushes);
    bsp.set_records(&sides);
    bsp.set_records(&bsp_models);
    bsp.set_records(&[bsp::Area::default(); 2]);
    bsp.set_records(&[bsp::AreaPortal::default()]);
    bsp.set_values(lump::VERTEXES, &geometry.vertices);
    bsp.set_records(&geometry.edges);
    bsp.set_values(lump::SURFEDGES, &geometry.surfedges);
    bsp.set_values(lump::LEAFFACES, &all_leaf_faces);
    bsp.set_values(lump::LEAFBRUSHES, &all_leaf_brushes);
    let normals: Vec<[f32; 3]> = planes.iter().map(|p| p.normal).collect();
    bsp.set_values(lump::VERTNORMALS, &normals);
    bsp.set_values(lump::VERTNORMALINDICES, &geometry.normal_indices);
    bsp.set_values(lump::FACE_MACRO_TEXTURE_INFO, &vec![-1i16; faces.len()]);
    bsp.set_values(lump::LEAFMINDISTTOWATER, &vec![u16::MAX; leaves.len()]);
    bsp.set_values(lump::LEAF_AMBIENT_INDEX, &vec![[0u16; 2]; leaves.len()]);
    bsp.set_values(lump::MAP_FLAGS, &[0i32]);
    bsp.set_values(lump::OCCLUSION, &[0i32; 3]);
    bsp.lumps[lump::OCCLUSION].version = 2;
    let mut visibility = Vec::new();
    for value in [1i32, 12, 13] {
        visibility.extend_from_slice(&value.to_le_bytes());
    }
    visibility.extend_from_slice(&[1, 1]);
    bsp.lumps[lump::VISIBILITY] = Lump {
        data: visibility,
        ..Lump::default()
    };
    let mut zip = b"PK\x05\x06".to_vec();
    zip.resize(22, 0);
    bsp.lumps[lump::PAKFILE] = Lump {
        data: zip,
        ..Lump::default()
    };
    for (id, version) in [(*b"sprp", 6), (*b"dprp", 4)] {
        bsp.game_lumps.push(GameLump {
            id,
            flags: 0,
            version,
            data: vec![0; 12],
        });
    }
    bsp.set_texdata_strings(&names);
    bsp.set_entities(&entity_text(map, models)?);
    Ok(bsp)
}
