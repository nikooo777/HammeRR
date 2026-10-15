use render::fog::Fog;
use serde_json::{Value, json};
use source_formats::vmf::Entity;

use super::Session;

const COLOR_ERROR: &str = "fogcolor needs exactly three integer RGB channels from 0 to 255";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Selection {
    pub enabled: bool,
    pub controller: Option<u32>,
}

fn property<'a>(entity: &'a Entity, key: &str) -> Result<Option<&'a str>, String> {
    let mut values = entity
        .properties
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case(key));
    let value = values.next().map(|(_, value)| value.trim());
    if values.next().is_some() {
        return Err(format!(
            "{key} has ambiguous case-insensitive duplicate keys"
        ));
    }
    Ok(value)
}

fn number(entity: &Entity, key: &str, default: Option<f32>) -> Result<f32, String> {
    let value = match property(entity, key)? {
        Some(value) => value
            .parse::<f32>()
            .map_err(|_| format!("{key} must be a finite number"))?,
        None => default.ok_or_else(|| format!("{key} is missing; author an explicit value"))?,
    };
    if !value.is_finite() {
        return Err(format!("{key} must be a finite number"));
    }
    Ok(value)
}

fn boolean(entity: &Entity, key: &str) -> Result<bool, String> {
    match property(entity, key)?.unwrap_or("0") {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(format!("{key} must be 0 or 1")),
    }
}

fn parse(entity: &Entity) -> Result<Fog, String> {
    if !entity.solids.is_empty() {
        return Err("env_fog_controller must be a point entity".into());
    }
    if boolean(entity, "fogblend")? {
        return Err("fogblend=1 directional color blending is unsupported".into());
    }
    if boolean(entity, "fogradial")? {
        return Err("fogradial=1 radial fog is unsupported; this preview uses view depth".into());
    }
    if number(entity, "farz", Some(-1.0))? > 0.0 {
        return Err("Positive farz clipping is unsupported".into());
    }
    for key in ["HDRColorScale", "ZoomFogScale"] {
        if number(entity, key, Some(1.0))? != 1.0 {
            return Err(format!("{key} must be 1 for this LDR preview"));
        }
    }
    if number(entity, "foglerptime", Some(0.0))? < 0.0 {
        return Err("foglerptime must be nonnegative".into());
    }
    let color =
        property(entity, "fogcolor")?.ok_or("fogcolor is missing; author an explicit RGB value")?;
    let mut channels = color.split_whitespace();
    let mut color = [0; 3];
    for channel in &mut color {
        *channel = channels
            .next()
            .ok_or(COLOR_ERROR)?
            .parse::<u8>()
            .map_err(|_| COLOR_ERROR)?;
    }
    if channels.next().is_some() {
        return Err(COLOR_ERROR.into());
    }
    Fog::new(
        color,
        number(entity, "fogstart", None)?,
        number(entity, "fogend", None)?,
        number(entity, "fogmaxdensity", Some(1.0))?,
        boolean(entity, "fogenable")?,
    )
}

fn settings(fog: Fog) -> Value {
    json!({"color":fog.color(), "start":fog.start(), "end":fog.end(), "max_density":fog.max_density(), "enabled":fog.enabled()})
}

impl Session {
    pub fn fog_preview_snapshot(&self) -> String {
        format!(
            "{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            self.view_serial
        )
    }

    fn fog_controller(&self, id: u32) -> Result<Fog, String> {
        let entity = self
            .document
            .entity(id)
            .ok_or_else(|| format!("entity:{id} no longer exists; choose a fog controller"))?;
        if !entity
            .classname()
            .is_some_and(|name| name.eq_ignore_ascii_case("env_fog_controller"))
        {
            return Err(format!("entity:{id} is not an env_fog_controller"));
        }
        parse(entity).map_err(|error| format!("entity:{id}: {error}"))
    }

    pub fn fog_preview_state(&self) -> Value {
        self.fog_preview_report(self.fog_preview.enabled, self.fog_preview.controller)
    }

