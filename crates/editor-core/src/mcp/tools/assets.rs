use super::super::*;

#[cfg(test)]
mod model_scale_tests;

#[tool_router(router = assets_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Browse mounted WAV/MP3 files and manifest sound-script names. Optional query, group (all/files/scripts/used/folder:…/script:…), offset and limit1–100. Returns variants, groups and load warnings. Shared with an entity sound field's Browse… control.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn browse_sounds(
        &self,
        Parameters(args): Parameters<BrowseSoundsArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().browse_sounds(
                args.query.as_deref().unwrap_or(""),
                args.group.as_deref().unwrap_or("all"),
                args.offset.unwrap_or(0),
                args.limit.unwrap_or(60),
            )
        })
    }

    #[tool(
        description = "Inspect a mounted sound path or sound-script name and zero-based wave variant (default0). Returns source/waves, file codec/size/rate/duration where known and authored script channel/volume/pitch/level. CPU-only; no audio playback or map edits.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn inspect_sound(
        &self,
        Parameters(args): Parameters<SoundArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .inspect_sound(&args.value, args.variant.unwrap_or(0))
        })
    }

    #[tool(
        description = "Begin an owned sound-field choice for an entity id and FGD sound property key (for example ambient_generic.message). resume=true reuses a valid same-target choice. Finish geometry previews first. Source, history, selection and camera remain unchanged."
    )]
    pub(in crate::mcp) fn begin_sound_choice(
        &self,
        Parameters(args): Parameters<BeginSoundChoiceArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let id = single_entity(&session, args.id.as_deref())?;
            session.begin_sound_choice(id, &args.key, args.resume)
        })
    }

    #[tool(
        description = "Choose a sound path or manifest script name and optional zero-based wave variant in an owned draft. Inspection validates the selected file. A script choice applies its name; the variant selects isolated audition only. Does not autoplay or edit source."
    )]
    pub(in crate::mcp) fn choose_sound(
        &self,
        Parameters(args): Parameters<ChooseSoundArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .choose_sound(args.token, &args.value, args.variant.unwrap_or(0))
        })
    }

    #[tool(
        description = "Inspect a shared sound choice. Omit token to discover the active or stale token and validity; an explicit token enforces captured map/selection/definition/asset/preview guards.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_sound_choice(
        &self,
        Parameters(args): Parameters<ModelChoiceQueryArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            match args.token {
                Some(token) => session.sound_choice(token),
                None => Ok(session.current_sound_choice()),
            }
        })
    }

    #[tool(
        description = "Apply the owned sound choice to its captured entity field in one undoable transaction. Revalidates file and guards. Script variants remain an audition setting; the native field receives the script name. UI: Use sound."
    )]
    pub(in crate::mcp) fn apply_sound_choice(
        &self,
        Parameters(args): Parameters<ModelChoiceTokenArgs>,
    ) -> ToolResult {
        run(|| self.session().apply_sound_choice(Actor::Agent, args.token))
    }

    #[tool(
        description = "Cancel only the captured sound choice, including a stale draft. Preserves replacement choices, map source/history and geometry previews. UI: Cancel/Escape."
    )]
    pub(in crate::mcp) fn cancel_sound_choice(
        &self,
        Parameters(args): Parameters<ModelChoiceTokenArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().cancel_sound_choice(args.token)?;
            Ok(json!({"cancelled":true}))
        })
    }

    #[tool(
        description = "Set shared audition/export volume0–1 for the next preview. Existing playback keeps its current volume. This does not change entity/game volume. UI: Preview volume slider."
    )]
    pub(in crate::mcp) fn set_sound_preview_volume(
        &self,
        Parameters(args): Parameters<SoundVolumeArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.set_sound_preview_volume(args.volume)?;
            Ok(json!({"volume":session.sound_preview_volume()}))
        })
    }

    #[tool(
        description = "Decode up to10 seconds of the same isolated sound preview used by human Play and return PCM WAV audio plus metadata. Takes value, optional zero-based variant and volume0–1 (shared volume by default). Requires FFmpeg. Does not play through local speakers or change source/camera/history. Game DSP, attenuation and random selection are not simulated.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn render_sound_preview(
        &self,
        Parameters(args): Parameters<SoundPreviewArgs>,
    ) -> ToolResult {
        match self.session().render_sound_preview(
            &args.value,
            args.variant.unwrap_or(0),
            args.volume,
        ) {
            Ok(preview) => Ok(CallToolResult::success(vec![
                ContentBlock::text(preview.report.to_string()),
                ContentBlock::audio(
                    base64::engine::general_purpose::STANDARD.encode(preview.bytes),
                    "audio/wav",
                ),
            ])),
            Err(error) => fail(error),
        }
    }

    #[tool(
        description = "Play the isolated sound preview through local audio using FFmpeg/ffplay, with value, optional zero-based variant and volume0–1. Returns an owned playback token. Replaces previous playback only after successful preparation; maximum10 seconds and no autoplay. Does not change map or game volume. UI: Play."
    )]
    pub(in crate::mcp) fn play_sound_preview(
        &self,
        Parameters(args): Parameters<SoundPreviewArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .play_sound_preview(&args.value, args.variant.unwrap_or(0), args.volume)
        })
    }

    #[tool(
        description = "Inspect shared sound preview playback token/state and volume for the next audition/export. Finished/failed playback retains its token for cleanup. UI: sound browser playback status.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_sound_preview(&self) -> ToolResult {
        run(|| Ok(self.session().sound_preview_status()))
    }

    #[tool(
        description = "Stop/clear only the captured sound playback token, including finished playback. A foreign/replaced token fails without stopping the replacement. UI: Stop or owning browser Cancel/close."
    )]
    pub(in crate::mcp) fn stop_sound_preview(
        &self,
        Parameters(args): Parameters<ModelChoiceTokenArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().stop_sound_preview(args.token)?;
            Ok(json!({"stopped":true}))
        })
    }

    #[tool(
        description = "Reload mounted sound files and the active sound-script manifest on next inspection, invalidate pending sound choices and stop playback. Does not change map/history. UI: Reload sounds."
    )]
    pub(in crate::mcp) fn reload_sound_assets(&self) -> ToolResult {
        self.session().reload_sound_assets();
        ok(json!({"reloaded":true}))
    }

    #[tool(
        description = "Inspect an MDL44/48 mesh with VVD4/VTX7 LOD0 geometry, selected skin/body, materials, counts, bounds and named reference-pose attachments. Static meshes and checked reference poses are supported; pose/reference_bones distinguish them. CPU-only and read-only; does not initialize graphics or edit source. Unsupported/missing/malformed assets return a recoverable error. UI: model entity → Model details….",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn inspect_model(
        &self,
        Parameters(args): Parameters<InspectModelArgs>,
    ) -> ToolResult {
        run(|| {
            let session = self.session();
            let skin = args.skin.unwrap_or(0);
            if args.body.unwrap_or(0) == 0 {
                session.inspect_model(&args.path, skin)
            } else {
                session.inspect_model_body(&args.path, skin, args.body.unwrap())
            }
        })
    }

    #[tool(
        description = "Inspect the selected/explicit model entity's shared viewport asset, skin/body, transformed bounds, RGB tint, authored render mode/amount/FX, model_scale authored/effective/support/applied/error and limits, named world attachment positions/axes, displayed state and fallback reason. Positive uniform modelscale uses native float precision in editor range1/64–64, with at most64 UTF-8 bytes and complete mesh bounds within±65536. Missing means1. Mesh/picks/bounds/default-pose attachments share the origin-relative transform; native physics/collision, animation and runtime scale inputs are unevaluated. Static-prop/native detail writers and literal FGD guides retain unit-scale admission. Textured dynamic-model previews support Normal (0) and Texture (2) with renderamt0–255 and renderfx0; Normal ignores the amount. Edit through set_entity_properties, shared with the inspector's Model scale, Render mode and Opacity controls. Unsupported values/modes/FX retain a diagnostic helper. Reads the rendered draft, preserving source/history. Static meshes, checked reference poses and selected bodygroups at LOD0 are supported. UI: Model details….",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn inspect_entity_model(
        &self,
        Parameters(args): Parameters<EntityArgs>,
    ) -> ToolResult {
        run(|| {
            let session = self.session();
            session.inspect_entity_model(single_entity(&session, args.id.as_deref())?)
        })
    }

    #[tool(
        description = "Clear the bounded shared model asset cache and refresh viewport geometry/picks after external asset changes. Cancels snap-to-grid drafts whose bounds may have changed; preview_snap_to_grid again before applying. Returns snap_preview_cancelled. No map/history changes; graphics remain lazy. UI: Model details… → Reload models."
    )]
    pub(in crate::mcp) fn reload_model_assets(&self) -> ToolResult {
        let snap_preview_cancelled = self.session().refresh_model_assets(Actor::Agent);
        ok(json!({"reloaded":true,"snap_preview_cancelled":snap_preview_cancelled}))
    }

    #[tool(
        description = "Browse mounted model paths without decoding meshes. Search words match names/folders; groups include all, used and exact folders. Shared with World Model → Browse….",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn browse_models(
        &self,
        Parameters(args): Parameters<BrowseModelsArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().browse_models(
                args.query.as_deref().unwrap_or(""),
                args.group.as_deref().unwrap_or("all"),
                args.offset.unwrap_or(0),
                args.limit.unwrap_or(60),
            )
        })
    }

    #[tool(
        description = "Begin an isolated model choice for a point entity's World Model field, including an empty field. Returns an owned token; resume=true reuses a valid same-target choice, otherwise replaces only the previous model choice. Does not change source, selection, camera or geometry previews. Finish active geometry previews first."
    )]
    pub(in crate::mcp) fn begin_model_choice(
        &self,
        Parameters(args): Parameters<BeginModelChoiceArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let id = single_entity(&session, args.id.as_deref())?;
            if args.resume {
                session.resume_model_choice(id)
            } else {
                session.begin_model_choice(id)
            }
        })
    }

    #[tool(
        description = "Choose a supported model, zero-based skin and native body value (defaults 0) in an owned model-choice draft. Validates the captured entity pose, asset, skin and body without changing source/history/camera. Returns CPU asset details; render_model_preview provides the browser image."
    )]
    pub(in crate::mcp) fn choose_model(
        &self,
        Parameters(args): Parameters<ChooseModelArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let skin = args.skin.unwrap_or(0);
            if args.body.unwrap_or(0) == 0 {
                session.choose_model(args.token, &args.path, skin)
            } else {
                session.choose_model_body(args.token, &args.path, skin, args.body.unwrap())
            }
        })
    }

    #[tool(
        description = "Choose a named bodygroup variant by its group/variant indices from inspect_model.bodygroups in an owned model choice. Preserves other group contributions and the skin; validates entity eligibility and updates the shared UI preview without editing source. UI: bodygroup dropdowns in Browse…."
    )]
    pub(in crate::mcp) fn choose_model_bodygroup(
        &self,
        Parameters(args): Parameters<ModelBodygroupArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .choose_model_bodygroup(args.token, args.group, args.variant)
        })
    }

    #[tool(
        description = "Show reference-pose axes at a named model attachment using its one-based index from inspect_entity_model.attachments. Returns an owned guide token. Requires a visible supported model and finished geometry previews; source, camera and history stay unchanged. UI: Model details… → Attachment → Show attachment axes."
    )]
    pub(in crate::mcp) fn show_model_attachment(
        &self,
        Parameters(args): Parameters<ModelAttachmentArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let id = single_entity(&session, args.id.as_deref())?;
            session.show_model_attachment(id, args.attachment)
        })
    }

    #[tool(
        description = "Inspect the active model attachment guide, including its owned token, world position/axes and validity. Stale guides retain their token for cancellation and are not rendered.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_model_attachment_guide(&self) -> ToolResult {
        run(|| Ok(self.session().model_attachment_guide()))
    }

    #[tool(
        description = "Clear only the captured model attachment guide token, including stale guides; a replacement guide is preserved. UI: turn off Show attachment axes or close Model details…."
    )]
    pub(in crate::mcp) fn clear_model_attachment(
        &self,
        Parameters(args): Parameters<ModelChoiceTokenArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().clear_model_attachment(args.token)?;
            Ok(json!({"cleared":true}))
        })
    }

    #[tool(
        description = "Frame a visible model's reference-pose attachment by one-based native index. Optional positive viewport aspect defaults to 1. Returns world attachment data; changes camera only. UI: Model details… → Frame attachment."
    )]
    pub(in crate::mcp) fn frame_model_attachment(
        &self,
        Parameters(args): Parameters<FrameModelAttachmentArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let id = single_entity(&session, args.id.as_deref())?;
            session.frame_model_attachment(
                Actor::Agent,
                id,
                args.attachment,
                args.aspect.unwrap_or(1.0),
            )
        })
    }

    #[tool(
        description = "Inspect a model choice and its shared model/skin/body draft. Omit token to discover the active token, including stale choices with valid=false and an error; explicit token checks map, selection, definitions and geometry-preview guards.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_model_choice(
        &self,
        Parameters(args): Parameters<ModelChoiceQueryArgs>,
    ) -> ToolResult {
        run(|| {
            let session = self.session();
            args.token.map_or_else(
                || Ok(session.current_model_choice()),
                |token| session.model_choice(token),
            )
        })
    }

    #[tool(
        description = "Apply the owned model choice's path and skin together through the shared entity operation, as one Undo step. Rejects stale/replaced drafts. UI: Use model."
    )]
    pub(in crate::mcp) fn apply_model_choice(
        &self,
        Parameters(args): Parameters<ModelChoiceTokenArgs>,
    ) -> ToolResult {
        run(|| self.session().apply_model_choice(Actor::Agent, args.token))
    }

    #[tool(
        description = "Cancel only the model choice with this token, including a stale draft. Does not touch another model choice or geometry preview. UI: Cancel / Escape."
    )]
    pub(in crate::mcp) fn cancel_model_choice(
        &self,
        Parameters(args): Parameters<ModelChoiceTokenArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().cancel_model_choice(args.token)?;
            Ok(json!({"cancelled":true}))
        })
    }

    #[tool(
        description = "Render the browser's isolated model/skin preview as a PNG using the same Session scene, camera framing and renderer as the UI. Does not replace the active map, selection, camera or render_view cache. GPU initializes lazily; CPU inspection works without it. Static meshes or reference poses, selected bodygroups/LOD0/scale1; 64–1024 pixels per side.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn render_model_preview(
        &self,
        Parameters(args): Parameters<RenderModelArgs>,
    ) -> ToolResult {
        let width = args.width.unwrap_or(640);
        let height = args.height.unwrap_or(400);
        let skin = args.skin.unwrap_or(0);
        let body = args.body.unwrap_or(0);
        let result = (|| {
            let mut renderer = self
                .model_renderer
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let session = self.session();
            let pixels = renderer.render(&session, &args.path, skin, body, width, height)?;
            let report = session.inspect_model_body(&args.path, skin, body)?;
            let mut bytes = Vec::new();
            let mut encoder = png::Encoder::new(Cursor::new(&mut bytes), width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .and_then(|mut writer| writer.write_image_data(&pixels))
                .map_err(|error| error.to_string())?;
            Ok::<_, String>((report, bytes))
        })();
        match result {
            Ok((report, bytes)) => Ok(CallToolResult::success(vec![
                ContentBlock::text(
                    json!({"asset":report,"width":width,"height":height}).to_string(),
                ),
                ContentBlock::image(
                    base64::engine::general_purpose::STANDARD.encode(bytes),
                    "image/png",
                ),
            ])),
            Err(error) => fail(error),
        }
    }
}
