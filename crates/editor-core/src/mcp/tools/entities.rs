use super::super::*;

#[tool_router(router = entities_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Capture whole source entities and their source-qualified connection rows for batch I/O editing. ids defaults to the current whole-entity selection. Returns snapshot, sources and rows with source, index and opaque identity; never infer that equal indices mean equal connections on different entities. Bounded to 1–64 distinct sources, 256 rows and 64 KiB captured text. Read-only; selection, map and history stay unchanged.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_connection_batch_state(
        &self,
        Parameters(args): Parameters<ConnectionBatchSourcesArgs>,
    ) -> ToolResult {
        run(|| {
            let session = self.session();
            let sources = connection_source_ids(args.ids.as_deref())?;
            session.connection_batch_state(sources.as_deref())
        })
    }

    #[tool(
        description = "Begin a captured connection batch through the shared Captured connections controls. Supply snapshot from get_connection_batch_state and action:add|edit|remove. Add takes ids of whole sources (or the current selection) and no rows; it starts one new connection per captured source with blank output/target/input and delay0/times-1. Edit/Remove take exact rows [{source:entity_ID,identity:opaque_row_identity}] and no ids; rows can span sources and different ordinals. Captures map generation/revision and definitions. Edit reports common fields as values or null for Mixed and starts with every field unchanged; use update_connection_batch to choose replacements. All actions report actual affected rows and per-source diagnostics. Custom/unresolved I/O remains warning-only as in the single editor. Malformed imported field counts require single-row repair before Edit; Remove retains exact raw identities. Replaces only the existing connection draft, leaves map/history unchanged, and retains captured sources across selection changes. Cancel through cancel_connection_edit with the returned token."
    )]
    pub(in crate::mcp) fn begin_connection_batch(
        &self,
        Parameters(args): Parameters<ConnectionBatchBeginArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let sources = connection_source_ids(args.ids.as_deref())?;
            let rows = args.row_targets()?;
            session.begin_connection_batch(
                args.action,
                sources.as_deref(),
                &rows,
                &args.snapshot,
            )?;
            Ok(session.connection_batch_report())
        })
    }

    #[tool(
        description = "Update the owned connection batch by token. patch contains optional output,target,input,parameter,delay,times strings. For Edit, this replaces the complete pending field mask: omitted/null fields keep each original row's value, including mixed values; explicit empty strings clear a field where allowed. Add updates only supplied fields of its new common connection. Numeric strings may be unfinished and return validation feedback without writing. Remove has no fields. Every row retains its original separator; per-source FGD and target diagnostics remain explicit. Selection never retargets the batch. Invalid plans cannot Apply and Cancel changes nothing."
    )]
    pub(in crate::mcp) fn update_connection_batch(
        &self,
        Parameters(args): Parameters<ConnectionBatchUpdateArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.update_connection_batch(args.token, args.patch)?;
            Ok(session.connection_batch_report())
        })
    }

    #[tool(
        description = "Apply the owned captured connection batch by token in one atomic Undo step. Revalidates every source and exact raw row, map generation/revision, definitions, requested fields and original separators before changing anything. Add creates one reviewed connection on every source; Edit replaces only explicitly chosen fields on every selected row; Remove deletes the exact captured rows in safe descending per-source order. Missing sources, foreign/stale row identities, stale definitions, invalid fields or work bounds reject the whole operation and retain the draft. Custom or unresolved I/O warnings do not silently drop rows. Returns affected_rows, sources and changed; a semantically unchanged request creates no extra history. Cancel uses cancel_connection_edit."
    )]
    pub(in crate::mcp) fn commit_connection_batch(
        &self,
        Parameters(args): Parameters<ConnectionBatchTokenArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .commit_connection_batch(Actor::Agent, args.token)
        })
    }

    #[tool(
        description = "Inspect world settings or common entity properties with typed controls, mixed values, individual stored values, FGD defaults and mixed flags. Absent values are reported separately from explicit overrides. World settings include Emission file, default-off Exact emission visibility (_hammerr_emission_exact_visibility, exactly 0/1) and default-off Native direct emission (_hammerr_emission_native_direct, exactly 0/1). Exact visibility requires a nonblank valid emission path and requests exact rational weighted finite-source byte-coverage partitioning for uniquely owned world/detail or eligible stationary Shadows Yes func_brush emitting faces at root or assembled-child scope. Exact sources use a certified ordinary native outward triangle fan with a simple convex projection, without primitive overrides or displacement. Planar and nonplanar faces are admitted only within that fan certificate. Each nondegenerate original native triangle uses its actual plane for facing, support, clipping and the finite-area kernel; all source coordinates are retained. Nonzero-residual planar texture partitions keep their exact original-face coverage proof. Nonplanar textures require exact subcoverage of each original native fan triangle before RGB groups merge. The native fan must cover the actual surface once. Each subcoverage must cover its original native triangle once, with exact target-plane support, positive outward area, target containment, pairwise disjoint interiors and exact total projected area across all RGB groups, including black, with no floating coverage allowance. Existing planar, sampled and native point approximation paths are unchanged. Colour selection retains nearest mip0 RGB and binary64 UV semantics; the coverage proof does not certify real-arithmetic UV boundaries or native filtering parity. The retained plane keeps original custody and native direct-emission semantics; every applicable owning hull still blocks light to other receivers, including real source self-occlusion. The whole emitting BSP face is excluded from its own emission; fan triangles do not exchange that face's light, and receiver grids stay unchanged, including retained-plane sampling for brush receivers. It uses applicable original opaque, binary-cutout or admitted scalar/RGB fractional brush hulls certified closed, bounded and convex from their final planes, including oblique planes, plus actual two-sided native terrain and static-prop mesh triangles with the same admitted coverage. Masks reuse retained prepared material byte coverage, transforms, nearest mip0 sampling and clamp/repeat. Brush and terrain masks require Brush texture shadows; prop masks require the original MDL CAST_TEXTURE_SHADOWS flag. Colored fractional coverage additionally requires Colored texture shadows. No prerequisite is enabled automatically. Independent primitive crossings and distinct hull entry/exit events add coverage with saturation per byte channel. Exactly tied projected faces of one hull event use the componentwise maximum. Final fragment visibility is (255-coverage)/255; sheet visibilities are not multiplied. Strict segment endpoints remain; sampled near-tie tolerance bands are not used. Integral representation is selected from each original source triangle plane before integration: axial planes retain certified native on-plane fragments and the existing analytic kernel; oblique planes retain their exact rational source domains and use an analytic boundary integral on the same exact rational domains. Ray cross/dot products remain exact; positive square roots and atan2 use certified outward enclosures. Bounded interval atan uses half-angle reduction and alternating Horner evaluation through degree 33 with a certified tail, plus an exact rational Machin bracket for pi. Rational integration cells count completed domain triangles without subdivision. Original source geometry and shadow points are unchanged; no conversion-error fallback selects a method. Dimensionless bounds remain 1e-12 per source integration and 1e-9 per receiver, before radiance. Terrain and prop emitting sources, moving, parented or unrequested brush-model emitting sources, and unqualified prop shadow providers remain unsupported. Uncertified nonbinary masked coplanar positive-footprint events and shared native terrain edge coplanar charts with positive strict-ended footprints also reject. Unsupported providers, uncertain support, native conversion or rational numerical enclosures and exhausted bounds reject compilation without sampled fallback; Fullbright validates without exact geometry admission. The native policy requires a nonblank valid emission path and opts into centroid-point native selected/folded runtime direct lighting, replacing intrinsic direct emission in indexed probes while preserving baked static receivers and reflected ambient; the exact visibility policy selects emission shadow handling. Native slots/folding are an approximation; full-VHV props affected by dynamic lights may also receive inherited static-cache lighting. World settings include Liquid current · world units/s (_hammerr_water_current_velocity), an XYZ vector of at most 128 units/s defaulting to zero, and Liquid pressure damping (_hammerr_water_damping), a dimensionless number from 0 to 0.01 defaulting to 0.01. These intrinsic defaults do not write overrides or invent FGD defaults. Damping affects the native projected-pressure term; zero retains the fixed pressure term. Stored controls require engine physics and emitted structural-world WATER or supported opaque SLIME and do not drive player/NPC movement. Use the shared property draft to stage, commit or remove overrides.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_property_inspector(
        &self,
        Parameters(args): Parameters<PropertyTargetArgs>,
    ) -> ToolResult {
        run(|| {
            let session = self.session();
            let report = session.inspect_properties(args.target(&session)?)?;
            serde_json::to_value(report).map_err(|error| error.to_string())
        })
    }

    #[tool(
        description = "Begin a shared cancellable property draft for world settings or selected entities. Returns a token required by update/commit/cancel. Replaces any previous property draft. handle identifies kind=property_edit with a decimal-string token, separate from source revisions. Starting another property draft replaces ownership; map or definition changes make it stale. Selection changes retain the captured targets; they do not retarget the draft."
    )]
    pub(in crate::mcp) fn begin_property_edit(
        &self,
        Parameters(args): Parameters<PropertyTargetArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let target = args.target(&session)?;
            session.begin_property_edit(target)?;
            session.property_edit_report()
        })
    }

    #[tool(
        description = "Replace pending property changes and validate every target before accepting. set writes a value; remove deletes the override; default explicitly writes the FGD default, or 0 for the intrinsic Exact emission visibility and Native direct emission policies; flag changes one declared bit preserving others; component edits one vector/color component. Names use the dedicated safe rename workflow. Whole-value $identifier tokens are supported only in the class-specific fields reported by get_compile_settings.light_value_bindings, using canonical replace01–replace10 values on containing func_instance helpers. The shared catalog gives fields, ranges, precedence and binding rules; ordinary effective validation still checks known values without defaulting unknowns. Templates remain editable and savable. Standalone compilation rejects unresolved templates in every mode; authored illumination excludes them. Unresolved values retain raw text controls and their FGD types without evaluated defaults, swatches or intensity. World emission policies and file changes validate together after the complete change list: disabling both policies and clearing the file may occur in any order, while enabling either policy without a nonblank valid file rejects atomically. Only canonical unique policy keys are accepted; case variants, duplicate aliases and values other than exactly 0 or 1 reject. No map mutation until commit. handle identifies kind=property_edit with a decimal-string token, separate from source revisions. Starting another property draft replaces ownership; map or definition changes make it stale. Selection changes retain the captured targets; they do not retarget the draft."
    )]
    pub(in crate::mcp) fn update_property_edit(
        &self,
        Parameters(args): Parameters<PropertyEditArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.update_property_edit(args.token, args.changes)?;
            session.property_edit_report()
        })
    }

    #[tool(
        description = "Read the shared property draft and preview; a closed or stale map/FGD draft returns an error. handle identifies the active property_edit token as a decimal string; commit/cancel return handle=null.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_property_edit(&self) -> ToolResult {
        run(|| {
            let session = self.session();
            session.property_edit_report()
        })
    }

    #[tool(
        description = "Apply the property draft as one undoable edit. Rejects stale tokens, map revisions, map replacement and changed entity definitions."
    )]
    pub(in crate::mcp) fn commit_property_edit(
        &self,
        Parameters(args): Parameters<PropertyTokenArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .commit_property_edit(Actor::Agent, args.token)?;
            Ok(json!({"handle":null,"applied":true}))
        })
    }

    #[tool(
        description = "Discard the shared property draft without changing the map or undo history. A replaced draft cannot be cancelled by an old token."
    )]
    pub(in crate::mcp) fn cancel_property_edit(
        &self,
        Parameters(args): Parameters<PropertyTokenArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().cancel_property_edit(args.token)?;
            Ok(json!({"handle":null,"cancelled":true}))
        })
    }

    #[tool(
        description = "Inspect an entity using the same human-readable labels, common/advanced fields, typed controls, colors, brightness, choices and FGD helper bounds/status/errors as the property panel. helpers.sprite_resource reports static Sprite resource guides, exact resolved VMT/provenance, positive-scale dimensions and unevaluated runtime fields; Entity images controls admission and drawing.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_entity_inspector(
        &self,
        Parameters(args): Parameters<InspectorArgs>,
    ) -> ToolResult {
        run(|| {
            let session = self.session();
            let id = single_entity(&session, args.id.as_deref())?;
            let inspector = session.entity_inspector(id)?;
            serde_json::to_value(inspector).map_err(|error| error.to_string())
        })
    }

    #[tool(
        description = "Set an entity color and optional brightness using the same color picker operation as the GUI. Supports color255 and normalized color1; preserves light brightness when omitted. rendercolor edits RGB only, preserves an authored fourth component and rejects intensity."
    )]
    pub(in crate::mcp) fn set_entity_color(
        &self,
        Parameters(args): Parameters<EntityColorArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let id = single_entity(&session, args.id.as_deref())?;
            session.set_entity_color(Actor::Agent, id, &args.key, args.color, args.intensity)?;
            detail(session.document(), ObjectId::Entity(id))
        })
    }

    #[tool(
        description = "Convert a color without editing the map. space is rgb255 (whole channels 0..255), rgb1 (normalized 0..1), hsv or hsl. HSV/HSL use hue 0..360 degrees and saturation/value/lightness 0..100 percent. Returns exact normalized RGB, nearest-byte RGB, HSV and HSL. Hue 360 equals 0; achromatic output hue is 0. No gamma or brightness conversion; all channels must be finite and within range.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn convert_color(
        &self,
        Parameters(args): Parameters<crate::color::Input>,
    ) -> ToolResult {
        run(|| {
            serde_json::to_value(Session::convert_color(args)?).map_err(|error| error.to_string())
        })
    }

    #[tool(
        description = "Apply an RGB, HSV or HSL color through the same Session operation as Apply color. color={space,channels}: rgb255 takes whole 0..255 channels; rgb1 takes 0..1 channels; hsv/hsl take hue 0..360 degrees and saturation/value/lightness 0..100 percent. color1 stores normalized precision; color255 rounds to nearest bytes. Optional intensity preserves existing brightness when omitted. rendercolor preserves an authored fourth component and rejects intensity. Validates before one undoable edit; malformed/nonfinite/out-of-range input never clamps or mutates the map."
    )]
    pub(in crate::mcp) fn set_entity_color_space(
        &self,
        Parameters(args): Parameters<EntityColorSpaceArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let id = single_entity(&session, args.id.as_deref())?;
            session.set_entity_color_space(
                Actor::Agent,
                id,
                &args.key,
                args.color,
                args.intensity,
            )?;
            detail(session.document(), ObjectId::Entity(id))
        })
    }

    #[tool(
        description = "Create and select a point entity, including lights and player spawns. Returns bounds_kind=point_helper and authored_bounds for its authored helper box, not model clearance. Use get_objects for editor presentation bounds or inspect_entity_model for resolved model geometry. Applies inherited FGD defaults and validates declared property types. Custom classes and keys are supported. Light brightness uses RGB plus intensity, e.g. _light='255 220 180 400'; Direct LDR compiles static and styled lights with optional diffuse bounces. Whole-value $identifier tokens are supported only in the class-specific fields reported by get_compile_settings.light_value_bindings, using canonical replace01–replace10 values on containing func_instance helpers. The shared catalog gives fields, ranges, precedence and binding rules; ordinary effective validation still checks known values without defaulting unknowns. Templates remain editable and savable. Standalone compilation rejects unresolved templates in every mode; authored illumination excludes them. Unresolved gains also exclude authored LDR preview and fail standalone Fullbright compilation."
    )]
    pub(in crate::mcp) fn create_entity(
        &self,
        Parameters(args): Parameters<CreateEntityArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let id = session.create_entity(
                Actor::Agent,
                &args.classname,
                args.origin.map(vec3),
                args.properties.unwrap_or_default(),
            )?;
            detail(session.document(), id)
        })
    }

    #[tool(
        description = "Search the loaded FGD point and brush entity classes by classname or description. Returns class summaries for human and agent entity creation.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn search_entity_classes(
        &self,
        Parameters(args): Parameters<SearchEntityClassesArgs>,
    ) -> ToolResult {
        run(|| {
            let kind = match args.kind.as_deref() {
                None => None,
                Some("point") => Some(ClassKind::Point),
                Some("solid") => Some(ClassKind::Solid),
                Some(_) => return Err("kind must be point or solid".into()),
            };
            let session = self.session();
            let classes: Vec<Value> = session
                .search_entity_classes(
                    args.query.as_deref().unwrap_or(""),
                    kind,
                    args.limit.unwrap_or(100),
                )
                .iter()
                .map(|class| {
                    json!({"classname": class.name, "kind": class.kind.label(), "description": class.description})
                })
                .collect();
            Ok(json!({"classes": classes, "definitions": definitions_json(&session)}))
        })
    }

    #[tool(
        description = "Inspect a loaded entity class: inherited property types, descriptions, defaults, choices, spawnflags, helpers, inputs and outputs.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_entity_class(
        &self,
        Parameters(args): Parameters<EntityClassArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .definitions()
                .class(&args.classname)
                .map(class_json)
                .ok_or_else(|| {
                    format!(
                        "no definition for {}; use search_entity_classes or load_entity_definitions",
                        args.classname
                    )
                })
        })
    }

    #[tool(
        description = "Inspect an entity's outgoing and incoming I/O connections, matching targets and validation warnings. Connection indices are zero-based; use generation/revision to guard edits.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_entity_connections(
        &self,
        Parameters(args): Parameters<EntityArgs>,
    ) -> ToolResult {
        run(|| {
            let session = self.session();
            let source = single_entity(&session, args.id.as_deref())?;
            Ok(
                json!({"source":id_string(ObjectId::Entity(source)), "generation":session.document_generation(), "revision":session.document().revision(),
                "outgoing":session.connection_records(source)?.into_iter().map(connection_json).collect::<Vec<_>>(),
                "incoming":session.incoming_connections(source)?.into_iter().map(connection_json).collect::<Vec<_>>(),
                "draft":connection_draft_json(&session)}),
            )
        })
    }

    #[tool(
        description = "Browse named map targets and procedural targets, source FGD outputs and the matching target classes' inputs. Suggestions are optional: custom names and I/O remain supported.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_connection_choices(
        &self,
        Parameters(args): Parameters<ConnectionChoicesArgs>,
    ) -> ToolResult {
        run(|| {
            let session = self.session();
            let source = single_entity(&session, args.id.as_deref())?;
            let io_json = |io: source_formats::fgd::Io| json!({"name":io.name,"type":io.kind,"description":io.description});
            Ok(
                json!({"outputs":session.connection_outputs(source).into_iter().map(io_json).collect::<Vec<_>>(),
                "inputs":session.connection_inputs(source, args.target.as_deref().unwrap_or_default()).into_iter().map(io_json).collect::<Vec<_>>(),
                "targets":session.connection_targets(args.query.as_deref().unwrap_or_default()).into_iter().map(|(name,description)| json!({"name":name,"description":description})).collect::<Vec<_>>() }),
            )
        })
    }

    #[tool(
        description = "Begin a shared, cancellable entity I/O form. Omit index to add; supply an outgoing index to edit. Leaves the map and undo history unchanged. handle identifies kind=connection_edit with a decimal-string token, separate from generation/revision. Starting another connection draft replaces ownership; map changes make it stale and clear its active handle. Selection changes retain the captured source; they do not retarget the draft."
    )]
    pub(in crate::mcp) fn begin_connection_edit(
        &self,
        Parameters(args): Parameters<ConnectionArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            connection_guard(&session, &args)?;
            let source = single_entity(&session, args.id.as_deref())?;
            session.begin_connection_edit(source, args.index)?;
            Ok(connection_draft_json(&session))
        })
    }

    #[tool(
        description = "Change fields in the shared connection draft. Unfinished or invalid values return feedback without editing the map. Delay and firing count are strings; -1 is unlimited. handle identifies kind=connection_edit with a decimal-string token, separate from generation/revision. Starting another connection draft replaces ownership; map changes make it stale and clear its active handle. Selection changes retain the captured source; they do not retarget the draft."
    )]
    pub(in crate::mcp) fn update_connection_draft(
        &self,
        Parameters(args): Parameters<ConnectionFieldsArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            if let Some(token) = args.token {
                session.check_connection_draft(token)?;
            }
            let fields = &session
                .connection_draft()
                .ok_or("Begin a connection edit first")?
                .fields;
            let fields = ConnectionFields {
                output: args.output.unwrap_or_else(|| fields.output.clone()),
                target: args.target.unwrap_or_else(|| fields.target.clone()),
                input: args.input.unwrap_or_else(|| fields.input.clone()),
                parameter: args.parameter.unwrap_or_else(|| fields.parameter.clone()),
                delay: args.delay.unwrap_or_else(|| fields.delay.clone()),
                times: args.times.unwrap_or_else(|| fields.times.clone()),
            };
            session.update_connection_draft(fields)?;
            Ok(connection_draft_json(&session))
        })
    }

    #[tool(
        description = "Apply the shared I/O draft in one undo step. Rejects invalid fields and drafts made stale by map edits; unresolved/custom I/O generates warnings, not rejection."
    )]
    pub(in crate::mcp) fn commit_connection_edit(
        &self,
        Parameters(args): Parameters<ConnectionDraftArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            if let Some(token) = args.token {
                session.check_connection_draft(token)?;
            }
            let source = session
                .connection_draft()
                .ok_or("Begin a connection edit first")?
                .source;
            let index = session.commit_connection_edit(Actor::Agent)?;
            let mut report = connection_json(
                session
                    .connection_records(source)?
                    .into_iter()
                    .nth(index)
                    .ok_or("Connection no longer exists")?,
            );
            report["handle"] = Value::Null;
            Ok(report)
        })
    }

    #[tool(
        description = "Discard the shared connection draft without changing the map or undo history."
    )]
    pub(in crate::mcp) fn cancel_connection_edit(
        &self,
        Parameters(args): Parameters<ConnectionDraftArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            if let Some(token) = args.token {
                session.check_connection_draft(token)?;
            }
            session.cancel_connection_edit();
            Ok(json!({"handle":null,"cancelled":true}))
        })
    }

    #[tool(
        description = "Delete one outgoing entity connection by index in one undo step. Supply generation/revision from the inspected list to reject stale indices.",
        annotations(destructive_hint = true)
    )]
    pub(in crate::mcp) fn delete_entity_connection(
        &self,
        Parameters(args): Parameters<ConnectionArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            connection_guard(&session, &args)?;
            let source = single_entity(&session, args.id.as_deref())?;
            let index = args
                .index
                .ok_or("Choose the outgoing connection index to delete")?;
            session.delete_entity_connection(Actor::Agent, source, index)?;
            Ok(json!({"deleted":index,"source":id_string(ObjectId::Entity(source))}))
        })
    }

    #[tool(
        description = "Inspect incoming I/O targets, entity reference parameters, FGD reference properties and exact untyped text candidates. Includes hidden sources. Use generation/revision guards when beginning a rename.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_entity_references(
        &self,
        Parameters(args): Parameters<EntityArgs>,
    ) -> ToolResult {
        run(|| {
            let session = self.session();
            let entity = single_entity(&session, args.id.as_deref())?;
            let references = session.entity_references(entity)?;
            Ok(json!({"entity":id_string(ObjectId::Entity(entity)),
                "generation":session.document_generation(), "revision":session.document().revision(),
                "references":references.iter().map(reference_json).collect::<Vec<_>>(),
                "draft":rename_draft_json(&session)}))
        })
    }

    #[tool(
        description = "Begin a shared entity rename preview without editing the map. Exact references can be propagated atomically; shared names require an explicit group rename. Wildcards and untyped text remain unchanged for review."
    )]
    pub(in crate::mcp) fn begin_entity_rename(
        &self,
        Parameters(args): Parameters<RenameArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            match (args.generation, args.revision) {
                (Some(generation), Some(revision))
                    if generation != session.document_generation()
                        || revision != session.document().revision() =>
                {
                    return Err("Map changed. Inspect entity references again.".into());
                }
                (Some(_), Some(_)) | (None, None) => (),
                _ => {
                    return Err(
                        "Supply both generation and revision for the stale-map guard".into(),
                    );
                }
            }
            let entity = single_entity(&session, args.id.as_deref())?;
            session.begin_entity_rename(entity)?;
            Ok(rename_draft_json(&session))
        })
    }

    #[tool(
        description = "Change the proposed name or group option and inspect all affected references. No map edit. accept_review acknowledges unchanged references; changing name/group resets it, so review the preview before acknowledging in a subsequent call."
    )]
    pub(in crate::mcp) fn update_entity_rename(
        &self,
        Parameters(args): Parameters<RenameFieldsArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            if let Some(token) = args.token {
                session.check_rename_draft(token)?;
            }
            let draft = session
                .rename_draft()
                .ok_or("Begin an entity rename first")?;
            let (name, group, review) = (
                args.new_name.unwrap_or_else(|| draft.new_name.clone()),
                args.rename_group.unwrap_or(draft.rename_group),
                args.accept_review.unwrap_or(draft.accept_review),
            );
            session.update_entity_rename(name, group, review)?;
            if let Some(show_details) = args.show_details {
                session.set_rename_details(show_details)?;
            }
            Ok(rename_draft_json(&session))
        })
    }

    #[tool(
        description = "Apply the shared rename and exact reference changes in one undo step. Rejects occupied names, invalid names, unreviewed ambiguous references, replaced drafts and map/FGD changes. Preserves imported connection formatting and unrelated data."
    )]
    pub(in crate::mcp) fn commit_entity_rename(
        &self,
        Parameters(args): Parameters<ConnectionDraftArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            if let Some(token) = args.token {
                session.check_rename_draft(token)?;
            }
            let plan = session.commit_entity_rename(Actor::Agent)?;
            Ok(rename_plan_json(&session, &plan))
        })
    }

    #[tool(
        description = "Discard the shared entity rename preview without changing the map or undo history."
    )]
    pub(in crate::mcp) fn cancel_entity_rename(
        &self,
        Parameters(args): Parameters<ConnectionDraftArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            if let Some(token) = args.token {
                session.check_rename_draft(token)?;
            }
            session.cancel_entity_rename();
            Ok(json!({"cancelled":true}))
        })
    }

    #[tool(
        description = "Read FGD availability, catalog serial, class count, source files, directory material exclusions, automatic visibility groups with literal class members, and @version resource freshness diagnostics with file/line provenance. @version does not select grammar or certify full vendor compatibility; absence is valid. Material exclusions compare case-insensitive relative-directory prefixes without wildcard or component-boundary matching; root material filenames are unaffected. Auto-vis members match complete entity classnames case-insensitively, without wildcard or inheritance expansion. Effective @mapsize includes authored values and sorted bounds; it is informational and does not set editor or compiler coordinate limits.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_entity_definitions(&self) -> ToolResult {
        run(|| Ok(definitions_json(&self.session())))
    }

    #[tool(
        description = "Load an FGD and its includes, resolving inherited definitions and ordered @ExtendClass merges, including extension bases. Updates the shared class catalog, optional material exclusion rules and automatic visibility categories together; existing map data is preserved. Returns source provenance, resource version diagnostics and informational @mapsize metadata. Unknown catalog directives, missing extension bases and exhausted metadata bounds reject; failed loads preserve the previous catalog. @version is vendor resource freshness metadata, not a grammar switch or a requirement to load a vendor FGD."
    )]
    pub(in crate::mcp) async fn load_entity_definitions(
        &self,
        Parameters(args): Parameters<DefinitionsArgs>,
    ) -> ToolResult {
        let request = match self.session().begin_definitions_load(Path::new(&args.path)) {
            Ok(request) => request,
            Err(error) => return fail(error),
        };
        let token = request.token();
        let session = self.session.clone();
        match tokio::task::spawn_blocking(self.worker(move || {
            let completed = request.run();
            let mut session = lock(&session);
            session.finish_definitions_load(Actor::Agent, completed)?;
            Ok::<_, String>(definitions_json(&session))
        }))
        .await
        {
            Ok(Ok(value)) => ok(value),
            Ok(Err(error)) => fail(error),
            Err(error) => {
                let message = format!("FGD load worker failed: {error}");
                self.session()
                    .fail_definitions_load(Actor::Agent, token, message.clone());
                fail(message)
            }
        }
    }

    #[tool(
        description = "Set and/or remove entity properties (including classname, origin, angles, targetname and light brightness). Validates declared FGD types, choices and read-only fields atomically. Unknown keys are preserved and editable. Whole-value $identifier tokens are supported only in the class-specific fields reported by get_compile_settings.light_value_bindings, using canonical replace01–replace10 values on containing func_instance helpers. The shared catalog gives fields, ranges, precedence and binding rules; ordinary effective validation still checks known values without defaulting unknowns. Templates remain editable and savable. Standalone compilation rejects unresolved templates in every mode; authored illumination excludes them. Unresolved gains also exclude authored LDR preview and fail standalone Fullbright compilation."
    )]
    pub(in crate::mcp) fn set_entity_properties(
        &self,
        Parameters(args): Parameters<SetPropertiesArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let entity = single_entity(&session, args.id.as_deref())?;
            let mut changes: Vec<(String, Option<String>)> = args
                .set
                .unwrap_or_default()
                .into_iter()
                .map(|(k, v)| (k, Some(v)))
                .collect();
            changes.extend(
                args.remove
                    .unwrap_or_default()
                    .into_iter()
                    .map(|k| (k, None)),
            );
            session.set_entity_properties(Actor::Agent, entity, &changes)?;
            detail(session.document(), ObjectId::Entity(entity))
        })
    }

    #[tool(
        description = "Turn world brushes into one brush entity (func_detail, func_door, trigger_*, ...) and select it."
    )]
    pub(in crate::mcp) fn tie_to_entity(
        &self,
        Parameters(args): Parameters<TieArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let ids = ids_or_selection(&session, args.ids.as_deref())?;
            let entity = session.tie_to_entity(
                Actor::Agent,
                &ids,
                &args.classname,
                args.properties.unwrap_or_default(),
            )?;
            detail(session.document(), entity)
        })
    }

    #[tool(description = "Move a brush entity's brushes back into the world, deleting the entity.")]
    pub(in crate::mcp) fn move_to_world(
        &self,
        Parameters(args): Parameters<EntityArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let entity = single_entity(&session, args.id.as_deref())?;
            let solids = session.move_to_world(Actor::Agent, entity)?;
            Ok(json!({ "solids": solids.iter().map(|&id| id_string(id)).collect::<Vec<_>>() }))
        })
    }
}
