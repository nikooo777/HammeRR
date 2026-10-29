use glam::DVec3;
use source_formats::{bsp::LightingKind, vhv};

use super::sources::SourceSnapshot;
use super::{Allocation, Budget, Candidates, Channel, Context, Indirect, Prepared, Report};

pub(in crate::lighting) struct ChannelState {
    kind: LightingKind,
    direct: Vec<DVec3>,
    snapshot: SourceSnapshot,
    work: usize,
    _allocation: Allocation,
}

impl ChannelState {
    pub(in crate::lighting) fn sources(&self) -> &SourceSnapshot {
        &self.snapshot
    }
}

pub(in crate::lighting) fn begin_channel(
    prepared: &Prepared,
    ctx: &Context<'_>,
    kind: LightingKind,
    candidates: &mut Candidates,
    report: &mut Report,
    budget: &mut Budget<'_>,
) -> Result<ChannelState, String> {
    let start = budget.work();
    let plan = prepared
        .reflection
        .as_ref()
        .ok_or("Reflected prop channel has no prepared source geometry")?;
    budget.plan_samples(plan.vertices)?;
    budget.samples_planned_complete();
    let allocation = budget.reserve::<DVec3>(plan.vertices)?;
    let mut direct = Vec::with_capacity(plan.vertices);
    let (mut colors, color_allocation) = SourceSnapshot::buffer(plan.vertices, budget)?;
    for receiver in &prepared.receivers {
        budget.consume(1)?;
        let data = &prepared.models[receiver.model];
        let used = &plan.models[receiver.model];
        let placement = receiver
            .prop
            .transform()
            .map_err(|error| error.to_string())?;
        let angular = if receiver.prop.flags & super::IGNORE_NORMALS != 0 {
            crate::lighting::ReceiverAngularMode::IgnoreNormals
        } else {
            crate::lighting::ReceiverAngularMode::Normal
        };
        let result = (|| {
            for &original in &used.originals {
                budget.consume(1)?;
                let color = super::vertex_color(
                    original as usize,
                    &data.model.vertices[original as usize],
                    receiver,
                    &placement,
                    ctx,
                    None,
                    angular,
                    &[],
                    0.0,
                    None,
                    candidates,
                    budget,
                )?;
                budget.consume(32)?;
                colors.push(super::color::encode(color)?);
                direct.push(color);
                budget.finish_sample();
            }
            Ok::<(), String>(())
        })();
        if let Err(message) = result {
            budget.consume(0)?;
            report.diagnostics.push(crate::Diagnostic::error(
                message,
                Some(format!("entity:{}", receiver.source)),
                Some(DVec3::from_array(receiver.prop.origin.map(f64::from))),
            ));
            return Err("Reflected static-prop direct lighting failed".into());
        }
    }
    Ok(ChannelState {
        kind,
        direct,
        snapshot: SourceSnapshot::new(plan.geometry.clone(), colors, color_allocation),
        work: budget.work() - start,
        _allocation: allocation,
    })
}

