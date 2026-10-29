use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use glam::DVec3;
use map_core::displacement::native::{vertex_key, vertex_position};
use source_formats::bsp::contents;

use super::Face;
use crate::Control;
use crate::map::{MAX_COMPILE_COORD, Map};

const MAX_FACES: usize = 65_536;
const MAX_OWNERS: usize = 262_144;
const MAX_RING_POINTS: usize = 1_000_000;
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_WORK: usize = 64_000_000;
const POINT_WORK: usize = 32;
const SORT_WORK: usize = 32;
const HASH_WORK: usize = 128;

#[derive(Clone, Copy, PartialEq, Eq)]
enum BrushState {
    Outside,
    Unchecked,
    Opaque,
    Rejected,
}

struct Owner {
    id: u32,
    brush: usize,
    side: usize,
}

struct Entry {
    face: usize,
    plane: usize,
    start: usize,
    count: usize,
    hash: u64,
    reversed: bool,
}

#[derive(Default)]
struct Budget {
    work: usize,
    bytes: usize,
}

impl Budget {
    fn charge(&mut self, work: usize, control: &Control) -> Result<(), String> {
        control.checkpoint()?;
        self.work = self
            .work
            .checked_add(work)
            .filter(|&work| work <= MAX_WORK)
            .ok_or("Internal detail caps exceed their cumulative work limit")?;
        Ok(())
    }

    fn reserve<T>(
        &mut self,
        values: &mut Vec<T>,
        count: usize,
        control: &Control,
    ) -> Result<(), String> {
        self.charge(count, control)?;
        self.bytes = count
            .checked_mul(size_of::<T>())
            .and_then(|bytes| self.bytes.checked_add(bytes))
            .filter(|&bytes| bytes <= MAX_BYTES)
            .ok_or("Internal detail caps exceed their prepared storage limit")?;
        values
            .try_reserve_exact(count)
            .map_err(|error| format!("Cannot allocate internal detail cap metadata: {error}"))
    }

    fn sort(&mut self, count: usize, control: &Control) -> Result<(), String> {
        let comparisons = count
            .checked_mul(count.checked_ilog2().unwrap_or(0) as usize + 1)
            .and_then(|count| count.checked_mul(SORT_WORK))
            .ok_or("Internal detail cap sort work overflow")?;
        self.charge(comparisons, control)
    }
}

fn owner<'a>(
    owners: &'a [Owner],
    id: u32,
    budget: &mut Budget,
    control: &Control,
) -> Result<Option<&'a Owner>, String> {
    budget.charge(
        (owners.len().checked_ilog2().unwrap_or(0) as usize + 1) * 4,
        control,
    )?;
    Ok(owners
        .binary_search_by_key(&id, |owner| owner.id)
        .ok()
        .map(|index| &owners[index]))
}

fn eligible_face(
    face: &Face,
    map: &Map<'_>,
    owners: &[Owner],
    budget: &mut Budget,
    control: &Control,
) -> Result<bool, String> {
    if face.displacement.is_some() || face.water_region.is_some() || face.source_sides.is_empty() {
        return Ok(false);
    }
    for &id in face.source_sides.iter() {
        let Some(owner) = owner(owners, id, budget, control)? else {
            return Ok(false);
        };
        if map.brushes[owner.brush].sides[owner.side].plane != face.plane {
            return Ok(false);
        }
    }
    Ok(true)
}

fn native_point(key: [i64; 3]) -> DVec3 {
    DVec3::from_array(vertex_position(key).map(f64::from))
}

