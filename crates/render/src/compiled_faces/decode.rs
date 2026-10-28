use super::*;

struct Plan {
    ordinal: usize,
    material: usize,
    info: TexInfo,
    dimensions: [u32; 2],
    styles: Vec<u8>,
    layout: SampleLayout,
    vertices: usize,
    indices: usize,
    wires: usize,
}

impl Channel {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn decode(
        bsp: &Bsp,
        geometry: &FaceGeometry,
        texinfo: &[TexInfo],
        materials: &mut [MaterialReference],
        transforms: &[DMat4],
        samples: &[RgbExp32],
        previous_bytes: usize,
        check: &mut dyn FnMut() -> io::Result<()>,
    ) -> Result<Self, String> {
        let lighting =
            source_formats::bsp::Lighting::parse(bytemuck::cast_slice(samples)).map_err(message)?;
        let plan_bytes = bytes(geometry.faces().len(), size_of::<Plan>())?;
        if sum_bytes(&[
            previous_bytes,
            plan_bytes,
            bytes(samples.len(), size_of::<RgbExp32>())?,
        ])? > MAX_CPU_BYTES
        {
            return Err(
                "Baked surface decoding plans exceed the 256 MiB CPU working budget".into(),
            );
        }
        let mut plans = Vec::with_capacity(geometry.faces().len());
        let mut vertex_count = 0usize;
        let mut index_count = 0usize;
        let mut wire_count = 0usize;
        for (ordinal, face) in geometry.faces().iter().enumerate() {
            check().map_err(message)?;
            let info = usize::try_from(face.texinfo)
                .ok()
                .and_then(|index| texinfo.get(index))
                .ok_or("Baked surface TexInfo reference is missing")?;
            if info.flags & (surf::NODRAW | surf::SKY | surf::SKY2D | surf::HINT | surf::SKIP) != 0
            {
                continue;
            }
            if info
                .texture_vecs
                .iter()
                .flatten()
                .chain(info.lightmap_vecs.iter().flatten())
                .any(|value| !value.is_finite())
            {
                return Err(format!(
                    "Baked face {ordinal} has nonfinite texture or lightmap projection"
                ));
            }
            let material = usize::try_from(info.texdata)
                .ok()
                .filter(|&index| index < materials.len())
                .ok_or("Baked surface TexData reference is missing")?;
            let layout = lighting.face(face, info).map_err(message)?;
            let styles = layout.map_or_else(Vec::new, |layout| layout.styles().to_vec());
            if styles.iter().any(|&style| style > 63)
                || styles.iter().copied().collect::<BTreeSet<_>>().len() != styles.len()
            {
                return Err(format!(
                    "Baked face {ordinal} requires distinct stored styles 0–63"
                ));
            }
            let dimensions = layout.map_or([0; 2], |layout| layout.dimensions());
            let maps = layout.map_or(0, |layout| layout.maps());
            let count = bytes(dimensions[0], dimensions[1])?;
            let (vertices, indices, wires) =
                if let Some(terrain) = geometry.terrain(ordinal).map_err(message)? {
                    (
                        terrain.positions.len(),
                        bytes(terrain.triangles.len(), 3)?,
                        bytes(
                            wire_edges(terrain.positions.len(), Some(&terrain.triangles)).len(),
                            2,
                        )?,
                    )
                } else {
                    let vertices = usize::try_from(face.edge_count)
                        .map_err(|_| "Negative compiled face edge count")?;
                    (
                        vertices,
                        bytes(
                            vertices
                                .checked_sub(2)
                                .ok_or("Compiled face requires at least three edges")?,
                            3,
                        )?,
                        bytes(vertices, 2)?,
                    )
                };
            vertex_count = vertex_count
                .checked_add(vertices)
                .ok_or("Baked surface vertex count overflow")?;
            index_count = index_count
                .checked_add(indices)
                .ok_or("Baked surface index count overflow")?;
            wire_count = wire_count
                .checked_add(wires)
                .ok_or("Baked surface wire count overflow")?;
            if vertex_count > u32::MAX as usize || index_count > u32::MAX as usize {
                return Err("Baked surface geometry exceeds 32-bit mesh ranges".into());
            }
            let proposed_bytes = sum_bytes(&[
                previous_bytes,
                plan_bytes,
                bytes(vertex_count, size_of::<Vertex>())?,
                bytes(index_count, size_of::<u32>())?,
                bytes(wire_count, size_of::<crate::scene::LineVertex>())?,
                bytes(samples.len(), size_of::<RgbExp32>())?,
                bytes(
                    plans.len() + 1,
                    size_of::<SurfaceInfo>() + size_of::<SampleLayout>() + size_of::<Part>() + 8,
                )?,
            ])?;
            if proposed_bytes > MAX_CPU_BYTES {
                return Err("Baked surface geometry exceeds the 256 MiB CPU working budget".into());
            }
            if materials[material].used && materials[material].lightmapped != (maps != 0) {
                return Err(
                    "A compiled material is shared by contradictory lit and unlit FACE records"
                        .into(),
                );
            }
            materials[material].used = true;
            materials[material].directional |= maps == 4;
            materials[material].lightmapped |= maps != 0;
            plans.push(Plan {
                ordinal,
                material,
                info: *info,
                dimensions: dimensions.map(|value| value as u32),
                styles,
                layout: SampleLayout {
                    offset: if maps == 0 {
                        0
                    } else {
                        face.light_offset as usize / size_of::<RgbExp32>()
                    },
                    count,
                    maps,
                },
                vertices,
                indices,
                wires,
            });
        }
        if plans.is_empty() {
            return Err("The selected FACE channel has no visible baked surfaces".into());
        }
        let mut channel = Self {
            surfaces: Vec::with_capacity(plans.len()),
            layouts: Vec::with_capacity(plans.len()),
            vertices: Vec::with_capacity(vertex_count),
            indices: Vec::with_capacity(index_count),
            wires: Vec::with_capacity(wire_count),
            parts: Vec::with_capacity(plans.len()),
            samples: samples.to_vec(),
        };
        for plan in plans {
            check().map_err(message)?;
            let face = &geometry.faces()[plan.ordinal];
            let winding = geometry.face(plan.ordinal).map_err(message)?;
            let terrain = geometry.terrain(plan.ordinal).map_err(message)?;
            let missing_surface_samples = terrain
                .as_ref()
                .filter(|_| plan.layout.maps != 0)
                .map_or(Ok(0), |terrain| {
                    validate_terrain_samples(
                        bsp,
                        face.dispinfo as usize,
                        terrain,
                        plan.dimensions,
                        check,
                    )
                })?;
            let (points, normals) = terrain
                .as_ref()
                .map_or((&winding.positions, &winding.normals), |terrain| {
                    (&terrain.positions, &terrain.normals)
                });
            let terrain_texture = terrain
                .as_ref()
                .map(|terrain| -> Result<_, String> {
                    let quad = winding
                        .positions
                        .as_slice()
                        .try_into()
                        .map_err(|_| "Baked displacement base face must be a quad")?;
                    let corners = terrain.info.start_oriented_quad(quad).map_err(message)?;
                    Ok((
                        terrain.grid_uvs.as_slice(),
                        corners.map(|point| {
                            glam::DVec2::from_array(project(
                                plan.info.texture_vecs,
                                DVec3::from_array(point.map(f64::from)),
                            ))
                        }),
                    ))
                })
                .transpose()?;
            if points.len() != plan.vertices || normals.len() != points.len() {
                return Err("Baked surface vertex or normal ranges changed during decoding".into());
            }
            let transform = transforms[winding.model];
            let first_vertex = channel.vertices.len() as u32;
            let first_index = channel.indices.len() as u32;
            let dense = channel.surfaces.len();
            let receiver = (dense as u32)
                .checked_shl(RECEIVER_SHIFT)
                .ok_or("Baked surface receiver index overflow")?
                | BAKED_FACE_RECEIVER;
            let mut mins = Vec3::MAX;
            let mut maxs = Vec3::MIN;
            for (index, (&point, &normal)) in points.iter().zip(normals).enumerate() {
                let local_point = DVec3::from_array(point.map(f64::from));
                let normal = DVec3::from_array(normal.map(f64::from));
                let position = transform.transform_point3(local_point);
                if !position.is_finite()
                    || position.abs().max_element() > map_core::checks::SOURCE_COORDINATE_LIMIT
                {
                    return Err(
                        "Baked surface world positions exceed finite Source compile coordinates"
                            .into(),
                    );
                }
                let projected = if let Some((grid, corners)) = &terrain_texture {
                    let [s, t] = grid[index].map(f64::from);
                    corners[0]
                        .lerp(corners[1], t)
                        .lerp(corners[3].lerp(corners[2], t), s)
                        .to_array()
                } else {
                    project(plan.info.texture_vecs, local_point)
                };
                let uv: [f64; 2] = std::array::from_fn(|axis| {
                    projected[axis] / f64::from(materials[plan.material].dimensions[axis])
                });
                let luxel: [f64; 2] = if plan.layout.maps == 0 {
                    [0.0; 2]
                } else if let Some(terrain) = &terrain {
                    let grid = terrain.grid_uvs[index];
                    std::array::from_fn(|axis| {
                        f64::from(grid[axis]) * f64::from(plan.dimensions[axis] - 1)
                    })
                } else {
                    let projected = project(plan.info.lightmap_vecs, local_point);
                    std::array::from_fn(|axis| {
                        projected[axis] - f64::from(face.lightmap_mins[axis])
                    })
                };
                if uv
                    .iter()
                    .chain(&luxel)
                    .any(|value| !value.is_finite() || value.abs() > 1_000_000.0)
                {
                    return Err(
                        "Baked surface texture or luxel coordinates exceed finite ±1000000".into(),
                    );
                }
                let tangent = final_tangent(
                    &plan.info,
                    DVec3::from_array(winding.normal.map(f64::from)),
                    normal,
                    transform,
                    plan.layout.maps == 4,
                )?;
                let normal = transform
                    .transform_vector3(normal)
                    .try_normalize()
                    .ok_or("Baked surface transformed normal is degenerate")?;
                let position = position.as_vec3();
                mins = mins.min(position);
                maxs = maxs.max(position);
                let alpha = terrain
                    .as_ref()
                    .map_or(0.0, |terrain| terrain.alphas[index]);
                channel.vertices.push(Vertex {
                    position: position.to_array(),
                    normal: normal.as_vec3().to_array(),
                    uv: [uv[0] as f32, uv[1] as f32],
                    color: [255, 255, 255, alpha.round() as u8],
                    lightmap: [luxel[0] as f32, luxel[1] as f32, 0.0],
                    smoothing: face.smoothing_groups,
                    opacity: alpha / 255.0,
                    texture_alpha: 0,
                    tangent,
                    lighting_receiver: receiver,
                    baked_irradiance: [0.0; 3],
                });
            }
            if let Some(terrain) = &terrain {
                for triangle in &terrain.triangles {
                    channel
                        .indices
                        .extend(triangle.map(|index| first_vertex + index as u32));
                }
            } else {
                for index in 1..plan.vertices as u32 - 1 {
                    channel.indices.extend([
                        first_vertex,
                        first_vertex + index,
                        first_vertex + index + 1,
                    ]);
                }
            }
            if channel.indices.len() - first_index as usize != plan.indices {
                return Err("Baked surface triangle count changed during decoding".into());
            }
            let density = if plan.layout.maps == 0 {
                -1.0
            } else {
                density_scale(&channel.vertices, &channel.indices[first_index as usize..])
            };
            for vertex in &mut channel.vertices[first_vertex as usize..] {
                vertex.lightmap[2] = density;
            }
            let first_wire = channel.wires.len();
            for edge in wire_edges(
                plan.vertices,
                terrain.as_ref().map(|terrain| terrain.triangles.as_slice()),
            ) {
                channel
                    .wires
                    .extend(edge.map(|index| crate::scene::LineVertex {
                        position: channel.vertices[first_vertex as usize + index].position,
                        color: [151, 169, 191, 255],
                    }));
            }
            if channel.wires.len() - first_wire != plan.wires {
                return Err("Baked surface wire topology changed during decoding".into());
            }
            channel.surfaces.push(SurfaceInfo {
                source: if plan.layout.maps == 0 {
                    LightingSource::UnlitMaterial
                } else {
                    LightingSource::BakedLightmap
                },
                ordinal: plan.ordinal,
                model: winding.model,
                material: plan.material,
                terrain: terrain.is_some(),
                vertices: plan.vertices,
                triangles: plan.indices / 3,
                dimensions: plan.dimensions,
                maps: plan.layout.maps,
                styles: plan.styles,
                missing_surface_samples,
                mins: mins.to_array(),
                maxs: maxs.to_array(),
            });
            channel.layouts.push(plan.layout);
            channel.parts.push(Part {
                surface: dense,
                material: plan.material,
                first_index,
                index_count: plan.indices as u32,
                wires: first_wire..channel.wires.len(),
            });
        }
        Ok(channel)
    }

