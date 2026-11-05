use super::super::*;

#[tool_router(router = particles_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Search named systems parsed from the mounted global particles/particles_manifest.txt and supported DMX binary2/pcf1 definitions. Query matches names, PCF paths and materials. Groups: all, used, folder:… or file:…. Page limit1–100, default60. Errors on missing/corrupt/unsupported active data. Definition metadata only, no animation.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn browse_particles(
        &self,
        Parameters(args): Parameters<BrowseParticlesArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().browse_particles(
                args.query.as_deref().unwrap_or(""),
                args.group.as_deref().unwrap_or("all"),
                args.offset.unwrap_or(0),
                args.limit.unwrap_or(60),
            )
        })
    }

    #[tool(
        description = "Inspect a named active-manifest particle system: source PCF, precache marker, material, authored maximum count, renderer/operator/initializer/emitter/force/constraint names and child systems. Rejects ambiguous names. CPU-only definition inspection; no simulated particles or map edits.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn inspect_particle(
        &self,
        Parameters(args): Parameters<ParticleNameArgs>,
    ) -> ToolResult {
        run(|| self.session().inspect_particle(&args.name))
    }

    #[tool(
        description = "Capture an owned effect_name choice for 1–64 whole info_particle_system point entities. Use id or ids; omit both for the exact current selection. Rejects incompatible members, duplicate or malformed target fields, unavailable/read-only definitions and active geometry previews without replacing the prior draft. Returns exact captured entities, actual stored source values and explicit mixed state; mixed values never select a default effect. resume=true reuses only a valid identical target set. Shared with Entity → Choose particles… and inspector Browse…. Source/history/camera remain unchanged until Apply."
    )]
    pub(in crate::mcp) fn begin_particle_choice(
        &self,
        Parameters(args): Parameters<BeginParticleChoiceArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let entities =
                entity_choice_targets(&session, args.id.as_deref(), args.ids.as_deref())?;
            session.begin_particle_choices(&entities, args.resume)
        })
    }

    #[tool(
        description = "Choose and inspect one unambiguous active-manifest particle system for every captured entity, validating every target against the current loaded definitions before accepting the choice. Does not edit effect_name until apply_particle_choice. Invalid target or particle inspection preserves the prior choice. Definition metadata only; no animation or viewport preview."
    )]
    pub(in crate::mcp) fn choose_particle(
        &self,
        Parameters(args): Parameters<ChooseParticleArgs>,
    ) -> ToolResult {
        run(|| self.session().choose_particle(args.token, &args.name))
    }

    #[tool(
        description = "Inspect exact captured entities, stored effect_name values, mixed state and the shared particle choice. Omit token to discover active/stale targets and validity; explicit token enforces captured map, selection, definitions, content, assets and geometry-preview guards. Definition metadata only; no animation or viewport preview.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_particle_choice(
        &self,
        Parameters(args): Parameters<ModelChoiceQueryArgs>,
    ) -> ToolResult {
        run(|| {
            let session = self.session();
            match args.token {
                Some(token) => session.particle_choice(token),
                None => Ok(session.current_particle_choice()),
            }
        })
    }

    #[tool(
        description = "Apply the owned particle name to every captured info_particle_system.effect_name in one atomic undoable transaction. Revalidates all target fields, reloads and compares the active manifest/PCF definition data, and enforces stale/replaced-token guards before writing anything. Preserves each entity's other properties and hidden state. UI: Apply."
    )]
    pub(in crate::mcp) fn apply_particle_choice(
        &self,
        Parameters(args): Parameters<ModelChoiceTokenArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .apply_particle_choice(Actor::Agent, args.token)
        })
    }

    #[tool(
        description = "Cancel only this owned particle choice, including stale drafts. A replaced token cannot cancel another choice. Preserves map/history and geometry previews. UI: Cancel/Escape/window close."
    )]
    pub(in crate::mcp) fn cancel_particle_choice(
        &self,
        Parameters(args): Parameters<ModelChoiceTokenArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().cancel_particle_choice(args.token)?;
            Ok(json!({"cancelled":true}))
        })
    }

    #[tool(
        description = "Clear the particle definition catalog and invalidate owned choices. The next search/inspection reloads the mounted global manifest. Failed loads are never cached as successful/empty catalogs. Does not change source/history. UI: Reload particles."
    )]
    pub(in crate::mcp) fn reload_particle_assets(&self) -> ToolResult {
        self.session().reload_particle_assets();
        ok(json!({"reloaded":true}))
    }
}
