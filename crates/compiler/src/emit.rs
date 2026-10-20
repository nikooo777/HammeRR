use std::collections::HashMap;
use std::sync::Arc;

use glam::DVec3;
use source_formats::bsp::{self, Bsp, GameLump, Lump, contents, lump};
use source_formats::vmf::format_vec3;

use crate::faces::Face;
use crate::map::Map;
use crate::tree::Tree;

const VERTEX_PRECISION: f64 = 10_000.0;
const MAX_FACE_SOURCE_BYTES: usize = 64 * 1024 * 1024;

fn vertex_key(point: DVec3) -> [i64; 3] {
    (point * VERTEX_PRECISION).round().as_i64vec3().to_array()
}

fn vertex_position(key: [i64; 3]) -> [f32; 3] {
    key.map(|v| (v as f64 / VERTEX_PRECISION) as f32)
}

pub(crate) fn stored_vertex(point: DVec3) -> DVec3 {
    DVec3::from_array(vertex_position(vertex_key(point)).map(f64::from))
}

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
    fn occlusion(&mut self, map: &Map, control: &crate::Control) -> Result<bsp::Occlusion, String> {
        let mut data = bsp::Occlusion::default();
        for definition in &map.occluders {
            control.checkpoint()?;
            let first_polygon = index(data.polygons.len(), "occluder polygon")?;
            for polygon in &definition.polygons {
                control.checkpoint()?;
                if data.polygons.len() >= bsp::MAX_OCCLUSION_POLYGONS
                    || data.vertex_indices.len() + polygon.winding.0.len()
                        > bsp::MAX_OCCLUSION_VERTEX_INDICES
                {
                    return Err("Occlusion geometry exceeds HammeRR's polygon or vertex-index allocation cap".into());
                }
                data.polygons.push(bsp::OccluderPolygon {
                    first_vertex_index: index(data.vertex_indices.len(), "occluder vertex index")?,
                    vertex_count: index(polygon.winding.0.len(), "occluder polygon vertex")?,
                    plane: index(polygon.plane, "occluder plane")?,
                });
                for &point in &polygon.winding.0 {
                    data.vertex_indices.push(i32::from(self.vertex(point)?));
                }
            }
            data.occluders.push(bsp::Occluder {
                flags: definition.flags,
                first_polygon,
                polygon_count: index(definition.polygons.len(), "occluder polygon count")?,
                mins: definition.mins.as_vec3().to_array(),
                maxs: definition.maxs.as_vec3().to_array(),
                area: definition.area,
            });
        }
        Ok(data)
    }

    fn vertex(&mut self, point: DVec3) -> Result<u16, String> {
        let key = vertex_key(point);
        if let Some(&id) = self.by_vertex.get(&key) {
            return Ok(id);
        }
        let id = index(self.vertices.len(), "vertex")?;
        self.vertices.push(vertex_position(key));
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

    fn face(
        &mut self,
        face: &Face,
        map: &Map,
        original: usize,
        model: &Model,
        water: &crate::water::Water,
    ) -> Result<bsp::Face, String> {
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
        let (lightmap_mins, lightmap_size) = face.lightmap_bounds(info)?;
        Ok(bsp::Face {
            plane,
            side: (face.plane & 1) as u8,
            on_node: u8::from(face.node.is_some()),
            first_edge,
            edge_count: index(vertices.len(), "face edge")?,
            texinfo: index(face.texinfo, "texinfo")?,
            dispinfo: face
                .displacement
                .map_or(Ok(-1), |value| index(value, "displacement"))?,
            fog_volume: if model.entity == 0 {
                water.face_volume(face, map, &model.tree)
            } else {
                -1
            },
            styles: [255; 4],
            light_offset: -1,
            area: face.winding.area() as f32,
            lightmap_mins,
            lightmap_size,
            original_face: index(original, "original face")?,
            smoothing_groups: face.smoothing_groups,
            ..bsp::Face::default()
        })
    }
}