    pub(super) fn byte_len(&self) -> Result<usize, String> {
        sum_bytes(&[
            bytes(self.surfaces.len(), size_of::<SurfaceInfo>())?,
            self.surfaces
                .iter()
                .map(|surface| surface.styles.len())
                .sum(),
            bytes(self.layouts.len(), size_of::<SampleLayout>())?,
            bytes(self.vertices.len(), size_of::<Vertex>())?,
            bytes(self.indices.len(), size_of::<u32>())?,
            bytes(self.wires.len(), size_of::<crate::scene::LineVertex>())?,
            bytes(self.parts.len(), size_of::<Part>())?,
            bytes(self.samples.len(), size_of::<RgbExp32>())?,
        ])
    }

    pub(super) fn compose(
        &self,
        settings: &LightingSettings,
        retained_bytes: usize,
    ) -> Result<LightingData, String> {
        let sample_count = self.sample_count()?;
        let gpu_bytes = self.lighting_bytes()?;
        let cpu_bytes = self.lighting_cpu_bytes()?;
        if sum_bytes(&[retained_bytes, cpu_bytes])? > MAX_CPU_BYTES || gpu_bytes > MAX_GPU_BYTES {
            return Err(
                "Baked surface style composition exceeds the 256 MiB working budget".into(),
            );
        }
        let mut samples = Vec::with_capacity(sample_count);
        let mut faces = Vec::with_capacity(self.layouts.len());
        for (layout, surface) in self.layouts.iter().zip(&self.surfaces) {
            let mut map_offsets = [0; 4];
            for (map, offset) in map_offsets.iter_mut().enumerate().take(layout.maps) {
                *offset = u32::try_from(samples.len())
                    .map_err(|_| "Baked surface sample offset exceeds 32 bits")?;
                for index in 0..layout.count {
                    let mut color = [0.0_f64; 3];
                    for (slot, &style) in surface.styles.iter().enumerate() {
                        let sample = self.samples
                            [layout.offset + (slot * layout.maps + map) * layout.count + index];
                        let gain = settings.gain(style);
                        for (total, value) in color.iter_mut().zip(sample.linear()) {
                            *total += value * (gain / 255.0);
                        }
                    }
                    if color
                        .iter()
                        .any(|&value| !value.is_finite() || value > f64::from(f32::MAX))
                    {
                        return Err(
                            "Baked surface style gains exceed finite floating-point radiance"
                                .into(),
                        );
                    }
                    samples.push([color[0] as f32, color[1] as f32, color[2] as f32, 0.0]);
                }
            }
            faces.push(FaceLighting {
                width: surface.dimensions[0],
                height: surface.dimensions[1],
                map_offsets,
                map_count: layout.maps as u32,
            });
        }
        let effective_style_gains = self
            .surfaces
            .iter()
            .flat_map(|surface| surface.styles.iter().copied())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|style| StyleGain {
                style,
                gain: settings.gain(style),
            })
            .collect();
        let data = LightingData {
            samples,
            faces,
            exposure_ev: settings.exposure_ev as f32,
            channel: settings.channel,
            effective_style_gains,
        };
        data.validate()?;
        Ok(data)
    }

    fn sample_count(&self) -> Result<usize, String> {
        self.layouts
            .iter()
            .try_fold(0usize, |total, layout| {
                layout
                    .count
                    .checked_mul(layout.maps)
                    .and_then(|count| total.checked_add(count))
            })
            .ok_or_else(|| "Baked surface composited sample count overflow".into())
    }

    pub(super) fn lighting_bytes(&self) -> Result<usize, String> {
        sum_bytes(&[
            bytes(self.sample_count()?, size_of::<[f32; 4]>())?,
            bytes(self.layouts.len(), GPU_FACE_BYTES)?,
            GPU_PARAMS_BYTES,
        ])
    }

    pub(super) fn lighting_cpu_bytes(&self) -> Result<usize, String> {
        let styles = self
            .surfaces
            .iter()
            .flat_map(|surface| surface.styles.iter().copied())
            .collect::<BTreeSet<_>>()
            .len();
        sum_bytes(&[
            bytes(self.sample_count()?, size_of::<[f32; 4]>())?,
            bytes(
                self.layouts.len(),
                size_of::<FaceLighting>() + GPU_FACE_BYTES,
            )?,
            bytes(styles, size_of::<StyleGain>())?,
            size_of::<LightingData>(),
        ])
    }
}

