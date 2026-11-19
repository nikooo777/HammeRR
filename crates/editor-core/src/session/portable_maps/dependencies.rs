use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use source_formats::{
    bsp::{Bsp, PakArchive, PakEntry, PakLimits},
    fgd::Catalog,
    gamefs::{GameFs, Mount, MountedPath, asset_relative},
    keyvalues::{self, Node},
    model, sound,
    vmf::{
        Vmf,
        portable::{MapAssets, asset_path},
    },
    vmt::{self, Material},
};

use crate::bounded_file::Source;

#[derive(Default, Clone)]
pub(in crate::session) struct Sources {
    pub bsp: Option<PathBuf>,
    pub roots: Vec<PathBuf>,
}

pub(in crate::session) struct Prepared {
    pub assets: Arc<MapAssets>,
    guards: Vec<(Arc<GameFs>, compiler::asset_sources::AssetSourceGuard)>,
    bsp: Option<Source>,
}

impl Prepared {
    pub fn verify(&self) -> Result<(), String> {
        for (game, guard) in &self.guards {
            guard
                .verify_sources(game, &compiler::Control::default())
                .map_err(|e| e.to_string())?;
        }
        if let Some(source) = &self.bsp {
            source.verify().map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

struct Collection {
    game: Arc<GameFs>,
    base: Arc<GameFs>,
    roots: Arc<GameFs>,
    captures: [compiler::asset_sources::BuildMaterialSources; 3],
    assets: BTreeMap<String, Vec<u8>>,
    base_paths: BTreeSet<String>,
    visited: BTreeSet<String>,
    queued: BTreeSet<String>,
    pending: VecDeque<(String, String)>,
    read_bytes: usize,
    emission: Option<String>,
    surfaces: Option<BTreeMap<String, Vec<Node>>>,
    surface_uses: BTreeSet<String>,
    registry_sources: BTreeSet<String>,
    cues: Option<BTreeMap<String, Option<Vec<String>>>>,
    sound_sources: BTreeSet<String>,
}

impl Collection {
    fn enqueue(&mut self, path: String, from: impl Into<String>) -> Result<(), String> {
        let path = asset_path(&path).map_err(|error| error.to_string())?;
        if !self.queued.insert(path.clone()) {
            return Ok(());
        }
        if self.queued.len() > 4096 {
            return Err("Portable dependency graph exceeds 4096 resources".into());
        }
        self.pending.push_back((path, from.into()));
        Ok(())
    }

    fn read(&mut self, path: &str, from: &str, optional: bool) -> Result<Option<Vec<u8>>, String> {
        let maximum = PakLimits::default().max_entry_bytes;
        self.roots
            .validate_case_identity(path)
            .map_err(|e| format!("{from} → {path}: {e}"))?;
        self.game
            .validate_case_identity(path)
            .map_err(|e| format!("{from} → {path}: {e}"))?;
        let root = self.captures[0]
            .read_asset(&self.roots, path, maximum)
            .map(|asset| asset.map(|asset| asset.bytes().to_vec()))
            .map_err(|error| format!("{from} → {path}: {error}"))?;
        let retained = self.assets.get(path).cloned();
        let bytes = match root {
            Some(bytes) => Some(bytes),
            None => match retained {
                Some(bytes) => Some(bytes),
                None => self.captures[1]
                    .read_asset(&self.game, path, maximum)
                    .map(|asset| asset.map(|asset| asset.bytes().to_vec()))
                    .map_err(|error| format!("{from} → {path}: {error}"))?,
            },
        };
        let Some(bytes) = bytes else {
            return if optional {
                Ok(None)
            } else {
                Err(format!("Missing portable dependency: {from} → {path}"))
            };
        };
        self.read_bytes = self
            .read_bytes
            .checked_add(bytes.len())
            .ok_or("Portable dependency read budget overflow")?;
        if self.read_bytes > 512 * 1024 * 1024 {
            return Err("Portable dependencies exceed the 512 MiB source-read budget".into());
        }
        let official = if self.assets.contains_key(path) {
            false
        } else {
            self.matches_base(path, &bytes)?
        };
        if official {
            self.base_paths.insert(path.into());
        } else {
            let previous = self.assets.get(path).map_or(0, Vec::len);
            let total = self.assets.values().map(Vec::len).sum::<usize>() - previous + bytes.len();
            if total > PakLimits::default().max_total_bytes
                || (!self.assets.contains_key(path)
                    && self.assets.len() >= PakLimits::default().max_entries)
            {
                return Err(format!(
                    "Portable asset budget exceeded while adding {path}"
                ));
            }
            self.base_paths.remove(path);
            self.assets.insert(path.into(), bytes.clone());
        }
        Ok(Some(bytes))
    }

    fn matches_base(&self, path: &str, bytes: &[u8]) -> Result<bool, String> {
        Ok(self.captures[2]
            .read_asset(&self.base, path, PakLimits::default().max_entry_bytes)
            .map_err(|e| format!("Base-game dependency {path}: {e}"))?
            .as_ref()
            .is_some_and(|asset| asset.bytes() == bytes))
    }

    fn enqueue_audio(&mut self, value: &str, from: impl Into<String>) -> Result<(), String> {
        let from = from.into();
        if value.to_ascii_lowercase().contains("$gender") {
            return Err(format!(
                "{from}: dynamic $gender sound paths are unsupported"
            ));
        }
        let relative = sound::audio_path(value).map_err(|e| format!("{from}: {e}"))?;
        self.enqueue(format!("sound/{relative}"), from)
    }

    fn sound_cue(&mut self, name: &str, from: &str) -> Result<(), String> {
        if name.is_empty() {
            return Ok(());
        }
        if self.cues.is_none() {
            let manifest = "scripts/game_sounds_manifest.txt";
            let bytes = self
                .read(manifest, from, false)?
                .ok_or("Sound manifest was not read")?;
            let mut total = bytes.len();
            let base_preloaded = match self.captures[2]
                .read_asset(&self.base, manifest, sound::MAX_SCRIPT_BYTES)
                .map_err(|e| format!("Base sound manifest: {e}"))?
            {
                Some(asset) => {
                    let text = std::str::from_utf8(asset.bytes()).map_err(|e| e.to_string())?;
                    sound::manifest_scripts(text)
                        .map_err(|e| e.to_string())?
                        .into_iter()
                        .filter(|script| script.preload)
                        .map(|script| script.path)
                        .collect::<BTreeSet<_>>()
                }
                None => BTreeSet::new(),
            };
            let text = std::str::from_utf8(&bytes).map_err(|e| format!("{manifest}: {e}"))?;
            let paths = sound::manifest_scripts(text).map_err(|e| format!("{manifest}: {e}"))?;
            self.sound_sources.insert(manifest.into());
            let preloaded = paths
                .iter()
                .filter(|script| script.preload)
                .map(|script| script.path.clone())
                .collect::<BTreeSet<_>>();
            let mut cues = BTreeMap::new();
            for script in paths {
                let path = script.path;
                if !self.sound_sources.insert(path.clone()) {
                    continue;
                }
                let bytes = self
                    .read(&path, manifest, false)?
                    .ok_or("Sound script was not read")?;
                total += bytes.len();
                if total > 32 * 1024 * 1024 {
                    return Err("Sound scripts exceed 32 MiB".into());
                }
                let text = std::str::from_utf8(&bytes).map_err(|e| format!("{path}: {e}"))?;
                let custom_preload = preloaded.contains(&path)
                    && (!base_preloaded.contains(&path) || !self.matches_base(&path, &bytes)?);
                for cue in sound::parse_script(text).map_err(|e| format!("{path}: {e}"))? {
                    if custom_preload {
                        for wave in &cue.waves {
                            self.enqueue_audio(
                                wave,
                                format!("preload file {path} → {}", cue.name),
                            )?;
                        }
                    }
                    let key = cue.name.to_ascii_lowercase();
                    if let std::collections::btree_map::Entry::Vacant(entry) =
                        cues.entry(key.clone())
                    {
                        entry.insert(Some(cue.waves));
                    } else {
                        cues.insert(key, None);
                    }
                    if cues.len() > 32768 {
                        return Err("Sound catalog exceeds 32768 cues".into());
                    }
                }
            }
            self.cues = Some(cues);
        }
        let waves = self
            .cues
            .as_ref()
            .and_then(|cues| cues.get(&name.to_ascii_lowercase()))
            .ok_or_else(|| format!("{from}: unresolved sound cue {name}"))?
            .as_ref()
            .ok_or_else(|| format!("{from}: ambiguous sound cue {name}"))?
            .clone();
        if waves.is_empty() {
            return Err(format!("{from}: sound cue {name} has no declared waves"));
        }
        for wave in waves {
            self.enqueue_audio(&wave, format!("{from} → sound {name}"))?;
        }
        Ok(())
    }

    fn surface_property(&mut self, name: &str, from: &str) -> Result<(), String> {
        if self.surfaces.is_none() {
            let manifest = "scripts/surfaceproperties_manifest.txt";
            let bytes = self
                .read(manifest, from, false)?
                .ok_or("Surface manifest was not read")?;
            self.registry_sources.insert(manifest.into());
            let text = std::str::from_utf8(&bytes).map_err(|e| format!("{manifest}: {e}"))?;
            let paths = compiler::surface_property_definition_paths(text)?;
            let mut surfaces = BTreeMap::<String, Vec<Node>>::new();
            let mut total = bytes.len();
            for path in paths {
                let path = asset_path(&path).map_err(|e| e.to_string())?;
                let bytes = self
                    .read(&path, manifest, false)?
                    .ok_or("Surface definitions were not read")?;
                total += bytes.len();
                if total > 8 * 1024 * 1024 {
                    return Err("Surface definitions exceed 8 MiB".into());
                }
                self.registry_sources.insert(path.clone());
                let text = std::str::from_utf8(&bytes).map_err(|e| format!("{path}: {e}"))?;
                let nodes =
                    keyvalues::parse_bounded(text, 2, 65536).map_err(|e| format!("{path}: {e}"))?;
                for node in nodes {
                    let fields = node
                        .children()
                        .ok_or_else(|| format!("{path}: surface {} requires a block", node.key))?;
                    surfaces
                        .entry(node.key.to_ascii_lowercase())
                        .or_default()
                        .extend_from_slice(fields);
                }
                if surfaces.len() > 4096 {
                    return Err("Surface definitions exceed 4096 names".into());
                }
            }
            self.surfaces = Some(surfaces);
        }
        let mut pending = vec![name.to_ascii_lowercase()];
        while let Some(name) = pending.pop() {
            if !self.surface_uses.insert(name.clone()) {
                continue;
            }
            let fields = self
                .surfaces
                .as_ref()
                .and_then(|surfaces| surfaces.get(&name))
                .ok_or_else(|| format!("{from}: undefined surface property {name}"))?
                .clone();
            for field in fields {
                let value = field
                    .as_str()
                    .ok_or_else(|| format!("{from}: nested surface declaration {}", field.key))?;
                let key = field.key.to_ascii_lowercase();
                if key == "base" {
                    pending.push(value.to_ascii_lowercase());
                } else if [
                    "stepleft",
                    "stepright",
                    "bulletimpact",
                    "scraperough",
                    "scrapesmooth",
                    "impacthard",
                    "impactsoft",
                    "rolling",
                    "break",
                    "strain",
                ]
                .contains(&key.as_str())
                {
                    self.sound_cue(value, &format!("{from} → surface {name}"))?;
                }
            }
        }
        Ok(())
    }

    fn material_nodes(&mut self, fields: &[Node], from: &str) -> Result<(), String> {
        for field in fields {
            if let Some(value) = field.as_str() {
                let key = field.key.to_ascii_lowercase();
                if key == "$surfaceprop" || key == "$surfaceprop2" {
                    self.surface_property(value, from)?;
                } else if key == "include" {
                    self.enqueue(vmt::material_path(value), from)?;
                } else if [
                    "$albedo",
                    "$basetexture",
                    "$basetexture2",
                    "$bumpmap",
                    "$bumpmap2",
                    "$bumpmap3",
                    "$normalmap",
                    "$detail",
                    "$envmap",
                    "$envmapmask",
                    "$envmapmask2",
                    "$iris",
                    "$selfillummask",
                    "$phongexponenttexture",
                    "$lightwarptexture",
                    "$blendmodulatetexture",
                    "$flowmap",
                    "$reflecttexture",
                    "$refracttexture",
                    "$dudvmap",
                    "$hdrbasetexture",
                    "$hdrcompressedtexture",
                    "$hdrcompressedtexture0",
                    "$hdrcompressedtexture1",
                    "$hdrcompressedtexture2",
                    "$texture2",
                    "%tooltexture",
                ]
                .contains(&key.as_str())
                    || key.ends_with("texture")
                    || ["texture1", "texture2", "texture3", "texture4"]
                        .iter()
                        .any(|suffix| key.ends_with(suffix))
                {
                    if value.eq_ignore_ascii_case("env_cubemap")
                        || value.to_ascii_lowercase().starts_with("_rt_")
                    {
                        continue;
                    }
                    if value.is_empty() {
                        return Err(format!("Empty resource declaration: {from} → {key}"));
                    }
                    self.enqueue(vmt::texture_path(value), from)?;
                }
            } else if let Some(children) = field.children() {
                if field.key.eq_ignore_ascii_case("proxies") {
                    for proxy in children {
                        if ![
                            "animatedtexture",
                            "texturescroll",
                            "texturetransform",
                            "sine",
                            "linearamp",
                            "equals",
                            "add",
                            "multiply",
                            "subtract",
                            "divide",
                            "clamp",
                            "abs",
                            "frac",
                            "int",
                            "exponential",
                            "selectfirstifnonzero",
                            "lessorequal",
                            "materialmodify",
                            "materialmodifyanimated",
                            "playerproximity",
                            "waterlod",
                        ]
                        .iter()
                        .any(|name| proxy.key.eq_ignore_ascii_case(name))
                        {
                            return Err(format!(
                                "Unsupported dynamic material dependency: {from} → proxy {}",
                                proxy.key
                            ));
                        }
                    }
                } else {
                    self.material_nodes(children, from)?;
                }
            }
        }
        Ok(())
    }

    fn process(&mut self, path: &str, from: &str) -> Result<(), String> {
        if self.registry_sources.contains(path) || self.sound_sources.contains(path) {
            return Ok(());
        }
        let bytes = self
            .read(path, from, false)?
            .ok_or("Required asset was not read")?;
        if path.ends_with(".vmt") {
            if bytes.len() > 1024 * 1024 {
                return Err(format!("Material exceeds 1 MiB: {path}"));
            }
            let text = std::str::from_utf8(&bytes).map_err(|error| format!("{path}: {error}"))?;
            let nodes = keyvalues::parse_lenient_bounded(text, 64, 4096)
                .map_err(|error| format!("{path}: {error}"))?;
            self.material_nodes(&nodes, path)?;
            Material::load_with_reader(path, &mut |include| {
                self.read(include, path, false)
                    .map_err(std::io::Error::other)
            })
            .map_err(|error| format!("{path}: {error}"))?
            .ok_or_else(|| format!("Missing material: {path}"))?;
        } else if path.ends_with(".mdl") {
            let (materials, directories) =
                model::material_resources(&bytes).map_err(|error| format!("{path}: {error}"))?;
            if let Some(surface) =
                model::surface_property_name(&bytes).map_err(|e| format!("{path}: {e}"))?
            {
                self.surface_property(&surface, path)?;
            }
            let stem = path.strip_suffix(".mdl").ok_or("Invalid model suffix")?;
            for suffix in [".vvd", ".dx90.vtx"] {
                self.enqueue(format!("{stem}{suffix}"), path)?;
            }
            for suffix in [".dx80.vtx", ".sw.vtx", ".phy"] {
                let companion = format!("{stem}{suffix}");
                if self.read(&companion, path, true)?.is_some() {
                    self.enqueue(companion, path)?;
                }
            }
            for material in materials {
                let mut winner = None;
                for name in model::material_candidates(&material, &directories) {
                    let candidate = vmt::material_path(&name);
                    if self.read(&candidate, path, true)?.is_some() {
                        winner = Some(candidate);
                        break;
                    }
                }
                self.enqueue(
                    winner.ok_or_else(|| format!("Missing model material: {path} → {material}"))?,
                    path,
                )?;
            }
        } else if path.ends_with(".vtx") || path.ends_with(".phy") {
            let stem = [".dx90.vtx", ".dx80.vtx", ".sw.vtx", ".phy"]
                .iter()
                .find_map(|suffix| path.strip_suffix(suffix))
                .ok_or_else(|| format!("Unsupported model companion: {path}"))?;
            let model_path = format!("{stem}.mdl");
            let mdl = self
                .read(&model_path, path, false)?
                .ok_or("Companion model was not read")?;
            let header = source_formats::mdl::ModelHeader::parse(&mdl)
                .map_err(|e| format!("{path}: {e}"))?;
            if path.ends_with(".phy") {
                let text = source_formats::mdl::physics_keyvalues(&bytes, header.checksum)
                    .map_err(|e| format!("{path}: {e}"))?;
                let nodes = keyvalues::parse_lenient_bounded(text, 32, 16384)
                    .map_err(|e| format!("{path}: {e}"))?;
                self.script_nodes(&nodes, path)?;
                Ok(())
            } else {
                model::validate_vtx_material_dependencies(&bytes, header.checksum)
            }
            .map_err(|e| format!("Unsupported model dependencies in {path}: {e}"))?;
            self.enqueue(model_path, path)?;
        } else if path.ends_with(".pcf") || path.ends_with(".vcd") || path.ends_with(".res") {
            return Err(format!(
                "Portable dependency discovery is unavailable for {path}; no complete package was produced"
            ));
        } else if self.emission.as_deref() != Some(path)
            && (path.ends_with(".vbsp") || path.starts_with("scripts/") && path.ends_with(".txt"))
        {
            let text = std::str::from_utf8(&bytes).map_err(|error| format!("{path}: {error}"))?;
            let nodes = keyvalues::parse_lenient_bounded(text, 64, 16384)
                .map_err(|error| format!("{path}: {error}"))?;
            self.script_nodes(&nodes, path)?;
        }
        Ok(())
    }

    fn script_nodes(&mut self, fields: &[Node], from: &str) -> Result<(), String> {
        for field in fields {
            if let Some(children) = field.children() {
                self.script_nodes(children, from)?;
            } else if let Some(value) = field.as_str() {
                if field.key.eq_ignore_ascii_case("surfaceprop") {
                    self.surface_property(value, from)?;
                } else if value.to_ascii_lowercase().ends_with(".mdl") {
                    self.enqueue(asset_relative(value).map_err(|e| e.to_string())?, from)?;
                } else if field.key.eq_ignore_ascii_case("model")
                    || field.key.eq_ignore_ascii_case("ragdoll")
                {
                    return Err(format!(
                        "{from}: unsupported non-file model dependency {value}"
                    ));
                } else if field.key.eq_ignore_ascii_case("wave") {
                    self.enqueue_audio(value, from)?;
                } else if ["precache_file", "preload_file", "#include", "#base"]
                    .iter()
                    .any(|key| field.key.eq_ignore_ascii_case(key))
                {
                    self.enqueue(asset_relative(value).map_err(|e| e.to_string())?, from)?;
                } else if field.key.eq_ignore_ascii_case("material") {
                    self.enqueue(vmt::material_path(value), from)?;
                }
            }
        }
        Ok(())
    }
}

pub(super) fn collect(
    vmf: &Vmf,
    game: &Arc<GameFs>,
    base: &Arc<GameFs>,
    catalog: &Catalog,
    sources: &Sources,
) -> Result<Prepared, String> {
    if sources.roots.len() > 32 {
        return Err("Choose at most 32 content roots".into());
    }
    let mut roots = GameFs::default();
    for root in &sources.roots {
        let path = root
            .canonicalize()
            .map_err(|e| format!("{}: {e}", root.display()))?;
        if !path.is_dir() {
            return Err(format!(
                "Content root is not a directory: {}",
                root.display()
            ));
        }
        roots.mounts.push(MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(path),
        });
    }
    let mut assets = BTreeMap::new();
    if let Some(package) = &vmf.assets {
        assets.extend(
            package
                .entries()
                .iter()
                .map(|entry| (entry.name.clone(), entry.data.clone())),
        );
    }
    let mut bsp_source = None;
    if let Some(path) = &sources.bsp {
        let source = Source::capture(path, PakLimits::default().max_bsp_bytes as u64)
            .map_err(|e| e.to_string())?;
        let bytes = source
            .clone()
            .read()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        bsp_source = Some(source);
        let bsp = Bsp::parse_bounded(&bytes, 256 * 1024 * 1024, 4096)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let archive = bsp
            .pak_data(PakLimits::default())
            .map_err(|e| format!("{} PAK: {e}", path.display()))?;
        let mut names = BTreeSet::new();
        for entry in archive.entries {
            let name = asset_path(&entry.name).map_err(|e| e.to_string())?;
            if !names.insert(name.clone()) {
                return Err(format!(
                    "Case-colliding companion BSP entry: {}",
                    entry.name
                ));
            }
            let previous = assets.get(&name).map_or(0, Vec::len);
            let total = assets.values().map(Vec::len).sum::<usize>() - previous + entry.data.len();
            if total > PakLimits::default().max_total_bytes
                || (!assets.contains_key(&name) && assets.len() >= PakLimits::default().max_entries)
            {
                return Err(format!(
                    "Imported assets exceed the portable package budget at {name}"
                ));
            }
            assets.insert(name, entry.data);
        }
    }
    let emission = compiler::material_emission_file_field(&vmf.world.properties)?
        .filter(|path| !path.trim().is_empty())
        .map(|path| {
            compiler::validate_material_emission_file(path)?;
            asset_path(path.trim()).map_err(|e| e.to_string())
        })
        .transpose()?;
    let mut c = Collection {
        game: game.clone(),
        base: base.clone(),
        roots: Arc::new(roots),
        captures: std::array::from_fn(|_| {
            compiler::asset_sources::BuildMaterialSources::bounded(
                256 * 1024 * 1024,
                4096,
                16384,
                16384,
            )
        }),
        assets,
        base_paths: BTreeSet::new(),
        visited: BTreeSet::new(),
        queued: BTreeSet::new(),
        pending: VecDeque::new(),
        read_bytes: 0,
        emission: emission.clone(),
        surfaces: None,
        surface_uses: BTreeSet::new(),
        registry_sources: BTreeSet::new(),
        cues: None,
        sound_sources: BTreeSet::new(),
    };
    c.surface_property("default", "map surface defaults")?;
    if let Some(path) = emission {
        c.enqueue(path, "world emission file")?;
    }
    for path in c.assets.keys().cloned().collect::<Vec<_>>() {
        c.enqueue(path, "imported package")?;
    }
    for solid in vmf.solids() {
        for side in &solid.sides {
            c.enqueue(
                vmt::material_path(&side.material),
                format!("brush {} side {}", solid.id, side.id),
            )?;
        }
    }
    if let Some(sky) = vmf
        .world
        .properties
        .get("skyname")
        .filter(|value| !value.is_empty())
    {
        for suffix in compiler::sky_assets::SUFFIXES {
            c.enqueue(
                vmt::material_path(&format!("skybox/{sky}{suffix}")),
                "world skyname",
            )?;
        }
    }
    if let Some(detail) = vmf
        .world
        .properties
        .get("detailvbsp")
        .filter(|value| !value.is_empty())
    {
        c.enqueue(detail.into(), "world detailvbsp")?;
    }
    if let Some(detail) = vmf
        .world
        .properties
        .get("detailmaterial")
        .filter(|value| !value.is_empty())
    {
        c.enqueue(vmt::material_path(detail), "world detailmaterial")?;
    }
    for entity in &vmf.entities {
        let class = entity
            .properties
            .get("classname")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let class = class.as_str();
        if class == "func_instance" {
            return Err(format!(
                "Entity {}: portable sharing of external VMF instances is not yet supported",
                entity.id
            ));
        }
        if matches!(class, "move_rope" | "keyframe_rope") {
            c.enqueue(
                vmt::material_path(&render::ropes::material_name(entity)?),
                format!("entity {} rope material", entity.id),
            )?;
        }
        let water_transition = map_core::overlay::water::is_transition(entity);
        if water_transition && map_core::overlay::water::has_transition_data(entity) {
            for projection in map_core::overlay::water::parse(entity)
                .map_err(|error| format!("Entity {} water transition: {error}", entity.id))?
            {
                c.enqueue(
                    vmt::material_path(&projection.options.material),
                    format!("entity {} water transition material", entity.id),
                )?;
            }
        }
        let definition = catalog.class(class);
        if definition.is_none() && !compiler::decals::is_class(class) && !water_transition {
            return Err(format!(
                "Entity {} class {class}: load its entity definitions before collecting portable dependencies",
                entity.id
            ));
        }
        for (key, value) in entity.properties.iter() {
            if value.is_empty() {
                continue;
            }
            let kind = definition
                .and_then(|definition| definition.property(key))
                .map(|property| property.kind.as_str());
            let from = format!("entity {} {key}", entity.id);
            if key.eq_ignore_ascii_case("vscripts") {
                return Err(format!(
                    "{from}: dynamic script dependencies are unsupported"
                ));
            }
            if matches!(class, "color_correction" | "color_correction_volume")
                && key.eq_ignore_ascii_case("filename")
            {
                c.enqueue(asset_relative(value).map_err(|e| e.to_string())?, from)?;
            } else if class.starts_with("env_soundscape") && key.eq_ignore_ascii_case("soundscape")
                || class == "info_particle_system" && key.eq_ignore_ascii_case("effect_name")
            {
                return Err(format!("{from}: unresolved named resource {value}"));
            } else if (key.eq_ignore_ascii_case("model")
                || matches!(kind, Some("studio" | "sprite")))
                && !value.starts_with('*')
            {
                if value.to_ascii_lowercase().ends_with(".mdl") {
                    c.enqueue(asset_relative(value).map_err(|e| e.to_string())?, from)?;
                } else {
                    c.enqueue(crate::sprite_resource::material_path(value)?, from)?;
                }
            } else if matches!(kind, Some("material" | "decal"))
                || (class == "info_overlay" || water_transition)
                    && key.eq_ignore_ascii_case("material")
                || compiler::decals::is_class(class)
                    && key.eq_ignore_ascii_case(compiler::decals::TEXTURE_KEY)
            {
                c.enqueue(vmt::material_path(value), from)?;
            } else if matches!(kind, Some("sound")) {
                if value.to_ascii_lowercase().ends_with(".wav")
                    || value.to_ascii_lowercase().ends_with(".mp3")
                {
                    c.enqueue_audio(value, from)?;
                } else {
                    c.sound_cue(value, &from)?;
                }
            } else if matches!(kind, Some("particlesystem" | "soundscape")) {
                return Err(format!("{from}: unresolved named resource {value}"));
            }
        }
    }
    while let Some((path, from)) = c.pending.pop_front() {
        if !c.visited.insert(path.clone()) {
            continue;
        }
        if c.visited.len() > PakLimits::default().max_entries {
            return Err("Portable dependency count exceeds 4096".into());
        }
        c.process(&path, &from)?;
    }
    let archive = PakArchive {
        entries: c
            .assets
            .into_iter()
            .map(|(name, data)| PakEntry { name, data })
            .collect(),
    };
    let assets = match &vmf.assets {
        Some(previous) if previous.same_content(&archive, &c.base_paths) => previous.clone(),
        Some(previous) if previous.same_assets(&archive) => Arc::new(
            previous
                .with_base_dependencies(c.base_paths)
                .map_err(|e| e.to_string())?,
        ),
        _ => Arc::new(MapAssets::new(archive, c.base_paths).map_err(|error| error.to_string())?),
    };
    let games = [c.roots, c.game, c.base];
    let mut guards = Vec::new();
    for (capture, game) in c.captures.into_iter().zip(games) {
        guards.push((
            game.clone(),
            capture
                .source_guard(&game, &compiler::Control::default())
                .map_err(|e| e.to_string())?,
        ));
    }
    let prepared = Prepared {
        assets,
        guards,
        bsp: bsp_source,
    };
    prepared.verify()?;
    Ok(prepared)
}