pub(in crate::lighting) fn advance_channel(
    state: &mut ChannelState,
    prepared: &Prepared,
    ctx: &Context<'_>,
    indirect: Indirect<'_>,
    candidates: &mut Candidates,
    report: &mut Report,
    budget: &mut Budget<'_>,
) -> Result<(), String> {
    let start = budget.work();
    let plan = prepared
        .reflection
        .as_ref()
        .ok_or("Reflected prop channel lost its source preparation")?;
    if state.direct.len() != plan.vertices || !state.snapshot.same_geometry(&plan.geometry) {
        return Err("Reflected prop channel does not match its prepared vertex bindings".into());
    }
    budget.plan_samples(plan.vertices)?;
    budget.samples_planned_complete();
    let count = indirect.quality.directions();
    let _directions = budget.reserve::<DVec3>(count)?;
    budget.consume(count)?;
    let directions = crate::lighting::bounce::directions(count);
    let normal_cone = super::indirect::cone(crate::lighting::ReceiverAngularMode::Normal, count);
    let sphere_cone = super::indirect::cone(
        crate::lighting::ReceiverAngularMode::IgnoreNormals,
        ctx.direct.ambient_directions.len(),
    );
    let (mut colors, allocation) = SourceSnapshot::buffer(plan.vertices, budget)?;
    for (receiver_index, receiver) in prepared.receivers.iter().enumerate() {
        budget.consume(1)?;
        let data = &prepared.models[receiver.model];
        let used = &plan.models[receiver.model];
        let placement = receiver
            .prop
            .transform()
            .map_err(|error| error.to_string())?;
        let angular = if receiver.prop.flags & super::IGNORE_NORMALS != 0 {
            crate::lighting::ReceiverAngularMode::IgnoreNormals
        } else {
            crate::lighting::ReceiverAngularMode::Normal
        };
        let cone = match angular {
            crate::lighting::ReceiverAngularMode::Normal => normal_cone,
            crate::lighting::ReceiverAngularMode::IgnoreNormals => sphere_cone,
        };
        let result = (|| {
            for (compact, &original) in used.originals.iter().enumerate() {
                budget.consume(1)?;
                let direct = state.direct[plan.ranges[receiver_index].start + compact];
                let color = super::vertex_color(
                    original as usize,
                    &data.model.vertices[original as usize],
                    receiver,
                    &placement,
                    ctx,
                    Some(&indirect),
                    angular,
                    &directions,
                    cone,
                    Some(direct),
                    candidates,
                    budget,
                )?;
                budget.consume(32)?;
                colors.push(super::color::encode(color)?);
                budget.finish_sample();
            }
            Ok::<(), String>(())
        })();
        if let Err(message) = result {
            budget.consume(0)?;
            report.diagnostics.push(crate::Diagnostic::error(
                message,
                Some(format!("entity:{}", receiver.source)),
                Some(DVec3::from_array(receiver.prop.origin.map(f64::from))),
            ));
            return Err("Reflected static-prop indirect lighting failed".into());
        }
    }
    state.snapshot = SourceSnapshot::new(plan.geometry.clone(), colors, allocation);
    state.work += budget.work() - start;
    Ok(())
}

pub(in crate::lighting) fn publish_channel(
    prepared: &mut Prepared,
    state: ChannelState,
    budget: &mut Budget<'_>,
) -> Result<(), String> {
    let start = budget.work();
    let plan = prepared
        .reflection
        .as_ref()
        .ok_or("Reflected prop publication has no source preparation")?;
    if state.direct.len() != plan.vertices || !state.snapshot.same_geometry(&plan.geometry) {
        return Err("Reflected prop publication lost its compact vertex bindings".into());
    }
    let channel = if state.kind == LightingKind::Hdr {
        Channel::Hdr
    } else {
        Channel::Ldr
    };
    if prepared
        .generated
        .iter()
        .any(|asset| asset.channel == channel)
    {
        return Err("Static-prop channel was already published".into());
    }
    for (receiver_index, receiver) in prepared.receivers.iter().enumerate() {
        budget.consume(1)?;
        let data = &prepared.models[receiver.model];
        let used = &plan.models[receiver.model];
        let _meshes = budget.reserve::<vhv::Mesh>(data.layout.sections.len())?;
        let _colors = budget.reserve::<vhv::Bgra8>(data.layout.vertices.len())?;
        let mut meshes = Vec::with_capacity(data.layout.sections.len());
        for section in &data.layout.sections {
            let mut colors = Vec::with_capacity(section.vertices.len());
            for hardware in section.vertices.clone() {
                budget.consume(1)?;
                let compact = plan.ranges[receiver_index].start + used.hardware[hardware] as usize;
                colors.push(state.snapshot.all_colors()[compact]);
            }
            meshes.push(vhv::Mesh {
                layout: vhv::MeshLayout {
                    lod: section.lod as u32,
                    vertex_count: colors.len() as u32,
                },
                colors,
            });
        }
        let (asset, total) = super::encode_output(
            receiver.metadata.ordinal,
            data.layout.checksum as u32,
            state.kind,
            meshes,
            prepared.generated_bytes,
            &mut prepared.output_allocation,
            budget,
        )?;
        let bytes = asset.data.len();
        prepared.generated.push(asset);
        prepared.generated_bytes = total;
        match state.kind {
            LightingKind::Ldr => prepared.stats.ldr_bytes += bytes,
            LightingKind::Hdr => prepared.stats.hdr_bytes += bytes,
        }
    }
    prepared.stats.work += state.work + budget.work() - start;
    Ok(())
}