fn project(rows: [[f32; 4]; 2], point: DVec3) -> [f64; 2] {
    rows.map(|row| {
        point.dot(DVec3::new(
            f64::from(row[0]),
            f64::from(row[1]),
            f64::from(row[2]),
        )) + f64::from(row[3])
    })
}

fn wire_edges(vertices: usize, triangles: Option<&[[usize; 3]]>) -> Vec<[usize; 2]> {
    if let Some(triangles) = triangles {
        let mut edges = BTreeSet::new();
        for &[a, b, c] in triangles {
            for (a, b) in [(a, b), (b, c), (c, a)] {
                edges.insert([a.min(b), a.max(b)]);
            }
        }
        edges.into_iter().collect()
    } else {
        (0..vertices)
            .map(|index| [index, (index + 1) % vertices])
            .collect()
    }
}

fn density_scale(vertices: &[Vertex], indices: &[u32]) -> f32 {
    let mut area = 0.0;
    let mut luxels = 0.0;
    for indices in indices.as_chunks::<3>().0 {
        let points = indices
            .map(|index| DVec3::from_array(vertices[index as usize].position.map(f64::from)));
        let coordinates = indices.map(|index| {
            let q = vertices[index as usize].lightmap;
            glam::DVec2::new(f64::from(q[0]), f64::from(q[1]))
        });
        area += (points[1] - points[0])
            .cross(points[2] - points[0])
            .length();
        luxels += (coordinates[1] - coordinates[0])
            .perp_dot(coordinates[2] - coordinates[0])
            .abs();
    }
    let scale = (area / luxels).sqrt() as f32;
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        -1.0
    }
}

