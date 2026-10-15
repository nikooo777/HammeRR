use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{c_char, c_void};
use std::io;
use std::path::Path;

use glam::DVec3;
use libloading::os::unix::{Library, RTLD_GLOBAL, RTLD_NOW};
use sha2::{Digest, Sha256};
use source_formats::bsp::{
    CollisionHeader, MAX_COLLISION_BYTES, PhysicsCollision, PhysicsModel, PhysicsSolid,
};

use super::{Body, Request, Shape, read_bounded};

#[path = "native_mass.rs"]
mod mass;
#[path = "native_soup.rs"]
mod soup;

const LIBRARY_SHA256: &str = "2676cdc02be4b4188f7121c201d5f5a681f9c1c7c924fe7f9159cefd5a718277";
const GUARD_BYTES: usize = 64;
const GEOMETRY_EPSILON: f64 = 0.02;
const MAX_TRIANGLES: i32 = 512;
const MAX_GEOMETRY_WORK: usize = 50_000_000;
type Pointer = *mut c_void;
type Factory = unsafe extern "C" fn(*const c_char, *mut i32) -> Pointer;
type FromVertices = unsafe extern "C" fn(Pointer, *const *const Vector, i32) -> Pointer;
type SetGameData = unsafe extern "C" fn(Pointer, Pointer, u32);
type Release = unsafe extern "C" fn(Pointer, Pointer);
type Convert = unsafe extern "C" fn(Pointer, *const Pointer, i32) -> Pointer;
type Size = unsafe extern "C" fn(Pointer, Pointer) -> i32;
type Write = unsafe extern "C" fn(Pointer, *mut u8, Pointer, bool) -> i32;
type Unserialize = unsafe extern "C" fn(Pointer, *mut u8, i32, i32) -> Pointer;
type Measure = unsafe extern "C" fn(Pointer, Pointer) -> f32;
type CreateQuery = unsafe extern "C" fn(Pointer, Pointer) -> Pointer;
type Count = unsafe extern "C" fn(Pointer) -> i32;
type TriangleCount = unsafe extern "C" fn(Pointer, i32) -> i32;
type GameData = unsafe extern "C" fn(Pointer, i32) -> u32;
type Vertices = unsafe extern "C" fn(Pointer, i32, i32, *mut Vector);
type Material = unsafe extern "C" fn(Pointer, i32, i32) -> i32;
type SetMaterial = unsafe extern "C" fn(Pointer, i32, i32, i32);
type CreateSoup = unsafe extern "C" fn(Pointer) -> Pointer;
type SoupTriangle =
    unsafe extern "C" fn(Pointer, Pointer, *const Vector, *const Vector, *const Vector, i32);
type ConvertSoup = unsafe extern "C" fn(Pointer, Pointer, bool) -> Pointer;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Vector([f32; 3]);

struct Api {
    pointer: Pointer,
    base: usize,
    from_vertices: FromVertices,
    set_game_data: SetGameData,
    convex_free: Release,
    convert: Convert,
    soup_create: CreateSoup,
    soup_destroy: Release,
    soup_triangle: SoupTriangle,
    soup_convert: ConvertSoup,
    destroy: Release,
    size: Size,
    write: Write,
    unserialize: Unserialize,
    volume: Measure,
    query: CreateQuery,
    destroy_query: Release,
    _physics: Library,
    _tier0: Library,
}

unsafe fn member<T: Copy>(
    pointer: Pointer,
    base: usize,
    slot: usize,
    offset: usize,
) -> Result<T, String> {
    if size_of::<T>() != size_of::<usize>() || pointer.is_null() {
        return Err("Invalid native physics method binding".into());
    }
    // The verified library owns this instance and its System V vtable.
    let address = unsafe { (*(pointer as *const *const usize)).add(slot).read() };
    if address != base + offset {
        return Err(format!("Unverified physics method at slot {slot}"));
    }
    Ok(unsafe { std::mem::transmute_copy(&address) })
}

