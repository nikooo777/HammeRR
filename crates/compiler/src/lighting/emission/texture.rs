use std::collections::{BTreeMap, btree_map::Entry};

use glam::{DVec2, DVec3};
use source_formats::gamefs::GameFs;
use source_formats::vmt::Material;
use source_formats::vtf::{ImageFormat, RgbaImage, Vtf};

use super::{Allocation, Budget};
use crate::asset_sources::{BuildMaterialSources, CapturedBuildAsset};

const MAX_ENCODED_BYTES: usize = 64 * 1024 * 1024 + 4096;
const MAX_TEXTURE_EDGE: u32 = 4096;
const MAX_IMAGE_RECTANGLES: usize = 65_536;
const MAX_RECTANGLE_VISITS: usize = 1_048_576;
const MAX_VISITED_TEXELS: usize = 8_388_608;
const MAX_ABS_UV: f64 = 16_777_216.0;
const INITIAL_CAPACITY: usize = 4;
const CLAMP_S: u32 = 0x4;
const CLAMP_T: u32 = 0x8;
const DATA_FLAGS: u32 = 0x80 | 0x800 | 0x4000 | 0x8000 | 0x1_0000 | 0x800_0000;

#[derive(Clone, Copy)]
struct Rectangle {
    x: [usize; 2],
    y: [usize; 2],
    rgb: [u8; 3],
}

pub(super) struct Texture {
    dimensions: [usize; 2],
    clamp: [bool; 2],
    rectangles: Vec<Rectangle>,
    _image: RgbaImage,
    _asset: CapturedBuildAsset,
    _allocation: Allocation,
}

impl Texture {
    pub(super) fn texels(&self) -> usize {
        self.dimensions[0] * self.dimensions[1]
    }

    pub(super) fn rectangles(&self) -> usize {
        self.rectangles.len()
    }
}

fn parameters(material: &Material) -> Result<String, String> {
    if !matches!(
        material.shader.as_str(),
        "lightmappedgeneric" | "unlitgeneric"
    ) || !material.blocks.is_empty()
    {
        return Err("Texture emission requires static opaque LightmappedGeneric or UnlitGeneric without conditional or animated blocks".into());
    }
    for (key, value) in &material.params {
        if key.contains("transform")
            || matches!(
                key.as_str(),
                "$basetexture2"
                    | "$albedo"
                    | "$seamless_scale"
                    | "$seamless_detail"
                    | "$seamless_base"
            )
        {
            return Err(format!(
                "Texture emission does not support material parameter {key}"
            ));
        }
        if key == "$frame" {
            let frame = value
                .trim()
                .parse::<f64>()
                .map_err(|_| "Texture emission has invalid $frame")?;
            if frame != 0.0 {
                return Err("Texture emission requires $frame zero".into());
            }
        }
    }
    let base = material
        .get("$basetexture")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or("Texture emission requires an explicit nonblank $basetexture")?;
    super::validate_material_emission_file(base)?;
    let path = source_formats::vmt::texture_path(base);
    super::validate_material_emission_file(&path)?;
    Ok(path)
}

pub(super) fn load(
    material: &Material,
    fs: &GameFs,
    sources: &BuildMaterialSources,
    budget: &mut Budget<'_>,
) -> Result<Texture, String> {
    let path = parameters(material)?;
    let read_allocation = budget.reserve_bytes(MAX_ENCODED_BYTES)?;
    budget.consume(1)?;
    let asset = sources
        .read_asset(fs, &path, MAX_ENCODED_BYTES)
        .map_err(|error| format!("Texture emission {path}: {error}"))?
        .ok_or_else(|| format!("Texture emission base texture {path} was not found"))?;
    drop(read_allocation);
    let mut allocation = budget.reserve_bytes(asset.bytes().len() + size_of::<Texture>() + 128)?;
    let _encoded_clone = budget.reserve_bytes(asset.bytes().len())?;
    budget.consume(asset.bytes().len().div_ceil(64))?;
    let vtf = Vtf::parse(asset.bytes().to_vec())
        .map_err(|error| format!("Texture emission {path}: {error}"))?;
    if vtf.authored_frames != 1
        || vtf.frames != 1
        || vtf.faces != 1
        || vtf.depth != 1
        || vtf.authored_depth.is_some_and(|depth| depth != 1)
        || vtf.first_frame != 0
        || vtf.flags & DATA_FLAGS != 0
    {
        return Err(format!(
            "Texture emission {path} requires one static ordinary RGB frame, face and depth; animated, normal/data, procedural and render-target textures are unsupported"
        ));
    }
    if vtf.width > MAX_TEXTURE_EDGE || vtf.height > MAX_TEXTURE_EDGE {
        return Err(format!(
            "Texture emission {path} exceeds the 4096-pixel edge limit"
        ));
    }
    if matches!(
        vtf.format,
        ImageFormat::I8
            | ImageFormat::Ia88
            | ImageFormat::A8
            | ImageFormat::Uv88
            | ImageFormat::Uvwq8888
            | ImageFormat::Uvlx8888
            | ImageFormat::Rgba16161616F
            | ImageFormat::Rgba16161616
    ) {
        return Err(format!(
            "Texture emission {path} does not support HDR, intensity, alpha-only or data image formats"
        ));
    }
    let dimensions = [vtf.width as usize, vtf.height as usize];
    let pixels = dimensions[0] * dimensions[1];
    allocation.grow(budget, pixels * 4)?;
    budget.consume(pixels)?;
    let image = vtf
        .decode_rgba8(0, 0, 0)
        .map_err(|error| format!("Texture emission {path}: {error}"))?;
    budget.consume(0)?;
    if image.pixels.len() != pixels * 4 {
        return Err(format!(
            "Texture emission {path} decoded an inconsistent mip0 image"
        ));
    }
    let rectangles = rectangles(&image, budget, &mut allocation)?;
    Ok(Texture {
        dimensions,
        clamp: [vtf.flags & CLAMP_S != 0, vtf.flags & CLAMP_T != 0],
        rectangles,
        _image: image,
        _asset: asset,
        _allocation: allocation,
    })
}

