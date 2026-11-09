use std::io;

use crc32fast::Hasher;

use super::{
    NativeTerrainCarrier, NativeTerrainDisplacement, NativeTerrainSample, NativeTerrainSnapshot,
    NativeTerrainSource, NativeTerrainVector,
};
use crate::bsp::{Bsp, GameLump, LightingKind, Lump, NativeDisplacementRounding, invalid};

mod arithmetic;
mod neighbors;
use arithmetic::{Arithmetic, MxcsrGuard, NormalizeOrder, read_mxcsr};

pub const MAX_NATIVE_BASIS_RETAINED_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_NATIVE_BASIS_SCRATCH_BYTES: usize = 384 * 1024 * 1024;
pub const MAX_NATIVE_BASIS_WORK: usize = 128 * 1024 * 1024;
const MAX_INPUT_BYTES: usize = 96 * 1024 * 1024;
const IMPLEMENTATION: &str = "css_linux64_build25338318_ordered_basis_v1";
const ENGINE_SHA256: &str = "965b90157cbec58187c553ce8bb65907e186c990b08453cb4abd75e909ec794a";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeTerrainBasisCpu {
    pub vendor: String,
    pub brand: String,
    pub signature: u32,
    pub rsqrt_probe_bits: [u32; 8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeTerrainBasisProfile {
    pub position_rounding: NativeDisplacementRounding,
    pub rounding: NativeDisplacementRounding,
    pub flush_to_zero: bool,
    pub denormals_are_zero: bool,
    pub cpu: NativeTerrainBasisCpu,
}
impl NativeTerrainBasisProfile {
    pub fn current_cpu(
        position_rounding: NativeDisplacementRounding,
        rounding: NativeDisplacementRounding,
        flush_to_zero: bool,
        denormals_are_zero: bool,
    ) -> io::Result<Self> {
        let _environment = MxcsrGuard::selected(rounding, flush_to_zero, denormals_are_zero)?;
        Ok(Self {
            position_rounding,
            rounding,
            flush_to_zero,
            denormals_are_zero,
            cpu: current_cpu()?,
        })
    }
    pub fn validate(&self) -> io::Result<()> {
        let actual = Self::current_cpu(
            self.position_rounding,
            self.rounding,
            self.flush_to_zero,
            self.denormals_are_zero,
        )?;
        if actual.cpu != self.cpu {
            return Err(invalid(
                "Native basis CPU identity or RSQRTSS probe differs from the selected profile",
            ));
        }
        Ok(())
    }
}

#[cfg(target_arch = "x86_64")]
fn current_cpu() -> io::Result<NativeTerrainBasisCpu> {
    use std::arch::x86_64::__cpuid;
    let root = __cpuid(0);
    let vendor_bytes = [
        root.ebx.to_le_bytes(),
        root.edx.to_le_bytes(),
        root.ecx.to_le_bytes(),
    ]
    .concat();
    let vendor = String::from_utf8(vendor_bytes).map_err(io::Error::other)?;
    let one = __cpuid(1);
    if one.edx & (1 << 25) == 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Native basis CPU does not advertise SSE",
        ));
    }
    let extended = __cpuid(0x8000_0000).eax;
    let mut brand = Vec::new();
    if extended >= 0x8000_0004 {
        for leaf in 0x8000_0002..=0x8000_0004 {
            let row = __cpuid(leaf);
            for word in [row.eax, row.ebx, row.ecx, row.edx] {
                brand.extend_from_slice(&word.to_le_bytes());
            }
        }
    }
    let brand = String::from_utf8(brand)
        .map_err(io::Error::other)?
        .trim_matches(['\0', ' '])
        .to_owned();
    let inputs = [
        0x2edbe6ff, 0x3dcccccd, 0x3f800000, 0x40000000, 0x40400000, 0x42c80000, 0x00800000,
        0x7f7fffff,
    ];
    let rsqrt_probe_bits = inputs.map(|bits| Arithmetic.rsqrt(f32::from_bits(bits)).to_bits());
    Ok(NativeTerrainBasisCpu {
        vendor,
        brand,
        signature: one.eax,
        rsqrt_probe_bits,
    })
}
#[cfg(not(target_arch = "x86_64"))]
fn current_cpu() -> io::Result<NativeTerrainBasisCpu> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Native basis prediction requires x86-64 SSE",
    ))
}

