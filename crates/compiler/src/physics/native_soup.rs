use super::*;
use crate::physics::Mesh;

const CENTROID_CELL_SIZE: f64 = 0.125;
const PERMUTATIONS: [[usize; 3]; 6] = [
    [0, 1, 2],
    [0, 2, 1],
    [1, 0, 2],
    [1, 2, 0],
    [2, 0, 1],
    [2, 1, 0],
];

struct Soup<'a> {
    api: &'a Api,
    pointer: Pointer,
}

impl Drop for Soup<'_> {
    fn drop(&mut self) {
        unsafe {
            (self.api.soup_destroy)(self.api.pointer, self.pointer);
        }
    }
}

impl Api {
    pub(super) fn soup(&self, mesh: &Mesh) -> Result<Collide<'_>, String> {
        let pointer = unsafe { (self.soup_create)(self.pointer) };
        if pointer.is_null() {
            return Err("Native terrain soup could not be created".into());
        }
        let soup = Soup { api: self, pointer };
        for (index, triangle) in mesh.triangles.iter().enumerate() {
            let [a, b, c] = triangle.map(|index| Vector(mesh.vertices[index as usize]));
            let material = mesh.materials.get(index).copied().unwrap_or(1);
            unsafe {
                (self.soup_triangle)(self.pointer, soup.pointer, &a, &b, &c, i32::from(material));
            }
        }
        let pointer = unsafe { (self.soup_convert)(self.pointer, soup.pointer, false) };
        if pointer.is_null() {
            return Err("Native terrain soup conversion failed".into());
        }
        Ok(Collide { api: self, pointer })
    }
}

fn cell(triangle: [DVec3; 3]) -> [i64; 3] {
    ((triangle[0] + triangle[1] + triangle[2]) / (3.0 * CENTROID_CELL_SIZE))
        .floor()
        .as_i64vec3()
        .to_array()
}

fn matches(actual: [DVec3; 3], expected: [DVec3; 3]) -> bool {
    PERMUTATIONS.iter().any(|permutation| {
        (0..3).all(|index| {
            actual[index].distance_squared(expected[permutation[index]]) <= GEOMETRY_EPSILON.powi(2)
        })
    })
}

impl Collide<'_> {
    pub(super) fn verify_soup(
        &self,
        mesh: &Mesh,
        material: i32,
        work: &mut usize,
    ) -> Result<f32, String> {
        let volume = unsafe { (self.api.volume)(self.api.pointer, self.pointer) };
        if !volume.is_finite() || f64::from(volume.abs()) > GEOMETRY_EPSILON {
            return Err("Native terrain mesh must have zero finite volume".into());
        }
        let pointer = unsafe { (self.api.query)(self.api.pointer, self.pointer) };
        if pointer.is_null() {
            return Err("Native terrain query could not be created".into());
        }
        let _query = Query {
            body: self,
            pointer,
        };
        let count: Count = unsafe { member(pointer, self.api.base, 2, 0x20b00)? };
        let triangles: TriangleCount = unsafe { member(pointer, self.api.base, 3, 0x20b10)? };
        let vertices: Vertices = unsafe { member(pointer, self.api.base, 5, 0x20b50)? };
        let get_material: Material = unsafe { member(pointer, self.api.base, 7, 0x20bf0)? };
        if unsafe { count(pointer) } != mesh.triangles.len() as i32 {
            return Err("Native terrain mesh changed its triangle hull count".into());
        }
        let expected: Vec<_> = mesh
            .triangles
            .iter()
            .map(|triangle| {
                triangle
                    .map(|index| DVec3::from_array(mesh.vertices[index as usize].map(f64::from)))
            })
            .collect();
        let mut spatial = BTreeMap::<[i64; 3], Vec<usize>>::new();
        for (index, &triangle) in expected.iter().enumerate() {
            spatial.entry(cell(triangle)).or_default().push(index);
        }
        let mut seen = vec![false; mesh.triangles.len()];
        for index in 0..mesh.triangles.len() as i32 {
            if unsafe { triangles(pointer, index) } != 2 {
                return Err("Native terrain triangle needs two collision sides".into());
            }
            let mut match_index = None;
            let mut actual_materials = [0; 2];
            for side in 0..2 {
                let mut corners = [Vector::default(); 3];
                unsafe {
                    vertices(pointer, index, side, corners.as_mut_ptr());
                }
                let corners = corners.map(point);
                actual_materials[side as usize] = unsafe { get_material(pointer, index, side) };
                if corners.iter().any(|point| !point.is_finite()) {
                    return Err("Native terrain vertices are invalid".into());
                }
                if let Some(expected_index) = match_index {
                    *work += 18;
                    if !matches(corners, expected[expected_index]) {
                        return Err("Native terrain collision sides disagree".into());
                    }
                    continue;
                }
                let center = cell(corners);
                'search: for x in -1..=1 {
                    for y in -1..=1 {
                        for z in -1..=1 {
                            if let Some(candidates) =
                                spatial.get(&[center[0] + x, center[1] + y, center[2] + z])
                            {
                                for &candidate in candidates {
                                    *work += 1;
                                    if *work > MAX_GEOMETRY_WORK {
                                        return Err(
                                            "Terrain mesh verification exceeded its work limit"
                                                .into(),
                                        );
                                    }
                                    if seen[candidate] {
                                        continue;
                                    }
                                    *work += 18;
                                    if matches(corners, expected[candidate]) {
                                        match_index = Some(candidate);
                                        break 'search;
                                    }
                                }
                            }
                        }
                    }
                }
                if match_index.is_none() {
                    return Err("Native terrain triangle does not match the compiled grid".into());
                }
            }
            if *work > MAX_GEOMETRY_WORK {
                return Err("Terrain mesh verification exceeded its work limit".into());
            }
            let expected_index = match_index.unwrap();
            let expected_material = mesh
                .materials
                .get(expected_index)
                .map_or(material, |&index| i32::from(index));
            if actual_materials != [expected_material; 2] {
                return Err("Native terrain did not retain the authored surface mapping".into());
            }
            seen[expected_index] = true;
        }
        if seen.iter().any(|&seen| !seen) {
            return Err("Native terrain mesh dropped compiled triangles".into());
        }
        Ok(volume)
    }
}