fn ring(
    face: &Face,
    keys: &mut Vec<[i64; 3]>,
    normal: DVec3,
    budget: &mut Budget,
    control: &Control,
) -> Result<Option<(u64, bool)>, String> {
    if face.winding.0.len() < 3 {
        return Err("Internal detail cap has fewer than three corners".into());
    }
    let start = keys.len();
    for &point in &face.winding.0 {
        budget.charge(POINT_WORK, control)?;
        if !point.is_finite() || point.abs().max_element() > MAX_COMPILE_COORD {
            return Err("Internal detail cap has nonfinite or out-of-bounds corners".into());
        }
        keys.push(vertex_key(point));
    }
    let points = &mut keys[start..];
    let mut first = 0;
    for index in 0..points.len() {
        budget.charge(POINT_WORK, control)?;
        if points[index] < points[first] {
            first = index;
        }
        for previous in 0..index {
            budget.charge(POINT_WORK, control)?;
            if native_point(points[index]) == native_point(points[previous]) {
                keys.truncate(start);
                return Ok(None);
            }
        }
    }
    let anchor = native_point(points[0]);
    let mut area = DVec3::ZERO;
    for index in 1..points.len() - 1 {
        budget.charge(2 * POINT_WORK, control)?;
        area +=
            (native_point(points[index + 1]) - anchor).cross(native_point(points[index]) - anchor);
    }
    let support = area.dot(normal);
    if !area.is_finite() || !support.is_finite() {
        return Err("Internal detail cap has nonfinite native area".into());
    }
    if area.length_squared() == 0.0 {
        keys.truncate(start);
        return Ok(None);
    }
    if support <= 0.0 {
        return Err("Internal detail cap winding disagrees with its source plane".into());
    }
    for index in 0..points.len() {
        let a = native_point(points[index]);
        let edge = native_point(points[(index + 1) % points.len()]) - a;
        for &point in points.iter() {
            budget.charge(2 * POINT_WORK, control)?;
            let distance = edge.cross(native_point(point) - a).dot(normal);
            if !distance.is_finite() {
                return Err("Internal detail cap has nonfinite native edge support".into());
            }
            if distance > 0.0 {
                keys.truncate(start);
                return Ok(None);
            }
        }
    }
    budget.charge(points.len() * 8, control)?;
    points.rotate_left(first);
    let reversed = points[1] > points[points.len() - 1];
    if reversed {
        points[1..].reverse();
    }
    budget.charge(POINT_WORK + points.len() * HASH_WORK, control)?;
    let mut hash = DefaultHasher::new();
    points.hash(&mut hash);
    Ok(Some((hash.finish(), reversed)))
}

