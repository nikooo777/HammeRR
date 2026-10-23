use super::*;

pub(super) fn component(name: &str) -> Result<(), String> {
    if !valid_component(name) {
        return Err(
            "Lighting origin name components require ASCII letters, digits or underscores".into(),
        );
    }
    Ok(())
}

fn valid_component(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

pub(super) fn charge(work: &mut usize, amount: usize, control: &Control) -> Result<(), String> {
    control.checkpoint()?;
    *work = work
        .checked_add(amount)
        .filter(|value| *value <= MAX_GEOMETRY_WORK)
        .ok_or("Instance names exceed the shared geometry-work limit")?;
    Ok(())
}

pub(super) fn check_point(entity: &Entity) -> Result<(), String> {
    if entity.properties.get("origin").is_none() {
        return Err("Child lighting origin requires canonical property origin".into());
    }
    crate::lighting_origins::point(entity)
        .map(|_| ())
        .map_err(|_| "Child lighting origin exceeds finite Source compile bounds".into())
}

pub(super) fn admit(entity: &Entity) -> Result<(), String> {
    if !entity.solids.is_empty()
        || !entity.extra.is_empty()
        || entity
            .connections
            .as_ref()
            .is_some_and(|pairs| !pairs.is_empty())
    {
        return Err(
            "Child lighting origins cannot have brushes, connections or opaque blocks".into(),
        );
    }
    let fields = [
        "classname",
        "origin",
        "targetname",
        "angles",
        "angle",
        "target",
        "parentname",
        "name",
        "globalname",
        "killtarget",
    ];
    let mut seen = 0_u16;
    for (key, value) in entity.properties.iter() {
        field(key)?;
        field(value)?;
        let index = fields
            .iter()
            .position(|field| field.eq_ignore_ascii_case(key))
            .ok_or_else(|| format!("Child lighting origin property {key} is unsupported"))?;
        if key != fields[index] {
            return Err(format!(
                "Child lighting origin requires canonical property {}",
                fields[index]
            ));
        }
        let bit = 1 << index;
        if seen & bit != 0 {
            return Err(format!("Child lighting origin repeats property {key}"));
        }
        seen |= bit;
        match key {
            "classname" | "origin" => {}
            "targetname" => component(value)?,
            "angles" => {
                vector(entity, "angles")?;
            }
            "angle" => {
                if !value.parse::<f64>().is_ok_and(f64::is_finite) {
                    return Err("Child lighting origin angle must be finite".into());
                }
            }
            _ if value.is_empty() => {}
            _ => return Err(format!("Child lighting origin requires empty {key}")),
        }
    }
    if entity.properties.get("classname") != Some("info_lighting") {
        return Err("Child lighting origin requires canonical classname info_lighting".into());
    }
    component(entity.properties.get("targetname").unwrap_or(""))?;
    check_point(entity)
}

pub(super) struct Scope {
    name: String,
    error: Option<&'static str>,
}

impl Scope {
    pub fn capture(entity: &Entity, work: &mut usize, control: &Control) -> Result<Self, String> {
        let name = entity.properties.get("targetname").unwrap_or("");
        let error = if entity.properties.iter().any(|(key, _)| {
            (key.eq_ignore_ascii_case("targetname") && key != "targetname")
                || (key.eq_ignore_ascii_case("fixup_style") && key != "fixup_style")
        }) {
            Some("Named instance branches require canonical targetname and fixup_style properties")
        } else if entity
            .properties
            .get("name")
            .is_some_and(|name| !name.is_empty())
        {
            Some("Named instance branches do not support a nonempty fallback name")
        } else if entity
            .properties
            .get("fixup_style")
            .is_some_and(|style| style.trim() != "0")
        {
            Some("Named instance branches require Prefix fixup_style 0 on every ancestor")
        } else if !valid_component(name) {
            Some("Named instance branches require an explicit ASCII Fix Up Name on every ancestor")
        } else {
            None
        };
        let name = if error.is_none() {
            charge(work, name.len(), control)?;
            name.to_owned()
        } else {
            String::new()
        };
        Ok(Self { name, error })
    }
}

pub(super) fn compose(
    scopes: &[Scope],
    local: &str,
    work: &mut usize,
    control: &Control,
) -> Result<String, String> {
    let mut length = local.len();
    for scope in scopes {
        if let Some(error) = scope.error {
            return Err(error.into());
        }
        length = length
            .checked_add(scope.name.len())
            .and_then(|value| value.checked_add(1))
            .ok_or("Scoped lighting origin name length overflow")?;
    }
    if length > MAX_LIGHTING_ORIGIN_NAME_BYTES {
        return Err(format!(
            "Scoped lighting origin name exceeds {MAX_LIGHTING_ORIGIN_NAME_BYTES} bytes"
        ));
    }
    charge(work, length, control)?;
    let mut name = String::with_capacity(length);
    for scope in scopes {
        name.push_str(&scope.name);
        name.push('-');
    }
    name.push_str(local);
    Ok(name)
}

#[derive(Default)]
pub(super) struct LocalIndex {
    pub helpers: BTreeMap<String, u32>,
}

impl LocalIndex {
    pub fn new(vmf: &Vmf, work: &mut usize, control: &Control) -> Result<Self, String> {
        let mut result = Self::default();
        let mut folded = BTreeSet::new();
        for helper in vmf.entities.iter().filter(|entity| !entity.hidden) {
            charge(work, 1, control)?;
            if helper.classname() != Some("info_lighting") {
                continue;
            }
            let name = helper.properties.get("targetname").unwrap();
            if result.helpers.len() >= MAX_OBJECTS {
                return Err("Instance lighting-origin index exceeds the object limit".into());
            }
            charge(
                work,
                name.len()
                    .checked_mul(2)
                    .ok_or("Instance name-work overflow")?,
                control,
            )?;
            if !folded.insert(name.to_ascii_lowercase()) {
                return Err(format!(
                    "Child lighting origin {} has a duplicate or case-ambiguous name: {name}",
                    helper.id
                ));
            }
            result.helpers.insert(name.to_owned(), helper.id);
        }
        Ok(result)
    }
}

pub(super) struct Registry {
    roots: BTreeSet<String>,
    target_roots: Option<BTreeSet<String>>,
    generated: BTreeSet<String>,
}

impl Registry {
    pub fn new(vmf: &Vmf, work: &mut usize, control: &Control) -> Result<Self, String> {
        let mut roots = BTreeSet::new();
        for helper in &vmf.entities {
            charge(work, 1, control)?;
            if helper.hidden
                || !helper
                    .classname()
                    .is_some_and(|class| class.eq_ignore_ascii_case("info_lighting"))
            {
                continue;
            }
            if let Some(name) = helper.properties.get("targetname") {
                charge(work, name.len(), control)?;
                roots.insert(name.to_ascii_lowercase());
            }
        }
        Ok(Self {
            roots,
            target_roots: None,
            generated: BTreeSet::new(),
        })
    }

    pub fn insert(
        &mut self,
        name: &str,
        work: &mut usize,
        control: &Control,
    ) -> Result<(), String> {
        charge(work, name.len(), control)?;
        let folded = name.to_ascii_lowercase();
        if self.roots.contains(&folded) {
            return Err(format!(
                "Scoped lighting origin name collides with a visible root helper: {name}"
            ));
        }
        if !self.generated.insert(folded) {
            return Err(format!(
                "Scoped lighting origin name collides with another generated helper: {name}"
            ));
        }
        Ok(())
    }

    pub fn insert_target(
        &mut self,
        name: &str,
        vmf: &Vmf,
        work: &mut usize,
        control: &Control,
    ) -> Result<(), String> {
        charge(work, name.len(), control)?;
        if self.generated.contains(&name.to_ascii_lowercase()) {
            return Err(format!(
                "Scoped spotlight target name collides with another generated helper: {name}"
            ));
        }
        if self.target_roots.is_none() {
            let mut roots = BTreeSet::new();
            for entity in vmf.entities.iter().filter(|entity| !entity.hidden) {
                charge(work, 1, control)?;
                for (_, name) in entity
                    .properties
                    .iter()
                    .filter(|(key, _)| key.eq_ignore_ascii_case("targetname"))
                {
                    charge(work, name.len(), control)?;
                    roots.insert(name.to_ascii_lowercase());
                }
            }
            self.target_roots = Some(roots);
        }
        if self
            .target_roots
            .as_ref()
            .unwrap()
            .contains(&name.to_ascii_lowercase())
        {
            return Err(format!(
                "Scoped spotlight target name collides with a visible root entity: {name}"
            ));
        }
        self.insert(name, work, control)
    }
}
