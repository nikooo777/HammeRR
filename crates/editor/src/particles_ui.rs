use crate::{
    ParticleBrowserWindow, ParticleChoiceRow,
    runtime::{SharedSession, lock},
    session::{Actor, Session},
};
use slint::ComponentHandle;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

const PAGE_SIZE: usize = 60;
type Choice = (u64, String);

#[derive(Default)]
struct Catalog {
    groups: Vec<String>,
    offset: usize,
}

pub struct Controller {
    pub window: ParticleBrowserWindow,
    token: Rc<Cell<Option<u64>>>,
    seen: Rc<RefCell<Option<Choice>>>,
    catalog: Rc<RefCell<Catalog>>,
}

fn choice(report: &serde_json::Value) -> Option<Choice> {
    (!report["particle"].is_null()).then(|| {
        (
            report["token"].as_u64().unwrap(),
            report["particle"]["name"].as_str().unwrap().into(),
        )
    })
}

fn present(
    window: &ParticleBrowserWindow,
    report: &serde_json::Value,
    seen: &RefCell<Option<Choice>>,
) {
    let Some(key) = choice(report) else {
        return;
    };
    let particle = &report["particle"];
    window.set_value(key.1.clone().into());
    let mut details = vec![
        format!("Particle file · {}", particle["source"].as_str().unwrap()),
        format!(
            "Material · {}",
            particle["material"].as_str().unwrap_or("Unspecified")
        ),
        format!("Authored maximum particles · {}", particle["max_particles"]),
        format!(
            "Manifest precache marker · {}",
            if particle["precache"] == true {
                "Yes"
            } else {
                "No"
            }
        ),
    ];
    for (field, label) in [
        ("renderers", "Renderers"),
        ("emitters", "Emitters"),
        ("initializers", "Initializers"),
        ("operators", "Operators"),
        ("forces", "Forces"),
        ("constraints", "Constraints"),
    ] {
        let names = particle["functions"][field]
            .as_array()
            .unwrap()
            .iter()
            .map(|name| name.as_str().unwrap())
            .collect::<Vec<_>>();
        if !names.is_empty() {
            details.push(format!("{label} ({})\n{}", names.len(), names.join("\n")));
        }
    }
    let children = particle["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap())
        .collect::<Vec<_>>();
    if !children.is_empty() {
        details.push(format!(
            "Child systems ({})\n{}",
            children.len(),
            children.join("\n")
        ));
    }
    details.push(format!(
        "{} elements · {:.1} KiB · DMX binary 2 / PCF 1",
        particle["elements"],
        particle["encoded_bytes"].as_u64().unwrap() as f64 / 1024.0
    ));
    window.set_details(details.join("\n\n").into());
    window.set_can_apply(true);
    window.set_message("Apply sets Particle effect on the captured entity. Search and inspection leave the map unchanged.".into());
    *seen.borrow_mut() = Some(key);
}

fn catalog(window: &ParticleBrowserWindow, session: &mut Session, state: &mut Catalog) {
    let group = state
        .groups
        .get(window.get_group_index().max(0) as usize)
        .map_or("all", String::as_str)
        .to_owned();
    let report =
        match session.browse_particles(&window.get_query(), &group, state.offset, PAGE_SIZE) {
            Ok(report) => report,
            Err(error) => {
                window.set_rows(Default::default());
                window.set_can_previous(false);
                window.set_can_next(false);
                window.set_page_info("Particle catalog unavailable".into());
                window.set_message(error.into());
                return;
            }
        };
    state.groups = report["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().into())
        .collect();
    window.set_groups(slint::ModelRc::new(slint::VecModel::from(
        report["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| format!("{} ({})", row["label"].as_str().unwrap(), row["count"]).into())
            .collect::<Vec<slint::SharedString>>(),
    )));
    let index = state
        .groups
        .iter()
        .position(|candidate| candidate == &group)
        .unwrap_or(0);
    window.set_group_index(index as i32);
    if state.groups[index] != group {
        state.offset = 0;
        catalog(window, session, state);
        return;
    }
    window.set_rows(slint::ModelRc::new(slint::VecModel::from(
        report["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| ParticleChoiceRow {
                name: row["name"].as_str().unwrap().into(),
                detail: format!(
                    "{} · {}{}",
                    row["source"].as_str().unwrap(),
                    row["material"].as_str().unwrap_or("No material"),
                    if row["ambiguous"] == true {
                        " · duplicate name"
                    } else {
                        ""
                    }
                )
                .into(),
            })
            .collect::<Vec<_>>(),
    )));
    let total = report["total"].as_u64().unwrap() as usize;
    window.set_can_previous(state.offset > 0);
    window.set_can_next(state.offset.saturating_add(PAGE_SIZE) < total);
    window.set_page_info(if total == 0 {
        "No particle systems".into()
    } else {
        format!(
            "{}–{} of {total}",
            state.offset + 1,
            (state.offset + PAGE_SIZE).min(total)
        )
        .into()
    });
}

impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = ParticleBrowserWindow::new()?;
        window
            .window()
            .set_size(slint::LogicalSize::new(1000.0, 720.0));
        let token = Rc::new(Cell::new(None));
        let seen = Rc::new(RefCell::new(None));
        let catalog_state = Rc::new(RefCell::new(Catalog::default()));
        window.on_search({
            let (window, shared, state) = (window.as_weak(), shared.clone(), catalog_state.clone());
            move || {
                let mut state = state.borrow_mut();
                state.offset = 0;
                catalog(&window.unwrap(), &mut lock(&shared), &mut state);
            }
        });
        window.on_group_changed({
            let window = window.as_weak();
            move || window.unwrap().invoke_search()
        });
        window.on_page({
            let (window, shared, state) = (window.as_weak(), shared.clone(), catalog_state.clone());
            move |direction| {
                let mut state = state.borrow_mut();
                state.offset = if direction < 0 {
                    state.offset.saturating_sub(PAGE_SIZE)
                } else {
                    state.offset.saturating_add(PAGE_SIZE)
                };
                catalog(&window.unwrap(), &mut lock(&shared), &mut state);
            }
        });
        window.on_choose({
            let (window, shared, token, seen) = (
                window.as_weak(),
                shared.clone(),
                token.clone(),
                seen.clone(),
            );
            move |name| {
                let window = window.unwrap();
                let result = token
                    .get()
                    .ok_or("Reopen the particle browser".to_owned())
                    .and_then(|token| lock(&shared).choose_particle(token, &name));
                match result {
                    Ok(report) => present(&window, &report, &seen),
                    Err(error) => {
                        window.set_can_apply(false);
                        window.set_message(error.into());
                    }
                }
            }
        });
        window.on_reload({
            let (window, shared) = (window.as_weak(), shared.clone());
            move || {
                lock(&shared).reload_particle_assets();
                let window = window.unwrap();
                window.set_can_apply(false);
                window.set_message("Particles reloaded. Cancel and reopen this choice.".into());
            }
        });
        window.on_apply({
            let (window, shared, token, seen) = (
                window.as_weak(),
                shared.clone(),
                token.clone(),
                seen.clone(),
            );
            move || {
                let window = window.unwrap();
                let result = token
                    .get()
                    .ok_or("Reopen the particle browser".to_owned())
                    .and_then(|token| {
                        let mut session = lock(&shared);
                        let report = session.particle_choice(token)?;
                        if *seen.borrow() != choice(&report) {
                            present(&window, &report, &seen);
                            return Ok(false);
                        }
                        session.apply_particle_choice(Actor::User, token)?;
                        Ok(true)
                    });
                match result {
                    Ok(true) => window.invoke_cancel(),
                    Ok(false) => window
                        .set_message("Particle choice changed. Review it before Apply.".into()),
                    Err(error) => {
                        window.set_can_apply(false);
                        window.set_message(error.into());
                    }
                }
            }
        });
        window.on_cancel({
            let (window, shared, token) = (window.as_weak(), shared.clone(), token.clone());
            move || {
                if let Some(token) = token.take() {
                    let _ = lock(&shared).cancel_particle_choice(token);
                }
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
        Ok(Self {
            window,
            token,
            seen,
            catalog: catalog_state,
        })
    }

    pub fn initialize(&self, session: &mut Session, id: u32) -> Result<(), String> {
        let report = session.begin_particle_choice(id, true)?;
        self.token.set(report["token"].as_u64());
        *self.seen.borrow_mut() = None;
        self.window.set_target(
            format!(
                "Particle system · entity:{id} · {}",
                report["label"].as_str().unwrap()
            )
            .into(),
        );
        self.window.set_query("".into());
        self.window.set_group_index(0);
        *self.catalog.borrow_mut() = Catalog::default();
        self.window
            .set_value(report["value"].as_str().unwrap_or("").into());
        self.window.set_details("".into());
        self.window.set_can_apply(false);
        self.window
            .set_message("Choose a named system to inspect its definition.".into());
        catalog(&self.window, session, &mut self.catalog.borrow_mut());
        if !report["particle"].is_null() {
            present(&self.window, &report, &self.seen);
        } else if !self.window.get_value().is_empty() {
            match session
                .choose_particle(report["token"].as_u64().unwrap(), &self.window.get_value())
            {
                Ok(report) => present(&self.window, &report, &self.seen),
                Err(error) => self.window.set_message(error.into()),
            }
        }
        Ok(())
    }

    pub fn refresh(&self, session: &Session) {
        if let Some(token) = self.token.get() {
            match session.particle_choice(token) {
                Ok(report) => {
                    if *self.seen.borrow() != choice(&report) {
                        present(&self.window, &report, &self.seen);
                    }
                }
                Err(error) => {
                    self.window.set_can_apply(false);
                    self.window.set_message(error.into());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
