use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::Serialize;
use source_formats::gamefs::GameFs;
use source_formats::vmf::Vmf;

use super::{binding, *};
use crate::asset_sources::{AssetSourceGuard, BuildMaterialSources};
use crate::instances::{DependencyGuard, OccurrencePath, SourceContext};
use crate::{Control, Report, map::Map};

#[derive(Debug, Serialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum CubemapHelperSource {
    Root {
        helper: u32,
    },
    Instance {
        owner: u32,
        file: String,
        helper: u32,
        #[serde(serialize_with = "crate::instances::serialize_occurrence")]
        occurrence: Arc<OccurrencePath>,
    },
}

#[derive(Debug)]
pub struct CubemapBindingSources {
    dependencies: Option<DependencyGuard>,
    materials: AssetSourceGuard,
    reflections: Option<ReflectionSourceGuard>,
}

impl CubemapBindingSources {
    pub fn verify_metadata(&self) -> Result<(), String> {
        if let Some(dependencies) = &self.dependencies {
            dependencies.verify_metadata().map_err(changed)?;
        }
        self.materials.verify_metadata().map_err(changed)?;
        if let Some(reflections) = &self.reflections {
            reflections.verify_metadata().map_err(changed)?;
        }
        Ok(())
    }

    pub fn verify(&self, game: &GameFs, control: &Control) -> Result<(), String> {
        self.verify_metadata()?;
        if let Some(dependencies) = &self.dependencies {
            dependencies.verify(control).map_err(changed)?;
        }
        self.materials
            .verify_sources(game, control)
            .map_err(changed)?;
        self.verify_metadata()
    }
}

fn changed(error: std::io::Error) -> String {
    format!("Cubemap inspection sources changed; refresh or preview bindings again: {error}")
}

fn report_errors(report: &Report) -> String {
    report
        .diagnostics
        .iter()
        .filter(|item| item.level == crate::Level::Error)
        .map(|item| item.message.as_str())
        .collect::<Vec<_>>()
        .join("; ")
}

#[derive(Debug)]
pub struct CubemapBindingInspection {
    pub report: CubemapBindingReport,
    pub helper_sources: BTreeMap<u32, CubemapHelperSource>,
    pub sources: Arc<CubemapBindingSources>,
}

pub fn inspect_cubemap_bindings(
    vmf: &Vmf,
    fs: &GameFs,
    query: &CubemapBindingQuery,
    control: &Control,
) -> Result<CubemapBindingReport, String> {
    inspect_root_cubemap_bindings(vmf, fs, None, query, control).map(|inspection| inspection.report)
}

