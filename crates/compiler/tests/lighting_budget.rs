mod common;

use compiler::lighting::{LightingChannel, LightingPhase, LightingProgress};
use compiler::{
    DEFAULT_LIGHTING_WORK_LIMIT, LightingMode, MAX_LIGHTING_WORK_LIMIT, MIN_LIGHTING_WORK_LIMIT,
    Options, SkyQuality,
};
use glam::DVec3;
use map_core::{Document, edit};

fn fixture() -> Document {
    let mut document = Document::new(common::room());
    document
        .edit("lights", |tx| {
            for (classname, origin, properties) in [
                (
                    "light",
                    DVec3::new(0.0, 0.0, 100.0),
                    vec![("_light".into(), "255 160 80 200".into())],
                ),
                (
                    "light_environment",
                    DVec3::new(0.0, 0.0, 128.0),
                    vec![
                        ("_light".into(), "255 255 255 200".into()),
                        ("_ambient".into(), "160 200 255 80".into()),
                        ("pitch".into(), "-90".into()),
                    ],
                ),
            ] {
                edit::create_entity(tx, classname, Some(origin), &properties, Vec::new())?;
            }
            Ok(())
        })
        .unwrap();
    document
}

fn options(lighting: LightingMode) -> Options {
    Options {
        lighting,
        sky_quality: SkyQuality::Fast,
        ..Default::default()
    }
}

fn assert_accounting(progress: &LightingProgress) {
    assert_eq!(
        progress.work,
        progress.preparation_work + progress.ldr_work + progress.hdr_work
    );
    assert_eq!(
        progress.work,
        progress.phases.iter().map(|phase| phase.work).sum::<u64>()
    );
    assert_eq!(
        progress.work,
        progress
            .contributors
            .iter()
            .map(|entry| entry.work)
            .sum::<u64>()
            + progress.other_contributors_work
            + progress.unattributed_work
    );
    assert_eq!(
        progress.lightmap_samples_completed,
        progress.ldr_lightmap_samples_completed + progress.hdr_lightmap_samples_completed
    );
    assert!(
        progress
            .phases
            .iter()
            .all(|phase| phase.samples_completed <= phase.samples_planned)
    );
}

