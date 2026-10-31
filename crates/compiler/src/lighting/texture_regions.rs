use std::collections::BTreeMap;

use super::bake::{Allocation, Budget};

pub(super) const MAX_RECTANGLES: usize = 65_536;
pub(super) const MAX_RECTANGLE_VISITS: usize = 1_048_576;
pub(super) const MAX_VISITED_TEXELS: usize = 8_388_608;
pub(super) const MAX_ABS_UV: f64 = 16_777_216.0;

#[derive(Clone, Copy)]
pub(super) struct Rectangle<T> {
    pub x: [usize; 2],
    pub y: [usize; 2],
    pub value: T,
}

pub(super) fn rectangles<T: Copy + Ord>(
    dimensions: [usize; 2],
    pixel: impl Fn(usize, usize) -> T,
    budget: &mut Budget<'_>,
    allocation: &mut Allocation,
) -> Result<Vec<Rectangle<T>>, String> {
    let [width, height] = dimensions;
    let _rows = budget.reserve_bytes(width * 256)?;
    let mut rectangles: Vec<Rectangle<T>> = Vec::new();
    let mut previous: BTreeMap<(usize, usize, T), usize> = BTreeMap::new();
    for y in 0..height {
        let mut current = BTreeMap::new();
        let mut x = 0;
        while x < width {
            budget.consume(1)?;
            let begin = x;
            let value = pixel(x, y);
            x += 1;
            while x < width {
                budget.consume(1)?;
                if pixel(x, y) != value {
                    break;
                }
                x += 1;
            }
            let key = (begin, x, value);
            let index = if let Some(&index) = previous.get(&key) {
                rectangles[index].y[1] = y + 1;
                index
            } else {
                if rectangles.len() == MAX_RECTANGLES {
                    return Err("Texture partition exceeds 65536 image rectangles".into());
                }
                if rectangles.len() == rectangles.capacity() {
                    let additional = rectangles.capacity().max(4);
                    let _reallocation = budget.reserve::<Rectangle<T>>(rectangles.capacity())?;
                    allocation.grow(budget, additional * size_of::<Rectangle<T>>())?;
                    rectangles
                        .try_reserve_exact(additional)
                        .map_err(|_| "Texture rectangles cannot be allocated")?;
                }
                rectangles.push(Rectangle {
                    x: [begin, x],
                    y: [y, y + 1],
                    value,
                });
                rectangles.len() - 1
            };
            current.insert(key, index);
        }
        previous = current;
    }
    Ok(rectangles)
}

pub(super) fn axis_overlap<T: Copy + PartialEq>(
    bounds: [T; 2],
    region: [T; 2],
    include_maximum: bool,
    compare: impl Fn(T, T) -> std::cmp::Ordering,
) -> bool {
    use std::cmp::Ordering::{Equal, Greater, Less};
    let order = |a, b| if a == b { Equal } else { compare(a, b) };
    if bounds[0] == bounds[1] {
        order(bounds[0], region[0]) != Less && order(bounds[0], region[1]) == Less
    } else {
        (order(bounds[1], region[0]) == Greater || (include_maximum && bounds[1] == region[0]))
            && order(bounds[0], region[1]) == Less
    }
}