fn entity_text(map: &Map, models: &[Model]) -> Result<String, String> {
    let mut text = String::new();
    for (entity_index, entity) in map.entities.iter().enumerate() {
        let mut properties = entity.properties.clone();
        if entity_index == 0 {
            let (mins, maxs) = map.model_bounds(0);
            properties.retain(|(key, _)| !matches!(key.as_str(), "world_mins" | "world_maxs"));
            properties.push(("world_mins".into(), format_vec3(mins)));
            properties.push(("world_maxs".into(), format_vec3(maxs)));
            properties.push(("hammerid".into(), entity.id.to_string()));
        } else if let Some(model) = models.iter().position(|m| m.entity == entity_index) {
            properties.retain(|(key, _)| key != "model");
            properties.push(("model".into(), format!("*{model}")));
        }
        append_entity(&mut text, &properties, entity.id)?;
    }
    for definition in &map.overlays.definitions {
        if let Some(properties) = &definition.accessor {
            append_entity(&mut text, properties, definition.entity)?;
        }
    }
    Ok(text)
}

fn append_entity(
    text: &mut String,
    properties: &[(String, String)],
    id: u32,
) -> Result<(), String> {
    text.push_str("{\n");
    for (key, value) in properties {
        if key.contains(['"', '\0']) || value.contains(['"', '\0']) {
            return Err(format!("entity:{} contains an invalid key/value", id));
        }
        text.push_str(&format!("\"{key}\" \"{value}\"\n"));
    }
    text.push_str("}\n");
    Ok(())
}

pub struct Emission {
    pub face_origins: Result<crate::face_origins::FaceOrigins, String>,
    pub model_entities: Vec<usize>,
    pub world_leaf_nodes: Vec<Option<usize>>,
    pub bsp: Bsp,
    pub smoothing_sources: Vec<Option<Arc<crate::faces::SmoothingSource>>>,
    pub face_sources: Vec<Arc<[u32]>>,
}