#[test]
fn compiler_owns_lighting_work_default_range_and_validation() {
    assert_eq!(
        Options::default().lighting_work_limit,
        DEFAULT_LIGHTING_WORK_LIMIT
    );
    assert_eq!(DEFAULT_LIGHTING_WORK_LIMIT, 1_000_000_000);
    for limit in [
        MIN_LIGHTING_WORK_LIMIT,
        DEFAULT_LIGHTING_WORK_LIMIT,
        MAX_LIGHTING_WORK_LIMIT,
    ] {
        assert_eq!(
            compiler::validate_lighting_work_limit(limit).unwrap(),
            limit
        );
        assert!(
            Options {
                lighting_work_limit: limit,
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
    }
    for limit in [0, MAX_LIGHTING_WORK_LIMIT + 1, u64::MAX] {
        assert!(compiler::validate_lighting_work_limit(limit).is_err());
        assert!(
            Options {
                lighting_work_limit: limit,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
}

#[test]
fn preparation_failure_reports_work_without_claiming_a_complete_sample_plan() {
    let document = fixture();
    let failed = compiler::compile_with_options(
        document.vmf(),
        &common::empty_game(),
        Options {
            lighting_work_limit: MIN_LIGHTING_WORK_LIMIT,
            ..options(LightingMode::DirectLdrHdr)
        },
    );
    assert!(failed.report.has_errors() && failed.bsp.is_none());
    let progress = failed.stats.lighting_progress.as_ref().unwrap();
    assert_eq!(progress.current_channel, LightingChannel::Preparation);
    assert!(progress.work > MIN_LIGHTING_WORK_LIMIT);
    assert_eq!(progress.work, progress.preparation_work);
    assert_eq!(progress.lightmap_samples_completed, 0);
    assert!(!progress.completed);
    assert!(!progress.lightmap_samples_planned_complete);
    assert_accounting(progress);
}

#[test]
fn low_explicit_limit_retains_attempts_phase_samples_and_original_light_ids() {
    let document = fixture();
    let before = document.vmf().write(&Default::default());
    let game = common::empty_game();
    let settings = options(LightingMode::DirectLdr);
    let control = compiler::compile_with_options(document.vmf(), &game, settings);
    assert!(!control.report.has_errors(), "{:?}", control.report);
    let progress = control.stats.lighting_progress.as_ref().unwrap();
    assert!(progress.completed && progress.lightmap_samples_planned_complete);
    assert_eq!(
        progress.lightmap_samples_completed,
        progress.lightmap_samples_planned
    );
    assert_accounting(progress);
    let direct_work = progress
        .phases
        .iter()
        .filter(|phase| {
            phase.channel == LightingChannel::Ldr
                && matches!(
                    phase.phase,
                    LightingPhase::Direct
                        | LightingPhase::PointSpot
                        | LightingPhase::Sun
                        | LightingPhase::SkyAmbient
                )
        })
        .map(|phase| phase.work)
        .sum::<u64>();
    let channel_preparation = progress
        .phases
        .iter()
        .filter(|phase| {
            phase.channel == LightingChannel::Ldr && phase.phase == LightingPhase::Preparation
        })
        .map(|phase| phase.work)
        .sum::<u64>();
    let limit = progress.preparation_work + channel_preparation + direct_work / 2;
    let failed = compiler::compile_with_options(
        document.vmf(),
        &game,
        Options {
            lighting_work_limit: limit,
            ..settings
        },
    );
    assert!(failed.report.has_errors() && failed.bsp.is_none());
    let progress = failed.stats.lighting_progress.as_ref().unwrap();
    assert!(!progress.completed && progress.lightmap_samples_planned_complete);
    assert_eq!(progress.current_channel, LightingChannel::Ldr);
    assert_eq!(progress.limit, limit);
    assert!(progress.work > limit);
    assert!(progress.ldr_lightmap_samples_completed > 0);
    assert!(progress.ldr_lightmap_samples_completed < progress.ldr_lightmap_samples_planned);
    assert_eq!(failed.stats.lighting_total_work as u64, progress.work);
    assert_eq!(
        failed.stats.lightmap_samples as u64,
        progress.ldr_lightmap_samples_completed
    );
    assert!(progress.contributors.iter().any(|entry| entry.work != 0));
    for entry in &progress.contributors {
        assert!(
            document
                .vmf()
                .entities
                .iter()
                .any(|entity| entity.id == entry.entity_id)
        );
        assert_eq!(entry.entity_id, entry.compiled_entity_id);
        assert!(entry.instance.is_none());
    }
    assert_accounting(progress);
    let messages = format!("{:?}", failed.report);
    assert!(messages.contains(&format!("{limit} work-unit limit")));
    assert!(!messages.contains("choose Fast"));
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("previous.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    assert!(!failed.write(&path).unwrap());
    assert_eq!(std::fs::read(path).unwrap(), b"previous BSP");
    assert_eq!(document.vmf().write(&Default::default()), before);
}

#[test]
fn explicit_limit_is_shared_across_preparation_ldr_and_hdr_without_changing_samples() {
    let document = fixture();
    let game = common::empty_game();
    let settings = options(LightingMode::DirectLdrHdr);
    let control = compiler::compile_with_options(document.vmf(), &game, settings);
    assert!(!control.report.has_errors(), "{:?}", control.report);
    let progress = control.stats.lighting_progress.as_ref().unwrap();
    assert!(progress.completed && progress.preparation_work > 0 && progress.hdr_work > 0);
    assert_accounting(progress);
    assert_eq!(
        progress.ldr_lightmap_samples_completed,
        progress.hdr_lightmap_samples_completed
    );
    let limit = progress.preparation_work + progress.ldr_work;
    let ldr = compiler::compile_with_options(
        document.vmf(),
        &game,
        Options {
            lighting: LightingMode::DirectLdr,
            lighting_work_limit: limit,
            ..settings
        },
    );
    assert!(!ldr.report.has_errors(), "{:?}", ldr.report);
    let failed = compiler::compile_with_options(
        document.vmf(),
        &game,
        Options {
            lighting_work_limit: limit,
            ..settings
        },
    );
    assert!(failed.report.has_errors() && failed.bsp.is_none());
    let partial = failed.stats.lighting_progress.as_ref().unwrap();
    assert!(!partial.completed);
    assert_eq!(partial.current_channel, LightingChannel::Hdr);
    assert_eq!(partial.ldr_work, progress.ldr_work);
    assert!(partial.hdr_work > 0 && partial.work > limit);
    assert_eq!(
        partial.ldr_lightmap_samples_completed,
        progress.ldr_lightmap_samples_planned
    );
    assert!(partial.hdr_lightmap_samples_completed < partial.hdr_lightmap_samples_planned);
    assert_accounting(partial);
    let exact = compiler::compile_with_options(
        document.vmf(),
        &game,
        Options {
            lighting_work_limit: progress.work,
            ..settings
        },
    );
    assert!(!exact.report.has_errors(), "{:?}", exact.report);
    assert_eq!(
        exact.bsp.as_ref().unwrap().to_bytes(),
        control.bsp.as_ref().unwrap().to_bytes()
    );
    assert_eq!(
        exact.stats.lighting_progress.as_ref().unwrap().work,
        progress.work
    );
}