fn rectangles(
    image: &RgbaImage,
    budget: &mut Budget<'_>,
    allocation: &mut Allocation,
) -> Result<Vec<Rectangle>, String> {
    let width = image.width as usize;
    let _rows = budget.reserve_bytes(width * 256)?;
    let mut rectangles: Vec<Rectangle> = Vec::new();
    let mut previous: BTreeMap<(usize, usize, [u8; 3]), usize> = BTreeMap::new();
    for y in 0..image.height as usize {
        let mut current = BTreeMap::new();
        let mut x = 0;
        while x < width {
            budget.consume(1)?;
            let begin = x;
            let pixel = (y * width + x) * 4;
            let rgb: [u8; 3] = image.pixels[pixel..pixel + 3].try_into().unwrap();
            x += 1;
            while x < width {
                budget.consume(1)?;
                let pixel = (y * width + x) * 4;
                if image.pixels[pixel..pixel + 3] != rgb {
                    break;
                }
                x += 1;
            }
            let key = (begin, x, rgb);
            let index = if let Some(&index) = previous.get(&key) {
                rectangles[index].y[1] = y + 1;
                index
            } else {
                if rectangles.len() == MAX_IMAGE_RECTANGLES {
                    return Err(
                        "Texture emission exceeds 65536 constant-color image rectangles".into(),
                    );
                }
                grow(&mut rectangles, budget, allocation)?;
                rectangles.push(Rectangle {
                    x: [begin, x],
                    y: [y, y + 1],
                    rgb,
                });
                rectangles.len() - 1
            };
            current.insert(key, index);
        }
        previous = current;
    }
    Ok(rectangles)
}

fn grow<T>(
    values: &mut Vec<T>,
    budget: &Budget<'_>,
    allocation: &mut Allocation,
) -> Result<(), String> {
    if values.len() == values.capacity() {
        let additional = values.capacity().max(INITIAL_CAPACITY);
        allocation.grow(budget, additional * size_of::<T>())?;
        values.reserve_exact(additional);
    }
    Ok(())
}

#[derive(Default)]
pub(super) struct Counters {
    pub(super) triangles: usize,
    visited_texels: usize,
    rectangle_visits: usize,
}

impl Counters {
    pub(super) fn triangles(&mut self, count: usize) -> Result<(), String> {
        self.triangles = self
            .triangles
            .checked_add(count)
            .filter(|count| *count <= super::MAX_TRIANGLES)
            .ok_or("Texture emission exceeds 262144 integration triangles")?;
        Ok(())
    }

    fn footprint(&mut self, count: usize) -> Result<(), String> {
        self.visited_texels = self
            .visited_texels
            .checked_add(count)
            .filter(|count| *count <= MAX_VISITED_TEXELS)
            .ok_or("Texture emission exceeds 8388608 projected texel visits")?;
        Ok(())
    }

    fn rectangles(&mut self, count: usize) -> Result<(), String> {
        self.rectangle_visits = self
            .rectangle_visits
            .checked_add(count)
            .filter(|count| *count <= MAX_RECTANGLE_VISITS)
            .ok_or("Texture emission exceeds 1048576 candidate rectangle visits")?;
        Ok(())
    }
}