impl Api {
    fn load(directory: &Path) -> Result<Self, String> {
        let path = directory
            .join("vphysics.so")
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let bytes = read_bounded(&path, 32 * 1024 * 1024)?;
        if format!("{:x}", Sha256::digest(&bytes)) != LIBRARY_SHA256 {
            return Err("Unverified CS:S vphysics library build; inspect its ABI before enabling engine physics".into());
        }
        let tier0 =
            unsafe { Library::open(Some(directory.join("libtier0.so")), RTLD_NOW | RTLD_GLOBAL) }
                .map_err(|error| error.to_string())?;
        let physics =
            unsafe { Library::open(Some(&path), RTLD_NOW) }.map_err(|error| error.to_string())?;
        let factory = unsafe { physics.get::<Factory>(b"CreateInterface\0") }
            .map_err(|error| error.to_string())?;
        let mut status = -1;
        let pointer = unsafe { factory(c"VPhysicsCollision007".as_ptr(), &mut status) };
        if pointer.is_null() || status != 0 {
            return Err("The installed VPhysicsCollision007 interface is unavailable".into());
        }
        let path = path.to_str().ok_or("Physics library path is not UTF-8")?;
        let maps = std::fs::read_to_string("/proc/self/maps").map_err(|error| error.to_string())?;
        let base = maps
            .lines()
            .find_map(|line| {
                if !line.ends_with(path) || line.split_whitespace().nth(2) != Some("00000000") {
                    return None;
                }
                usize::from_str_radix(line.split('-').next()?, 16).ok()
            })
            .ok_or("Cannot verify native physics library mapping")?;
        Ok(Self {
            pointer,
            base,
            from_vertices: unsafe { member(pointer, base, 2, 0x23030)? },
            set_game_data: unsafe { member(pointer, base, 6, 0x20aa0)? },
            convex_free: unsafe { member(pointer, base, 7, 0x20c90)? },
            convert: unsafe { member(pointer, base, 15, 0x20980)? },
            soup_create: unsafe { member(pointer, base, 11, 0x23230)? },
            soup_destroy: unsafe { member(pointer, base, 12, 0x22590)? },
            soup_triangle: unsafe { member(pointer, base, 13, 0x21830)? },
            soup_convert: unsafe { member(pointer, base, 14, 0x23c40)? },
            destroy: unsafe { member(pointer, base, 17, 0x227b0)? },
            size: unsafe { member(pointer, base, 18, 0x21ce0)? },
            write: unsafe { member(pointer, base, 19, 0x20a90)? },
            unserialize: unsafe { member(pointer, base, 20, 0x23a00)? },
            volume: unsafe { member(pointer, base, 21, 0x23f90)? },
            query: unsafe { member(pointer, base, 43, 0x24180)? },
            destroy_query: unsafe { member(pointer, base, 44, 0x22860)? },
            _physics: physics,
            _tier0: tier0,
        })
    }

    fn convex(&self, shape: &Shape) -> Result<Convex<'_>, String> {
        let vertices = shape
            .corners
            .iter()
            .map(|point| Vector(*point))
            .collect::<Vec<_>>();
        let pointers = vertices
            .iter()
            .map(|vertex| vertex as *const Vector)
            .collect::<Vec<_>>();
        let pointer =
            unsafe { (self.from_vertices)(self.pointer, pointers.as_ptr(), pointers.len() as i32) };
        if pointer.is_null() {
            return Err(format!("Native physics rejected brush {}", shape.brush));
        }
        let convex = Convex { api: self, pointer };
        unsafe {
            (self.set_game_data)(self.pointer, convex.pointer, shape.brush);
        }
        Ok(convex)
    }

    fn body(&self, body: &Body) -> Result<Collide<'_>, String> {
        if let Some(mesh) = &body.mesh {
            return self.soup(mesh);
        }
        let mut convexes = body
            .shapes
            .iter()
            .map(|shape| self.convex(shape))
            .collect::<Result<Vec<_>, _>>()?;
        let pointers = convexes
            .iter()
            .map(|convex| convex.pointer)
            .collect::<Vec<_>>();
        // Conversion takes ownership of the convex inputs.
        for convex in &mut convexes {
            convex.pointer = std::ptr::null_mut();
        }
        let pointer =
            unsafe { (self.convert)(self.pointer, pointers.as_ptr(), pointers.len() as i32) };
        if pointer.is_null() {
            return Err("Native physics could not build the compound body".into());
        }
        Ok(Collide { api: self, pointer })
    }
}

