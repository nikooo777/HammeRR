use std::rc::Rc;

use slint::{ComponentHandle, VecModel};

use crate::mcp::{SharedSession, lock};
use crate::session::Session;
use crate::session::statistics::{PAGE_SIZE, Section};
use crate::{StatisticRow, StatisticsWindow};

pub fn refresh(window: &StatisticsWindow, session: &Session) {
    let section = match window.get_section() {
        1 => Section::EntityClasses,
        2 => Section::Models,
        3 => Section::LightmapScales,
        4 => Section::DisplacementPowers,
        _ => Section::Materials,
    };
    let offset = window.get_page().max(0) as usize * PAGE_SIZE;
    let report = session.map_statistics(section, offset, PAGE_SIZE).unwrap();
    if report.rows.is_empty() && offset > 0 {
        window.set_page(0);
        refresh(window, session);
        return;
    }
    let totals = report.totals;
    window.set_brushes(
        (totals.world_brushes + totals.entity_brushes)
            .to_string()
            .into(),
    );
    window.set_brush_detail(
        format!(
            "{} world · {} in entities",
            totals.world_brushes, totals.entity_brushes
        )
        .into(),
    );
    window.set_faces(totals.brush_sides.to_string().into());
    window.set_face_detail(
        format!(
            "{} materials · {} displacements",
            totals.materials, totals.displacements
        )
        .into(),
    );
    window.set_entities(
        (totals.point_entities + totals.brush_entities)
            .to_string()
            .into(),
    );
    window.set_entity_detail(
        format!(
            "{} point · {} brush entities",
            totals.point_entities, totals.brush_entities
        )
        .into(),
    );
    window.set_details(format!("{} objects  ·  {} entity classes  ·  {} connections\n{} model paths  ·  {} hidden brushes  ·  {} hidden entities", totals.objects, totals.entity_classes, totals.connections, totals.model_paths, totals.hidden_brushes, totals.hidden_entities).into());
    window.set_has_next(report.next_offset.is_some());
    window.set_summary(if report.total_rows == 0 {
        "No entries in this map".into()
    } else {
        format!(
            "{}–{} of {} · most used first",
            offset + 1,
            offset + report.rows.len(),
            report.total_rows
        )
        .into()
    });
    window.set_rows(
        Rc::new(VecModel::from(
            report
                .rows
                .into_iter()
                .map(|row| StatisticRow {
                    name: row.name.into(),
                    count: row.count.to_string().into(),
                })
                .collect::<Vec<_>>(),
        ))
        .into(),
    );
    window.set_generation(report.generation.into());
    window.set_revision(report.revision.into());
}

pub fn refresh_changed(window: &StatisticsWindow, session: &Session) {
    if window.get_generation() != session.document_generation().to_string()
        || window.get_revision() != session.document().revision().to_string()
    {
        refresh(window, session);
    }
}

pub fn bind(window: &StatisticsWindow, shared: &SharedSession) {
    window.on_refresh({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || refresh(&window.unwrap(), &lock(&shared))
    });
    window.on_cancel({
        let window = window.as_weak();
        move || {
            let _ = window.unwrap().hide();
        }
    });
    window.window().on_close_requested({
        let window = window.as_weak();
        move || {
            window.unwrap().invoke_cancel();
            slint::CloseRequestResponse::HideWindow
        }
    });
}
