use sha2::{Digest, Sha256};
use source_formats::{
    bsp::{self, Bsp, LightingKind},
    vmf::{Solid, Vmf},
};

pub const MAX_FACES: usize = 65_536;
pub const MAX_REFERENCES: usize = 262_144;
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_GEOMETRY_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Origin {
    Root {
        entity: u32,
        solid: u32,
        side: u32,
    },
    Instance {
        instance: u32,
        file: String,
        solid: u32,
        side: u32,
        #[serde(serialize_with = "crate::instances::serialize_occurrence")]
        occurrence: std::sync::Arc<crate::instances::OccurrencePath>,
    },
    GeneratedRegion {
        solid: u32,
        side: u32,
    },
    Unresolved {
        side: u32,
    },
}

#[derive(Debug)]
struct Source {
    expanded_side: u32,
    origin: Origin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Geometry {
    plane: u16,
    side: u8,
    on_node: u8,
    first_edge: i32,
    edge_count: i16,
    displacement: i16,
    original_face: i32,
    area: u32,
    smoothing_groups: u32,
    primitive_count: u16,
    first_primitive: u16,
}

impl From<&bsp::Face> for Geometry {
    fn from(face: &bsp::Face) -> Self {
        Self {
            plane: face.plane,
            side: face.side,
            on_node: face.on_node,
            first_edge: face.first_edge,
            edge_count: face.edge_count,
            displacement: face.dispinfo,
            original_face: face.original_face,
            area: face.area.to_bits(),
            smoothing_groups: face.smoothing_groups,
            primitive_count: face.primitive_count,
            first_primitive: face.first_primitive,
        }
    }
}

#[derive(Debug)]
struct Record {
    geometry: Geometry,
    model: usize,
    first: usize,
    count: usize,
}

#[derive(Debug)]
pub struct FaceOrigins {
    sources: Vec<Source>,
    faces: Vec<Record>,
    references: Vec<u32>,
    bytes: usize,
    geometry_digest: Option<[u8; 32]>,
}

fn budget(bytes: usize, additional: usize) -> Result<usize, String> {
    bytes
        .checked_add(additional)
        .filter(|&value| value <= MAX_BYTES)
        .ok_or_else(|| {
            format!("Authored face mapping exceeds its {MAX_BYTES}-byte metadata budget")
        })
}

impl FaceOrigins {
    pub(crate) fn prepare(
        vmf: &Vmf,
        instances: &crate::instances::Prepared<'_>,
    ) -> Result<Self, String> {
        let mut count = 0usize;
        let mut strings = 0usize;
        for solid in vmf
            .world
            .solids
            .iter()
            .chain(vmf.entities.iter().flat_map(|entity| &entity.solids))
        {
            count = count
                .checked_add(solid.sides.len())
                .filter(|&count| count <= MAX_REFERENCES)
                .ok_or("Authored face mapping exceeds its source reference budget")?;
            for side in &solid.sides {
                if let Some((_, file, _, _)) = instances.face_origin(side.id) {
                    strings = budget(strings, file.len())?;
                }
            }
        }
        let bytes = budget(
            size_of::<Self>() + instances.occurrence_bytes(),
            count
                .checked_mul(size_of::<Source>())
                .ok_or("Source metadata size overflow")?,
        )?;
        let bytes = budget(bytes, strings)?;
        let mut sources = Vec::new();
        sources
            .try_reserve_exact(count)
            .map_err(|_| "Authored face sources cannot be allocated")?;
        let mut result = Self {
            sources,
            faces: Vec::new(),
            references: Vec::new(),
            bytes,
            geometry_digest: None,
        };
        for (entity, solid) in vmf
            .world
            .solids
            .iter()
            .map(|solid| (vmf.world.id, solid))
            .chain(
                vmf.entities
                    .iter()
                    .flat_map(|entity| entity.solids.iter().map(move |solid| (entity.id, solid))),
            )
        {
            for side in &solid.sides {
                let origin = if let Some((instance, file, original_solid, original_side)) =
                    instances.face_origin(side.id)
                {
                    let occurrence = instances
                        .face_occurrence(side.id)
                        .ok_or("Expanded face has no captured occurrence path")?;
                    let mut path = String::new();
                    path.try_reserve_exact(file.len())
                        .map_err(|_| "Authored face source filename cannot be allocated")?;
                    path.push_str(file);
                    Origin::Instance {
                        instance,
                        file: path,
                        solid: original_solid,
                        side: original_side,
                        occurrence: std::sync::Arc::clone(occurrence),
                    }
                } else {
                    Origin::Root {
                        entity,
                        solid: solid.id,
                        side: side.id,
                    }
                };
                result.sources.push(Source {
                    expanded_side: side.id,
                    origin,
                });
            }
        }
        result.sort_sources()?;
        Ok(result)
    }

    fn sort_sources(&mut self) -> Result<(), String> {
        self.sources
            .sort_unstable_by_key(|source| source.expanded_side);
        if self
            .sources
            .windows(2)
            .any(|pair| pair[0].expanded_side == pair[1].expanded_side)
        {
            return Err("Authored face mapping has duplicate source-side IDs".into());
        }
        Ok(())
    }

    pub(crate) fn add_region(
        &mut self,
        vmf: &Vmf,
        region: Option<&crate::RegionSummary>,
    ) -> Result<(), String> {
        let Some(region) = region else {
            return Ok(());
        };
        let generated = |solid: &&Solid| region.generated_brushes.contains(&solid.id);
        let count = vmf
            .world
            .solids
            .iter()
            .filter(generated)
            .map(|solid| solid.sides.len())
            .sum::<usize>();
        if self
            .sources
            .len()
            .checked_add(count)
            .is_none_or(|count| count > MAX_REFERENCES)
        {
            return Err("Generated region exceeds the authored mapping source budget".into());
        }
        self.bytes = budget(
            self.bytes,
            count
                .checked_mul(size_of::<Source>())
                .ok_or("Generated source metadata size overflow")?,
        )?;
        self.sources.reserve_exact(count);
        for solid in vmf.world.solids.iter().filter(generated) {
            for side in &solid.sides {
                self.sources.push(Source {
                    expanded_side: side.id,
                    origin: Origin::GeneratedRegion {
                        solid: solid.id,
                        side: side.id,
                    },
                });
            }
        }
        self.sort_sources()
    }

