use map_core::Plane;
use map_core::brush::BrushGeometry;

use super::*;
use crate::physics::{Body, MASK_SOLID, Model, Request, default_materials};

fn box_geometry() -> BrushGeometry {
    let planes = [
        DVec3::X,
        DVec3::NEG_X,
        DVec3::Y,
        DVec3::NEG_Y,
        DVec3::Z,
        DVec3::NEG_Z,
    ]
    .map(|normal| Plane { normal, dist: 32.0 });
    BrushGeometry::from_planes(&planes).unwrap()
}

fn shape(geometry: &BrushGeometry) -> Shape {
    let sides = geometry.faces.iter().map(|face| {
        let normal = face.plane.normal.as_vec3();
        (
            [normal.x, normal.y, normal.z, face.plane.dist as f32],
            face.vertices.as_slice(),
        )
    });
    build(0, sides, &mut Budget::default()).unwrap()
}

fn request(shape: Shape) -> Request {
    Request {
        model_count: 1,
        models: vec![Model {
            index: 0,
            bodies: vec![Body {
                contents: MASK_SOLID,
                shapes: vec![shape],
                surface: None,
                current_hu: None,
                damping: None,
                surface_property: None,
                mass_area: None,
                mesh: None,
            }],
        }],
        materials: default_materials(),
        surface_data: Vec::new(),
    }
}

#[test]
fn source_box_retains_shared_corners_and_outward_nontriangular_faces() {
    let shape = shape(&box_geometry());
    assert_eq!(shape.corners.len(), 8);
    assert_eq!(shape.faces.len(), 6);
    assert!(shape.faces.iter().all(|face| face.len() == 4));
    for (face, plane) in shape.faces.iter().zip(&shape.planes) {
        let [a, b, c] =
            [face[0], face[1], face[2]].map(|index| point(shape.corners[index as usize]));
        assert!(
            (b - a)
                .cross(c - a)
                .dot(point([plane[0], plane[1], plane[2]]))
                > 0.0
        );
    }
    crate::physics::validate(&request(shape)).unwrap();
}

#[test]
fn redundant_source_planes_are_absent_and_generated_fluid_splits_keep_closed_faces() {
    let mut source_planes = vec![
        Plane {
            normal: DVec3::X,
            dist: 32.0,
        },
        Plane {
            normal: DVec3::NEG_X,
            dist: 32.0,
        },
        Plane {
            normal: DVec3::Y,
            dist: 32.0,
        },
        Plane {
            normal: DVec3::NEG_Y,
            dist: 32.0,
        },
        Plane {
            normal: DVec3::Z,
            dist: 32.0,
        },
        Plane {
            normal: DVec3::NEG_Z,
            dist: 32.0,
        },
    ];
    source_planes.push(source_planes[0]);
    source_planes.push(Plane {
        normal: DVec3::X,
        dist: 64.0,
    });
    let geometry = BrushGeometry::from_planes(&source_planes).unwrap();
    assert_eq!(shape(&geometry).faces.len(), 6);
    let mut planes = crate::planes::PlaneSet::default();
    let brush = crate::map::MapBrush {
        id: 7,
        entity: 0,
        contents: source_formats::bsp::contents::WATER,
        sides: geometry
            .faces
            .iter()
            .map(|face| crate::map::MapSide {
                source_id: None,
                plane: planes.find(face.plane.normal, face.plane.dist),
                material: 0,
                texinfo: 0,
                smoothing_groups: 0,
                winding: Some(crate::winding::Winding(face.vertices.clone())),
                bevel: false,
                visible: true,
            })
            .collect(),
        mins: geometry.mins,
        maxs: geometry.maxs,
    };
    let volume = crate::tree::Polyhedron::from_brush(&brush);
    let split = planes.find(DVec3::new(0.6, 0.8, 0.0), 7.0);
    let halves = volume.split(split, &planes);
    for half in halves.into_iter().flatten() {
        let sides = half.sides.iter().map(|side| {
            let plane = planes.get(side.plane);
            let normal = plane.normal.as_vec3();
            (
                [normal.x, normal.y, normal.z, plane.dist as f32],
                side.winding.0.as_slice(),
            )
        });
        let shape = build(99, sides, &mut Budget::default()).unwrap();
        let mut fluid = request(shape);
        fluid.models[0].bodies[0].contents = source_formats::bsp::contents::WATER;
        fluid.models[0].bodies[0].surface = Some(32.0);
        crate::physics::validate(&fluid).unwrap();
    }
}