type Fragments = BTreeMap<[u8; 3], Vec<[DVec3; 3]>>;

#[derive(Clone, Copy, PartialEq)]
struct Vertex {
    point: DVec3,
    uv: DVec2,
}

fn append_vertex(output: &mut Vec<Vertex>, vertex: Vertex) {
    if output.last() != Some(&vertex) {
        output.push(vertex);
    }
}

fn intersection(a: Vertex, b: Vertex, axis: usize, boundary: f64) -> Result<Vertex, String> {
    let fraction = (boundary - a.uv[axis]) / (b.uv[axis] - a.uv[axis]);
    if !fraction.is_finite()
        || !(0.0..=1.0).contains(&fraction)
        || (fraction == 0.0 && boundary != a.uv[axis])
        || (fraction == 1.0 && boundary != b.uv[axis])
    {
        return Err("Texture emission clipping cannot represent its UV intersection".into());
    }
    if fraction == 0.0 {
        return Ok(a);
    }
    if fraction == 1.0 {
        return Ok(b);
    }
    let point = a.point + (b.point - a.point) * fraction;
    let mut uv = a.uv + (b.uv - a.uv) * fraction;
    uv[axis] = boundary;
    if !point.is_finite() || !uv.is_finite() || point == a.point || point == b.point {
        return Err("Texture emission clipping lost native world-coordinate precision".into());
    }
    Ok(Vertex { point, uv })
}

fn clip(
    polygon: &[Vertex],
    output: &mut Vec<Vertex>,
    axis: usize,
    boundary: f64,
    lower: bool,
) -> Result<(), String> {
    output.clear();
    for index in 0..polygon.len() {
        let a = polygon[index];
        let b = polygon[(index + 1) % polygon.len()];
        let inside_a = if lower {
            a.uv[axis] >= boundary
        } else {
            a.uv[axis] <= boundary
        };
        let inside_b = if lower {
            b.uv[axis] >= boundary
        } else {
            b.uv[axis] <= boundary
        };
        if inside_a {
            append_vertex(output, a);
        }
        if inside_a != inside_b {
            append_vertex(output, intersection(a, b, axis, boundary)?);
        }
    }
    if output.len() > 1 && output.first() == output.last() {
        output.pop();
    }
    if output.len() > 12 {
        return Err("Texture emission clipping exceeded its bounded convex polygon".into());
    }
    Ok(())
}

fn axis_overlap(bounds: [f64; 2], region: [f64; 2]) -> bool {
    if bounds[0] == bounds[1] {
        bounds[0] >= region[0] && bounds[0] < region[1]
    } else {
        bounds[1] > region[0] && bounds[0] < region[1]
    }
}

fn world_area(points: [DVec3; 3]) -> f64 {
    (points[1] - points[0])
        .cross(points[2] - points[0])
        .length()
        * 0.5
}