    pub fn fog_preview_report(&self, enabled: bool, controller: Option<u32>) -> Value {
        let selected = controller.map(|id| self.fog_controller(id));
        let issue = match &selected {
            Some(Err(error)) => Some(error.clone()),
            None if enabled => {
                Some("Choose an existing env_fog_controller; no controller is selected".into())
            }
            _ => None,
        };
        let fog = selected.and_then(Result::ok);
        let mut controllers = self.document.vmf().entities.iter()
            .filter(|entity| entity.classname().is_some_and(|name| name.eq_ignore_ascii_case("env_fog_controller")))
            .map(|entity| {
                let result = parse(entity);
                json!({"id":format!("entity:{}",entity.id), "targetname":entity.properties.get("targetname"),
                    "ready":result.is_ok(), "settings":result.as_ref().ok().copied().map(settings), "issue":result.err()})
            }).collect::<Vec<_>>();
        controllers.sort_by_key(|value| value["id"].as_str().unwrap().to_string());
        let summary = if let Some(issue) = &issue {
            issue.clone()
        } else if !enabled {
            "Fog preview is off".into()
        } else if fog.is_some_and(|fog| !fog.enabled()) {
            "Selected controller has fogenable=0; no fog is drawn".into()
        } else {
            "Static linear range fog · perspective Textured view".into()
        };
        json!({"snapshot":self.fog_preview_snapshot(), "enabled":enabled,
            "controller":controller.map(|id|format!("entity:{id}")), "ready":fog.is_some(),
            "active":enabled && issue.is_none() && fog.is_some_and(|fog|fog.enabled() && fog.max_density()>0.0),
            "settings":fog.map(settings), "issue":issue, "summary":summary, "controllers":controllers,
            "limits":"Static authored initial values only. View-depth ramp capped at max density; squared factor blends sRGB-decoded color in linear space. Perspective Textured surfaces/models only. Sky surfaces, background, helpers, selection and grid are unchanged. No radial/directional blend, far-Z clipping, HDR/zoom scaling, water fog, fog volumes, runtime I/O, transitions or automatic Master selection. Not a native parity claim.",
            "defaults":{"fogenable":0,"fogmaxdensity":1,"fogblend":0,"fogradial":0}})
    }

    pub fn set_fog_preview(
        &mut self,
        enabled: bool,
        controller: Option<u32>,
        snapshot: &str,
    ) -> Result<Value, String> {
        if snapshot != self.fog_preview_snapshot() {
            return Err("Map or view settings changed; refresh Fog preview before applying".into());
        }
        if enabled {
            let id = controller
                .ok_or("Choose an existing env_fog_controller; no controller is selected")?;
            self.fog_controller(id)?;
        }
        self.fog_preview = Selection {
            enabled,
            controller,
        };
        self.view_serial += 1;
        Ok(self.fog_preview_state())
    }

    pub fn fog_preview_for_render(
        &self,
        enabled: Option<bool>,
        controller: Option<u32>,
    ) -> Result<Option<Fog>, String> {
        let enabled = enabled.unwrap_or(self.fog_preview.enabled);
        let controller = controller.or(self.fog_preview.controller);
        if !enabled {
            return Ok(None);
        }
        let id =
            controller.ok_or("Choose an existing env_fog_controller; no controller is selected")?;
        self.fog_controller(id).map(Some)
    }

    pub fn fog_render_state(
        &self,
        enabled: Option<bool>,
        controller: Option<u32>,
    ) -> Result<(Option<Fog>, Value), String> {
        let fog = self.fog_preview_for_render(enabled, controller)?;
        let report = self.fog_preview_report(
            enabled.unwrap_or(self.fog_preview.enabled),
            controller.or(self.fog_preview.controller),
        );
        Ok((fog, report))
    }

    pub(super) fn current_fog(&self) -> Option<Fog> {
        self.fog_preview_for_render(None, None).ok().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::super::Actor;
    use super::*;
    use map_core::ObjectId;
    use std::collections::BTreeMap;

    fn controller(session: &mut Session, extra: &[(&str, &str)]) -> u32 {
        let mut properties: BTreeMap<_, _> = [
            ("fogcolor", "128 160 255"),
            ("fogstart", "100"),
            ("fogend", "500"),
            ("fogenable", "1"),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
        properties.extend(
            extra
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string())),
        );
        let ObjectId::Entity(id) = session
            .create_entity(Actor::Agent, "env_fog_controller", None, properties)
            .unwrap()
        else {
            panic!()
        };
        id
    }