struct Convex<'a> {
    api: &'a Api,
    pointer: Pointer,
}

impl Drop for Convex<'_> {
    fn drop(&mut self) {
        if !self.pointer.is_null() {
            unsafe {
                (self.api.convex_free)(self.api.pointer, self.pointer);
            }
        }
    }
}

struct Collide<'a> {
    api: &'a Api,
    pointer: Pointer,
}

impl Drop for Collide<'_> {
    fn drop(&mut self) {
        unsafe {
            (self.api.destroy)(self.api.pointer, self.pointer);
        }
    }
}

struct Query<'a> {
    body: &'a Collide<'a>,
    pointer: Pointer,
}

impl Drop for Query<'_> {
    fn drop(&mut self) {
        unsafe {
            (self.body.api.destroy_query)(self.body.api.pointer, self.pointer);
        }
    }
}

fn point(vector: Vector) -> DVec3 {
    DVec3::from_array(vector.0.map(f64::from))
}

impl Collide<'_> {
    fn verify(
        &self,
        body: &Body,
        material: i32,
        assign: bool,
        work: &mut usize,
    ) -> Result<f32, String> {
        if let Some(mesh) = &body.mesh {
            return self.verify_soup(mesh, material, work);
        }
        let volume = unsafe { (self.api.volume)(self.api.pointer, self.pointer) };
        if !volume.is_finite() || volume <= 0.0 {
            return Err("Native physics body has invalid volume".into());
        }
        let pointer = unsafe { (self.api.query)(self.api.pointer, self.pointer) };
        if pointer.is_null() {
            return Err("Native physics query could not be created".into());
        }
        let query = Query {
            body: self,
            pointer,
        };
        let count: Count = unsafe { member(pointer, self.api.base, 2, 0x20b00)? };
        let triangles: TriangleCount = unsafe { member(pointer, self.api.base, 3, 0x20b10)? };
        let data: GameData = unsafe { member(pointer, self.api.base, 4, 0x20b30)? };
        let vertices: Vertices = unsafe { member(pointer, self.api.base, 5, 0x20b50)? };
        let get_material: Material = unsafe { member(pointer, self.api.base, 7, 0x20bf0)? };
        let set_material: SetMaterial = unsafe { member(pointer, self.api.base, 8, 0x20c20)? };
        if unsafe { count(pointer) } != body.shapes.len() as i32 {
            return Err("Native physics changed the number of convex hulls".into());
        }
        let shapes: BTreeMap<_, _> = body
            .shapes
            .iter()
            .map(|shape| (shape.brush, shape))
            .collect();
        let mut seen = BTreeSet::new();
        for index in 0..body.shapes.len() as i32 {
            let brush = unsafe { data(pointer, index) };
            let shape = shapes
                .get(&brush)
                .filter(|_| seen.insert(brush))
                .ok_or("Native physics changed convex brush references")?;
            let triangle_count = unsafe { triangles(pointer, index) };
            if !(4..=MAX_TRIANGLES).contains(&triangle_count) {
                return Err("Native physics has an invalid convex triangle count".into());
            }
            let center = shape
                .corners
                .iter()
                .map(|p| DVec3::from_array(p.map(f64::from)))
                .sum::<DVec3>()
                / shape.corners.len() as f64;
            for triangle in 0..triangle_count {
                *work += shape.planes.len() * 7 + shape.corners.len();
                if *work > MAX_GEOMETRY_WORK {
                    return Err("Physics hull verification exceeded its work limit".into());
                }
                let mut corners = [Vector::default(); 3];
                unsafe {
                    vertices(pointer, index, triangle, corners.as_mut_ptr());
                }
                let corners = corners.map(point);
                for corner in corners {
                    if !corner.is_finite()
                        || shape.planes.iter().any(|plane| {
                            let normal = DVec3::from_array([
                                plane[0] as f64,
                                plane[1] as f64,
                                plane[2] as f64,
                            ]);
                            normal.dot(corner)
                                > plane[3] as f64 + GEOMETRY_EPSILON * normal.length()
                        })
                    {
                        return Err(format!(
                            "Native physics expanded brush {brush} beyond its checked hull"
                        ));
                    }
                }
                let mut normal = (corners[1] - corners[0])
                    .cross(corners[2] - corners[0])
                    .normalize();
                if !normal.is_finite() {
                    return Err("Native physics emitted a degenerate triangle".into());
                }
                if normal.dot(center - corners[0]) > 0.0 {
                    normal = -normal;
                }
                if shape.corners.iter().any(|corner| {
                    normal.dot(DVec3::from_array(corner.map(f64::from)) - corners[0])
                        > GEOMETRY_EPSILON
                }) {
                    return Err(format!(
                        "Native physics cropped brush {brush} beyond the geometry tolerance"
                    ));
                }
                let material = triangle_material(shape, corners, normal, material)?;
                if assign {
                    unsafe {
                        set_material(pointer, index, triangle, material);
                    }
                }
                if unsafe { get_material(pointer, index, triangle) } != material {
                    return Err("Native physics did not retain the authored surface mapping".into());
                }
            }
        }
        drop(query);
        Ok(volume)
    }

    fn serialize(&self) -> Result<Vec<u8>, String> {
        let size = unsafe { (self.api.size)(self.api.pointer, self.pointer) };
        if size < 28 || size as usize > MAX_COLLISION_BYTES {
            return Err("Native physics serialization exceeds supported bounds".into());
        }
        let size = size as usize;
        let mut guarded = vec![0xa5; size + 2 * GUARD_BYTES];
        let written = unsafe {
            (self.api.write)(
                self.api.pointer,
                guarded.as_mut_ptr().add(GUARD_BYTES),
                self.pointer,
                false,
            )
        };
        if written != size as i32
            || guarded[..GUARD_BYTES]
                .iter()
                .chain(&guarded[GUARD_BYTES + size..])
                .any(|&byte| byte != 0xa5)
        {
            return Err("Native physics overwrote its serialization bounds".into());
        }
        let data = guarded[GUARD_BYTES..GUARD_BYTES + size].to_vec();
        if !matches!(
            PhysicsSolid::parse(&data)
                .map_err(|error| error.to_string())?
                .header(),
            CollisionHeader::Convex { .. }
        ) {
            return Err("Native physics produced an unexpected VPHY body type".into());
        }
        Ok(data)
    }
}