fn disjoint_brushes(
    a: &Face,
    b: &Face,
    owners: &[Owner],
    budget: &mut Budget,
    control: &Control,
) -> Result<bool, String> {
    for &id in a.source_sides.iter() {
        let a = owner(owners, id, budget, control)?
            .ok_or("Internal detail cap source owner is unavailable")?;
        for &id in b.source_sides.iter() {
            let b = owner(owners, id, budget, control)?
                .ok_or("Internal detail cap opposing owner is unavailable")?;
            if a.brush == b.brush {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn opaque_face(
    face: &Face,
    map: &Map<'_>,
    owners: &[Owner],
    states: &mut [BrushState],
    opaque: &mut dyn FnMut(usize) -> Result<bool, String>,
    budget: &mut Budget,
    control: &Control,
) -> Result<bool, String> {
    for &id in face.source_sides.iter() {
        let owner = owner(owners, id, budget, control)?
            .ok_or("Internal detail cap source owner is unavailable")?;
        match states[owner.brush] {
            BrushState::Opaque => continue,
            BrushState::Outside | BrushState::Rejected => return Ok(false),
            BrushState::Unchecked => {}
        }
        let brush = &map.brushes[owner.brush];
        budget.charge(brush.sides.len(), control)?;
        for side in brush.sides.iter().filter(|side| !side.bevel) {
            budget.charge(4, control)?;
            let id = side
                .source_id
                .ok_or("Internal detail cap lacks original physical side identity")?;
            let admitted = opaque(side.material).map_err(|error| {
                format!("Internal detail caps solid:{} side:{id}: {error}", brush.id,)
            })?;
            if !admitted {
                states[owner.brush] = BrushState::Rejected;
                return Ok(false);
            }
        }
        states[owner.brush] = BrushState::Opaque;
    }
    Ok(true)
}

pub(crate) fn remove_internal_caps(
    map: &Map<'_>,
    faces: &mut Vec<Face>,
    opaque: &mut dyn FnMut(usize) -> Result<bool, String>,
    control: &Control,
) -> Result<usize, String> {
    let mut budget = Budget::default();
    let world = map
        .entities
        .first()
        .ok_or("Internal detail caps require world geometry")?;
    let mut has_details = false;
    for &index in &world.brushes {
        budget.charge(1, control)?;
        let brush = map
            .brushes
            .get(index)
            .ok_or("Internal detail cap brush is unavailable")?;
        has_details |= brush.entity == 0 && brush.contents == (contents::SOLID | contents::DETAIL);
    }
    if !has_details {
        return Ok(0);
    }
    let mut states = Vec::new();
    budget.reserve(&mut states, map.brushes.len(), control)?;
    states.resize(map.brushes.len(), BrushState::Outside);
    let mut owner_count = 0_usize;
    for &index in &world.brushes {
        budget.charge(1, control)?;
        let brush = &map.brushes[index];
        if brush.entity != 0 || brush.contents != (contents::SOLID | contents::DETAIL) {
            continue;
        }
        let mut original = true;
        let mut count = 0;
        budget.charge(brush.sides.len(), control)?;
        for side in brush.sides.iter().filter(|side| !side.bevel) {
            budget.charge(4, control)?;
            original &= side.source_id.is_some();
            count += 1;
        }
        if original && count >= 4 {
            owner_count = owner_count
                .checked_add(count)
                .filter(|&count| count <= MAX_OWNERS)
                .ok_or("Internal detail caps exceed their original-side index limit")?;
            states[index] = BrushState::Unchecked;
        }
    }
    if owner_count == 0 {
        return Ok(0);
    }
    let mut owners = Vec::new();
    budget.reserve(&mut owners, owner_count, control)?;
    for &index in &world.brushes {
        budget.charge(1, control)?;
        if states[index] != BrushState::Unchecked {
            continue;
        }
        budget.charge(map.brushes[index].sides.len(), control)?;
        for (side_index, side) in map.brushes[index]
            .sides
            .iter()
            .enumerate()
            .filter(|(_, side)| !side.bevel)
        {
            budget.charge(4, control)?;
            owners.push(Owner {
                id: side
                    .source_id
                    .ok_or("Internal detail cap lacks original side identity")?,
                brush: index,
                side: side_index,
            });
        }
    }
    budget.sort(owners.len(), control)?;
    owners.sort_unstable_by_key(|owner| owner.id);
    budget.charge(owners.len(), control)?;
    if owners.windows(2).any(|pair| pair[0].id == pair[1].id) {
        return Err("Internal detail caps have ambiguous original side ownership".into());
    }
    if faces.len() > MAX_FACES {
        return Err("Internal detail caps exceed their face index limit".into());
    }
    let mut entries = Vec::new();
    budget.reserve(&mut entries, faces.len(), control)?;
    let mut point_count = 0_usize;
    for (face_index, face) in faces.iter().enumerate() {
        budget.charge(1, control)?;
        if !eligible_face(face, map, &owners, &mut budget, control)? {
            continue;
        }
        if face.plane >= map.planes.len() {
            return Err("Internal detail cap source plane is unavailable".into());
        }
        point_count = point_count
            .checked_add(face.winding.0.len())
            .filter(|&count| count <= MAX_RING_POINTS)
            .ok_or("Internal detail caps exceed their native ring point limit")?;
        entries.push(Entry {
            face: face_index,
            plane: face.plane,
            start: 0,
            count: 0,
            hash: 0,
            reversed: false,
        });
    }
    let mut keys = Vec::new();
    budget.reserve(&mut keys, point_count, control)?;
    for entry in &mut entries {
        let face = &faces[entry.face];
        entry.start = keys.len();
        if let Some((hash, reversed)) = ring(
            face,
            &mut keys,
            map.planes.get(entry.plane).normal,
            &mut budget,
            control,
        )? {
            entry.count = face.winding.0.len();
            entry.hash = hash;
            entry.reversed = reversed;
        }
    }
    budget.charge(entries.len(), control)?;
    entries.retain(|entry| entry.count != 0);
    budget.sort(entries.len(), control)?;
    entries.sort_unstable_by_key(|entry| (entry.plane, entry.hash, entry.count, entry.face));
    let mut removed = Vec::new();
    budget.reserve(&mut removed, faces.len(), control)?;
    removed.resize(faces.len(), false);
    for source in &entries {
        budget.charge(4, control)?;
        if removed[source.face] {
            continue;
        }
        let key = (source.plane ^ 1, source.hash, source.count);
        budget.charge(
            (entries.len().checked_ilog2().unwrap_or(0) as usize + 1) * 16,
            control,
        )?;
        let first = entries.partition_point(|entry| (entry.plane, entry.hash, entry.count) < key);
        let last = entries.partition_point(|entry| (entry.plane, entry.hash, entry.count) <= key);
        for candidate in &entries[first..last] {
            budget.charge(4, control)?;
            if source.reversed == candidate.reversed {
                continue;
            }
            budget.charge(source.count * 4, control)?;
            if keys[source.start..source.start + source.count]
                != keys[candidate.start..candidate.start + candidate.count]
                || !disjoint_brushes(
                    &faces[source.face],
                    &faces[candidate.face],
                    &owners,
                    &mut budget,
                    control,
                )?
                || !opaque_face(
                    &faces[source.face],
                    map,
                    &owners,
                    &mut states,
                    opaque,
                    &mut budget,
                    control,
                )?
                || !opaque_face(
                    &faces[candidate.face],
                    map,
                    &owners,
                    &mut states,
                    opaque,
                    &mut budget,
                    control,
                )?
            {
                continue;
            }
            removed[source.face] = true;
            removed[candidate.face] = true;
            break;
        }
    }
    budget.charge(faces.len() * 2, control)?;
    let count = removed.iter().filter(|&&removed| removed).count();
    if count != 0 {
        control.checkpoint()?;
        let mut index = 0;
        faces.retain(|_| {
            let keep = !removed[index];
            index += 1;
            keep
        });
    }
    Ok(count)
}
