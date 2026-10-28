use std::collections::BTreeSet;

use glam::DVec3;
use source_formats::bsp::contents;

use crate::map::MapBrush;
use crate::planes::PlaneSet;
use crate::winding::{ON_EPSILON, Side, Winding};
use crate::{Control, Diagnostic, Report};

const BOUNDS_PADDING: f64 = 8.0;
const MAX_TREE_NODES: usize = 65_535;
const MAX_SPLIT_SOURCES: usize = 4;

mod budget;
use budget::Budget;
pub use budget::Limits;

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

    fn bounding_box(mins: DVec3, maxs: DVec3, planes: &mut PlaneSet) -> Result<Self, String> {
        if !mins.is_finite() || !maxs.is_finite() || !mins.cmplt(maxs).all() {
            return Err(
                "cannot construct BSP volume: bounds must be finite and have positive size".into(),
            );
        }
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
                    winding = winding.clip_back(clip.normal, clip.dist).ok_or_else(|| {
                        "cannot construct BSP volume: bounding planes enclose no supported volume".to_owned()
                    })?;
                }
                Ok(Surface {
                    plane: index,
                    winding,
                    tested: true,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self { sides })
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

    pub fn split(&self, index: usize, planes: &PlaneSet) -> Result<[Option<Self>; 2], String> {
        self.split_budgeted(index, planes, &mut Budget::unlimited())
            .map_err(|error| split_geometry(&error, None, self, index, planes))
    }

    fn split_budgeted(
        &self,
        index: usize,
        planes: &PlaneSet,
        budget: &mut Budget<'_>,
    ) -> Result<[Option<Self>; 2], String> {
        let points = self.sides.iter().map(|side| side.winding.0.len() + 2).sum();
        budget.charge(points, points)?;
        let mut shape = self.clone();
        for side in &mut shape.sides {
            if side.plane & !1 == index & !1 {
                side.tested = true;
            }
        }
        match shape.classify(index, planes) {
            Side::Front => return Ok([Some(shape), None]),
            Side::Back => return Ok([None, Some(shape)]),
            Side::On => return Ok([None, None]),
            Side::Cross => {}
        }
        let plane = planes.get(index);
        budget.charge(4, 4)?;
        let mut cap = Some(Winding::base(plane.normal, plane.dist));
        for side in &shape.sides {
            let clip = planes.get(side.plane);
            if let Some(winding) = &cap {
                budget.split(winding)?;
            }
            cap = cap.and_then(|w| w.clip_back(clip.normal, clip.dist));
        }
        let Some(cap) = cap else {
            return Err("cannot reconstruct a crossed BSP volume's split cap".into());
        };
        let mut halves = [Vec::new(), Vec::new()];
        for side in &shape.sides {
            budget.split(&side.winding)?;
            let (front, back) = side.winding.split(plane.normal, plane.dist);
            for (half, winding) in halves.iter_mut().zip([front, back]) {
                if let Some(winding) = winding {
                    budget.charge(side.winding.0.len() + 4, side.winding.0.len() + 4)?;
                    half.push(Surface {
                        winding,
                        ..side.clone()
                    });
                }
            }
        }
        budget.charge(cap.0.len() + 8, cap.0.len() + 8)?;
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
        if halves.iter().any(|sides| sides.len() < 4) {
            return Err("crossed BSP volume split produced an unsupported half".into());
        }
        Ok(halves.map(|sides| Some(Self { sides })))
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

struct BuildFailure {
    diagnostics: Vec<Diagnostic>,
}

impl From<String> for BuildFailure {
    fn from(message: String) -> Self {
        Self {
            diagnostics: vec![Diagnostic::error(message, None, None)],
        }
    }
}

impl From<&str> for BuildFailure {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

impl BuildFailure {
    fn message(&self) -> String {
        self.diagnostics[0].message.clone()
    }
}

fn split_geometry(
    error: &str,
    node: Option<usize>,
    volume: &Polyhedron,
    plane: usize,
    planes: &PlaneSet,
) -> String {
    let split = planes.get(plane);
    let (mins, maxs) = volume.bounds();
    let node = node.map_or_else(String::new, |index| format!("node:{index}; "));
    format!(
        "{error}; {node}split plane:{plane} normal {:?} dist {}; volume bounds {:?} to {:?}",
        split.normal.to_array(),
        split.dist,
        mins.to_array(),
        maxs.to_array(),
    )
}

struct SplitContext<'a> {
    index: usize,
    volume: &'a Polyhedron,
    plane: usize,
    planes: &'a PlaneSet,
    fragments: &'a [Fragment],
    brushes: &'a [MapBrush],
    hints: &'a [crate::hints::Hint],
}

impl SplitContext<'_> {
    fn volumes(&self, budget: &mut Budget<'_>) -> Result<[Polyhedron; 2], BuildFailure> {
        let volumes = self
            .volume
            .split_budgeted(self.plane, self.planes, budget)
            .map_err(|error| self.failure(error, None))?;
        let [Some(front), Some(back)] = volumes else {
            return Err(self.failure("BSP split produced a degenerate volume".into(), None));
        };
        Ok([front, back])
    }

    fn failure(&self, message: String, affected: Option<&MapBrush>) -> BuildFailure {
        let Self {
            index,
            volume,
            plane,
            planes,
            fragments,
            brushes,
            hints,
        } = *self;
        let sources = fragments
            .iter()
            .flat_map(|fragment| {
                let brush = &brushes[fragment.brush];
                brush.sides.iter().filter_map(move |side| {
                    (!side.bevel && side.plane & !1 == plane & !1)
                        .then_some(side.source_id)
                        .flatten()
                        .map(|side| (brush.id, side))
                })
            })
            .chain(hints.iter().filter_map(|hint| hint.plane_source(plane)))
            .collect::<BTreeSet<_>>();
        let examples = sources
            .iter()
            .take(MAX_SPLIT_SOURCES)
            .map(|(solid, side)| format!("solid:{solid} side:{side}"))
            .collect::<Vec<_>>();
        let mut message = split_geometry(&message, Some(index), volume, plane, planes);
        message.push_str(&format!(
            "; split-plane source candidates: {} authored sides, showing {} [{}]",
            sources.len(),
            examples.len(),
            examples.join(", "),
        ));
        let (mins, maxs) = volume.bounds();
        let mut position = (mins + maxs) * 0.5;
        let object = affected.map(|brush| format!("solid:{}", brush.id));
        if let Some(brush) = affected {
            message.push_str(&format!(
                "; affected brush bounds {:?} to {:?}",
                brush.mins.to_array(),
                brush.maxs.to_array()
            ));
            position = (brush.mins + brush.maxs) * 0.5;
        }
        if let Some(object) = &object {
            message = format!("{object}: {message}");
        }
        let mut diagnostics = vec![Diagnostic::error(message, object, Some(position))];
        let candidates = sources.iter().take(MAX_SPLIT_SOURCES).map(|&(solid, side)| {
            let message = format!(
                "Split plane:{plane} source candidate captured side:{side}; this side shares the chosen compiler plane"
            );
            Diagnostic::info(message, Some(format!("solid:{solid}")), Some(position))
        });
        diagnostics.extend(candidates);
        BuildFailure { diagnostics }
    }
}

fn choose_plane(
    fragments: &[Fragment],
    planes: &PlaneSet,
    control: &Control,
    hints: &[crate::hints::Fragment],
    volume: &Polyhedron,
    priority: &mut crate::hints::Priority,
    budget: &mut Budget<'_>,
) -> Result<Option<(usize, bool)>, String> {
    let sides = fragments.iter().map(|f| f.shape.sides.len()).sum();
    budget.charge(sides, sides * 2)?;
    let mut candidates: BTreeSet<usize> = fragments
        .iter()
        .flat_map(|f| &f.shape.sides)
        .filter(|s| !s.tested)
        .map(|s| s.plane & !1)
        .collect();
    let mut added_planes = BTreeSet::new();
    let mut hint_planes = BTreeSet::new();
    for hint in hints {
        priority.consume(volume.sides.iter().map(|side| side.winding.0.len()).sum())?;
        if volume.classify(hint.plane, planes) == Side::Cross {
            hint_planes.insert(hint.plane);
            if candidates.insert(hint.plane) {
                added_planes.insert(hint.plane);
            }
        }
    }
    let mut best = None;
    let mut best_score = (false, i64::MIN);
    for candidate in candidates {
        control.checkpoint()?;
        let (mut front, mut back, mut splits, mut coplanar) = (0i64, 0i64, 0i64, 0i64);
        for fragment in fragments {
            budget.charge(
                fragment
                    .shape
                    .sides
                    .iter()
                    .map(|side| side.winding.0.len() + 1)
                    .sum(),
                0,
            )?;
            if added_planes.contains(&candidate) {
                priority.consume(
                    fragment
                        .shape
                        .sides
                        .iter()
                        .map(|side| side.winding.0.len() + 1)
                        .sum(),
                )?;
            }
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
        let mut cuts_hint = false;
        if !hint_planes.contains(&candidate) {
            for hint in hints {
                control.checkpoint()?;
                priority.consume(hint.winding.0.len())?;
                let plane = planes.get(candidate);
                let front = hint
                    .winding
                    .0
                    .iter()
                    .any(|&point| plane.distance(point) > ON_EPSILON);
                let back = hint
                    .winding
                    .0
                    .iter()
                    .any(|&point| plane.distance(point) < -ON_EPSILON);
                cuts_hint |= front && back;
            }
        }
        let score = (
            !cuts_hint,
            5 * coplanar - 5 * splits - (front - back).abs() + 5 * axial,
        );
        if score > best_score {
            best = Some((candidate, added_planes.contains(&candidate)));
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
        Self::build_impl(brushes, indices, planes, &[], control, None)
            .map(|(tree, _)| tree)
            .map_err(|failure| failure.message())
    }

    pub fn build_limited(
        brushes: &[MapBrush],
        indices: &[usize],
        planes: &mut PlaneSet,
        control: &Control,
        limits: Limits,
    ) -> Result<Self, String> {
        Self::build_impl(brushes, indices, planes, &[], control, Some(limits))
            .map(|(tree, _)| tree)
            .map_err(|failure| failure.message())
    }

    pub(crate) fn build_with_hints(
        brushes: &[MapBrush],
        indices: &[usize],
        planes: &mut PlaneSet,
        hints: &[crate::hints::Hint],
        control: &Control,
        report: &mut Report,
    ) -> Result<(Self, crate::hints::Priority), String> {
        Self::build_impl(brushes, indices, planes, hints, control, None).map_err(|failure| {
            let message = failure.message();
            report.diagnostics.extend(failure.diagnostics);
            message
        })
    }

    fn build_impl(
        brushes: &[MapBrush],
        indices: &[usize],
        planes: &mut PlaneSet,
        hints: &[crate::hints::Hint],
        control: &Control,
        limits: Option<Limits>,
    ) -> Result<(Self, crate::hints::Priority), BuildFailure> {
        control.checkpoint()?;
        let mut budget = Budget::new(limits, control);
        budget.nodes(1)?;
        budget.pending(1)?;
        if indices.is_empty() {
            return Err("no brushes to build a BSP tree".into());
        }
        let (mins, maxs) = indices
            .iter()
            .fold((DVec3::MAX, DVec3::MIN), |(lo, hi), &index| {
                (lo.min(brushes[index].mins), hi.max(brushes[index].maxs))
            });
        budget.charge(6 * 6 * 64, 6 * 6 * 64)?;
        let volume = Polyhedron::bounding_box(
            mins - DVec3::splat(BOUNDS_PADDING),
            maxs + DVec3::splat(BOUNDS_PADDING),
            planes,
        )?;
        let points = indices
            .iter()
            .map(|&index| {
                brushes[index]
                    .sides
                    .iter()
                    .map(|side| side.winding.as_ref().map_or(0, |w| w.0.len()) + 4)
                    .sum::<usize>()
                    + 4
            })
            .sum();
        budget.charge(points, points)?;
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
        let mut priority = crate::hints::Priority::default();
        let source_hints = hints;
        let hints = priority.fragments(hints, &tree.nodes[0].volume, planes, control)?;
        let mut pending = vec![(0, fragments, hints)];
        while let Some((index, fragments, hints)) = pending.pop() {
            control.checkpoint()?;
            let Some((plane, added)) = choose_plane(
                &fragments,
                planes,
                control,
                &hints,
                &tree.nodes[index].volume,
                &mut priority,
                &mut budget,
            )?
            else {
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
            budget.nodes(tree.nodes.len() + 2)?;
            budget.pending(pending.len() + 2)?;
            if added {
                priority.record(index, plane, &hints)?;
            }
            let [front_hints, back_hints] = priority.split(hints, plane, planes, control)?;
            let split = SplitContext {
                index,
                volume: &tree.nodes[index].volume,
                plane,
                planes,
                fragments: &fragments,
                brushes,
                hints: source_hints,
            };
            let [front_volume, back_volume] = split.volumes(&mut budget)?;
            let mut halves = [Vec::new(), Vec::new()];
            for fragment in &fragments {
                let shapes = fragment
                    .shape
                    .split_budgeted(plane, planes, &mut budget)
                    .map_err(|error| split.failure(error, Some(&brushes[fragment.brush])))?;
                for (half, shape) in halves.iter_mut().zip(shapes) {
                    if let Some(shape) = shape {
                        budget.charge(1, 4)?;
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
            pending.push((children[1], back, back_hints));
            pending.push((children[0], front, front_hints));
        }
        Ok((tree, priority))
    }

    pub fn leaf_at(&self, point: DVec3, planes: &PlaneSet) -> usize {
        let mut index = 0;
        while let Some((plane, children)) = self.nodes[index].split {
            index = children[usize::from(planes.get(plane).distance(point) < 0.0)];
        }
        index
    }

    pub(crate) fn split_leaf(
        &mut self,
        index: usize,
        plane: usize,
        planes: &PlaneSet,
    ) -> Result<(), String> {
        if self.nodes.len() + 2 > MAX_TREE_NODES {
            return Err("BSP hint partitioning exceeds the node allocation budget".into());
        }
        let node = &self.nodes[index];
        let volumes = node
            .volume
            .split_budgeted(plane, planes, &mut Budget::unlimited())
            .map_err(|error| split_geometry(&error, Some(index), &node.volume, plane, planes))?;
        let [Some(front), Some(back)] = volumes else {
            return Err(split_geometry(
                "Hint plane produced a degenerate BSP leaf volume",
                Some(index),
                &node.volume,
                plane,
                planes,
            ));
        };
        let children = [self.nodes.len(), self.nodes.len() + 1];
        let contents = node.contents;
        let brushes = node.brushes.clone();
        self.nodes[index].split = Some((plane, children));
        self.nodes[index].contents = contents::EMPTY;
        self.nodes[index].brushes.clear();
        for volume in [front, back] {
            self.nodes.push(Node {
                split: None,
                volume,
                contents,
                brushes: brushes.clone(),
            });
        }
        Ok(())
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

    pub fn brush_leaves(&self, brush: &MapBrush, planes: &PlaneSet) -> Result<Vec<usize>, String> {
        let mut leaves = Vec::new();
        let mut pending = vec![(0, Polyhedron::from_brush(brush))];
        while let Some((index, shape)) = pending.pop() {
            if let Some((plane, children)) = self.nodes[index].split {
                let halves = shape
                    .split_budgeted(plane, planes, &mut Budget::unlimited())
                    .map_err(|error| {
                        split_geometry(
                            &error,
                            Some(index),
                            &self.nodes[index].volume,
                            plane,
                            planes,
                        )
                    })
                    .map_err(|error| format!("solid:{}: {error}", brush.id))?;
                for (child, half) in children.into_iter().zip(halves) {
                    if let Some(half) = half {
                        pending.push((child, half));
                    }
                }
            } else {
                leaves.push(index);
            }
        }
        Ok(leaves)
    }
}
