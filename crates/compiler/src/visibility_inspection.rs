use glam::DVec3;
use sha2::{Digest, Sha256};
use source_formats::bsp::{Bsp, Leaf, lump};

use crate::{Control, portals::Portal, tree::Tree, winding::Winding};

pub const MAX_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PORTALS: usize = 262_144;
pub const MAX_POINTS: usize = 1_048_576;
pub const MAX_PORTAL_POINTS: usize = 4096;
pub const MAX_LEAVES: usize = 65_536;
const MAX_SEAL_BYTES: usize = 128 * 1024 * 1024;
const HASH_CHUNK_BYTES: usize = 65_536;

#[derive(Debug, Clone, Copy)]
pub struct WorldLeaf {
    pub ordinal: u32,
    pub prepared_node: u32,
    pub cluster: i16,
}

#[derive(Debug)]
pub struct PortalRecord {
    pub ordinal: u32,
    pub leaves: [u32; 2],
    winding: Winding,
}

impl PortalRecord {
    pub fn points(&self) -> &[DVec3] {
        &self.winding.0
    }
}

#[derive(Debug)]
pub struct Snapshot {
    leaves: Vec<WorldLeaf>,
    portals: Vec<PortalRecord>,
    excluded_portals: usize,
    bytes: usize,
    digest: [u8; 32],
}

pub(crate) struct Pending {
    leaves: Vec<WorldLeaf>,
    portals: Vec<PortalRecord>,
    excluded_portals: usize,
    bytes: usize,
}

fn charged(bytes: usize, additional: usize) -> Result<usize, String> {
    bytes
        .checked_add(additional)
        .filter(|&bytes| bytes <= MAX_BYTES)
        .ok_or_else(|| "Visibility inspection exceeds its 64 MiB metadata limit".into())
}

fn storage<T>(capacity: usize) -> Result<usize, String> {
    capacity
        .checked_mul(size_of::<T>())
        .ok_or_else(|| "Visibility inspection allocation size overflow".into())
}