    pub(crate) fn reserve_faces(&mut self, faces: usize, references: usize) -> Result<(), String> {
        if faces > MAX_FACES || references > MAX_REFERENCES {
            return Err(
                "Authored face mapping exceeds its face or contributor reference budget".into(),
            );
        }
        let face_bytes = faces
            .checked_mul(size_of::<Record>())
            .ok_or("Face metadata size overflow")?;
        let reference_bytes = references
            .checked_mul(size_of::<u32>())
            .ok_or("Contributor metadata size overflow")?;
        self.bytes = budget(budget(self.bytes, face_bytes)?, reference_bytes)?;
        self.faces.reserve_exact(faces);
        self.references.reserve_exact(references);
        Ok(())
    }

    pub(crate) fn push(
        &mut self,
        face: &bsp::Face,
        model: usize,
        sides: &[u32],
    ) -> Result<(), String> {
        if self.faces.len() == MAX_FACES
            || self
                .references
                .len()
                .checked_add(sides.len())
                .is_none_or(|count| count > MAX_REFERENCES)
        {
            return Err(
                "Authored face mapping exceeds its face or contributor reference budget".into(),
            );
        }
        if self.faces.len() == self.faces.capacity()
            || self.references.len() + sides.len() > self.references.capacity()
        {
            return Err("Authored face mapping exceeded its reserved metadata".into());
        }
        let first = self.references.len();
        for &side in sides {
            let index = match self
                .sources
                .binary_search_by_key(&side, |source| source.expanded_side)
            {
                Ok(index) => index,
                Err(_) => {
                    return Err(format!(
                        "Authored face mapping cannot resolve retained side {side}"
                    ));
                }
            };
            self.references.push(index as u32);
        }
        self.faces.push(Record {
            geometry: face.into(),
            model,
            first,
            count: sides.len(),
        });
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.faces.len()
    }
    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn contributors(&self, face: usize) -> Option<impl ExactSizeIterator<Item = &Origin>> {
        let record = self.faces.get(face)?;
        Some(
            self.references[record.first..record.first + record.count]
                .iter()
                .map(|&index| &self.sources[index as usize].origin),
        )
    }

    pub fn model(&self, face: usize) -> Option<usize> {
        self.faces.get(face).map(|record| record.model)
    }

    pub(crate) fn seal_geometry(&mut self, bsp: &Bsp) -> Result<(), String> {
        self.geometry_digest = Some(geometry_digest(bsp)?);
        Ok(())
    }

    pub fn validate_channel(&self, bsp: &Bsp, kind: LightingKind) -> Result<(), String> {
        if self.geometry_digest != Some(geometry_digest(bsp)?) {
            return Err("Compiled geometry bytes differ from the retained emission mapping".into());
        }
        let faces = bsp
            .lighting_faces(kind)
            .map_err(|error| error.to_string())?;
        if faces.len() != self.faces.len()
            || faces
                .iter()
                .zip(&self.faces)
                .any(|(face, record)| Geometry::from(face) != record.geometry)
        {
            return Err(
                "Compiled face geometry differs from the exact retained emission mapping".into(),
            );
        }
        Ok(())
    }

    pub(crate) fn finish(self, bsp: &Bsp) -> Result<Self, String> {
        self.validate_channel(bsp, LightingKind::Ldr)?;
        if !bsp
            .lighting_faces(LightingKind::Hdr)
            .map_err(|error| error.to_string())?
            .is_empty()
        {
            self.validate_channel(bsp, LightingKind::Hdr)?;
        }
        Ok(self)
    }
}

fn geometry_digest(bsp: &Bsp) -> Result<[u8; 32], String> {
    use bsp::lump;
    let indices = [
        lump::PLANES,
        lump::VERTEXES,
        lump::EDGES,
        lump::SURFEDGES,
        lump::MODELS,
        lump::ORIGINALFACES,
        lump::DISP_VERTS,
        lump::DISP_TRIS,
        lump::DISPINFO,
    ];
    let bytes = indices.iter().try_fold(0usize, |sum, &index| {
        sum.checked_add(bsp.lumps[index].data.len())
    });
    if bytes.is_none_or(|bytes| bytes > MAX_GEOMETRY_BYTES) {
        return Err("Emitted geometry exceeds the 128 MiB mapping verification budget".into());
    }
    let mut hash = Sha256::new();
    for index in indices {
        let data = &bsp.lumps[index].data;
        hash.update((index as u32).to_le_bytes());
        hash.update((data.len() as u64).to_le_bytes());
        if index == lump::DISPINFO {
            let (records, remainder) = data.as_chunks::<{ size_of::<bsp::DisplacementInfo>() }>();
            if !remainder.is_empty() {
                return Err("Invalid emitted displacement geometry records".into());
            }
            for record in records {
                hash.update(
                    &record[..std::mem::offset_of!(bsp::DisplacementInfo, lightmap_alpha_start)],
                );
                hash.update(
                    &record[std::mem::offset_of!(bsp::DisplacementInfo, lightmap_sample_start)
                        + size_of::<i32>()..],
                );
            }
        } else {
            hash.update(data);
        }
    }
    Ok(hash.finalize().into())
}

#[cfg(test)]
mod tests;
