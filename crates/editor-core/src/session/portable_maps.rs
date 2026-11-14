use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use source_formats::{
    bsp::PakArchive,
    file_publication::{BoundDestination, Policy},
    gamefs::{GameFs, Mount},
    keyvalues::WriteOptions,
    vmf::{
        Vmf,
        portable::{MAX_VMF_BYTES, MapAssets},
    },
};

use super::{Actor, Session};
use crate::bounded_file::Source;

mod dependencies;
pub(super) use dependencies::{Prepared, Sources};

#[derive(Clone, Copy, Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Attach,
    Share,
    ExportPlain,
    ExportAssets,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub action: Action,
    pub source_vmf: Option<PathBuf>,
    pub destination: Option<PathBuf>,
    pub companion_bsp: Option<PathBuf>,
    #[serde(default)]
    pub content_roots: Vec<PathBuf>,
}

fn cache_directory() -> Result<PathBuf, String> {
    crate::recovery::default_directory().map(|path| path.join("map-assets"))
}

impl Session {
    pub fn portable_map_state(&self) -> Value {
        let assets = self.document.vmf().assets.as_deref();
        json!({"enabled":assets.is_some(),"files":assets.map_or(0,|p|p.entries().len()),"decoded_bytes":assets.map_or(0,MapAssets::decoded_bytes),"compressed_bytes":assets.map_or(0,MapAssets::compressed_bytes),"sha256":assets.map(MapAssets::digest),"base_dependencies":assets.map_or(0,|p|p.base_dependencies().len()),"source_bsp":self.portable_sources.bsp,"content_roots":self.portable_sources.roots,"version":1,"limits":{"files":4096,"file_bytes":16*1024*1024,"decoded_bytes":64*1024*1024,"archive_bytes":64*1024*1024}})
    }

    pub(super) fn prepare_map_asset_game(
        &self,
        game: &Arc<GameFs>,
        assets: Option<&MapAssets>,
    ) -> Result<Arc<GameFs>, String> {
        let digest = assets.map(MapAssets::digest);
        let mounted = if game.map_assets_digest() == digest {
            game.clone()
        } else {
            let cache = if assets.is_some() {
                cache_directory()?
            } else {
                PathBuf::new()
            };
            Arc::new(
                game.with_map_assets(assets, &cache)
                    .map_err(|e| format!("Cannot mount portable map assets: {e}"))?,
            )
        };
        if let Some(assets) = assets {
            for path in assets.base_dependencies() {
                if !mounted
                    .contains(path)
                    .map_err(|e| format!("Base content {path}: {e}"))?
                {
                    return Err(format!(
                        "Portable map requires unavailable base content: {path}; mount the required game/content profile"
                    ));
                }
            }
        }
        Ok(mounted)
    }

