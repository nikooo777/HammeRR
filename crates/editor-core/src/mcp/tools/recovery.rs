use super::super::*;

#[cfg(test)]
mod error_domain_tests;

#[cfg(test)]
mod authority_tests;

#[tool_router(router = recovery_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect autosave settings, pending changes, settings/storage/snapshot/retention/drain errors, their compatible combined error, storage location and retained recovery snapshots (newest first). Application-attached Sessions share settings, settings revision and settings errors; storage, snapshots, clocks and capture errors remain local. Successful settings Save clears only settings errors; successful snapshots clear only snapshot and retention errors. Snapshots contain committed map data, not editor previews. Default page 50, maximum 200; current_map_only filters by the current document key.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_recovery(
        &self,
        Parameters(args): Parameters<RecoveryListArgs>,
    ) -> ToolResult {
        run(|| {
            let session = self.session();
            let status = session.recovery_status();
            let listing =
                session
                    .recovery_entries()
                    .unwrap_or_else(|error| crate::recovery::Listing {
                        entries: Vec::new(),
                        warnings: vec![error],
                    });
            let entries: Vec<_> = listing
                .entries
                .into_iter()
                .filter(|entry| {
                    !args.current_map_only
                        || Some(entry.document_key.as_str()) == status["document_key"].as_str()
                })
                .collect();
            let offset = args.offset.unwrap_or(0);
            Ok(
                json!({"status": status, "total": entries.len(), "offset": offset, "snapshots": entries.into_iter().skip(offset).take(args.limit.unwrap_or(50).min(200)).collect::<Vec<_>>(), "warnings": listing.warnings}),
            )
        })
    }

    #[tool(
        description = "Configure persistent autosave settings through the same operation as Recovery settings. enabled controls periodic/exit autosaves; manual snapshots and previous-file backups remain available. Interval is 30–3600 seconds, retention 1–50 snapshots per map. Omitted values merge with the latest settings under the shared Application authority lock. expected_revision rejects forms made stale by any attached Session. Changes do not edit the map."
    )]
    pub(in crate::mcp) fn set_recovery_settings(
        &self,
        Parameters(args): Parameters<RecoverySettingsArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().update_recovery_settings(
                Actor::Agent,
                crate::recovery::SettingsUpdate {
                    enabled: args.enabled,
                    interval_seconds: args.interval_seconds,
                    keep_per_map: args.keep_per_map,
                },
                args.expected_revision,
            )
        })
    }

    #[tool(
        description = "Reload recovery/settings.json through the same Session operation as Recovery -> Reload settings. Reads a regular target up to 32768 bytes; valid regular-file symlinks are followed and an absent file supplies factory defaults. Optional expected_revision rejects stale settings requests before reading. Failed read/validation preserves current settings and revision, retaining a settings_error. Success adopts saved settings and advances settings_revision; storage/snapshot/retention/drain errors, requests, map/history/camera and previews remain unchanged. Does not write storage, reset the autosave clock, initialize graphics or coordinate separate processes or outside writers."
    )]
    pub(in crate::mcp) fn reload_recovery_settings(
        &self,
        Parameters(args): Parameters<ReloadRecoverySettingsArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .reload_recovery_settings(Actor::Agent, args.expected_revision)
        })
    }

    #[tool(
        description = "Write a manual recovery snapshot without saving over the map, bumping mapversion, changing dirty state or adding undo history. Uses committed content and excludes active previews. Does not hold the Session lock during serialization/file writing; edits made while it writes remain pending for the next snapshot."
    )]
    pub(in crate::mcp) async fn create_recovery_snapshot(&self) -> ToolResult {
        let session = self.session.clone();
        match tokio::task::spawn_blocking(move || {
            crate::recovery::capture(&session, Actor::Agent, crate::recovery::Kind::Manual)
        })
        .await
        {
            Ok(Ok(entry)) => ok(json!({"snapshot": entry})),
            Ok(Err(error)) => fail(error),
            Err(error) => fail(format!("Recovery worker failed: {error}")),
        }
    }

    #[tool(
        description = "Open a retained recovery snapshot as a dirty, unnamed copy. It leaves the source file untouched and clears selection/history/previews like opening a map. Save with an explicit path to keep the recovered copy. Refuses to replace unsaved edits unless discard_changes is explicitly true. Invalid snapshots leave the current map unchanged."
    )]
    pub(in crate::mcp) fn restore_recovery_snapshot(
        &self,
        Parameters(args): Parameters<RestoreRecoveryArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let expected = match (args.generation, args.revision) {
                (None, None) => None,
                (Some(generation), Some(revision)) => Some((generation, revision)),
                _ => {
                    return Err(
                        "Supply both generation and revision for the stale-map guard".into(),
                    );
                }
            };
            let review = match args.expected_review.as_deref() {
                Some(token) => session.buffer_replacement_review_from_token(token)?,
                None => session.buffer_replacement_review(),
            };
            session.restore_recovery_snapshot_reviewed(
                Actor::Agent,
                &args.id,
                args.discard_changes,
                expected,
                &review,
            )?;
            Ok(
                json!({"restored": args.id, "path": session.path(), "unsaved_changes": session.document().is_dirty(), "status": session.recovery_status()}),
            )
        })
    }

    #[tool(
        description = "Permanently delete one recovery snapshot by its listed ID. Does not delete or edit the original map. Refuses during a snapshot write. Retention also prunes the oldest snapshots automatically when new ones are created."
    )]
    pub(in crate::mcp) fn delete_recovery_snapshot(
        &self,
        Parameters(args): Parameters<RecoveryIdArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .delete_recovery_snapshot(Actor::Agent, &args.id)?;
            Ok(json!({"deleted": args.id}))
        })
    }
}
