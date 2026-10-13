use source_formats::bsp::{
    self, Bsp, DisplacementCorner, DisplacementInfo, DisplacementNeighbor, DisplacementTriangle,
    DisplacementVertex, Displacements, Edge, Face,
};

fn fixture() -> (Bsp, Displacements) {
    let mut bsp = Bsp::new();
    bsp.set_records(&[Face {
        edge_count: 4,
        dispinfo: 0,
        ..Default::default()
    }]);
    bsp.set_values(
        bsp::lump::VERTEXES,
        &[
            [0.0_f32, 0.0, 0.0],
            [0.0, 16.0, 0.0],
            [16.0, 16.0, 0.0],
            [16.0, 0.0, 0.0],
        ],
    );
    bsp.set_records(&[
        Edge([0, 0]),
        Edge([0, 1]),
        Edge([1, 2]),
        Edge([2, 3]),
        Edge([3, 0]),
    ]);
    bsp.set_values(bsp::lump::SURFEDGES, &[1_i32, 2, 3, 4]);
    let mut info = DisplacementInfo {
        padding: 0xabcd,
        min_tessellation: i32::MIN,
        ..Default::default()
    };
    info.edges[0][0].padding = 0xa5;
    info.edges[0][0].orientation = 255;
    info.corners[0].neighbors = [u16::MAX; 4];
    info.corners[0].padding = 0x5a;
    let mut vertices = vec![DisplacementVertex::default(); 25];
    vertices[12] = DisplacementVertex {
        direction: [0.0, 0.0, 1.0],
        distance: 8.0,
        alpha: 300.0,
    };
    let data = Displacements {
        infos: vec![info],
        vertices,
        triangles: vec![DisplacementTriangle { tags: 0x8007 }; 32],
    };
    (bsp, data)
}

#[test]
fn native_displacement_layout_padding_inactive_neighbors_and_mesh_positions_round_trip() {
    assert_eq!(size_of::<DisplacementInfo>(), 176);
    assert_eq!(size_of::<DisplacementVertex>(), 20);
    assert_eq!(size_of::<DisplacementTriangle>(), 2);
    assert_eq!(size_of::<DisplacementNeighbor>(), 6);
    assert_eq!(size_of::<DisplacementCorner>(), 10);
    let (mut bsp, data) = fixture();
    assert_eq!(data.infos[0].flags(), Some(0));
    assert_eq!(
        DisplacementInfo {
            min_tessellation: i32::MIN | 14,
            ..data.infos[0]
        }
        .flags(),
        Some(14)
    );
    assert_eq!(
        DisplacementInfo {
            min_tessellation: 0,
            ..data.infos[0]
        }
        .flags(),
        None
    );
    bsp.set_displacements(&data).unwrap();
    let bsp = Bsp::parse(&bsp.to_bytes()).unwrap();
    assert_eq!(bsp.displacements().unwrap(), Some(data.clone()));
    let quad = bsp.displacement_quad(0).unwrap();
    let positions = data.positions(0, quad).unwrap();
    assert_eq!(positions[0], [0.0, 0.0, 0.0]);
    assert_eq!(positions[12], [8.0, 8.0, 8.0]);
    assert_eq!(positions[24], [16.0, 16.0, 0.0]);
    for power in [2, 3, 4] {
        let info = DisplacementInfo {
            power,
            ..Default::default()
        };
        assert_eq!(
            info.vertex_count().unwrap(),
            ((1 << power) + 1_usize).pow(2)
        );
        assert_eq!(
            info.triangle_count().unwrap(),
            2 * (1 << power) * (1 << power)
        );
    }
}

#[test]
fn malformed_displacement_ranges_fields_and_neighbors_preserve_existing_records() {
    let (mut bsp, data) = fixture();
    bsp.set_displacements(&data).unwrap();
    let old = bsp.clone();
    let mut invalids = Vec::new();
    for info in [
        DisplacementInfo {
            power: i32::MAX,
            ..data.infos[0]
        },
        DisplacementInfo {
            vertex_start: -1,
            ..data.infos[0]
        },
        DisplacementInfo {
            vertex_start: i32::MAX,
            ..data.infos[0]
        },
        DisplacementInfo {
            triangle_start: 1,
            ..data.infos[0]
        },
        DisplacementInfo {
            face: 1,
            ..data.infos[0]
        },
        DisplacementInfo {
            start: [f32::NAN; 3],
            ..data.infos[0]
        },
        DisplacementInfo {
            lightmap_alpha_start: 1,
            ..data.infos[0]
        },
    ] {
        invalids.push(Displacements {
            infos: vec![info],
            ..data.clone()
        });
    }
    for neighbor in [
        DisplacementNeighbor {
            index: 1,
            ..Default::default()
        },
        DisplacementNeighbor {
            index: 0,
            orientation: 4,
            ..Default::default()
        },
        DisplacementNeighbor {
            index: 0,
            span: 3,
            ..Default::default()
        },
    ] {
        let mut invalid = data.clone();
        invalid.infos[0].edges[0][0] = neighbor;
        invalids.push(invalid);
    }
    let mut invalid = data.clone();
    invalid.infos[0].corners[0].count = 5;
    invalids.push(invalid);
    let mut invalid = data.clone();
    invalid.vertices[0].distance = f32::INFINITY;
    invalids.push(invalid);
    for invalid in invalids {
        assert!(bsp.set_displacements(&invalid).is_err());
        assert_eq!(bsp, old);
    }
}

#[test]
fn unsupported_displacement_lumps_and_broken_base_quad_references_reject() {
    let (mut bsp, data) = fixture();
    bsp.set_displacements(&data).unwrap();
    for index in [
        bsp::lump::DISPINFO,
        bsp::lump::DISP_VERTS,
        bsp::lump::DISP_TRIS,
    ] {
        let mut invalid = bsp.clone();
        invalid.lumps[index].version = 1;
        assert!(invalid.displacements().is_err());
        invalid.lumps[index].version = 0;
        invalid.lumps[index].fourcc = [1, 0, 0, 0];
        assert!(invalid.displacements().is_err());
        invalid.lumps[index].fourcc = [0; 4];
        invalid.lumps[index].data.pop();
        assert!(invalid.displacements().is_err());
    }
    for edge in [i32::MIN, 99, -1] {
        let mut invalid = bsp.clone();
        invalid.set_values(bsp::lump::SURFEDGES, &[edge, 2, 3, 4]);
        assert!(invalid.displacement_quad(0).is_err());
    }
}