#[allow(clippy::too_many_arguments)]
pub fn build(
    map: &Map,
    models: &[Model],
    visibility: &crate::visibility::Visibility,
    water: &crate::water::Water,
    areas: &crate::areas::Areas,
    revision: i32,
    control: &crate::Control,
    mut face_origins: Result<crate::face_origins::FaceOrigins, String>,
) -> Result<Emission, String> {
    let mut source_faces = 0usize;
    let mut source_bytes = 0usize;
    for face in models.iter().flat_map(|model| &model.faces) {
        control.checkpoint()?;
        source_faces = source_faces
            .checked_add(1)
            .filter(|&count| count <= bsp::MAX_LIGHTING_FACES)
            .ok_or("Radiance source face count exceeds the native face limit")?;
        source_bytes = source_bytes
            .checked_add(size_of::<Arc<[u32]>>())
            .and_then(|bytes| {
                face.source_sides
                    .len()
                    .checked_mul(size_of::<u32>())
                    .and_then(|ids| bytes.checked_add(ids))
            })
            .filter(|&bytes| bytes <= MAX_FACE_SOURCE_BYTES)
            .ok_or("Radiance face contributors exceed their 64 MiB retained metadata limit")?;
    }
    control.checkpoint()?;
    let mut face_sources = Vec::with_capacity(source_faces);
    face_origins = face_origins.and_then(|mut origins| {
        let count = models
            .iter()
            .try_fold(0usize, |sum, model| sum.checked_add(model.faces.len()))
            .ok_or("Face metadata count overflow")?;
        let references = models
            .iter()
            .flat_map(|model| &model.faces)
            .try_fold(0usize, |sum, face| sum.checked_add(face.source_sides.len()))
            .ok_or("Contributor metadata count overflow")?;
        origins.reserve_faces(count, references)?;
        Ok(origins)
    });
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
        control.checkpoint()?;
        let mut mapping = vec![0i32; model.tree.nodes.len()];
        for node_index in model.tree.preorder() {
            control.checkpoint()?;
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
    let mut world_leaf_nodes = vec![None; leaves.len()];
    for node in models[0].tree.preorder() {
        control.checkpoint()?;
        if models[0].tree.nodes[node].split.is_none() {
            world_leaf_nodes[(-1 - mappings[0][node]) as usize] = Some(node);
        }
    }
    let mut geometry = Geometry {
        edges: vec![bsp::Edge::default()],
        ..Geometry::default()
    };
    let mut faces = Vec::new();
    let mut smoothing_sources = Vec::new();
    let mut overlay_bindings = crate::overlays::Bindings::default();
    let mut bsp_models = Vec::new();
    let mut water_distances = vec![u16::MAX; leaves.len()];
    let mut leaf_faces = vec![Vec::<u16>::new(); leaves.len()];
    let mut leaf_brushes = vec![Vec::<u16>::new(); leaves.len()];
    for (model_index, (model, mapping)) in models.iter().zip(&mappings).enumerate() {
        let model_start = faces.len();
        let mut groups: HashMap<Option<usize>, Vec<&Face>> = HashMap::new();
        for face in &model.faces {
            groups.entry(face.node).or_default().push(face);
        }
        for node_index in model.tree.preorder() {
            control.checkpoint()?;
            let node = &model.tree.nodes[node_index];
            let (mins, maxs) = short_bounds(node.volume.bounds())?;
            if let Some((plane, children)) = node.split {
                let start = faces.len();
                for face in groups.remove(&Some(node_index)).unwrap_or_default() {
                    let id = index(faces.len(), "leaf face")?;
                    overlay_bindings.add(&face.source_sides, id, &map.overlays)?;
                    let emitted = geometry.face(face, map, faces.len(), model, water)?;
                    face_origins = face_origins.and_then(|mut origins| {
                        origins.push(&emitted, model_index, &face.source_sides)?;
                        Ok(origins)
                    });
                    faces.push(emitted);
                    smoothing_sources.push(face.smoothing_source.clone());
                    face_sources.push(face.source_sides.clone());
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
                    area: if model.entity == 0 {
                        areas.indices[node_index]
                    } else {
                        1
                    },
                    padding: 0,
                };
            } else {
                let open = !node.solid();
                let leaf = (-1 - mapping[node_index]) as usize;
                let water_data = if model.entity == 0 {
                    water.leaves[node_index]
                } else {
                    -1
                };
                if model.entity == 0 && open {
                    water_distances[leaf] = if water_data >= 0 {
                        0
                    } else {
                        water.distance(node.volume.bounds())
                    };
                }
                leaves[leaf] = bsp::Leaf {
                    contents: node.contents,
                    cluster: if model.entity == 0 {
                        visibility.clusters[node_index]
                            .map(|cluster| cluster as i16)
                            .unwrap_or(-1)
                    } else {
                        -1
                    },
                    area_flags: if open {
                        let flags = if model.entity == 0 {
                            visibility.clusters[node_index]
                                .map(|cluster| visibility.sky[cluster])
                                .unwrap_or_default()
                        } else {
                            0
                        };
                        (if model.entity == 0 {
                            areas.indices[node_index]
                        } else {
                            1
                        }) | (flags << 9)
                    } else {
                        0
                    },
                    mins,
                    maxs,
                    water_data,
                    ..bsp::Leaf::default()
                };
            }
        }
        for face in groups.remove(&None).unwrap_or_default() {
            let id = index(faces.len(), "leaf face")?;
            overlay_bindings.add(&face.source_sides, id, &map.overlays)?;
            let emitted = geometry.face(face, map, faces.len(), model, water)?;
            face_origins = face_origins.and_then(|mut origins| {
                origins.push(&emitted, model_index, &face.source_sides)?;
                Ok(origins)
            });
            faces.push(emitted);
            smoothing_sources.push(face.smoothing_source.clone());
            face_sources.push(face.source_sides.clone());
            for &leaf in &face.leaves {
                leaf_faces[(-1 - mapping[leaf]) as usize].push(id);
            }
        }
        for &brush in &map.entities[model.entity].brushes {
            control.checkpoint()?;
            let id = index(brush, "leaf brush")?;
            for leaf in model.tree.brush_leaves(&map.brushes[brush], &map.planes) {
                leaf_brushes[(-1 - mapping[leaf]) as usize].push(id);
            }
        }
        let (mins, maxs) = map.model_bounds(model.entity);
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
        control.checkpoint()?;
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
            let metrics = info.texture_metrics()?;
            Ok(bsp::TexData {
                reflectivity: metrics.reflectivity,
                name_index: index as i32,
                width: metrics.width,
                height: metrics.height,
                view_width: metrics.width,
                view_height: metrics.height,
            })
        })
        .collect::<Result<_, String>>()?;
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
    bsp.set_overlay_data(&map.overlays.data(&overlay_bindings, control)?)
        .map_err(|error| error.to_string())?;
    bsp.set_water_overlay_data(&map.overlays.water_data(&overlay_bindings, control)?)
        .map_err(|error| error.to_string())?;
    bsp.set_values(lump::ORIGINALFACES, &faces);
    bsp.set_records(&brushes);
    bsp.set_records(&sides);
    bsp.set_records(&bsp_models);
    bsp.set_records(&map.cubemaps);
    bsp.set_records(&water.records);
    bsp.set_area_data(&areas.data)
        .map_err(|error| error.to_string())?;
    let occlusion = geometry.occlusion(map, control)?;
    bsp.set_values(lump::VERTEXES, &geometry.vertices);
    bsp.set_occlusion(&occlusion)
        .map_err(|error| error.to_string())?;
    bsp.set_records(&geometry.edges);
    bsp.set_values(lump::SURFEDGES, &geometry.surfedges);
    bsp.set_values(lump::LEAFFACES, &all_leaf_faces);
    bsp.set_values(lump::LEAFBRUSHES, &all_leaf_brushes);
    let normals: Vec<[f32; 3]> = planes.iter().map(|p| p.normal).collect();
    bsp.set_values(lump::VERTNORMALS, &normals);
    bsp.set_values(lump::VERTNORMALINDICES, &geometry.normal_indices);
    bsp.set_values(lump::FACE_MACRO_TEXTURE_INFO, &vec![-1i16; faces.len()]);
    bsp.set_values(lump::LEAFMINDISTTOWATER, &water_distances);
    bsp.set_values(lump::LEAF_AMBIENT_INDEX, &vec![[0u16; 2]; leaves.len()]);
    bsp.set_values(lump::MAP_FLAGS, &[0i32]);
    bsp.lumps[lump::VISIBILITY] = Lump {
        data: visibility.data.clone(),
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
    if !map.terrains.is_empty() {
        let mut data = bsp::Displacements {
            infos: Vec::new(),
            vertices: Vec::new(),
            triangles: Vec::new(),
        };
        for (terrain_index, terrain) in map.terrains.iter().enumerate() {
            control.checkpoint()?;
            let face = faces
                .iter()
                .position(|face| face.dispinfo == terrain_index as i16)
                .ok_or("Displacement face was not emitted")?;
            let info = bsp::DisplacementInfo {
                face: index(face, "displacement face")?,
                vertex_start: index(data.vertices.len(), "displacement vertex")?,
                triangle_start: index(data.triangles.len(), "displacement triangle")?,
                ..terrain.info
            };
            data.infos.push(info);
            data.vertices.extend_from_slice(&terrain.vertices);
            data.triangles.extend_from_slice(&terrain.triangles);
        }
        bsp.set_displacements(&data).map_err(|e| e.to_string())?;
        for (index, terrain) in map.terrains.iter().enumerate() {
            let quad = terrain
                .base
                .map(|point| stored_vertex(point).as_vec3().to_array());
            let positions = data.positions(index, quad).map_err(|e| e.to_string())?;
            if terrain.indices.iter().any(|&[a, b, c]| {
                let [a, b, c] =
                    [a, b, c].map(|index| DVec3::from_array(positions[index].map(f64::from)));
                (b - a).cross(c - a).length_squared() < 1e-12
            }) {
                return Err(format!(
                    "Displacement {} has collapsed native float triangles; enlarge its base or reduce its power",
                    terrain.side
                ));
            }
            if positions
                .iter()
                .zip(&terrain.mesh.positions)
                .any(|(point, &source)| {
                    DVec3::from_array(point.map(f64::from)).distance(source) > 0.02
                })
            {
                return Err(format!(
                    "Displacement {} native float grid does not preserve its authored vertices",
                    terrain.side
                ));
            }
        }
    }
    face_origins = face_origins.and_then(|mut origins| {
        origins.seal_geometry(&bsp)?;
        Ok(origins)
    });
    if face_sources.len() != faces.len() || face_sources.len() != source_faces {
        return Err("Emitted radiance face contributors do not match native face order".into());
    }
    Ok(Emission {
        face_origins,
        model_entities: models.iter().map(|model| model.entity).collect(),
        world_leaf_nodes,
        bsp,
        smoothing_sources,
        face_sources,
    })
}