fn final_tangent(
    info: &TexInfo,
    plane: DVec3,
    normal: DVec3,
    transform: DMat4,
    required: bool,
) -> Result<[f32; 4], String> {
    if !required {
        return Ok([0.0; 4]);
    }
    let [s, t] = info
        .texture_vecs
        .map(|row| DVec3::new(f64::from(row[0]), f64::from(row[1]), f64::from(row[2])));
    let orientation = plane.dot(s.cross(t));
    if !orientation.is_finite() || orientation == 0.0 {
        return Err(
            "Baked directional lighting requires a nonsingular final texture projection".into(),
        );
    }
    let (tangent, _, _) = map_core::surface::texture_tangents(normal, s, orientation < 0.0)?;
    let tangent = transform
        .transform_vector3(tangent)
        .try_normalize()
        .ok_or("Baked surface tangent is degenerate after model placement")?;
    Ok([
        tangent.x as f32,
        tangent.y as f32,
        tangent.z as f32,
        if orientation < 0.0 { -1.0 } else { 1.0 },
    ])
}

fn validate_terrain_samples(
    bsp: &Bsp,
    displacement: usize,
    terrain: &source_formats::bsp::FaceTerrain,
    dimensions: [u32; 2],
    check: &mut dyn FnMut() -> io::Result<()>,
) -> Result<usize, String> {
    let records = bsp
        .displacement_lightmap_samples(displacement)
        .map_err(message)?
        .ok_or("Baked terrain requires its retained displacement lightmap sample stream")?;
    if dimensions.iter().any(|&value| value < 2)
        || records.len() != bytes(dimensions[0] as usize, dimensions[1] as usize)?
    {
        return Err("Baked terrain lightmap sample dimensions are inconsistent".into());
    }
    let sampler =
        source_formats::bsp::DisplacementSurfaceSampler::new(terrain.info, &terrain.positions)
            .map_err(message)?;
    let triangles = terrain.info.lightmap_triangle_indices().map_err(message)?;
    let mut missing = 0;
    for (index, record) in records.into_iter().enumerate() {
        if index % 256 == 0 {
            check().map_err(message)?;
        }
        let expected = sampler
            .sample([
                (index % dimensions[0] as usize) as f64 / f64::from(dimensions[0] - 1),
                (index / dimensions[0] as usize) as f64 / f64::from(dimensions[1] - 1),
            ])
            .map_err(message)?;
        let Some(actual) = record
            .surface(&triangles, &terrain.positions)
            .map_err(message)?
        else {
            missing += 1;
            continue;
        };
        let triangle = triangles[usize::from(record.triangle)]
            .map(|index| DVec3::from_array(terrain.positions[index].map(f64::from)));
        let longest = triangle[0]
            .distance(triangle[1])
            .max(triangle[1].distance(triangle[2]))
            .max(triangle[2].distance(triangle[0]));
        let tolerance = longest * (3.0 / 255.0) + 1e-4;
        if DVec3::from_array(actual.position).distance(DVec3::from_array(expected.surface.position))
            > tolerance
        {
            return Err("Baked terrain luxel stream does not correspond to the start-oriented displacement grid".into());
        }
    }
    Ok(missing)
}