impl Pending {
    pub(crate) fn capture(
        portals: Vec<Portal>,
        world_leaf_nodes: &[Option<usize>],
        tree: &Tree,
        clusters: &[Option<usize>],
        control: &Control,
    ) -> Result<Self, String> {
        control.checkpoint()?;
        if world_leaf_nodes.len() > MAX_LEAVES
            || tree.nodes.len() > MAX_LEAVES
            || clusters.len() != tree.nodes.len()
            || tree.nodes.is_empty()
            || portals.len() > crate::portals::Limits::default().portals
        {
            return Err("Visibility inspection leaf mapping exceeds its bounds".into());
        }
        let scratch =
            tree.nodes.len() * (size_of::<Option<u32>>() + size_of::<usize>() + size_of::<bool>());
        let mut bytes = charged(size_of::<Snapshot>() + 2 * size_of::<usize>(), scratch)?;
        bytes = charged(bytes, world_leaf_nodes.len() * size_of::<WorldLeaf>())?;
        let mut inverse = Vec::new();
        inverse
            .try_reserve_exact(tree.nodes.len())
            .map_err(|_| "Cannot reserve visibility leaf mapping")?;
        inverse.resize(tree.nodes.len(), None);
        let mut reached = Vec::new();
        reached
            .try_reserve_exact(tree.nodes.len())
            .map_err(|_| "Cannot reserve visibility reachability")?;
        reached.resize(tree.nodes.len(), false);
        let mut pending = Vec::new();
        pending
            .try_reserve_exact(tree.nodes.len())
            .map_err(|_| "Cannot reserve visibility tree traversal")?;
        let reached_bytes = storage::<bool>(reached.capacity())?;
        let pending_bytes = storage::<usize>(pending.capacity())?;
        let actual_scratch = storage::<Option<u32>>(inverse.capacity())?
            .checked_add(reached_bytes)
            .and_then(|bytes| bytes.checked_add(pending_bytes))
            .ok_or("Visibility inspection scratch size overflow")?;
        bytes = charged(
            bytes,
            actual_scratch
                .checked_sub(scratch)
                .ok_or("Visibility inspection scratch capacity is inconsistent")?,
        )?;
        let scratch = actual_scratch;
        pending.push(0);
        while let Some(index) = pending.pop() {
            control.checkpoint()?;
            let node = tree
                .nodes
                .get(index)
                .ok_or("Visibility tree child is out of range")?;
            if std::mem::replace(&mut reached[index], true) {
                return Err("Visibility tree has repeated child ownership".into());
            }
            if let Some((_, children)) = node.split {
                if pending.len() + 2 > tree.nodes.len() {
                    return Err("Visibility tree traversal exceeds its bounded stack".into());
                }
                pending.extend(children);
            }
        }
        let mut leaves = Vec::new();
        leaves
            .try_reserve_exact(world_leaf_nodes.len())
            .map_err(|_| "Cannot reserve visibility leaves")?;
        bytes = charged(
            bytes,
            storage::<WorldLeaf>(leaves.capacity())?
                .checked_sub(storage::<WorldLeaf>(world_leaf_nodes.len())?)
                .ok_or("Visibility inspection leaf capacity is inconsistent")?,
        )?;
        for (ordinal, node) in world_leaf_nodes.iter().enumerate() {
            control.checkpoint()?;
            let Some(node) = *node else { continue };
            let source = tree
                .nodes
                .get(node)
                .ok_or("Visibility world leaf mapping is out of range")?;
            if !reached[node]
                || source.split.is_some()
                || inverse[node].replace(ordinal as u32).is_some()
            {
                return Err("Visibility world leaf mapping is not one-to-one".into());
            }
            let cluster = clusters[node]
                .map(i16::try_from)
                .transpose()
                .map_err(|_| "Visibility cluster exceeds its native ordinal")?
                .unwrap_or(-1);
            if source.solid() != (cluster < 0) {
                return Err("Visibility world leaf contents disagree with its cluster".into());
            }
            leaves.push(WorldLeaf {
                ordinal: ordinal as u32,
                prepared_node: node as u32,
                cluster,
            });
        }
        for (index, node) in tree.nodes.iter().enumerate() {
            control.checkpoint()?;
            if reached[index] && node.split.is_none() && inverse[index].is_none() {
                return Err("An emitted world leaf is missing from its exact mapping".into());
            }
        }
        let applicable = |portal: &Portal| {
            let [a, b] = portal.leaves;
            a != b
                && portal.leaves.iter().all(|&node| {
                    inverse.get(node).is_some_and(Option::is_some)
                        && clusters.get(node).is_some_and(Option::is_some)
                })
        };
        let mut count = 0usize;
        let mut points = 0usize;
        let mut excluded_portals = 0usize;
        for portal in &portals {
            control.checkpoint()?;
            if !applicable(portal) {
                excluded_portals += 1;
                continue;
            }
            count = count
                .checked_add(1)
                .filter(|&count| count <= MAX_PORTALS)
                .ok_or("Visibility inspection exceeds its portal limit")?;
            points = points
                .checked_add(portal.winding.0.len())
                .filter(|&points| points <= MAX_POINTS)
                .ok_or("Visibility inspection exceeds its point limit")?;
            if !(3..=MAX_PORTAL_POINTS).contains(&portal.winding.0.len()) {
                return Err("Visibility inspection portal exceeds its polygon limit".into());
            }
            bytes = charged(bytes, storage::<DVec3>(portal.winding.0.capacity())?)?;
            for point in &portal.winding.0 {
                control.checkpoint()?;
                if !point.is_finite()
                    || point.abs().max_element() > map_core::checks::SOURCE_COORDINATE_LIMIT
                {
                    return Err("Visibility portal has invalid world coordinates".into());
                }
            }
        }
        bytes = charged(bytes, count * size_of::<PortalRecord>())?;
        let mut records = Vec::new();
        records
            .try_reserve_exact(count)
            .map_err(|_| "Cannot reserve visibility portals")?;
        bytes = charged(
            bytes,
            storage::<PortalRecord>(records.capacity())?
                .checked_sub(storage::<PortalRecord>(count)?)
                .ok_or("Visibility inspection portal capacity is inconsistent")?,
        )?;
        for (ordinal, portal) in portals.into_iter().enumerate() {
            control.checkpoint()?;
            if applicable(&portal) {
                records.push(PortalRecord {
                    ordinal: u32::try_from(ordinal)
                        .map_err(|_| "Visibility portal ordinal overflow")?,
                    leaves: portal.leaves.map(|node| inverse[node].unwrap()),
                    winding: portal.winding,
                });
            }
        }
        Ok(Self {
            leaves,
            portals: records,
            excluded_portals,
            bytes: bytes - scratch,
        })
    }