fn triangle_material(
    shape: &crate::physics::Shape,
    corners: [DVec3; 3],
    normal: DVec3,
    fallback: i32,
) -> Result<i32, String> {
    if shape.materials.is_empty() {
        return Ok(fallback);
    }
    if shape
        .materials
        .iter()
        .all(|&index| index == shape.materials[0])
    {
        return Ok(i32::from(shape.materials[0]));
    }
    let mut best = None;
    for (plane, &material) in shape.planes.iter().zip(&shape.materials) {
        let axis = DVec3::new(plane[0] as f64, plane[1] as f64, plane[2] as f64);
        let alignment = axis.normalize().dot(normal);
        if alignment < 0.99
            || corners.iter().any(|&point| {
                (axis.dot(point) - plane[3] as f64).abs() > GEOMETRY_EPSILON * axis.length()
            })
        {
            continue;
        }
        if best.is_none_or(|(score, _)| alignment > score) {
            best = Some((alignment, material));
        }
    }
    best.map(|(_, material)| i32::from(material))
        .ok_or_else(|| {
            format!(
                "Native physics triangle cannot be matched to brush {} surface properties",
                shape.brush
            )
        })
}

pub(super) fn limits() -> io::Result<()> {
    let memory = libc::rlimit {
        rlim_cur: 24 * 1024 * 1024 * 1024,
        rlim_max: 24 * 1024 * 1024 * 1024,
    };
    let core = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_AS, &memory) } != 0
        || unsafe { libc::setrlimit(libc::RLIMIT_CORE, &core) } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn write(request: &Request, directory: &Path, output: &Path) -> Result<(), String> {
    limits().map_err(|error| error.to_string())?;
    let api = Api::load(directory)?;
    let mut mass_api = None;
    let mut records = Vec::new();
    let mut work = 0;
    let mut bytes = 16;
    for model in &request.models {
        let mut solids = Vec::new();
        let mut keys = String::new();
        for (index, body) in model.bodies.iter().enumerate() {
            let collide = api.body(body)?;
            let material = if model.index == 0 && body.surface.is_none() {
                1
            } else {
                if mass_api.is_none() {
                    mass_api = Some(mass::MassApi::load(&api, &request.surface_data)?);
                }
                0
            };
            let volume = collide.verify(body, material, true, &mut work)?;
            let mut solid = collide.serialize()?;
            let pointer = unsafe {
                (api.unserialize)(
                    api.pointer,
                    solid.as_mut_ptr(),
                    solid.len() as i32,
                    index as i32,
                )
            };
            if pointer.is_null() {
                return Err("Native physics could not reload its checked generated body".into());
            }
            let loaded = Collide { api: &api, pointer };
            loaded.verify(body, material, false, &mut work)?;
            drop(loaded);
            bytes += 4 + solid.len();
            if bytes > source_formats::bsp::MAX_PHYSICS_BYTES {
                return Err("Native physics data exceeds its 64 MiB limit".into());
            }
            if let Some(surface) = body.surface {
                let property = if body.contents == source_formats::bsp::contents::SLIME {
                    "slime"
                } else {
                    "water"
                };
                keys.push_str(&format!("fluid {{\n\"index\" \"{index}\"\n\"surfaceprop\" \"{property}\"\n\"damping\" \"0.01\"\n\"contents\" \"{}\"\n\"surfaceplane\" \"0 0 1 {surface}\"\n\"currentvelocity\" \"0 0 0\"\n}}\n", body.contents));
            } else if model.index == 0 {
                keys.push_str(&format!(
                    "staticsolid {{\n\"index\" \"{index}\"\n\"contents\" \"{}\"\n}}\n",
                    body.contents
                ));
            } else {
                let property = body
                    .surface_property
                    .as_deref()
                    .ok_or("Brush-model mass is missing its dominant surface property")?;
                let area = body
                    .mass_area
                    .ok_or("Brush-model mass is missing its emitted area")?;
                let mass = mass_api
                    .as_ref()
                    .ok_or("Brush-model mass surface database is unavailable")?
                    .mass(property, volume, area)?;
                keys.push_str(&format!("solid {{\n\"index\" \"{index}\"\n\"mass\" \"{mass}\"\n\"surfaceprop\" \"{property}\"\n\"volume\" \"{volume}\"\n}}\n"));
            }
            solids.push(solid);
        }
        if model.index == 0 {
            keys.push_str("materialtable {\n");
            for (index, name) in request.materials.iter().enumerate() {
                keys.push_str(&format!("\"{name}\" \"{}\"\n", index + 1));
            }
            keys.push_str("}\n");
        }
        keys.push('\0');
        bytes += 16 + keys.len();
        records.push((model.index, solids, keys));
    }
    let models = records
        .iter()
        .map(|(index, solids, keys)| {
            Ok(PhysicsModel {
                index: *index,
                solids: solids
                    .iter()
                    .map(|data| PhysicsSolid::parse(data).map_err(|error| error.to_string()))
                    .collect::<Result<Vec<_>, _>>()?,
                key_data: keys.as_bytes(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let data = PhysicsCollision { models }
        .to_bytes(request.model_count)
        .map_err(|error| error.to_string())?;
    std::fs::write(output, data).map_err(|error| error.to_string())
}
