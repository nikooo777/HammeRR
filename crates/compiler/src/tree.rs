use std::collections::BTreeSet;

use glam::DVec3;
use source_formats::bsp::contents;

use crate::Control;
use crate::map::MapBrush;
use crate::planes::PlaneSet;
use crate::winding::{ON_EPSILON, Side, Winding};

const BOUNDS_PADDING: f64 = 8.0;
const MAX_TREE_NODES: usize = 65_535;

#[derive(Debug, Clone)]
pub struct Surface {
    pub plane: usize,
    pub winding: Winding,
    tested: bool,
}

#[derive(Debug, Clone)]
pub struct Polyhedron {
    pub sides: Vec<Surface>,
}

impl Polyhedron {
    pub fn from_brush(brush: &MapBrush) -> Self {
        Self {
            sides: brush
                .sides
                .iter()
                .filter(|s| !s.bevel)
                .filter_map(|s| {
                    Some(Surface {
                        plane: s.plane,
                        winding: s.winding.clone()?,
                        tested: false,
                    })
                })
                .collect(),
        }
    }

    fn bounding_box(mins: DVec3, maxs: DVec3, planes: &mut PlaneSet) -> Self {
        let mut indices = Vec::with_capacity(6);
        for axis in 0..3 {
            let mut normal = DVec3::ZERO;
            normal[axis] = 1.0;
            indices.push(planes.find(normal, maxs[axis]));
            indices.push(planes.find(-normal, -mins[axis]));
        }
        let sides = indices
            .iter()
            .map(|&index| {
                let plane = planes.get(index);
                let mut winding = Winding::base(plane.normal, plane.dist);
                for &other in &indices {
                    if other == index {
                        continue;
                    }
                    let clip = planes.get(other);
                    winding = winding.clip_back(clip.normal, clip.dist).unwrap();
                }
                Surface {
                    plane: index,
                    winding,
                    tested: true,
                }
            })
            .collect();
        Self { sides }
    }

    pub fn bounds(&self) -> (DVec3, DVec3) {
        self.sides
            .iter()
            .flat_map(|s| s.winding.0.iter())
            .fold((DVec3::MAX, DVec3::MIN), |(lo, hi), &p| {
                (lo.min(p), hi.max(p))
            })
    }

    pub fn classify(&self, index: usize, planes: &PlaneSet) -> Side {
        let plane = planes.get(index);
        let (mut front, mut back) = (false, false);
        for p in self.sides.iter().flat_map(|s| &s.winding.0) {
            let d = plane.distance(*p);
            front |= d > ON_EPSILON;
            back |= d < -ON_EPSILON;
        }
        match (front, back) {
            (true, true) => Side::Cross,
            (true, false) => Side::Front,
            (false, true) => Side::Back,
            (false, false) => Side::On,
        }
    }

    pub fn split(&self, index: usize, planes: &PlaneSet) -> [Option<Self>; 2] {
        let mut shape = self.clone();
        for side in &mut shape.sides {
            if side.plane & !1 == index & !1 {
                side.tested = true;
            }
        }
        match shape.classify(index, planes) {
            Side::Front => return [Some(shape), None],
            Side::Back => return [None, Some(shape)],
            Side::On => return [None, None],
            Side::Cross => {}
        }
        let plane = planes.get(index);
        let mut cap = Some(Winding::base(plane.normal, plane.dist));
        for side in &shape.sides {
            let clip = planes.get(side.plane);
            cap = cap.and_then(|w| w.clip_back(clip.normal, clip.dist));
        }
        let Some(cap) = cap else {
            return [None, None];
        };
        let mut halves = [Vec::new(), Vec::new()];
        for side in &shape.sides {
            let (front, back) = side.winding.split(plane.normal, plane.dist);
            for (half, winding) in halves.iter_mut().zip([front, back]) {
                if let Some(winding) = winding {
                    half.push(Surface {
                        winding,
                        ..side.clone()
                    });
                }
            }
        }
        halves[0].push(Surface {
            plane: index ^ 1,
            winding: cap.reversed(),
            tested: true,
        });
        halves[1].push(Surface {
            plane: index,
            winding: cap,
            tested: true,
        });
        halves.map(|sides| (sides.len() >= 4).then_some(Self { sides }))
    }
}

#[derive(Debug)]
pub struct Node {
    pub split: Option<(usize, [usize; 2])>,
    pub volume: Polyhedron,
    pub contents: i32,
    pub brushes: Vec<usize>,
}

impl Node {
    pub fn solid(&self) -> bool {
        self.contents & contents::SOLID != 0
    }
}

#[derive(Debug)]
pub struct Tree {
    pub nodes: Vec<Node>,
}

#[derive(Clone)]
struct Fragment {
    brush: usize,
    contents: i32,
    shape: Polyhedron,
}

fn choose_plane(
    fragments: &[Fragment],
    planes: &PlaneSet,
    control: &Control,
) -> Result<Option<usize>, String> {
    let candidates: BTreeSet<usize> = fragments
        .iter()
        .flat_map(|f| &f.shape.sides)
        .filter(|s| !s.tested)
        .map(|s| s.plane & !1)
        .collect();
    let mut best = None;
    let mut best_score = i64::MIN;
    for candidate in candidates {
        control.checkpoint()?;
        let (mut front, mut back, mut splits, mut coplanar) = (0i64, 0i64, 0i64, 0i64);
        for fragment in fragments {
            coplanar += fragment
                .shape
                .sides
                .iter()
                .filter(|s| !s.tested && s.plane & !1 == candidate)
                .count() as i64;
            match fragment.shape.classify(candidate, planes) {
                Side::Front => front += 1,
                Side::Back => back += 1,
                Side::Cross => splits += 1,
                Side::On => {}
            }
        }
        let axial = i64::from(planes.get(candidate).kind < 3);
        let score = 5 * coplanar - 5 * splits - (front - back).abs() + 5 * axial;
        if score > best_score {
            best = Some(candidate);
            best_score = score;
        }
    }
    Ok(best)
}