pub fn inspect_root_cubemap_bindings(
    vmf: &Vmf,
    fs: &GameFs,
    source: Option<&SourceContext>,
    query: &CubemapBindingQuery,
    control: &Control,
) -> Result<CubemapBindingInspection, String> {
    control.checkpoint()?;
    if query.limit > 1024 || query.query.as_ref().is_some_and(|text| text.len() > 256) {
        return Err("Cubemap binding query exceeds its limit".into());
    }
    let requested: BTreeSet<_> = query.face_ids.iter().copied().collect();
    if requested.len() > 1024 || requested.len() != query.face_ids.len() || requested.contains(&0) {
        return Err(
            "Cubemap inspection face IDs must be distinct positive IDs, at most 1024".into(),
        );
    }
    if crate::instances::requires_source(vmf) {
        binding::validate_instance_root_bindings(vmf, control)?;
    }
    let root_sides: BTreeSet<_> = vmf.side_ids().collect();
    if !requested.is_subset(&root_sides) {
        return Err("Cubemap inspection targets must belong to the original root map".into());
    }
    let mut report = Report::default();
    let context = CompileContext::new(query.namespace.as_deref().unwrap_or("untitled"))?;
    let instances = crate::instances::resolve_for_compile_with_texture_shadows(
        vmf,
        source,
        fs,
        query.brush_texture_shadows,
        control,
        &mut report,
    )
    .map_err(|error| error.message)?;
    let materials = instances.material_sources.clone().unwrap_or_else(|| {
        if query.brush_texture_shadows {
            BuildMaterialSources::with_retained_material_text()
        } else {
            BuildMaterialSources::without_retained_payloads()
        }
    });
    if instances.has_brush_models() {
        instances
            .validate_brush_model_materials(fs, &materials, control, query.brush_texture_shadows)
            .map_err(|error| error.message)?;
    }
    let assembled = instances.compilation_vmf()?;
    let prepared = crate::cordons::prepare_with_material_sources(
        assembled,
        fs,
        &mut report,
        control,
        Some(&materials),
    )?;
    let included: BTreeSet<_> = prepared
        .vmf
        .entities
        .iter()
        .map(|entity| entity.id)
        .collect();
    let helpers: Vec<_> = vmf
        .entities
        .iter()
        .filter(|entity| {
            entity
                .classname()
                .is_some_and(|name| name.eq_ignore_ascii_case("env_cubemap"))
        })
        .take(4097)
        .map(|entity| CubemapHelperStatus {
            helper_id: entity.id,
            hidden: entity.hidden,
            excluded: !included.contains(&entity.id),
            origin: entity.origin().map(|origin| origin.to_array()),
        })
        .collect();
    if helpers.len() > 4096 {
        return Err("Cubemap helper inspection limit exceeded".into());
    }
    if query
        .helper_id
        .is_some_and(|id| !helpers.iter().any(|helper| helper.helper_id == id))
    {
        return Err("Requested root cubemap helper is missing".into());
    }
    let mut map = Map::load_with_material_sources_controlled(
        prepared.vmf.as_ref(),
        fs,
        &mut report,
        control,
        None,
        &materials,
        query.brush_texture_shadows,
    )?;
    if report.has_errors() {
        instances.annotate(&mut report);
        return Err(report_errors(&report));
    }
    let mut helper_sources = BTreeMap::new();
    let mut reflections = None;
    let (samples, assignments) = if map.cubemaps.is_empty() {
        if !requested.is_empty() {
            return Err(
                "Explicit cubemap face inspection requires a visible included helper".into(),
            );
        }
        (Vec::new(), Vec::new())
    } else {
        let plan = binding::prepare(
            prepared.vmf.as_ref(),
            fs,
            &mut map,
            &context,
            binding::PrepareOptions::default(),
            &mut report,
            control,
        );
        let plan = match plan {
            Ok(plan) => plan,
            Err(message) => {
                if report.has_errors() {
                    instances.annotate(&mut report);
                    return Err(report_errors(&report));
                }
                return Err(message);
            }
        };
        if !requested.is_subset(&plan.eligible_faces) {
            return Err("Requested cubemap face is missing, hidden, outside the compile region or does not produce a visible compiled side".into());
        }
        reflections = Some(
            capture_reflection_source_guard(&plan.manifest, fs, control)
                .map_err(|error| error.to_string())?,
        );
        for sample in &plan.manifest.samples {
            let origin = match instances.entity_origin(sample.helper_id) {
                Some((owner, file, helper)) => CubemapHelperSource::Instance {
                    owner,
                    file: file.into(),
                    helper,
                    occurrence: instances
                        .entity_occurrence(sample.helper_id)
                        .ok_or("Cubemap sample occurrence provenance is missing")?
                        .clone(),
                },
                None => CubemapHelperSource::Root {
                    helper: sample.helper_id,
                },
            };
            helper_sources.insert(sample.helper_id, origin);
        }
        (plan.manifest.samples, plan.manifest.assignments)
    };
    let needle = query.query.as_deref().unwrap_or("").to_ascii_lowercase();
    let filtered: Vec<_> = assignments
        .into_iter()
        .filter(|assignment| {
            root_sides.contains(&assignment.face_id)
                && (requested.is_empty() || requested.contains(&assignment.face_id))
                && query
                    .helper_id
                    .is_none_or(|helper| helper == assignment.helper_id)
                && (needle.is_empty()
                    || assignment.material.contains(&needle)
                    || assignment.face_id.to_string().contains(&needle))
        })
        .collect();
    let total = filtered.len();
    let limit = if query.limit == 0 { 256 } else { query.limit };
    let sources = Arc::new(CubemapBindingSources {
        dependencies: instances.dependencies().cloned(),
        materials: materials
            .source_guard(fs, control)
            .map_err(|error| error.to_string())?,
        reflections,
    });
    sources.verify(fs, control)?;
    Ok(CubemapBindingInspection {
        report: CubemapBindingReport {
            namespace: context.namespace().into(),
            helpers,
            samples,
            total,
            assignments: filtered
                .into_iter()
                .skip(query.offset)
                .take(limit)
                .collect(),
        },
        helper_sources,
        sources,
    })
}
