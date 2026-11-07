use crate::clip::Keep;
use crate::displacement::grid_clip;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Window {
    pub(super) row: usize,
    pub(super) column: usize,
    pub(super) row_cells: usize,
    pub(super) column_cells: usize,
    pub(super) output_power: u32,
}

fn square(child: grid_clip::Child, output_power: u32) -> Window {
    Window {
        row: child.row,
        column: child.column,
        row_cells: child.cells(),
        column_cells: child.cells(),
        output_power,
    }
}

fn retained(window: Window, rows: bool, cut: usize, first_back: bool, keep: Keep) -> bool {
    let coordinate = if rows { window.row } else { window.column };
    let back = (coordinate < cut) == first_back;
    !matches!((keep, back), (Keep::Back, false) | (Keep::Front, true))
}

pub(super) fn whole(power: u32) -> Option<Vec<Window>> {
    match power {
        2 => Some(vec![Window {
            row: 0,
            column: 0,
            row_cells: 4,
            column_cells: 4,
            output_power: 3,
        }]),
        3 | 4 => Some(
            grid_clip::quadrants(grid_clip::Child {
                row: 0,
                column: 0,
                power,
            })
            .into_iter()
            .map(|child| square(child, power))
            .collect(),
        ),
        _ => None,
    }
}

pub(super) fn cut_children(
    power: u32,
    rows: bool,
    cut: usize,
    first_back: bool,
    keep: Keep,
) -> Option<Vec<Window>> {
    if !matches!(power, 3 | 4) {
        return None;
    }
    let cells = 1usize << power;
    if !(1..cells).contains(&cut) {
        return None;
    }
    if let Some(children) = grid_clip::cut_children(power, rows, cut, first_back, keep) {
        return Some(
            children
                .into_iter()
                .map(|child| square(child, power))
                .collect(),
        );
    }
    let mut windows = Vec::new();
    for child in grid_clip::quadrants(grid_clip::Child {
        row: 0,
        column: 0,
        power,
    }) {
        let origin = if rows { child.row } else { child.column };
        if origin < cut && cut < origin + child.cells() {
            let band = child.cells() / 2;
            let row_ranges = if rows {
                [(child.row, cut), (cut, child.row + child.cells())]
            } else {
                [
                    (child.row, child.row + band),
                    (child.row + band, child.row + child.cells()),
                ]
            };
            let column_ranges = if rows {
                [
                    (child.column, child.column + band),
                    (child.column + band, child.column + child.cells()),
                ]
            } else {
                [(child.column, cut), (cut, child.column + child.cells())]
            };
            for (row, row_end) in row_ranges {
                for (column, column_end) in column_ranges {
                    let window = Window {
                        row,
                        column,
                        row_cells: row_end - row,
                        column_cells: column_end - column,
                        output_power: power - 1,
                    };
                    if retained(window, rows, cut, first_back, keep) {
                        windows.push(window);
                    }
                }
            }
        } else {
            let window = square(child, power);
            if retained(window, rows, cut, first_back, keep) {
                windows.push(window);
            }
        }
    }
    Some(windows)
}
