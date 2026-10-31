use super::*;
use map_core::subobject::Component;

#[derive(Default)]
struct Sources {
    snapshot: String,
    corners: Vec<Component>,
}

pub(super) fn refresh(app: &AppWindow, session: &Session) {
    let state = session.magnetic_snapping();
    let revision = state["settings_revision"].to_string();
    if app.get_magnetic_settings_revision().as_str() != revision.as_str() {
        app.set_magnetic_settings_revision(revision.into());
        app.set_magnetic_enabled(state["enabled"] == true);
        app.set_magnetic_radius(state["radius"].to_string().into());
    }
    let preview = &state["preview"];
    let result = &preview["result"];
    let status = if state["source_stale"] == true {
        "Source is unavailable in this selection. Refresh magnetic sources and choose a corner."
            .to_owned()
    } else if preview["stale"] == true {
        "Magnetic preview is stale. Preview movement again before Apply.".to_owned()
    } else if result["captured"] == true {
        let label = match result["target"]["kind"].as_str() {
            Some("brush_corner") => "brush corner",
            Some("terrain_vertex") => "terrain vertex",
            Some("model_vertex") => "model vertex",
            _ => "vertex",
        };
        format!(
            "Captured {label} · {:.3} units from proposed source\nMove {}, {}, {}",
            result["distance"].as_f64().unwrap(),
            result["translation"][0],
            result["translation"][1],
            result["translation"][2]
        )
    } else if result["reason"] == "outside_radius" {
        format!(
            "Outside radius · {:.3} units. Ordinary handle/grid movement retained.",
            result["distance"].as_f64().unwrap()
        )
    } else if result["reason"] == "no_surface" {
        "No target surface under the pointer. Ordinary handle/grid movement retained.".to_owned()
    } else if state["enabled"] == true {
        "Drag Move handles over a visible target. Capture can leave the handle axis or plane."
            .to_owned()
    } else {
        "Choose a Source corner, enable Magnetic snapping, then Apply magnetic settings.".to_owned()
    };
    app.set_magnetic_status(status.into());
}

fn refresh_sources(
    app: &AppWindow,
    session: &mut Session,
    form: &RefCell<Sources>,
) -> Result<(), String> {
    let report = session.magnetic_snap_sources()?;
    let state = session.magnetic_snapping();
    let mut form = form.borrow_mut();
    let mut choices = vec![SharedString::from("Choose a Source corner")];
    form.corners.clear();
    let mut selected = 0;
    for source in report["sources"]
        .as_array()
        .ok_or("Magnetic source list is unavailable")?
    {
        let component = Component {
            solid: serde_json::from_value(source["solid"].clone())
                .map_err(|error| error.to_string())?,
            sides: serde_json::from_value(source["sides"].clone())
                .map_err(|error| error.to_string())?,
        };
        choices.push(
            format!(
                "{} · {}, {}, {} · brush {} · faces {}",
                source["owner"].as_str().unwrap(),
                source["position"][0],
                source["position"][1],
                source["position"][2],
                component.solid,
                source["sides"]
            )
            .into(),
        );
        if source["solid"] == state["source_corner"]["solid"]
            && source["sides"] == state["source_corner"]["sides"]
            && state["source_stale"] != true
        {
            selected = choices.len() - 1;
        }
        form.corners.push(component);
    }
    form.snapshot = report["snapshot"]
        .as_str()
        .ok_or("Magnetic source snapshot is unavailable")?
        .into();
    app.set_magnetic_source_choices(Rc::new(VecModel::from(choices)).into());
    app.set_magnetic_source_index(selected as i32);
    refresh(app, session);
    Ok(())
}

pub(super) fn bind(app: &AppWindow, access: &runtime::DocumentAccess) {
    let form = Rc::new(RefCell::new(Sources::default()));
    app.on_magnetic_refresh_sources({
        let callback = admission::SessionCallback::new(app, access);
        let form = form.clone();
        move || {
            callback.input(|app, session| match refresh_sources(&app, session, &form) {
                Ok(()) => app.set_message(
                    if form.borrow().corners.is_empty() {
                        "No visible flat brush corners are available in this selection."
                    } else {
                        "Magnetic sources refreshed."
                    }
                    .into(),
                ),
                Err(error) => {
                    form.borrow_mut().snapshot.clear();
                    app.set_message(error.into());
                }
            })
        }
    });
    app.on_magnetic_apply_settings({
        let callback = admission::SessionCallback::new(app, access);
        move || {
            callback.input(|app, session| {
                let result = (|| {
                    let radius = app
                        .get_magnetic_radius()
                        .parse::<f64>()
                        .map_err(|_| "Enter a magnetic radius in world units")?;
                    let form = form.borrow();
                    let enabled = app.get_magnetic_enabled();
                    let current = session.magnetic_sources_snapshot();
                    let source = if enabled || form.snapshot == current {
                        usize::try_from(app.get_magnetic_source_index() - 1)
                            .ok()
                            .and_then(|index| form.corners.get(index))
                            .cloned()
                    } else {
                        None
                    };
                    let snapshot = if enabled {
                        form.snapshot.clone()
                    } else {
                        current
                    };
                    session.set_magnetic_snapping(Actor::User, enabled, radius, source, &snapshot)
                })();
                match result {
                    Ok(_) => match refresh_sources(&app, session, &form) {
                        Ok(()) => app.set_message("Magnetic settings applied.".into()),
                        Err(error) => app.set_message(error.into()),
                    },
                    Err(error) => app.set_message(error.into()),
                }
                refresh(&app, session);
            })
        }
    });
}