pub(super) fn partition(
    texture: &Texture,
    triangles: Vec<[DVec3; 3]>,
    coordinates: &[[DVec2; 3]],
    counters: &mut Counters,
    budget: &mut Budget<'_>,
    allocation: &mut Allocation,
) -> Result<Fragments, String> {
    if triangles.len() != coordinates.len() {
        return Err("Texture emission source lost its native texture coordinates".into());
    }
    for uv in coordinates.iter().flatten() {
        budget.consume(1)?;
        if !uv.is_finite() || uv.abs().max_element() > MAX_ABS_UV {
            return Err(
                "Texture emission UV coordinates must be finite and within ±16777216 texels".into(),
            );
        }
    }
    allocation.grow(budget, 256)?;
    if let [rectangle] = texture.rectangles.as_slice() {
        counters.triangles(triangles.len())?;
        return Ok(BTreeMap::from([(rectangle.rgb, triangles)]));
    }
    let _clipping = budget.reserve_bytes(2 * 24 * size_of::<Vertex>())?;
    let mut polygon = Vec::with_capacity(24);
    let mut scratch = Vec::with_capacity(24);
    let mut fragments: Fragments = BTreeMap::new();
    for (points, coordinates) in triangles.into_iter().zip(coordinates) {
        budget.consume(1)?;
        let bounds: [[f64; 2]; 2] = std::array::from_fn(|axis| {
            [
                coordinates
                    .iter()
                    .map(|uv| uv[axis])
                    .fold(f64::INFINITY, f64::min),
                coordinates
                    .iter()
                    .map(|uv| uv[axis])
                    .fold(f64::NEG_INFINITY, f64::max),
            ]
        });
        let footprint: [usize; 2] = std::array::from_fn(|axis| {
            let [low, high] = bounds[axis];
            let first = low.floor();
            let last = if low == high {
                first
            } else {
                high.ceil() - 1.0
            };
            let clamp = |value: f64| {
                if texture.clamp[axis] {
                    value.clamp(0.0, (texture.dimensions[axis] - 1) as f64)
                } else {
                    value
                }
            };
            (clamp(last) - clamp(first) + 1.0) as usize
        });
        counters.footprint(
            footprint[0]
                .checked_mul(footprint[1])
                .ok_or("Texture emission texel footprint overflow")?,
        )?;
        let tiles: [[i64; 2]; 2] = std::array::from_fn(|axis| {
            if texture.clamp[axis] {
                [0, 0]
            } else {
                bounds[axis]
                    .map(|value| (value.floor() as i64).div_euclid(texture.dimensions[axis] as i64))
            }
        });
        let candidates = (tiles[0][1] - tiles[0][0] + 1) as usize;
        let candidates = candidates
            .checked_mul((tiles[1][1] - tiles[1][0] + 1) as usize)
            .and_then(|count| count.checked_mul(texture.rectangles.len()))
            .ok_or("Texture emission rectangle visit overflow")?;
        counters.rectangles(candidates)?;
        let original_area = world_area(points);
        let original_normal = (points[1] - points[0]).cross(points[2] - points[0]);
        let mut clipped_area = 0.0;
        let mut generated = 0usize;
        for y in tiles[1][0]..=tiles[1][1] {
            for x in tiles[0][0]..=tiles[0][1] {
                for rectangle in &texture.rectangles {
                    budget.consume(8)?;
                    let region: [[f64; 2]; 2] = std::array::from_fn(|axis| {
                        let extent = if axis == 0 { rectangle.x } else { rectangle.y };
                        let tile = if axis == 0 { x } else { y };
                        let offset = tile * texture.dimensions[axis] as i64;
                        if texture.clamp[axis] {
                            [
                                if extent[0] == 0 {
                                    f64::NEG_INFINITY
                                } else {
                                    extent[0] as f64
                                },
                                if extent[1] == texture.dimensions[axis] {
                                    f64::INFINITY
                                } else {
                                    extent[1] as f64
                                },
                            ]
                        } else {
                            extent.map(|value| (value as i64 + offset) as f64)
                        }
                    });
                    if !axis_overlap(bounds[0], region[0]) || !axis_overlap(bounds[1], region[1]) {
                        continue;
                    }
                    polygon.clear();
                    polygon.extend((0..3).map(|index| Vertex {
                        point: points[index],
                        uv: coordinates[index],
                    }));
                    for (axis, bounds) in region.into_iter().enumerate() {
                        for (boundary, lower) in [(bounds[0], true), (bounds[1], false)] {
                            if boundary.is_finite() {
                                budget.consume(polygon.len())?;
                                clip(&polygon, &mut scratch, axis, boundary, lower)?;
                                std::mem::swap(&mut polygon, &mut scratch);
                            }
                        }
                    }
                    if polygon.len() < 3 {
                        continue;
                    }
                    for pair in polygon[1..].windows(2) {
                        budget.consume(8)?;
                        let triangle = [polygon[0].point, pair[0].point, pair[1].point];
                        let area = world_area(triangle);
                        if area == 0.0 {
                            continue;
                        }
                        let normal = (triangle[1] - triangle[0]).cross(triangle[2] - triangle[0]);
                        if !area.is_finite() || normal.dot(original_normal) <= 0.0 {
                            return Err(
                                "Texture emission clipping lost a finite outward source triangle"
                                    .into(),
                            );
                        }
                        counters.triangles(1)?;
                        let group = match fragments.entry(rectangle.rgb) {
                            Entry::Occupied(entry) => entry.into_mut(),
                            Entry::Vacant(entry) => {
                                allocation.grow(budget, 256)?;
                                entry.insert(Vec::new())
                            }
                        };
                        grow(group, budget, allocation)?;
                        group.push(triangle);
                        clipped_area += area;
                        generated += 1;
                    }
                }
            }
        }
        let allowance =
            super::ARITHMETIC_OPERATIONS * f64::EPSILON * original_area * (generated + 1) as f64;
        if !clipped_area.is_finite() || (clipped_area - original_area).abs() > allowance {
            return Err(
                "Texture emission clipping cannot conserve the actual native source area".into(),
            );
        }
    }
    Ok(fragments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Control;

    #[test]
    fn image_runs_merge_rgb_without_treating_alpha_as_emission() {
        let control = Control::default();
        let mut budget = Budget::new(&control);
        let mut allocation = budget.reserve_bytes(0).unwrap();
        let image = RgbaImage {
            width: 4,
            height: 2,
            pixels: [
                [128, 0, 0, 0],
                [128, 0, 0, 255],
                [0, 0, 255, 0],
                [0, 0, 255, 255],
                [128, 0, 0, 255],
                [128, 0, 0, 0],
                [0, 0, 255, 255],
                [0, 0, 255, 0],
            ]
            .concat(),
        };
        let regions = rectangles(&image, &mut budget, &mut allocation).unwrap();
        assert_eq!(regions.len(), 2);
        assert_eq!(
            (regions[0].x, regions[0].y, regions[0].rgb),
            ([0, 2], [0, 2], [128, 0, 0])
        );
        assert_eq!(
            (regions[1].x, regions[1].y, regions[1].rgb),
            ([2, 4], [0, 2], [0, 0, 255])
        );
    }

    #[test]
    fn constant_texture_axes_own_integer_edges_once_and_keep_world_area() {
        for value in [-1.0, 0.0, 1.0] {
            let owners = (-2..3)
                .filter(|index| {
                    axis_overlap([value; 2], [f64::from(*index), f64::from(*index + 1)])
                })
                .count();
            assert_eq!(owners, 1);
            assert!(axis_overlap(
                [value; 2],
                [value.floor(), value.floor() + 1.0]
            ));
        }
        let mut polygon = vec![
            Vertex {
                point: DVec3::ZERO,
                uv: DVec2::new(1.0, 0.0),
            },
            Vertex {
                point: DVec3::X * 4.0,
                uv: DVec2::new(1.0, 4.0),
            },
            Vertex {
                point: DVec3::Y * 4.0,
                uv: DVec2::new(1.0, 0.0),
            },
        ];
        let mut scratch = Vec::new();
        for (axis, boundary, lower) in [
            (0, 1.0, true),
            (0, 2.0, false),
            (1, 1.0, true),
            (1, 3.0, false),
        ] {
            clip(&polygon, &mut scratch, axis, boundary, lower).unwrap();
            std::mem::swap(&mut polygon, &mut scratch);
        }
        let area: f64 = polygon[1..]
            .windows(2)
            .map(|pair| world_area([polygon[0].point, pair[0].point, pair[1].point]))
            .sum();
        assert_eq!(area, 4.0);
        for vertex in &mut polygon {
            vertex.uv = DVec2::new(-1.0, 1.0);
        }
        let original = polygon.clone();
        for (axis, boundary, lower) in [
            (0, -1.0, true),
            (0, 0.0, false),
            (1, 1.0, true),
            (1, 2.0, false),
        ] {
            clip(&polygon, &mut scratch, axis, boundary, lower).unwrap();
            std::mem::swap(&mut polygon, &mut scratch);
        }
        assert!(polygon == original);
    }

    #[test]
    fn clipping_refuses_a_world_intersection_that_rounds_to_its_endpoint() {
        let a = Vertex {
            point: DVec3::splat(1_000_000.0),
            uv: DVec2::new(f64::EPSILON, 0.0),
        };
        let b = Vertex {
            point: a.point + DVec3::X,
            uv: DVec2::new(-MAX_ABS_UV, 0.0),
        };
        let error = intersection(a, b, 0, 0.0).err().unwrap();
        assert!(error.contains("precision"));
    }

    #[test]
    fn image_partition_obeys_shared_work_and_cancellation() {
        let control = Control::default();
        let mut budget = Budget::with_limit(&control, 8);
        let mut allocation = budget.reserve_bytes(0).unwrap();
        let image = RgbaImage {
            width: 64,
            height: 1,
            pixels: vec![255; 64 * 4],
        };
        let error = rectangles(&image, &mut budget, &mut allocation)
            .err()
            .unwrap();
        assert!(error.contains("work-unit limit"));
        control.cancel().unwrap();
        let error = rectangles(&image, &mut budget, &mut allocation)
            .err()
            .unwrap();
        assert!(error.contains("cancelled"));
    }

    #[test]
    fn image_rectangle_cap_refuses_instead_of_merging_different_colors() {
        let control = Control::default();
        let mut budget = Budget::new(&control);
        let mut allocation = budget.reserve_bytes(0).unwrap();
        let mut image = RgbaImage {
            width: 257,
            height: 257,
            pixels: vec![0; 257 * 257 * 4],
        };
        for (index, pixel) in image.pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            pixel[0] = (index & 1) as u8;
        }
        let error = rectangles(&image, &mut budget, &mut allocation)
            .err()
            .unwrap();
        assert!(error.contains("65536"));
    }
}