#[derive(Debug, Clone)]
pub struct NativeTerrainBasisReceipt {
    pub implementation: &'static str,
    pub engine_sha256: &'static str,
    pub local_digest: u32,
    pub t_junction_digest: u32,
    pub corner_digest: u32,
    pub final_digest: u32,
    pub displacement_count: usize,
    pub sample_count: usize,
    pub work_units: usize,
    pub scratch_reserved_bytes: usize,
    pub retained_bytes: usize,
    pub caller_mxcsr: u32,
    pub selected_mxcsr: u32,
    pub t_junction_events: usize,
    pub corner_events: usize,
    pub edge_pairs: usize,
    pub fine_samples: usize,
}

pub struct NativeTerrainBasisPrediction {
    snapshot: NativeTerrainSnapshot,
    profile: NativeTerrainBasisProfile,
    receipt: NativeTerrainBasisReceipt,
}
impl NativeTerrainBasisPrediction {
    pub const fn snapshot(&self) -> &NativeTerrainSnapshot {
        &self.snapshot
    }
    pub const fn profile(&self) -> &NativeTerrainBasisProfile {
        &self.profile
    }
    pub const fn receipt(&self) -> &NativeTerrainBasisReceipt {
        &self.receipt
    }
    pub fn retained_bytes(&self) -> usize {
        size_of::<Self>() + self.snapshot.retained_bytes() - size_of::<NativeTerrainSnapshot>()
            + self.profile.cpu.vendor.capacity()
            + self.profile.cpu.brand.capacity()
    }
}

pub(super) struct Patch {
    pub source: NativeTerrainSource,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub tangent_s: Vec<[f32; 3]>,
    pub tangent_t: Vec<[f32; 3]>,
}

pub(super) struct Work<'a> {
    check: &'a mut dyn FnMut() -> io::Result<()>,
    units: usize,
    caller_mxcsr: u32,
}
impl Work<'_> {
    pub fn charge(&mut self, units: usize) -> io::Result<()> {
        let previous = self.units;
        self.units = self
            .units
            .checked_add(units)
            .filter(|units| *units <= MAX_NATIVE_BASIS_WORK)
            .ok_or_else(|| invalid("Native basis prediction exceeds its bounded work budget"))?;
        if previous / 1024 != self.units / 1024 {
            self.checkpoint()?;
        }
        Ok(())
    }
    pub fn checkpoint(&mut self) -> io::Result<()> {
        let _restore = MxcsrGuard::raw(self.caller_mxcsr)?;
        (self.check)()
    }
}

fn allocated_bsp_bytes(bsp: &Bsp) -> io::Result<usize> {
    let mut bytes = size_of::<Bsp>()
        + bsp.lumps.capacity() * size_of::<Lump>()
        + bsp.game_lumps.capacity() * size_of::<GameLump>();
    for length in bsp
        .lumps
        .iter()
        .map(|lump| lump.data.capacity())
        .chain(bsp.game_lumps.iter().map(|lump| lump.data.capacity()))
    {
        bytes = bytes
            .checked_add(length)
            .ok_or_else(|| invalid("Native basis BSP storage overflows"))?;
    }
    if bytes > MAX_INPUT_BYTES {
        return Err(invalid(
            "Native basis input exceeds its 96 MiB decoded BSP budget",
        ));
    }
    Ok(bytes)
}