    pub(crate) fn finish(self, bsp: &Bsp, control: &Control) -> Result<Snapshot, String> {
        control.checkpoint()?;
        if bsp.lumps[lump::LEAFS].data.len() > MAX_LEAVES * 56 {
            return Err("Visibility inspection native leaves exceed their storage limit".into());
        }
        let leaves = bsp.records::<Leaf>().map_err(|error| error.to_string())?;
        if leaves.len() > MAX_LEAVES {
            return Err("Visibility inspection native leaf count exceeds its limit".into());
        }
        let visibility = bsp
            .visibility()
            .map_err(|error| error.to_string())?
            .ok_or("Compiled visibility is missing")?;
        for leaf in &self.leaves {
            control.checkpoint()?;
            let native = leaves
                .get(leaf.ordinal as usize)
                .ok_or("Retained visibility leaf was not emitted")?;
            if native.cluster != leaf.cluster
                || (leaf.cluster >= 0 && leaf.cluster as usize >= visibility.clusters())
            {
                return Err("Retained visibility cluster differs from the native leaf".into());
            }
        }
        let digest = digest(bsp, control)?;
        Ok(Snapshot {
            leaves: self.leaves,
            portals: self.portals,
            excluded_portals: self.excluded_portals,
            bytes: self.bytes,
            digest,
        })
    }
}

impl Snapshot {
    pub fn leaves(&self) -> &[WorldLeaf] {
        &self.leaves
    }
    pub fn portals(&self) -> &[PortalRecord] {
        &self.portals
    }
    pub fn portal(&self, ordinal: usize) -> Option<&PortalRecord> {
        self.portals
            .binary_search_by_key(&ordinal, |portal| portal.ordinal as usize)
            .ok()
            .map(|index| &self.portals[index])
    }
    pub fn excluded_portals(&self) -> usize {
        self.excluded_portals
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }
    pub fn validate(&self, bsp: &Bsp, control: &Control) -> Result<(), String> {
        if self.digest != digest(bsp, control)? {
            return Err(
                "Compiled visibility bytes differ from the retained compiler mapping".into(),
            );
        }
        Ok(())
    }
}

