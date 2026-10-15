use super::super::*;

#[tool_router(router = terrain_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Preview sewing 2–16 selected/explicit terrain faces along unambiguous full/half base edges or matching base corners, including mixed powers. Shared coarse vertex groups average original world positions once, including junctions; unmatched fine border vertices interpolate the sewn coarse edge. blend=true also sews/interpolates alpha(default false). Interiors/IDs/material/mapping/flags remain. Rejects ambiguous edges and unsupported caches atomically. Returns neighbor pairs, corner_joins (additional corner unions), shared groups, interpolated_vertices and a shared summary. Inspect the draft then commit/cancel_face_preview with state.preview_token. Other fractional spans are not supported. The saved subdivision marker alone does not block sewing. UI: Edit terrain… → Sew borders & corners."
    )]
    pub(in crate::mcp) fn preview_terrain_sew(
        &self,
        Parameters(args): Parameters<TerrainSewArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let result =
                session.preview_terrain_sew(Actor::Agent, &faces, args.blend.unwrap_or(false))?;
            Ok(
                json!({"faces_changed":result.changed,"neighbor_pairs":result.pairs,"corner_joins":result.corner_joins,"sewn_vertices":result.vertices,"interpolated_vertices":result.interpolated_vertices,"summary":result.summary(),"state":face_state(&session)}),
            )
        })
    }

    #[tool(
        description = "Preview border lighting smoothing on 1–16 selected/explicit terrain faces. Omitted/null angle restores Automatic; angle=0 keeps borders hard; positive finite angles up to180 degrees limit shared border-normal blending using both surfaces' choices. Geometry, alpha, flags, tags and topology remain. Inspect border_smoothing_angle/error with get_terrain_grids, then commit/cancel_face_preview using state.preview_token. The viewport and Direct LDR share the normal decisions. Uses the standard VMF smooth field and repairs malformed/duplicate entries when explicitly changed. UI: Edit terrain… → Border smoothing."
    )]
    pub(in crate::mcp) fn preview_terrain_smoothing(
        &self,
        Parameters(args): Parameters<TerrainSmoothingArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let changed = session.preview_terrain_edit(
                Actor::Agent,
                &faces,
                map_core::displacement::Operation::Smoothing(args.angle),
            )?;
            Ok(json!({"faces_changed":changed,"state":face_state(&session)}))
        })
    }

    #[tool(
        description = "Preview optional positive physics_collision, player_collision and ray_collision booleans on 1–16 selected/explicit terrain faces. Omitted settings preserve each face. Changes only inverted native bits2/4/8, retaining other flags, geometry, alpha, tags and metadata. Unknown native flags still require compiler support. Commit/cancel the shared face preview with its token. UI: Edit terrain… → Collision, with Keep current/Enabled/Disabled choices. Individual triangle removal remainsunsupported."
    )]
    pub(in crate::mcp) fn preview_terrain_collision(
        &self,
        Parameters(args): Parameters<TerrainCollisionArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let settings = map_core::displacement::collision::Settings {
                physics: args.physics_collision,
                player: args.player_collision,
                ray: args.ray_collision,
            };
            let changed = session.preview_terrain_edit(
                Actor::Agent,
                &faces,
                map_core::displacement::Operation::Collision(settings),
            )?;
            Ok(json!({"faces_changed":changed,"state":face_state(&session)}))
        })
    }

    #[tool(
        description = "Preview a terrain brush on up to 16 selected or explicit faces. mode=raise adds signed amount (default16, ±4096); flatten moves toward the center plane; smooth averages immutable local neighbors; noise uses amount and deterministic seed(default1). center is world coordinates, radius is1–8192(default128), direction=world_z(default) or face_normal. flatten/smooth strength is0–1(default0.5). Smooth radial falloff applies. Source/history remain unchanged; inspect get_terrain_grids(preview=true), then commit/cancel_face_preview with state.preview_token. Bounds ±65536 and malformed/cache data reject atomically. Optional mask_token from get_terrain_vertex_mask confines writes to selected terrain vertices. Replaced/stale tokens reject. No automatic edge sewing. UI: Edit terrain…."
    )]
    pub(in crate::mcp) fn preview_terrain_sculpt(
        &self,
        Parameters(args): Parameters<TerrainSculptArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let operation = terrain_sculpt_operation(&args);
            let count = if let Some(token) = args.mask_token {
                session.preview_terrain_masked_edit(
                    Actor::Agent,
                    &faces,
                    operation,
                    parse_token(&token)?,
                )?
            } else {
                session.preview_terrain_edit(Actor::Agent, &faces, operation)?
            };
            Ok(json!({"faces_changed":count,"state":face_state(&session)}))
        })
    }

    #[tool(
        description = "Preview terrain alpha painting, full-face fill or invert on up to16 selected/explicit faces. value is0–255(default128); paint moves toward it with strength0–1(default1), smooth radial falloff and required world center. radius is1–8192(default128), direction=world_z(default) or face_normal. fill/invert affect complete grids and do not use brush coordinates. Optional mask_token from get_terrain_vertex_mask confines paint/fill/invert to selected terrain vertices. Replaced/stale tokens reject. Geometry/material/mapping remain fixed. A two-material shader is needed to display alpha blending. Inspect get_terrain_grids(preview=true), then commit/cancel_face_preview with state.preview_token. UI: Edit terrain…."
    )]
    pub(in crate::mcp) fn preview_terrain_blend(
        &self,
        Parameters(args): Parameters<TerrainBlendArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let operation = terrain_blend_operation(&args)?;
            let count = if let Some(token) = args.mask_token {
                session.preview_terrain_masked_edit(
                    Actor::Agent,
                    &faces,
                    operation,
                    parse_token(&token)?,
                )?
            } else {
                session.preview_terrain_edit(Actor::Agent, &faces, operation)?
            };
            Ok(json!({"faces_changed":count,"state":face_state(&session)}))
        })
    }

    #[tool(
        description = "Inspect up to 16 selected or explicit terrain faces: power, grid size, saved subdivided marker, ordered world positions, alpha blend values 0–255, flags and border_smoothing_angle/error. Null angle is Automatic; 0 is a hard border; positive angles are bounded limits. preview=true reads the rendered draft. Uses the same grid reconstruction as the viewport.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_terrain_grids(
        &self,
        Parameters(args): Parameters<TerrainGridsArgs>,
    ) -> ToolResult {
        run(|| {
            let session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let grids = session.terrain_grids(&faces, args.preview.unwrap_or(false))?;
            Ok(
                json!({"faces":grids.iter().map(|grid|json!({"solid":grid.face.solid,"side":grid.face.side,"power":grid.power,"subdivided":grid.subdivided,"grid_size":grid.mesh.size,"flags":grid.flags,"physics_collision":grid.collision.physics,"player_collision":grid.collision.player,"ray_collision":grid.collision.ray,"border_smoothing_angle":grid.smoothing_angle,"border_smoothing_error":grid.smoothing_error,"positions":grid.mesh.positions.iter().map(|p|p.to_array()).collect::<Vec<_>>(),"alphas":grid.mesh.alphas})).collect::<Vec<_>>() }),
            )
        })
    }

    #[tool(
        description = "Inspect 1–16 selected/explicit terrain faces in native triangle order (IDs0–31/127/511). Returns each triangle's grid indices, world positions, centroid, optional raw VMF source_tags, effective walkable/buildable classifications, optional forced overrides, removed and unknown_tags. VMF source bits differ from BSP bits. preview=true inspects the rendered draft. Removed/unknown tags remain inspectable but cannot compile. UI: Terrain triangles…. No selection or history change.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_terrain_triangles(
        &self,
        Parameters(args): Parameters<TerrainGridsArgs>,
    ) -> ToolResult {
        run(|| {
            let session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let rows = session.terrain_triangles(&faces, args.preview.unwrap_or(false))?;
            Ok(
                json!({"faces":rows.iter().map(|row|json!({"solid":row.face.solid,"side":row.face.side,"triangles":row.triangles.iter().map(|triangle|json!({"id":triangle.id,"indices":triangle.indices,"positions":triangle.positions.map(|p|p.to_array()),"centroid":triangle.centroid.to_array(),"source_tags":triangle.source_tags,"walkable":triangle.walkable,"buildable":triangle.buildable,"walkable_override":triangle.walkable_override,"buildable_override":triangle.buildable_override,"removed":triangle.removed,"unknown_tags":triangle.unknown_tags})).collect::<Vec<_>>()})).collect::<Vec<_>>()}),
            )
        })
    }

    #[tool(
        description = "Enable shared move handles for retained terrain vertices or triangle-union vertices. Requires mask_token; faces default to selected faces. Captures source, mask and shared grid snapping. Rejects active geometry previews. UI: Edit terrain… → Move selected vertices → Move with handles…. Returns token for drag lifecycle."
    )]
    pub(in crate::mcp) fn enable_terrain_move_handles(
        &self,
        Parameters(args): Parameters<TerrainMoveHandlesArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            session.enable_terrain_move_handles(
                Actor::Agent,
                &faces,
                parse_token(&args.mask_token)?,
            )?;
            Ok(session.terrain_move_handles_json())
        })
    }

    #[tool(
        description = "Inspect shared terrain move handle state and world/pixel paths. Optional camera inspects without moving the editor; point hit-tests visible paths. Camera overrides do not change the camera captured by a drag.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_terrain_move_handles(
        &self,
        Parameters(args): Parameters<HandlesArgs>,
    ) -> ToolResult {
        run(|| {
            let session = self.session();
            let size = render_size(args.width, args.height);
            let camera = args
                .camera
                .unwrap_or_default()
                .resolve(&session, size.0 as f32 / size.1 as f32)?;
            let guide = session.terrain_move_guide();
            let shapes = guide.map_or_else(Vec::new, |guide| {
                render::handles::shapes(&camera, size, &guide)
            });
            let picked = if let (Some(guide), Some(point)) = (guide, args.point) {
                let point = glam::Vec2::new(point[0] as f32, point[1] as f32);
                if !point.is_finite() {
                    return Err("Handle point must be finite".into());
                }
                render::handles::pick(&camera, size, &guide, point)
                    .map(render::handles::Handle::name)
            } else {
                None
            };
            Ok(
                json!({"state":session.terrain_move_handles_json(),"camera":camera_json(&camera),"width":size.0,"height":size.1,"picked":picked,"origin":guide.map(|guide|guide.origin.to_array()),
                "handles":shapes.iter().map(|shape|json!({"handle":shape.handle.name(),"color":shape.color,"segments":shape.segments.iter().map(|points|points.map(|point|point.to_array())).collect::<Vec<_>>(),"pixel_segments":shape.segments.iter().filter_map(|points|Some([render::handles::project(&camera,points[0],size)?.to_array(),render::handles::project(&camera,points[1],size)?.to_array()])).collect::<Vec<_>>()})).collect::<Vec<_>>()}),
            )
        })
    }

    #[tool(
        description = "Begin the enabled terrain handle token using x/y/z, xy/xz/yz or view. Optional pointer captures starting pixels and viewport size; omit for numeric total offsets. Captures camera. No source changes."
    )]
    pub(in crate::mcp) fn begin_terrain_move_drag(
        &self,
        Parameters(args): Parameters<BeginTerrainMoveDragArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.begin_terrain_move_drag(
                parse_token(&args.token)?,
                render::handles::Handle::parse(&args.handle)
                    .ok_or("Unknown terrain move handle")?,
                args.pointer.map(PointerArgs::resolve).transpose()?,
            )?;
            Ok(session.terrain_move_handles_json())
        })
    }

    #[tool(
        description = "Update the enabled terrain handle token with exactly one pointer or total offset pair since begin. Axis uses first amount only; planes use world axes, view uses captured screen right/up. Shared grid snapping applies. Invalid moves preserve valid drafts and block finish until corrected. Source stays unchanged."
    )]
    pub(in crate::mcp) fn update_terrain_move_drag(
        &self,
        Parameters(args): Parameters<UpdateTerrainMoveDragArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let changed = session.update_terrain_move_drag(
                Actor::Agent,
                parse_token(&args.token)?,
                args.pointer.map(PointerArgs::resolve).transpose()?,
                args.offset.map(glam::DVec2::from_array),
            )?;
            Ok(
                json!({"faces_changed":changed,"handles":session.terrain_move_handles_json(),"state":face_state(&session)}),
            )
        })
    }

    #[tool(
        description = "Finish the exact terrain handle token and leave its owned face preview for review. Apply with commit_face_preview or discard with cancel_face_preview using state.preview_token. One Apply creates one Undo step; a zero offset changes nothing."
    )]
    pub(in crate::mcp) fn finish_terrain_move_drag(
        &self,
        Parameters(args): Parameters<VolumeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.finish_terrain_move_drag(parse_token(&args.token)?)?;
            Ok(json!({"handles":session.terrain_move_handles_json(),"state":face_state(&session)}))
        })
    }

    #[tool(
        description = "Cancel the exact enabled terrain handle token, including a stale gesture. Clears only its owned preview, preserving replacement work. Source and Undo history stay unchanged."
    )]
    pub(in crate::mcp) fn cancel_terrain_move_handles(
        &self,
        Parameters(args): Parameters<VolumeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.cancel_terrain_move_handles(parse_token(&args.token)?)?;
            Ok(json!({"handles":session.terrain_move_handles_json(),"state":face_state(&session)}))
        })
    }

    #[tool(
        description = "Generate a rounded terrain preview from up to16 selected/explicit base quad cages. Replaces sculpted positions; retains powers, start corners, blend values, flags/tags, materials and mapping. Uses four Catmull-Clark rounds and fixed base boundary edges. Matching full edges share samples; half-edge and non-manifold/corner-only networks reject atomically. Saved explicit grids remain editable unless unsupported caches are present. Inspect get_terrain_grids(preview=true), then commit/cancel_face_preview with state.preview_token. UI: Edit terrain… → Subdivide base cage."
    )]
    pub(in crate::mcp) fn preview_terrain_subdivision(
        &self,
        Parameters(args): Parameters<TerrainSubdivisionArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let result = session.preview_terrain_subdivision(Actor::Agent, &faces)?;
            Ok(
                json!({"faces_changed":result.changed,"shared_edges":result.shared_edges,"vertices":result.vertices,"state":face_state(&session)}),
            )
        })
    }

    #[tool(
        description = "Begin a bounded terrain brush stroke without editing source/history. Supply exactly one sculpt or blend object using the same arguments as preview_terrain_sculpt/blend; blend must be paint. Selected/explicit faces and optional mask_token are captured. An active geometry preview rejects rather than replacing it. Flatten keeps the supplied center plane; other brush centers follow the path. Returns stroke token, spacing and limits. UI: Edit terrain… → Draw stroke in viewport…."
    )]
    pub(in crate::mcp) fn begin_terrain_stroke(
        &self,
        Parameters(args): Parameters<TerrainStrokeArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let (faces, operation, mask) = match (&args.sculpt, &args.blend) {
                (Some(args), None) => (
                    args.faces.as_ref(),
                    terrain_sculpt_operation(args),
                    args.mask_token.as_deref(),
                ),
                (None, Some(args)) => (
                    args.faces.as_ref(),
                    terrain_blend_operation(args)?,
                    args.mask_token.as_deref(),
                ),
                _ => return Err("Supply exactly one sculpt or blend brush".into()),
            };
            let faces = faces.map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            session.begin_terrain_stroke(
                Actor::Agent,
                &faces,
                operation,
                mask.map(parse_token).transpose()?,
            )?;
            Ok(session.terrain_stroke_json())
        })
    }

    #[tool(
        description = "Append terrain stroke input using its token and either points (1–64 finite world positions) or origin+direction (one visible terrain ray hit). Dabs are spaced at radius/4 clamped1–64 brush-plane units, carried across updates; motion along the captured sampling normal adds no dabs. Face-normal spacing follows the first selected base face. Each preview replays ordered dabs from captured source, so inspection does not accumulate edits. A ray miss breaks continuity; next hit starts a new segment. Invalid/stale/replaced updates preserve prior drafts. Bounds ±65536, max512 dabs/4096 input points/two-million vertex-sample work. Masks, radius/falloff and immutable-neighbor smoothing stay shared. Returns stroke plus face preview state. UI: drag on selected terrain."
    )]
    pub(in crate::mcp) fn update_terrain_stroke(
        &self,
        Parameters(args): Parameters<TerrainStrokeUpdateArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let token = parse_token(&args.token)?;
            let changed = match (args.points, args.origin, args.direction) {
                (Some(points), None, None) => session.update_terrain_stroke(
                    Actor::Agent,
                    token,
                    &points.into_iter().map(vec3).collect::<Vec<_>>(),
                )?,
                (None, Some(origin), Some(direction)) => session.update_terrain_stroke_at_ray(
                    Actor::Agent,
                    token,
                    &Ray {
                        origin: vec3(origin),
                        direction: vec3(direction),
                    },
                )?,
                _ => return Err("Supply points or both origin and direction".into()),
            };
            Ok(
                json!({"faces_changed":changed,"stroke":session.terrain_stroke_json(),"state":face_state(&session)}),
            )
        })
    }

    #[tool(
        description = "Inspect the active terrain stroke, token, source/preview/mask stale state, captured mode/radius, sample spacing/counts, cursor and work limits. Does not change source/history or compound the draft.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_terrain_stroke(&self) -> ToolResult {
        ok(self.session().terrain_stroke_json())
    }

    #[tool(
        description = "Break terrain stroke continuity using its token, keeping existing dabs/draft. The next world point or ray hit starts a new segment. Ray misses do this automatically. Invalid/stale tokens preserve the prior state. UI: move off visible selected terrain during a stroke."
    )]
    pub(in crate::mcp) fn break_terrain_stroke(
        &self,
        Parameters(args): Parameters<TerrainStrokeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.interrupt_terrain_stroke(parse_token(&args.token)?)?;
            Ok(session.terrain_stroke_json())
        })
    }

    #[tool(
        description = "Finish an owned terrain stroke into its existing face preview. Requires at least one dab and current source/mask/preview tokens. Source/history stay fixed; inspect get_terrain_grids(preview=true) or render_view, then commit_face_preview/cancel_face_preview using state.preview_token. One Apply makes one Undo step. UI: release the terrain stroke."
    )]
    pub(in crate::mcp) fn finish_terrain_stroke(
        &self,
        Parameters(args): Parameters<TerrainStrokeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.finish_terrain_stroke(Actor::Agent, parse_token(&args.token)?)?;
            Ok(json!({"stroke":session.terrain_stroke_json(),"state":face_state(&session)}))
        })
    }

    #[tool(
        description = "Cancel a terrain stroke by token and discard only its owned face preview. A replaced stroke token rejects; a foreign face preview remains intact. Source/history stay fixed. UI: Escape or Cancel stroke while drawing."
    )]
    pub(in crate::mcp) fn cancel_terrain_stroke(
        &self,
        Parameters(args): Parameters<TerrainStrokeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.cancel_terrain_stroke(parse_token(&args.token)?)?;
            Ok(json!({"stroke":session.terrain_stroke_json(),"state":face_state(&session)}))
        })
    }

    #[tool(
        description = "Inspect row-major terrain vertex IDs on 1–16 selected/explicit faces. Returns power, grid size and each vertex's row, column, world position, base position and alpha. preview=true reads rendered draft geometry. IDs match get_terrain_grids and native triangle indices; start-corner rotation changes their world placement. No source/history change. UI: Terrain triangles… → Selection type → Vertices · exact brush masks.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_terrain_vertices(
        &self,
        Parameters(args): Parameters<TerrainGridsArgs>,
    ) -> ToolResult {
        run(|| {
            let session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            session.terrain_vertices_json(&faces, args.preview.unwrap_or(false))
        })
    }

    #[tool(
        description = "Retain exact row-major terrain vertex subsets as cyan viewport/render_view guides and a brush mask. targets=[{face:{solid,side},vertices:[IDs]}], one entry per face, 1–16 faces; [] creates an empty selection. Replaces the shared triangle/vertex highlight atomically, returning kind, token and stale state. Invalid IDs/duplicate faces/unknown arguments preserve the prior selection. VMF/history/face selection stay fixed. Inspect with get_terrain_triangle_highlight, clear with clear_terrain_triangle_highlight and token. Use get_terrain_vertex_mask then mask_token on sculpt/blend. UI: Vertices selection checkboxes or Use selection."
    )]
    pub(in crate::mcp) fn highlight_terrain_vertices(
        &self,
        Parameters(args): Parameters<TerrainVertexTargetsArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let targets = args
                .targets
                .iter()
                .map(|target| (face_id(&target.face), target.vertices.clone()))
                .collect::<Vec<_>>();
            session.highlight_terrain_vertices(Actor::Agent, &targets)?;
            Ok(session.terrain_triangle_highlight_json())
        })
    }

    #[tool(
        description = "Pick the nearest vertex of the visible native terrain triangle hit by a finite world ray. faces defaults to selected terrain (1–16). Uses normalized rays, rendered drafts, front faces, nearest brush-face occlusion, hidden owners and tool-material visibility. Stable vertex-ID ties; returns hit=null when blocked/missed. Returns vertex, power, position, hit point and ray distance. Read-only; retain with highlight_terrain_vertices. UI: Vertices mode → Pick vertex in viewport….",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn pick_terrain_vertex(
        &self,
        Parameters(args): Parameters<TerrainTrianglePickArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let hit = session.pick_terrain_vertex(
                &Ray {
                    origin: vec3(args.origin),
                    direction: vec3(args.direction),
                },
                &faces,
            )?;
            Ok(
                json!({"hit":hit.map(|hit|json!({"solid":hit.vertex.face.solid,"side":hit.vertex.face.side,"vertex":hit.vertex.vertex,"power":hit.power,"position":hit.position.to_array(),"point":hit.point.to_array(),"distance":hit.distance}))}),
            )
        })
    }

    #[tool(
        description = "Frame explicit terrain vertex subsets at rendered draft positions. targets=[{face:{solid,side},vertices:[IDs]}], optional positive finite aspect (default16/9). Hidden/tool-filtered vertices are excluded. Empty/invalid targets or a different-power draft reject without moving the camera. Source/history/face selection stay fixed. UI: Vertices mode → Frame selected."
    )]
    pub(in crate::mcp) fn frame_terrain_vertices(
        &self,
        Parameters(args): Parameters<TerrainVertexFrameArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let targets = args
                .targets
                .iter()
                .map(|target| (face_id(&target.face), target.vertices.clone()))
                .collect::<Vec<_>>();
            session.frame_terrain_vertices(
                Actor::Agent,
                &targets,
                args.aspect.unwrap_or(16.0 / 9.0),
            )?;
            Ok(json!({"camera":camera_json(&session.camera())}))
        })
    }

    #[tool(
        description = "Preview exact world XYZ movement of a retained terrain vertex mask. offset=[x,y,z] is within±4096 per axis; mask_token from get_terrain_vertex_mask is required; faces defaults to selected terrain(1–16). Explicit vertices or triangle-derived unions work. Writes selected offset rows only; base/start corners, IDs, alpha, normals/distances, material/mapping, tags and flags remain. Zero is an exact no-op. Invalid coordinates/caches/stale or replaced tokens preserve the last valid preview. Inspect get_terrain_vertices(preview=true), then commit/cancel_face_preview with state.preview_token. Apply refreshes its matching selection and makes one Undo step. UI: Edit terrain… → Move selected vertices → World offset · units."
    )]
    pub(in crate::mcp) fn preview_terrain_vertex_move(
        &self,
        Parameters(args): Parameters<TerrainVertexMoveArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let changed = session.preview_terrain_vertex_move(
                Actor::Agent,
                &faces,
                vec3(args.offset),
                parse_token(&args.mask_token)?,
            )?;
            Ok(json!({"faces_changed":changed,"state":face_state(&session)}))
        })
    }

    #[tool(
        description = "Find exact terrain vertices inside a current select_volume(preview=true) box. volume_token is required; faces defaults to selected terrain (1–16). Uses displaced draft positions, hidden/tool filtering and inclusive point containment; both box rules produce the same vertex set. Read-only, returning targets for highlight_terrain_vertices. Stale/replaced volume tokens reject. UI: Vertices mode → Select in volume.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_terrain_volume_vertices(
        &self,
        Parameters(args): Parameters<TerrainVolumeArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let targets =
                session.terrain_vertices_in_volume(&faces, parse_token(&args.volume_token)?)?;
            Ok(
                json!({"targets":targets.iter().map(|(face,ids)|json!({"face":{"solid":face.solid,"side":face.side},"vertices":ids})).collect::<Vec<_>>()}),
            )
        })
    }

    #[tool(
        description = "Atomically retain exact terrain vertices inside a guarded volume box as the shared cyan selection/brush mask. Arguments and filtering match get_terrain_volume_vertices. Returns targets and highlight token/kind/stale state. Source/history/face selection stay fixed; invalid/stale batches preserve prior guides. UI: preview Select a volume… then Vertices mode → Select in volume."
    )]
    pub(in crate::mcp) fn select_terrain_vertices_in_volume(
        &self,
        Parameters(args): Parameters<TerrainVolumeArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let selected = session.select_terrain_vertices_in_volume(
                Actor::Agent,
                &faces,
                parse_token(&args.volume_token)?,
            )?;
            Ok(
                json!({"highlight":session.terrain_triangle_highlight_json(),"targets":selected.targets.iter().map(|(face,ids)|json!({"face":{"solid":face.solid,"side":face.side},"vertices":ids})).collect::<Vec<_>>()}),
            )
        })
    }

    #[tool(
        description = "Preview walkability/buildability classifications on arbitrary native triangle subsets in1–16 terrain faces. targets=[{face:{solid,side},triangles:[native IDs]}], one target per face. walkable/buildable accept automatic/enabled/disabled; omitted settings preserve current values. Automatic recomputes selected cached classification from source slope; enabled/disabled author a VMF force override. These are engine classifications, not collision switches. Geometry, alpha, IDs, other triangle tags and metadata remain. Invalid IDs/malformed rows reject atomically. Inspect get_terrain_triangles(preview=true), then commit/cancel_face_preview with state.preview_token. No removal authoring yet. UI: Terrain triangles…."
    )]
    pub(in crate::mcp) fn preview_terrain_triangle_tags(
        &self,
        Parameters(args): Parameters<TerrainTriangleTagsArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let targets = args
                .targets
                .iter()
                .map(|target| (face_id(&target.face), target.triangles.clone()))
                .collect::<Vec<_>>();
            let changed = session.preview_terrain_triangle_tags(
                Actor::Agent,
                &targets,
                map_core::displacement::triangles::Settings {
                    walkable: args.walkable.map(terrain_triangle_choice),
                    buildable: args.buildable.map(terrain_triangle_choice),
                },
            )?;
            Ok(json!({"faces_changed":changed,"state":face_state(&session)}))
        })
    }

    #[tool(
        description = "Select and highlight native triangle subsets from a current volume box in one shared atomic operation. volume_token and optional terrain faces match get_terrain_volume_triangles. Uses the volume's contained/intersecting rule, actual displaced geometry and hidden/tool filtering. Replaces triangle highlights while preserving face selection, VMF/history and the box. Returns highlight state and target lists. UI: Terrain triangles… → Select in volume."
    )]
    pub(in crate::mcp) fn select_terrain_triangles_in_volume(
        &self,
        Parameters(args): Parameters<TerrainVolumeArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let selected = session.select_terrain_triangles_in_volume(
                Actor::Agent,
                &faces,
                parse_token(&args.volume_token)?,
            )?;
            Ok(
                json!({"highlight":session.terrain_triangle_highlight_json(),"targets":selected.targets.iter().map(|(face,ids)|json!({"face":{"solid":face.solid,"side":face.side},"triangles":ids})).collect::<Vec<_>>()}),
            )
        })
    }

    #[tool(
        description = "Find native terrain triangles within a current select_volume(preview=true) box, using its contained/intersecting rule and actual displaced positions. volume_token is required; faces defaults to selected terrain faces (1–16). Hidden owners/tool-filtered faces stay excluded. Returns bounded targets suitable for highlight_terrain_triangles or classification edits, without changing VMF/history/face selection. Stale/replaced volume tokens reject. UI: preview a Select a volume… box, then Terrain triangles… → Select in volume.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_terrain_volume_triangles(
        &self,
        Parameters(args): Parameters<TerrainVolumeArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let targets =
                session.terrain_triangles_in_volume(&faces, parse_token(&args.volume_token)?)?;
            Ok(
                json!({"volume_token":args.volume_token,"triangle_count":targets.iter().map(|(_,ids)|ids.len()).sum::<usize>(),"targets":targets.iter().map(|(face,ids)|json!({"face":{"solid":face.solid,"side":face.side},"triangles":ids})).collect::<Vec<_>>()}),
            )
        })
    }

    #[tool(
        description = "Inspect explicit selected vertices or highlighted triangles as a deduplicated native vertex mask within selected/explicit terrain faces. Optional token guards the current highlight. Returns token, vertex count and per-face vertex IDs/triangle counts. Empty/stale/replaced selections reject. Use token as mask_token on preview_terrain_sculpt/blend. Only these vertices change. UI: Affect only selected terrain vertices.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_terrain_vertex_mask(
        &self,
        Parameters(args): Parameters<TerrainVertexMaskArgs>,
    ) -> ToolResult {
        run(|| {
            let session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            Ok(session
                .terrain_vertex_mask(&faces, args.token.as_deref().map(parse_token).transpose()?)?
                .json())
        })
    }

    #[tool(
        description = "Read the current shared terrain selection, kind (triangles/vertices), token and stale state. Does not change VMF/history/face selection. Stale highlights do not draw; query current triangle IDs before replacing them.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_terrain_triangle_highlight(&self) -> ToolResult {
        ok(self.session().terrain_triangle_highlight_json())
    }

    #[tool(
        description = "Highlight arbitrary native triangle subsets in both viewport and render_view. targets=[{face:{solid,side},triangles:[IDs]}] supports1–16 faces; [] creates an empty highlight. Does not alter VMF/history or face selection. Invalid batches preserve previous highlights. Returned token guards clear_terrain_triangle_highlight; replacing the map/source or previewing another grid power makes this highlight stale. UI: Terrain triangles… checkboxes and Select surface."
    )]
    pub(in crate::mcp) fn highlight_terrain_triangles(
        &self,
        Parameters(args): Parameters<TerrainTriangleTargetsArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let targets = args
                .targets
                .iter()
                .map(|target| (face_id(&target.face), target.triangles.clone()))
                .collect::<Vec<_>>();
            session.highlight_terrain_triangles(Actor::Agent, &targets)?;
            Ok(session.terrain_triangle_highlight_json())
        })
    }

    #[tool(
        description = "Clear the shared terrain triangle/vertex highlight, optionally guarded by its token. A replaced token rejects without clearing another actor's guides. No map/history/selection edit. UI: Cancel or Clear selection in Terrain triangles…."
    )]
    pub(in crate::mcp) fn clear_terrain_triangle_highlight(
        &self,
        Parameters(args): Parameters<FacePreviewArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.clear_terrain_triangle_highlight(
                args.token.as_deref().map(parse_token).transpose()?,
            )?;
            Ok(session.terrain_triangle_highlight_json())
        })
    }

    #[tool(
        description = "Pick a native terrain triangle from a finite world ray, normalized internally. faces defaults to selected terrain faces (1–16). Uses rendered preview geometry, front faces, hidden owners, tool-material visibility and nearest visible brush-face occlusion. Returns hit=null if blocked or missed. IDs follow get_terrain_triangles(preview=true). Read-only: use highlight_terrain_triangles for guides and preview_terrain_triangle_tags for edits. UI: Pick triangle in viewport….",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn pick_terrain_triangle(
        &self,
        Parameters(args): Parameters<TerrainTrianglePickArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let hit = session.pick_terrain_triangle(
                &Ray {
                    origin: vec3(args.origin),
                    direction: vec3(args.direction),
                },
                &faces,
            )?;
            Ok(
                json!({"hit":hit.map(|hit|json!({"solid":hit.triangle.face.solid,"side":hit.triangle.face.side,"triangle":hit.triangle.triangle,"power":hit.power,"point":hit.point.to_array(),"distance":hit.distance}))}),
            )
        })
    }

    #[tool(
        description = "Frame visible terrain triangle subsets using the same rendered source positions and camera policy as the human Frame selected control. targets=[{face:{solid,side},triangles:[IDs]}], optional positive finite aspect (default16/9). No VMF/history/face selection changes. Hidden/tool-filtered triangles do not frame; invalid/empty selections reject without moving the camera."
    )]
    pub(in crate::mcp) fn frame_terrain_triangles(
        &self,
        Parameters(args): Parameters<TerrainTriangleFrameArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let targets = args
                .targets
                .iter()
                .map(|target| (face_id(&target.face), target.triangles.clone()))
                .collect::<Vec<_>>();
            session.frame_terrain_triangles(
                Actor::Agent,
                &targets,
                args.aspect.unwrap_or(16.0 / 9.0),
            )?;
            Ok(json!({"camera":camera_json(&session.camera())}))
        })
    }

    #[tool(
        description = "Preview changing selected or explicit terrain grids to power 2/3/4. Refinement samples the old triangles, preserving shared heights/blends; coarsening can lose detail. Preserves start corners, IDs and face metadata. Rejects malformed data, mixed triangle tags, restricted vertex masks, nonzero offset normals and extra terrain blocks atomically. Preserves the subdivision marker; the marker alone does not block resizing. Source/history stay unchanged. Review get_terrain_grids(preview=true) or render_view, then commit_face_preview or cancel_face_preview with state.preview_token. UI: Edit terrain… with Grid resolution."
    )]
    pub(in crate::mcp) fn preview_terrain_resolution(
        &self,
        Parameters(args): Parameters<TerrainResolutionArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let count = session.preview_terrain_resolution(Actor::Agent, &faces, args.power)?;
            Ok(json!({"faces_changed":count,"state":face_state(&session)}))
        })
    }

    #[tool(
        description = "Inspect eligible selected or explicit world quad faces and their four ordered world-space corners before terrain creation. Indices 0–3 match create_terrain.start_corner. Rows point toward the next corner, columns toward the previous one. Invalid selections return an error without changing the map.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_terrain_creation(
        &self,
        Parameters(args): Parameters<TerrainFacesArgs>,
    ) -> ToolResult {
        run(|| {
            let session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let faces = session.terrain_creation_faces(&faces)?;
            Ok(
                json!({"faces": faces.iter().map(|face|json!({"solid":face.face.solid,"side":face.face.side,"corners":face.corners.map(|point|point.to_array())})).collect::<Vec<_>>(), "default_start_corner":0}),
            )
        })
    }

    #[tool(
        description = "Create flat terrain on 1–16 selected or explicit world quad faces, one per ordinary parent brush. power is 2, 3 or 4 (default 2), producing 5x5, 9x9 or 17x17 grids. Preserves IDs, materials, projections and lighting; rejects existing terrain, entity parents, ambiguous/nonquad targets and invalid powers atomically. One undo step. start_corner is 0–3 (default 0) in the winding order returned by get_terrain_creation; rows follow the next corner and columns the previous one. Parent brushes do not seal maps or become collision boxes. Sculpt and blend tools use shared terrain previews. UI: Terrain grid, Start corner and Create terrain in the face inspector."
    )]
    pub(in crate::mcp) fn create_terrain(
        &self,
        Parameters(args): Parameters<CreateTerrainArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let power = args.power.unwrap_or(2);
            let start_corner = args.start_corner.unwrap_or(0);
            let count = session.create_terrain(Actor::Agent, &faces, power, start_corner)?;
            Ok(
                json!({"surfaces_created": count, "power": power, "grid_size": (1_u32 << power) + 1, "start_corner": start_corner, "state": face_state(&session)}),
            )
        })
    }

    #[tool(
        description = "Remove terrain from 1–16 selected or explicit faces in one atomic undoable edit. Every target must contain terrain; missing/ambiguous/ordinary targets reject the entire batch. Preserves underlying brush geometry,IDs,material,mapping and lighting; undo restores all imported grid rows/extras. The parent becomes a solid brush once no terrain faces remain. Can clean up imported entity terrain. UI: Remove terrain in the face inspector."
    )]
    pub(in crate::mcp) fn remove_terrain(
        &self,
        Parameters(args): Parameters<TerrainFacesArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let count = session.remove_terrain(Actor::Agent, &faces)?;
            Ok(json!({"surfaces_removed":count,"state":face_state(&session)}))
        })
    }
}