pub fn predict_native_terrain_basis_controlled(
    bsp: &Bsp,
    channel: LightingKind,
    profile: NativeTerrainBasisProfile,
    check: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<NativeTerrainBasisPrediction> {
    check()?;
    profile.validate()?;
    let input_bytes = allocated_bsp_bytes(bsp)?;
    let carrier = NativeTerrainCarrier::new_controlled(bsp, channel, check)?;
    if carrier.displacements.is_empty() {
        return Err(invalid(
            "Native basis prediction requires a nonempty complete displacement catalog",
        ));
    }
    let sample_count = carrier
        .displacements
        .iter()
        .map(|source| source.vertex_count())
        .sum::<usize>();
    let scratch_reserved_bytes = input_bytes
        .checked_mul(3)
        .and_then(|bytes| {
            bytes.checked_add(
                sample_count * (8 * size_of::<[f32; 3]>() + size_of::<NativeTerrainSample>())
                    + carrier.displacements.capacity() * size_of::<NativeTerrainSource>()
                    + 4 * 1024 * 1024,
            )
        })
        .ok_or_else(|| invalid("Native basis working storage overflows"))?;
    if scratch_reserved_bytes > MAX_NATIVE_BASIS_SCRATCH_BYTES {
        return Err(invalid(
            "Native basis prediction exceeds its 384 MiB working budget",
        ));
    }
    let mode = NativeDisplacementRounding::ALL
        .iter()
        .position(|rounding| *rounding == profile.position_rounding)
        .ok_or_else(|| invalid("Unsupported native basis position rounding"))?;
    let environment = MxcsrGuard::selected(
        profile.rounding,
        profile.flush_to_zero,
        profile.denormals_are_zero,
    )?;
    let caller_mxcsr = environment.original();
    let selected_mxcsr = read_mxcsr()?;
    let mut work = Work {
        check,
        units: 0,
        caller_mxcsr,
    };
    work.charge(input_bytes.div_ceil(4) + sample_count * 32)?;
    let fp = Arithmetic;
    let mut patches = Vec::with_capacity(carrier.displacements.len());
    for (source, positions) in carrier
        .displacements
        .iter()
        .zip(&carrier.position_candidates)
    {
        work.checkpoint()?;
        let count = source.vertex_count();
        work.charge(count * 8)?;
        let mut patch = Patch {
            source: *source,
            positions: positions[mode].clone(),
            normals: vec![[0.0; 3]; count],
            tangent_s: vec![[0.0; 3]; count],
            tangent_t: vec![[0.0; 3]; count],
        };
        local_basis(&mut patch, &fp, &mut work)?;
        patches.push(patch);
    }
    work.charge(sample_count * 12)?;
    let local_digest = stage_digest(&patches);
    let neighbor = neighbors::apply_neighbors(&mut patches, &fp, &mut work)?;
    work.charge(sample_count * 12)?;
    let final_digest = stage_digest(&patches);
    work.checkpoint()?;
    if current_cpu()? != profile.cpu {
        return Err(invalid("Native basis CPU changed during prediction"));
    }
    let mut displacements = Vec::with_capacity(patches.len());
    for patch in patches {
        work.checkpoint()?;
        let mut samples = Vec::with_capacity(patch.positions.len());
        for index in 0..patch.positions.len() {
            samples.push(NativeTerrainSample {
                position: patch.positions[index].map(f32::to_bits),
                normal: NativeTerrainVector::new(patch.normals[index].map(f32::to_bits)),
                tangent_s: NativeTerrainVector::new(patch.tangent_s[index].map(f32::to_bits)),
                tangent_t: NativeTerrainVector::new(patch.tangent_t[index].map(f32::to_bits)),
            });
        }
        displacements.push(NativeTerrainDisplacement {
            source: patch.source,
            samples,
        });
    }
    let snapshot = NativeTerrainSnapshot {
        channel,
        bsp_version: bsp.version,
        map_revision: bsp.map_revision,
        position_rounding_mask: 1 << mode,
        position_roundings: vec![profile.position_rounding],
        displacements,
    };
    let mut prediction = NativeTerrainBasisPrediction {
        snapshot,
        profile,
        receipt: NativeTerrainBasisReceipt {
            implementation: IMPLEMENTATION,
            engine_sha256: ENGINE_SHA256,
            local_digest,
            t_junction_digest: neighbor.t_junction_digest,
            corner_digest: neighbor.corner_digest,
            final_digest,
            displacement_count: carrier.displacements.len(),
            sample_count,
            work_units: work.units,
            scratch_reserved_bytes,
            retained_bytes: 0,
            caller_mxcsr,
            selected_mxcsr,
            t_junction_events: neighbor.t_junction_events,
            corner_events: neighbor.corner_events,
            edge_pairs: neighbor.edge_pairs,
            fine_samples: neighbor.fine_samples,
        },
    };
    let retained_bytes = prediction.retained_bytes();
    if retained_bytes > MAX_NATIVE_BASIS_RETAINED_BYTES {
        return Err(invalid(
            "Native basis prediction exceeds its 64 MiB retained budget",
        ));
    }
    prediction.receipt.retained_bytes = retained_bytes;
    work.checkpoint()?;
    drop(environment);
    Ok(prediction)
}

pub(super) fn stage_digest(patches: &[Patch]) -> u32 {
    let mut digest = Hasher::new();
    for patch in patches {
        digest.update(&patch.source.ordinal().to_le_bytes());
        for index in 0..patch.positions.len() {
            for vector in [
                patch.positions[index],
                patch.normals[index],
                patch.tangent_s[index],
                patch.tangent_t[index],
            ] {
                for value in vector {
                    digest.update(&value.to_bits().to_le_bytes());
                }
            }
        }
    }
    digest.finalize()
}

fn local_basis(patch: &mut Patch, fp: &Arithmetic, work: &mut Work<'_>) -> io::Result<()> {
    let size = patch.source.grid_size();
    for y in 0..size {
        work.checkpoint()?;
        for x in 0..size {
            let mut sum = [0.0; 3];
            let mut count = 0;
            for (quadrant, (dx, dy)) in [(0, 0), (-1, 0), (-1, -1), (0, -1)].into_iter().enumerate()
            {
                let cx = x as isize + dx;
                let cy = y as isize + dy;
                if cx < 0 || cy < 0 || cx >= size as isize - 1 || cy >= size as isize - 1 {
                    continue;
                }
                work.charge(2)?;
                let index = cy as usize * size + cx as usize;
                let a = patch.positions[index];
                let b = patch.positions[index + 1];
                let c = patch.positions[index + size];
                let d = patch.positions[index + size + 1];
                let first = fp.normalize(
                    fp.cross(fp.vec_sub(b, a), fp.vec_sub(c, a)),
                    NormalizeOrder::ZFirst,
                );
                let second_cross = fp.cross(fp.vec_sub(d, b), fp.vec_sub(c, b));
                let second = local_normalize(
                    fp,
                    second_cross,
                    if quadrant == 1 {
                        NormalizeOrder::XyFirst
                    } else {
                        NormalizeOrder::ZFirst
                    },
                    quadrant == 1 || quadrant == 3,
                );
                let pair = [
                    fp.add(first[0], second[0]),
                    if quadrant == 1 || quadrant == 3 {
                        fp.add(second[1], first[1])
                    } else {
                        fp.add(first[1], second[1])
                    },
                    if quadrant == 2 {
                        fp.add(second[2], first[2])
                    } else {
                        fp.add(first[2], second[2])
                    },
                ];
                sum = if quadrant == 0 {
                    pair
                } else {
                    [
                        fp.add(sum[0], pair[0]),
                        fp.add(pair[1], sum[1]),
                        fp.add(sum[2], pair[2]),
                    ]
                };
                count += 2;
            }
            let reciprocal = fp.div(1.0, count as f32);
            patch.normals[y * size + x] = sum.map(|value| fp.mul(value, reciprocal));
        }
    }
    let texture = patch.source.texinfo().texture_vecs;
    let source_s = [texture[0][0], texture[0][1], texture[0][2]];
    let source_t = [texture[1][0], texture[1][1], texture[1][2]];
    let s_xy = fp.add(
        fp.mul(source_s[1], source_s[1]),
        fp.mul(source_s[0], source_s[0]),
    );
    let s_q = fp.add(
        fp.add(fp.mul(source_s[2], source_s[2]), arithmetic::EPSILON),
        s_xy,
    );
    let t_xy = fp.add(
        fp.mul(source_t[0], source_t[0]),
        fp.mul(source_t[1], source_t[1]),
    );
    let t_q = fp.add(
        t_xy,
        fp.add(arithmetic::EPSILON, fp.mul(source_t[2], source_t[2])),
    );
    let s_inverse = fp.inverse_length(s_q);
    let t_inverse = fp.inverse_length(t_q);
    let s_axis = [
        fp.mul(source_s[0], s_inverse),
        fp.mul(source_s[1], s_inverse),
        fp.mul(s_inverse, source_s[2]),
    ];
    let t_axis = [
        fp.mul(source_t[0], t_inverse),
        fp.mul(source_t[1], t_inverse),
        fp.mul(t_inverse, source_t[2]),
    ];
    let source_cross = fp.cross(s_axis, t_axis);
    let base = patch.source.native_base_quad();
    let plane = fp.cross(fp.vec_sub(base[3], base[0]), fp.vec_sub(base[1], base[0]));
    for index in 0..patch.positions.len() {
        work.charge(4)?;
        let normal = patch.normals[index];
        let initial_t = local_normalize(fp, t_axis, NormalizeOrder::ZFirst, true);
        let s_cross = [
            fp.sub(
                fp.mul(normal[1], initial_t[2]),
                fp.mul(normal[2], initial_t[1]),
            ),
            fp.sub(
                fp.mul(normal[2], initial_t[0]),
                fp.mul(initial_t[2], normal[0]),
            ),
            fp.sub(
                fp.mul(initial_t[1], normal[0]),
                fp.mul(initial_t[0], normal[1]),
            ),
        ];
        let mut s = local_normalize(fp, s_cross, NormalizeOrder::ZFirst, true);
        let t_cross = [
            fp.sub(fp.mul(normal[2], s[1]), fp.mul(s[2], normal[1])),
            fp.sub(fp.mul(s[2], normal[0]), fp.mul(normal[2], s[0])),
            fp.sub(fp.mul(normal[1], s[0]), fp.mul(s[1], normal[0])),
        ];
        let t = local_normalize(fp, t_cross, NormalizeOrder::ZFirst, true);
        let xy = fp.add(fp.mul(plane[0], plane[0]), fp.mul(plane[1], plane[1]));
        let q = fp.add(fp.add(fp.mul(plane[2], plane[2]), arithmetic::EPSILON), xy);
        let xy_dot = fp.add(
            fp.mul(plane[0], source_cross[0]),
            fp.mul(plane[1], source_cross[1]),
        );
        let dot = fp.mul(
            fp.add(fp.mul(plane[2], source_cross[2]), xy_dot),
            fp.inverse_length(q),
        );
        if dot > 0.0 {
            s = s.map(|value| f32::from_bits(value.to_bits() ^ 0x80000000));
        }
        patch.tangent_s[index] = s;
        patch.tangent_t[index] = t;
    }
    Ok(())
}

fn local_normalize(
    fp: &Arithmetic,
    v: [f32; 3],
    order: NormalizeOrder,
    z_component_first: bool,
) -> [f32; 3] {
    let inverse = fp.vector_inverse_length(v, order);
    [
        fp.mul(v[0], inverse),
        fp.mul(v[1], inverse),
        if z_component_first {
            fp.mul(v[2], inverse)
        } else {
            fp.mul(inverse, v[2])
        },
    ]
}
