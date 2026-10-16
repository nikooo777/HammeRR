use map_core::{Document, edit};
use source_formats::vmf::Vmf;

use super::*;

fn fixtures() -> Vec<Solid> {
    let mut document = Document::new(Vmf::default());
    document
        .edit("union fixtures", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-64.0, -64.0, 16.0),
                DVec3::new(16.0, 64.0, 144.0),
                "tools/toolsoccluder",
            )?;
            edit::create_block(
                tx,
                DVec3::new(-16.0, -64.0, 16.0),
                DVec3::new(64.0, 64.0, 144.0),
                "tools/toolsoccluder",
            )?;
            Ok(())
        })
        .unwrap();
    document.vmf().solids().cloned().collect()
}

fn run(solids: &[Solid], budget: &mut Budget, control: &Control) -> Result<Vec<Polygon>, String> {
    let geometries = solids
        .iter()
        .map(|solid| (solid, BrushGeometry::from_solid(solid).unwrap()))
        .collect::<Vec<_>>();
    let mut planes = PlaneSet::default();
    let mut polygons = Vec::new();
    let mut active = Vec::new();
    for (_, geometry) in &geometries {
        let mut faces = Vec::new();
        for face in &geometry.faces {
            let plane =
                crate::map::support::find_authored(&mut planes, geometry, face, control)?.unwrap();
            faces.push(Polygon {
                plane,
                winding: Winding(face.vertices.iter().copied().map(stored_vertex).collect()),
            });
        }
        polygons.push(faces);
        active.push(vec![true; geometry.faces.len()]);
    }
    boundary(&geometries, polygons, &active, &planes, budget, control)
}

#[test]
fn union_budgets_bound_work_allocation_and_fragments_before_clipping() {
    let solids = fixtures();
    let control = Control::default();
    for (budget, expected) in [
        (
            Budget {
                max_work: 0,
                ..Budget::default()
            },
            "work-unit",
        ),
        (
            Budget {
                max_points: 0,
                ..Budget::default()
            },
            "allocated-point",
        ),
        (
            Budget {
                max_fragments: 0,
                ..Budget::default()
            },
            "generated-fragment",
        ),
    ] {
        let mut budget = budget;
        let error = run(&solids, &mut budget, &control).unwrap_err();
        assert!(error.contains(expected), "{error}");
        assert_eq!(budget.points, 0);
        assert_eq!(budget.fragments, 0);
    }
    let mut budget = Budget::default();
    let result = run(&solids, &mut budget, &control).unwrap();
    assert_eq!(result.len(), 10);
    assert!(budget.points > 0 && budget.fragments > 0 && budget.work > 0);
    let used = (budget.work, budget.points, budget.fragments);
    budget.max_work = budget.work;
    assert!(run(&solids, &mut budget, &control).is_err());
    assert_eq!((budget.work, budget.points, budget.fragments), used);
    let cancelled = Control::default();
    let mut budget = Budget {
        cancel_after_work: Some(used.0 / 2),
        ..Budget::default()
    };
    let error = run(&solids, &mut budget, &cancelled).unwrap_err();
    assert_eq!(error, "Compilation cancelled");
    assert!(budget.work > 0 && budget.fragments > 0);
}

#[test]
fn split_budget_checks_and_cancellation_precede_winding_allocation() {
    let winding = Winding(vec![
        DVec3::new(-16.0, 16.0, 0.0),
        DVec3::new(16.0, 16.0, 0.0),
        DVec3::new(16.0, -16.0, 0.0),
        DVec3::new(-16.0, -16.0, 0.0),
    ]);
    let control = Control::default();
    let mut budget = Budget {
        max_points: 27,
        ..Budget::default()
    };
    assert!(split(&winding, DVec3::X, 0.0, &mut budget, &control).is_err());
    assert_eq!(budget.points, 0);
    budget = Budget::default();
    let (front, back) = split(&winding, DVec3::X, 0.0, &mut budget, &control).unwrap();
    assert_eq!(front.area() + back.area(), winding.area());
    let used = (budget.work, budget.points, budget.fragments);
    control.cancel().unwrap();
    assert_eq!(
        split(&winding, DVec3::X, 0.0, &mut budget, &control).unwrap_err(),
        "Compilation cancelled"
    );
    assert_eq!((budget.work, budget.points, budget.fragments), used);
}

#[test]
fn legacy_separated_polygons_retain_the_exact_original_vertices_and_order() {
    let mut solids = fixtures();
    for side in &mut solids[1].sides {
        for point in &mut side.plane.0 {
            point.x += 200.0;
        }
    }
    let control = Control::default();
    let result = run(&solids, &mut Budget::default(), &control).unwrap();
    let expected = solids
        .iter()
        .flat_map(|solid| {
            BrushGeometry::from_solid(solid)
                .unwrap()
                .faces
                .into_iter()
                .map(|face| {
                    face.vertices
                        .into_iter()
                        .map(stored_vertex)
                        .collect::<Vec<_>>()
                })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        result
            .iter()
            .map(|polygon| &polygon.winding.0)
            .collect::<Vec<_>>(),
        expected.iter().collect::<Vec<_>>()
    );
}