    #[test]
    fn fog_selection_guards_errors_changes_and_history() {
        let mut session = Session::new(Err("no assets".into()));
        assert!(session.view().fog.is_none());
        let snapshot = session.fog_preview_snapshot();
        assert!(session.set_fog_preview(true, None, &snapshot).is_err());
        assert!(!session.fog_preview_state()["enabled"].as_bool().unwrap());
        let first = controller(&mut session, &[]);
        let bad = controller(&mut session, &[("fogblend", "1")]);
        let source = session.document.vmf().clone();
        let history = session.document.undo_labels().count();
        let stale = session.fog_preview_snapshot();
        session.set_fog_preview(true, Some(first), &stale).unwrap();
        assert_eq!(session.view().fog.unwrap().color(), [128, 160, 255]);
        assert!(session.set_fog_preview(false, Some(first), &stale).is_err());
        let snapshot = session.fog_preview_snapshot();
        assert!(
            session
                .set_fog_preview(true, Some(bad), &snapshot)
                .unwrap_err()
                .contains("fogblend")
        );
        assert_eq!(session.fog_preview.controller, Some(first));
        assert_eq!(session.document.vmf(), &source);
        assert_eq!(session.document.undo_labels().count(), history);
        session
            .set_entity_properties(
                Actor::Agent,
                first,
                &[("fogend".into(), Some("NaN".into()))],
            )
            .unwrap();
        assert!(session.view().fog.is_none());
        assert!(
            session.fog_preview_state()["issue"]
                .as_str()
                .unwrap()
                .contains("finite")
        );
        session.undo(Actor::Agent).unwrap();
        assert!(session.view().fog.is_some());
        session
            .edit(Actor::Agent, "delete fog controller", |tx| {
                map_core::edit::delete(tx, &[ObjectId::Entity(first)])
            })
            .unwrap();
        assert!(session.view().fog.is_none());
        assert!(
            session.fog_preview_state()["issue"]
                .as_str()
                .unwrap()
                .contains("no longer exists")
        );
        session.undo(Actor::Agent).unwrap();
        assert!(session.view().fog.is_some());
        session.new_map(Actor::Agent, true).unwrap();
        assert_eq!(session.fog_preview, Selection::default());
    }

    #[test]
    fn fog_parameters_reject_malformed_and_unsupported_without_fallback() {
        for (key, value) in [
            ("fogcolor", "1 2"),
            ("fogcolor", "256 0 0"),
            ("fogcolor", "1 2 3 4"),
            ("fogstart", "-1"),
            ("fogend", "100"),
            ("fogend", "inf"),
            ("fogmaxdensity", "1.1"),
            ("fogenable", "true"),
            ("fogblend", "1"),
            ("fogradial", "1"),
            ("farz", "1000"),
            ("HDRColorScale", "2"),
            ("foglerptime", "NaN"),
        ] {
            let mut session = Session::new(Err("no assets".into()));
            let id = controller(&mut session, &[(key, value)]);
            assert!(session.fog_controller(id).is_err(), "{key}={value}");
            assert!(session.view().fog.is_none());
            assert_eq!(
                session.fog_preview_state()["controllers"][0]["ready"],
                false
            );
        }
        let mut session = Session::new(Err("no assets".into()));
        let id = controller(&mut session, &[("fogenable", "0"), ("foglerptime", "2")]);
        let before = session.fog_preview_state();
        assert!(
            !session
                .fog_preview_for_render(Some(true), Some(id))
                .unwrap()
                .unwrap()
                .enabled()
        );
        assert_eq!(session.fog_preview_state(), before);
        let mut entity = Entity::new(100, "env_fog_controller");
        entity.properties.set("fogcolor", "0 128 255");
        entity.properties.set("fogstart", "0");
        entity.properties.set("fogend", "1024");
        let initial = parse(&entity).unwrap();
        assert!(!initial.enabled());
        assert_eq!(initial.max_density(), 1.0);
        entity.properties.0.push(("FOGEND".into(), "2048".into()));
        assert!(parse(&entity).unwrap_err().contains("duplicate"));
        let missing = Entity::new(101, "env_fog_controller");
        assert!(parse(&missing).unwrap_err().contains("missing"));
    }
}