#[test]
fn exact_202_authored_planes_keep_every_tiny_source_facet_and_corner() {
    let authored = [
        [
            [50.49610398338425, -124.45402565027105, 195.4364950386774],
            [44.89961088252561, -167.5676290536579, 148.46952295671846],
            [-17.600813310652967, -173.16412215451655, 161.05428519701445],
        ],
        [
            [178.19333615040586, -242.193336150406, 194.8388809802324],
            [189.38632235212313, -189.3863223521233, 229.2210908218954],
            [136.5793085538404, -200.57930855384058, 263.60330066355834],
        ],
        [
            [151.66581989144115, 117.27105244120868, 338.80458923801154],
            [153.55204595610203, 53.93356986061436, 347.79377449508047],
            [94.92224251666595, 55.81979592527524, 373.386513697559],
        ],
        [
            [329.5547724251598, -121.87635466144334, 11.070249698994754],
            [340.4581561282255, -59.79120510516356, 0.0],
            [338.5433056845053, -70.69458880822923, -63.03530416839365],
        ],
    ];
    let planes = authored.map(|points| Plane::from_points(points.map(DVec3::from_array)).unwrap());
    let shape = shape(&BrushGeometry::from_planes(&planes).unwrap());
    assert_eq!(shape.corners.len(), 4);
    assert_eq!(shape.faces.len(), 4);
    assert!(shape.faces.iter().all(|face| face.len() == 3));
    let retained = [
        [354.05544963976786, 18.33473902500767, 264.79132320373594],
        [354.14989338934663, 18.17115765227978, 264.5781385058847],
        [354.17632156894047, 18.321642816085028, 264.7399887405181],
        [354.18772917372087, 18.38659905977551, 264.7248792602863],
    ];
    for point in retained {
        assert!(shape.corners.contains(&point.map(|value| value as f32)));
    }
    crate::physics::validate(&request(shape)).unwrap();
}

#[test]
fn source_identity_only_joins_clipping_roundoff_and_refuses_f32_collapses() {
    let mut geometry = box_geometry();
    geometry.faces[0].vertices[0].x += SOURCE_CORNER_ROUNDOFF * 0.5;
    assert_eq!(shape(&geometry).corners.len(), 8);
    geometry.faces[0].vertices[0].x += SOURCE_CORNER_ROUNDOFF * 2.0;
    let sides = geometry.faces.iter().map(|face| {
        let normal = face.plane.normal.as_vec3();
        (
            [normal.x, normal.y, normal.z, face.plane.dist as f32],
            face.vertices.as_slice(),
        )
    });
    assert!(
        build(42, sides, &mut Budget::default())
            .unwrap_err()
            .contains("Physics brush 42 topology: distinct source corners collapse")
    );
}

#[test]
fn source_face_incidence_preserves_real_edges_smaller_than_the_roundoff_bound() {
    let size = SOURCE_CORNER_ROUNDOFF * 0.125;
    let points = [
        DVec3::ZERO,
        DVec3::X * size,
        DVec3::Y * size,
        DVec3::Z * size,
    ];
    let mut sides = Vec::new();
    for omitted in 0..points.len() {
        let mut face = points
            .iter()
            .enumerate()
            .filter_map(|(index, &point)| (index != omitted).then_some(point))
            .collect::<Vec<_>>();
        let mut normal = (face[1] - face[0]).cross(face[2] - face[0]).normalize();
        if normal.dot(points[omitted] - face[0]) > 0.0 {
            face.swap(1, 2);
            normal = -normal;
        }
        let dist = normal.dot(face[0]);
        face.reverse();
        sides.push((
            [
                normal.x as f32,
                normal.y as f32,
                normal.z as f32,
                dist as f32,
            ],
            face,
        ));
    }
    let source = sides.iter().map(|(plane, face)| (*plane, face.as_slice()));
    let shape = build(202, source, &mut Budget::default()).unwrap();
    assert_eq!(shape.corners.len(), 4);
    assert_eq!(shape.faces.len(), 4);
}

#[test]
fn received_requests_revalidate_open_reversed_nonconvex_and_malformed_faces() {
    let original = shape(&box_geometry());
    for case in 0..14 {
        let mut invalid = original.clone();
        match case {
            0 => {
                invalid.faces.pop();
            }
            1 => invalid.faces[0].reverse(),
            2 => invalid.faces[0][0] = u16::MAX,
            3 => invalid.faces[0][1] = invalid.faces[0][0],
            4 => {
                invalid.faces[0].pop();
            }
            5 => invalid.faces[0].swap(1, 2),
            6 => invalid.corners[0][0] = f32::NAN,
            7 => invalid.corners[0] = invalid.corners[1],
            8 => invalid.corners[0][0] = MAX_COMPILE_COORD as f32 + 1.0,
            9 => invalid.planes[0][3] += 1.0,
            10 => invalid.corners.push([0.0; 3]),
            11 => invalid.planes[0] = [0.0; 4],
            12 => invalid.corners[0][2] += 0.25,
            13 => {
                invalid.faces.push(invalid.faces[0].clone());
                invalid.planes.push(invalid.planes[0]);
            }
            _ => unreachable!(),
        }
        assert!(
            crate::physics::validate(&request(invalid)).is_err(),
            "case {case}"
        );
    }
}

#[test]
fn request_schema_requires_explicit_faces_and_bounds_topology_before_work() {
    let original = request(shape(&box_geometry()));
    let mut json = serde_json::to_value(&original).unwrap();
    json["models"][0]["bodies"][0]["shapes"][0]
        .as_object_mut()
        .unwrap()
        .remove("faces");
    assert!(serde_json::from_value::<Request>(json).is_err());
    let mut invalid = original;
    let shape = &mut invalid.models[0].bodies[0].shapes[0];
    shape.faces[0].resize(MAX_CORNERS + 1, 0);
    assert!(
        crate::physics::validate(&invalid)
            .unwrap_err()
            .contains("unique corner indices")
    );
    let mut budget = Budget::default();
    budget
        .charge(MAX_REQUEST_INDICES, MAX_REQUEST_WORK)
        .unwrap();
    assert!(budget.charge(1, 0).is_err());
    let mut budget = Budget::default();
    budget.charge(0, MAX_REQUEST_WORK).unwrap();
    assert!(budget.charge(0, 1).is_err());
}