fn digest(bsp: &Bsp, control: &Control) -> Result<[u8; 32], String> {
    let mut bytes = 0usize;
    let mut hash = Sha256::new();
    for index in [
        lump::PLANES,
        lump::NODES,
        lump::LEAFS,
        lump::MODELS,
        lump::VISIBILITY,
    ] {
        control.checkpoint()?;
        let value = &bsp.lumps[index];
        bytes = bytes
            .checked_add(value.data.len())
            .filter(|&bytes| bytes <= MAX_SEAL_BYTES)
            .ok_or("Visibility inspection exceeds its BSP verification limit")?;
        hash.update((index as u32).to_le_bytes());
        hash.update(value.version.to_le_bytes());
        hash.update(value.fourcc);
        hash.update((value.data.len() as u64).to_le_bytes());
        for chunk in value.data.chunks(HASH_CHUNK_BYTES) {
            control.checkpoint()?;
            hash.update(chunk);
        }
    }
    Ok(hash.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{Node, Polyhedron};

    fn fixture() -> (Tree, Vec<Portal>, Bsp) {
        let tree = Tree {
            nodes: (0..3)
                .map(|index| Node {
                    split: (index == 0).then_some((0, [1, 2])),
                    volume: Polyhedron { sides: Vec::new() },
                    contents: 0,
                    brushes: Vec::new(),
                })
                .collect(),
        };
        let points = vec![DVec3::ZERO, DVec3::Y, DVec3::Z];
        let portals = vec![
            Portal {
                leaves: [1, 3],
                winding: Winding(points.clone()),
            },
            Portal {
                leaves: [1, 2],
                winding: Winding(points),
            },
        ];
        let mut bsp = Bsp::new();
        bsp.set_records(&[
            Leaf {
                cluster: -1,
                ..Leaf::default()
            },
            Leaf {
                cluster: 0,
                ..Leaf::default()
            },
            Leaf {
                cluster: -1,
                ..Leaf::default()
            },
            Leaf {
                cluster: 0,
                ..Leaf::default()
            },
        ]);
        bsp.lumps[lump::VISIBILITY].data = [
            1_i32.to_le_bytes(),
            12_i32.to_le_bytes(),
            12_i32.to_le_bytes(),
        ]
        .concat();
        bsp.lumps[lump::VISIBILITY].data.push(1);
        (tree, portals, bsp)
    }

    #[test]
    fn capture_moves_original_rings_and_joins_nonspatial_ordinals_after_coarsening() {
        let (tree, portals, mut bsp) = fixture();
        let pointer = portals[1].winding.0.as_ptr();
        let control = Control::default();
        let pending = Pending::capture(
            portals,
            &[None, Some(2), None, Some(1)],
            &tree,
            &[None, Some(0), Some(0)],
            &control,
        )
        .unwrap();
        let snapshot = pending.finish(&bsp, &control).unwrap();
        assert_eq!(
            snapshot
                .leaves()
                .iter()
                .map(|leaf| (leaf.ordinal, leaf.prepared_node))
                .collect::<Vec<_>>(),
            [(1, 2), (3, 1)]
        );
        assert_eq!(snapshot.portals().len(), 1);
        let portal = snapshot.portal(1).unwrap();
        assert_eq!(portal.leaves, [3, 1]);
        assert_eq!(portal.points().as_ptr(), pointer);
        assert_eq!(snapshot.excluded_portals(), 1);
        assert!(snapshot.portal(0).is_none());
        snapshot.validate(&bsp, &control).unwrap();
        bsp.lumps[lump::VISIBILITY].data[12] = 0;
        assert!(snapshot.validate(&bsp, &control).is_err());
    }

    #[test]
    fn capture_rejects_missing_duplicate_and_oversized_metadata_without_guessing() {
        let (tree, portals, bsp) = fixture();
        let control = Control::default();
        assert!(
            Pending::capture(
                portals,
                &[None, Some(1), Some(1)],
                &tree,
                &[None, Some(0), Some(0)],
                &control
            )
            .is_err()
        );
        let (_, portals, _) = fixture();
        assert!(
            Pending::capture(
                portals,
                &[None, Some(1)],
                &tree,
                &[None, Some(0), Some(0)],
                &control
            )
            .is_err()
        );
        let (_, mut portals, _) = fixture();
        portals[1]
            .winding
            .0
            .resize(MAX_PORTAL_POINTS + 1, DVec3::ZERO);
        assert!(
            Pending::capture(
                portals,
                &[None, Some(2), None, Some(1)],
                &tree,
                &[None, Some(0), Some(0)],
                &control
            )
            .is_err()
        );
        let (_, portals, _) = fixture();
        let pending = Pending::capture(
            portals,
            &[None, Some(2), None, Some(1)],
            &tree,
            &[None, Some(1), Some(0)],
            &control,
        )
        .unwrap();
        assert!(pending.finish(&bsp, &control).is_err());
        let (_, portals, _) = fixture();
        control.cancel().unwrap();
        assert!(
            Pending::capture(
                portals,
                &[None, Some(2), None, Some(1)],
                &tree,
                &[None, Some(0), Some(0)],
                &control
            )
            .is_err()
        );
    }
}