    fn portable_base_game(&self) -> Result<Arc<GameFs>, String> {
        let configured = self
            .current_content_profile_config()
            .and_then(|config| config.engine_root)
            .map(|root| root.join("cstrike/gameinfo.txt"));
        let path = configured
            .or_else(|| {
                self.game
                    .base_dir
                    .join("cstrike/gameinfo.txt")
                    .is_file()
                    .then(|| self.game.base_dir.join("cstrike/gameinfo.txt"))
            })
            .or_else(crate::steam::find_css_gameinfo)
            .ok_or("Base CS:S content is unavailable; select its content profile")?;
        let mut base = GameFs::from_gameinfo(&path).map_err(|e| e.to_string())?;
        let root = path
            .parent()
            .and_then(Path::parent)
            .ok_or("Base game path has no install root")?;
        let directories = [
            root.join("cstrike"),
            root.join("hl2"),
            root.join("platform"),
        ];
        base.mounts.retain(|mounted| match &mounted.mount {
            Mount::Directory(_) => false,
            Mount::Pak(pak) => {
                pak.path()
                    .parent()
                    .is_some_and(|parent| directories.iter().any(|directory| directory == parent))
                    && pak
                        .path()
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| {
                            [
                                "cstrike_pak",
                                "cstrike_english",
                                "hl2_textures",
                                "hl2_misc",
                                "hl2_sound_misc",
                                "hl2_sound_vo_english",
                                "platform_misc",
                            ]
                            .iter()
                            .any(|stem| {
                                name == format!("{stem}.vpk") || name == format!("{stem}_dir.vpk")
                            })
                        })
            }
        });
        if base.mounts.is_empty() {
            return Err("No official base-game VPKs are mounted; portable sharing cannot distinguish custom assets".into());
        }
        Ok(Arc::new(base))
    }

    pub(super) fn refresh_portable_map(&self, vmf: &mut Vmf) -> Result<Option<Prepared>, String> {
        if vmf.assets.is_none() {
            return Ok(None);
        }
        let prepared = dependencies::collect(
            vmf,
            &self.game,
            &self.portable_base_game()?,
            &self.definitions,
            &self.portable_sources,
        )?;
        vmf.assets = Some(prepared.assets.clone());
        Ok(Some(prepared))
    }

    pub fn portable_map(&mut self, actor: Actor, options: Options) -> Result<Value, String> {
        self.ensure_not_closing()?;
        self.ensure_partitions_resolved()?;
        self.ensure_entity_placement_resolved()?;
        self.ensure_entity_gallery_resolved()?;
        self.ensure_bulk_entities_resolved()?;
        if self.compiling() || self.content_profile_worker_busy() {
            return Err(
                "Wait for active compilation or content changes before portable sharing".into(),
            );
        }
        if matches!(options.action, Action::Attach)
            && (options.source_vmf.is_some() || options.destination.is_some())
        {
            return Err("Attach uses the current map and accepts no source/destination VMF".into());
        }
        if !matches!(options.action, Action::Attach) && options.destination.is_none() {
            return Err("Choose an explicit export destination".into());
        }
        if matches!(options.action, Action::ExportPlain | Action::ExportAssets)
            && (options.companion_bsp.is_some() || !options.content_roots.is_empty())
        {
            return Err("Import content with Attach or Share before unpacked export".into());
        }
        let input = options
            .source_vmf
            .as_ref()
            .map(|path| Source::capture(path, MAX_VMF_BYTES as u64).map_err(|e| e.to_string()))
            .transpose()?;
        let mut vmf = match &input {
            Some(source) => {
                let bytes = source.clone().read().map_err(|e| e.to_string())?;
                Vmf::parse(std::str::from_utf8(&bytes).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?
            }
            None => self.document.snapshot_for_save(),
        };
        let mut sources = Sources {
            bsp: options.companion_bsp,
            roots: options.content_roots,
        };
        if options.source_vmf.is_none() && sources.bsp.is_none() && sources.roots.is_empty() {
            sources = self.portable_sources.clone();
        }
        let prepared = if matches!(options.action, Action::Attach | Action::Share) {
            let source_game = if options.source_vmf.is_some() {
                Arc::new(
                    self.game
                        .with_map_assets(None, Path::new(""))
                        .map_err(|e| e.to_string())?,
                )
            } else {
                self.game.clone()
            };
            let prepared = dependencies::collect(
                &vmf,
                &source_game,
                &self.portable_base_game()?,
                &self.definitions,
                &sources,
            )?;
            vmf.assets = Some(prepared.assets.clone());
            Some(prepared)
        } else {
            None
        };
        if let Some(input) = &input {
            input.verify().map_err(|e| e.to_string())?;
        }
        if matches!(options.action, Action::Attach) {
            let game = self.prepare_map_asset_game(&self.game, vmf.assets.as_deref())?;
            prepared
                .as_ref()
                .ok_or("Asset import was not prepared")?
                .verify()?;
            self.document.set_map_assets(vmf.assets);
            self.portable_sources = sources;
            self.game = game;
            self.reset_mounted_content_assets();
            self.content_sources_changed(None);
            self.changed();
            self.log(
                actor,
                "Attached portable map assets; Save will retain and refresh the package",
            );
            return Ok(self.portable_map_state());
        }
        let destination = options
            .destination
            .as_ref()
            .ok_or("Missing export destination")?;
        let extension = if matches!(options.action, Action::ExportAssets) {
            "zip"
        } else {
            "vmf"
        };
        if !destination
            .extension()
            .is_some_and(|value| value.eq_ignore_ascii_case(extension))
        {
            return Err(format!("Choose a .{extension} export destination"));
        }
        let bound = BoundDestination::capture(destination, Policy::FollowSource)
            .map_err(|e| e.to_string())?;
        if self
            .file_claims
            .source
            .as_ref()
            .is_some_and(|claim| claim.destination().physical_path() == bound.physical_path())
        {
            return Err("Export to a different file; use Save to update the open VMF".into());
        }
        if let Some(bsp) = &sources.bsp
            && bsp.canonicalize().map_err(|e| e.to_string())? == bound.physical_path()
        {
            return Err("Export cannot overwrite the companion BSP".into());
        }
        if let Some(source) = &options.source_vmf
            && source.canonicalize().map_err(|e| e.to_string())? == bound.physical_path()
        {
            return Err(
                "Export the source VMF to a different file; original input is preserved".into(),
            );
        }
        let claim = self.file_claims.source(bound.clone())?;
        let assets = vmf.assets.clone();
        let bytes = match options.action {
            Action::ExportAssets => PakArchive {
                entries: assets
                    .as_ref()
                    .ok_or("This VMF has no packaged assets")?
                    .entries()
                    .to_vec(),
            }
            .to_bytes(Default::default())
            .map_err(|e| e.to_string())?,
            _ => {
                if matches!(options.action, Action::ExportPlain) {
                    vmf.assets = None;
                }
                vmf.try_write(&WriteOptions::default())
                    .map_err(|e| e.to_string())?
                    .into_bytes()
            }
        };
        if bytes.len() > MAX_VMF_BYTES {
            return Err("Portable output exceeds 160 MiB".into());
        }
        let mut stage = bound.stage().map_err(|e| e.to_string())?;
        use std::io::Write;
        stage
            .file_mut()
            .write_all(&bytes)
            .map_err(|e| e.to_string())?;
        stage.sync_all().map_err(|e| e.to_string())?;
        if let Some(prepared) = &prepared {
            prepared.verify()?;
        }
        if let Some(input) = &input {
            input.verify().map_err(|e| e.to_string())?;
        }
        let receipt = stage.commit().map_err(|e| e.to_string())?;
        drop(claim);
        self.log(actor, format!("Exported {}", destination.display()));
        Ok(
            json!({"path":destination,"files":assets.as_ref().map_or(0,|p|p.entries().len()),"decoded_bytes":assets.as_ref().map_or(0,|p|p.decoded_bytes()),"sha256":assets.as_ref().map(|p|p.digest()),"warning":receipt.warning,"map_data":"preserved; packaging does not repair or validate map geometry"}),
        )
    }
}
