use super::*;
use crate::test_common as common;
use crate::tree::{Node, Polyhedron};

fn limits() -> Limits {
    Limits {
        portals: 16,
        points: 1024,
        pending: 16,
        work: 1024,
    }
}

fn node(split: Option<(usize, [usize; 2])>) -> Node {
    Node {
        split,
        volume: Polyhedron { sides: Vec::new() },
        contents: 0,
        brushes: Vec::new(),
    }
}

fn square() -> Winding {
    Winding(vec![
        DVec3::new(-8.0, 8.0, 0.0),
        DVec3::new(8.0, 8.0, 0.0),
        DVec3::new(8.0, -8.0, 0.0),
        DVec3::new(-8.0, -8.0, 0.0),
    ])
}

fn split_tree() -> (Tree, PlaneSet) {
    let mut planes = PlaneSet::default();
    let plane = planes.find(DVec3::Z, 0.0);
    (
        Tree {
            nodes: vec![node(Some((plane, [1, 2]))), node(None), node(None)],
        },
        planes,
    )
}

#[test]
fn exact_entry_budgets_preserve_portals_and_report_each_exhaustion() {
    let (tree, planes) = split_tree();
    let control = Control::default();
    let exact = Limits {
        portals: 1,
        points: 4,
        pending: 1,
        work: 18,
    };
    let baseline = build_controlled(&tree, &planes, &control).unwrap();
    let output = build_limited(&tree, &planes, &control, exact).unwrap();
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].leaves, baseline[0].leaves);
    assert_eq!(output[0].winding, baseline[0].winding);
    for (budget, message) in [
        (
            Limits {
                portals: 0,
                ..exact
            },
            "output portal",
        ),
        (Limits { points: 3, ..exact }, "winding allocation"),
        (
            Limits {
                pending: 0,
                ..exact
            },
            "pending fragment",
        ),
        (Limits { work: 17, ..exact }, "traversal work"),
    ] {
        assert!(
            build_limited(&tree, &planes, &control, budget)
                .unwrap_err()
                .contains(message)
        );
    }
}

#[test]
fn coplanar_fanout_admits_pending_before_cloning_and_preserves_both_sides() {
    let (tree, planes) = split_tree();
    let control = Control::default();
    let mut budget = Budget::new(
        Limits {
            pending: 1,
            points: 4,
            ..limits()
        },
        &control,
    );
    let mut output = Vec::new();
    let error = connect(&tree, &planes, [0, 3], square(), &mut output, &mut budget).unwrap_err();
    assert!(error.contains("pending fragment"));
    assert_eq!(budget.points, 0);
    assert!(output.is_empty());
    let mut budget = Budget::new(
        Limits {
            pending: 2,
            points: 4,
            portals: 2,
            ..limits()
        },
        &control,
    );
    connect(&tree, &planes, [0, 3], square(), &mut output, &mut budget).unwrap();
    assert_eq!(output.len(), 2);
    assert_eq!(budget.points, 4);
    assert_eq!(
        output
            .iter()
            .map(|portal| portal.winding.0.len())
            .sum::<usize>(),
        8
    );
    assert!(output.iter().any(|portal| portal.leaves == [1, 3]));
    assert!(output.iter().any(|portal| portal.leaves == [2, 3]));
}

#[test]
fn split_scratch_is_admitted_before_allocation_including_nonconvex_growth() {
    let control = Control::default();
    let mut budget = Budget::new(
        Limits {
            points: 39,
            ..limits()
        },
        &control,
    );
    let error = clip_back(square(), DVec3::X, 0.0, &mut budget).unwrap_err();
    assert!(error.contains("winding allocation"));
    assert_eq!(budget.points, 0);
    let mut budget = Budget::new(
        Limits {
            points: 40,
            ..limits()
        },
        &control,
    );
    let result = clip_back(square(), DVec3::X, 0.0, &mut budget)
        .unwrap()
        .unwrap();
    assert_eq!(budget.points, 40);
    assert_eq!(result, square().clip_back(DVec3::X, 0.0).unwrap());
    let alternating = Winding(
        (0..16)
            .map(|index| DVec3::new(if index % 2 == 0 { -8.0 } else { 8.0 }, index as f64, 0.0))
            .collect(),
    );
    let allowance = 7 * alternating.0.len() + 12;
    let mut budget = Budget::new(
        Limits {
            points: allowance,
            ..limits()
        },
        &control,
    );
    let expected = alternating.clip_back(DVec3::X, 0.0);
    assert_eq!(
        clip_back(alternating, DVec3::X, 0.0, &mut budget).unwrap(),
        expected
    );
    assert_eq!(budget.points, allowance);
}

#[test]
fn cumulative_point_allowance_does_not_reset_between_clips() {
    let control = Control::default();
    let mut budget = Budget::new(
        Limits {
            points: 79,
            ..limits()
        },
        &control,
    );
    clip_back(square(), DVec3::X, 0.0, &mut budget).unwrap();
    assert!(
        clip_back(square(), DVec3::X, 0.0, &mut budget)
            .unwrap_err()
            .contains("winding allocation")
    );
    assert_eq!(budget.points, 40);
}

#[test]
fn cancellation_wins_before_entry_and_inner_clipping_admission() {
    let (tree, planes) = split_tree();
    let control = Control::default();
    control.cancel().unwrap();
    assert_eq!(
        build_limited(&tree, &planes, &control, limits()).unwrap_err(),
        "Compilation cancelled"
    );
    let mut budget = Budget::new(limits(), &control);
    assert_eq!(
        clip_back(square(), DVec3::X, 0.0, &mut budget).unwrap_err(),
        "Compilation cancelled"
    );
    assert_eq!(budget.points, 0);
    assert_eq!(budget.work, 0);
    let control = Control::default();
    let mut budget = Budget::new(limits(), &control);
    clip_back(square(), DVec3::Z, 0.0, &mut budget).unwrap();
    let admitted_work = budget.work;
    control.cancel().unwrap();
    assert_eq!(
        clip_back(square(), DVec3::X, 0.0, &mut budget).unwrap_err(),
        "Compilation cancelled"
    );
    assert_eq!(budget.points, 0);
    assert_eq!(budget.work, admitted_work);
}

#[test]
fn existing_sealed_room_preserves_its_interior_apertures_and_flood() {
    let mut report = Report::default();
    let game = common::empty_game();
    let mut map = crate::map::Map::load(&common::room(), &game, &mut report).unwrap();
    let mut tree = Tree::build(&map.brushes, &map.entities[0].brushes, &mut map.planes).unwrap();
    let portals =
        build_limited(&tree, &map.planes, &Control::default(), Limits::default()).unwrap();
    let interior = tree.leaf_at(DVec3::new(0.0, 0.0, 128.0), &map.planes);
    let area: f64 = portals
        .iter()
        .filter(|portal| portal.leaves.contains(&interior))
        .map(|portal| portal.winding.area())
        .sum();
    assert!(
        (area - 1_048_576.0).abs() < 1e-6,
        "Interior aperture area: {area}"
    );
    assert!(flood(&mut tree, &map.planes, &portals, &map.entities, &mut report).is_none());
    assert!(!report.has_errors(), "{report:?}");
    assert!(!tree.nodes[tree.leaf_at(DVec3::new(0.0, 0.0, 128.0), &map.planes)].solid());
}