impl Tree {
    pub fn build(
        brushes: &[MapBrush],
        indices: &[usize],
        planes: &mut PlaneSet,
    ) -> Result<Self, String> {
        Self::build_controlled(brushes, indices, planes, &Control::default())
    }

    pub fn build_controlled(
        brushes: &[MapBrush],
        indices: &[usize],
        planes: &mut PlaneSet,
        control: &Control,
    ) -> Result<Self, String> {
        control.checkpoint()?;
        if indices.is_empty() {
            return Err("no brushes to build a BSP tree".into());
        }
        let (mins, maxs) = indices
            .iter()
            .fold((DVec3::MAX, DVec3::MIN), |(lo, hi), &index| {
                (lo.min(brushes[index].mins), hi.max(brushes[index].maxs))
            });
        let volume = Polyhedron::bounding_box(
            mins - DVec3::splat(BOUNDS_PADDING),
            maxs + DVec3::splat(BOUNDS_PADDING),
            planes,
        );
        let fragments: Vec<Fragment> = indices
            .iter()
            .map(|&brush| Fragment {
                brush,
                contents: brushes[brush].contents,
                shape: Polyhedron::from_brush(&brushes[brush]),
            })
            .collect();
        let mut tree = Self {
            nodes: vec![Node {
                split: None,
                volume,
                contents: contents::EMPTY,
                brushes: Vec::new(),
            }],
        };
        let mut pending = vec![(0, fragments)];
        while let Some((index, fragments)) = pending.pop() {
            control.checkpoint()?;
            let Some(plane) = choose_plane(&fragments, planes, control)? else {
                let node = &mut tree.nodes[index];
                node.contents = fragments.iter().fold(0, |c, f| c | f.contents);
                node.brushes = fragments.iter().map(|f| f.brush).collect();
                node.brushes.sort_unstable();
                node.brushes.dedup();
                continue;
            };
            if tree.nodes.len() + 2 > MAX_TREE_NODES {
                return Err("BSP tree exceeds the node limit".into());
            }
            let volumes = tree.nodes[index].volume.split(plane, planes);
            let [Some(front_volume), Some(back_volume)] = volumes else {
                return Err("BSP split produced a degenerate volume".into());
            };
            let mut halves = [Vec::new(), Vec::new()];
            for fragment in fragments {
                for (half, shape) in halves.iter_mut().zip(fragment.shape.split(plane, planes)) {
                    if let Some(shape) = shape {
                        half.push(Fragment {
                            brush: fragment.brush,
                            contents: fragment.contents,
                            shape,
                        });
                    }
                }
            }
            let children = [tree.nodes.len(), tree.nodes.len() + 1];
            tree.nodes[index].split = Some((plane, children));
            for volume in [front_volume, back_volume] {
                tree.nodes.push(Node {
                    split: None,
                    volume,
                    contents: contents::EMPTY,
                    brushes: Vec::new(),
                });
            }
            let [front, back] = halves;
            pending.push((children[1], back));
            pending.push((children[0], front));
        }
        Ok(tree)
    }

    pub fn leaf_at(&self, point: DVec3, planes: &PlaneSet) -> usize {
        let mut index = 0;
        while let Some((plane, children)) = self.nodes[index].split {
            index = children[usize::from(planes.get(plane).distance(point) < 0.0)];
        }
        index
    }

    pub fn preorder(&self) -> Vec<usize> {
        let mut order = Vec::new();
        let mut pending = vec![0];
        while let Some(index) = pending.pop() {
            order.push(index);
            if let Some((_, children)) = self.nodes[index].split {
                pending.push(children[1]);
                pending.push(children[0]);
            }
        }
        order
    }

    pub fn prune(&mut self) {
        for index in self.preorder().into_iter().rev() {
            let Some((_, children)) = self.nodes[index].split else {
                continue;
            };
            if children
                .iter()
                .all(|&c| self.nodes[c].split.is_none() && self.nodes[c].solid())
            {
                let mut brushes = self.nodes[children[0]].brushes.clone();
                brushes.extend_from_slice(&self.nodes[children[1]].brushes);
                brushes.sort_unstable();
                brushes.dedup();
                let contents = self.nodes[children[0]].contents | self.nodes[children[1]].contents;
                self.nodes[index].split = None;
                self.nodes[index].contents = contents;
                self.nodes[index].brushes = brushes;
            }
        }
    }

    pub fn brush_leaves(&self, brush: &MapBrush, planes: &PlaneSet) -> Vec<usize> {
        let mut leaves = Vec::new();
        let mut pending = vec![(0, Polyhedron::from_brush(brush))];
        while let Some((index, shape)) = pending.pop() {
            if let Some((plane, children)) = self.nodes[index].split {
                for (child, half) in children.into_iter().zip(shape.split(plane, planes)) {
                    if let Some(half) = half {
                        pending.push((child, half));
                    }
                }
            } else {
                leaves.push(index);
            }
        }
        leaves
    }
}
