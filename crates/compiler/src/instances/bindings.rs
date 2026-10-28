use super::*;

const MAX_BINDINGS: usize = 10;

#[derive(Clone, Copy)]
struct Binding<'a> {
    name: &'a str,
    value: &'a str,
}

pub(super) struct Bindings<'a> {
    values: [Option<Binding<'a>>; MAX_BINDINGS],
    len: usize,
}

fn replacement_key(key: &str) -> bool {
    key.get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("replace"))
}

fn parse_replacement<'a>(
    key: &str,
    value: &'a str,
    template: bool,
) -> Result<Option<Binding<'a>>, String> {
    if value.trim().is_empty() {
        return Ok(None);
    }
    let canonical = key.len() == 9
        && key.starts_with("replace")
        && matches!(
            &key[7..],
            "01" | "02" | "03" | "04" | "05" | "06" | "07" | "08" | "09" | "10"
        );
    if !canonical {
        return Err(
            "Nonempty instance bindings require canonical replace01 through replace10".into(),
        );
    }
    let (token, literal) = value.split_once(' ').ok_or(
        "Instance light binding needs a whole $name followed by an ASCII space and literal value",
    )?;
    let name = map_core::light::parameter_name(token)
        .ok_or("Instance light binding requires a whole $name with 1–64 ASCII identifier bytes")?;
    if !template && map_core::light::parameter_name(literal).is_some() {
        return Err("Unresolved instance light forwarding requires an immediately containing parent binding scope".into());
    }
    if literal.trim().is_empty()
        || (literal.contains('$')
            && !(template && map_core::light::parameter_name(literal).is_some()))
    {
        return Err("Instance light bindings require nonempty literals or an exact whole immediate-parent token in a nested helper; recursive and embedded replacements are unsupported".into());
    }
    field(value)?;
    field(literal)?;
    Ok(Some(Binding {
        name,
        value: literal,
    }))
}

pub(super) fn replacement(key: &str, value: &str, template: bool) -> Result<(), String> {
    parse_replacement(key, value, template).map(|_| ())
}

fn forwarding_name(value: &str) -> Option<&str> {
    let (_, operand) = value.split_once(' ')?;
    map_core::light::parameter_name(operand)
}

pub(super) fn has_forwarding(entity: &Entity) -> bool {
    is_instance(entity)
        && entity
            .properties
            .iter()
            .any(|(key, value)| replacement_key(key) && forwarding_name(value).is_some())
}

pub(super) fn has_parameters(entity: &Entity) -> bool {
    entity.properties.iter().any(|(key, value)| {
        map_core::light::value_parameter(entity.classname().unwrap_or(""), key, value).is_some()
    })
}

fn charge(work: &mut usize, amount: usize, control: &Control) -> Result<(), String> {
    control.checkpoint()?;
    *work = work
        .checked_add(amount)
        .filter(|value| *value <= MAX_GEOMETRY_WORK)
        .ok_or("Instance light bindings exceed the shared geometry-work limit")?;
    Ok(())
}

impl<'a> Bindings<'a> {
    pub fn new(helper: &'a Entity) -> Result<Self, String> {
        Self::read(helper, false)
    }

    pub fn validate_template(helper: &'a Entity) -> Result<(), String> {
        Self::read(helper, true).map(|_| ())
    }

    fn read(helper: &'a Entity, template: bool) -> Result<Self, String> {
        let mut result = Self {
            values: [None; MAX_BINDINGS],
            len: 0,
        };
        for (key, value) in helper
            .properties
            .iter()
            .filter(|(key, _)| replacement_key(key))
        {
            let Some(binding) = parse_replacement(key, value, template)? else {
                continue;
            };
            if result.get(binding.name).is_some() {
                return Err(format!("Instance repeats light binding ${}", binding.name));
            }
            if result.len == MAX_BINDINGS {
                return Err("Instance exceeds ten literal light bindings".into());
            }
            result.values[result.len] = Some(binding);
            result.len += 1;
        }
        Ok(result)
    }

    fn get(&self, name: &str) -> Option<(usize, Binding<'a>)> {
        self.values[..self.len]
            .iter()
            .enumerate()
            .find_map(|(index, binding)| {
                binding
                    .filter(|binding| binding.name.eq_ignore_ascii_case(name))
                    .map(|binding| (index, binding))
            })
    }

    fn unsupported(&self, value: &str) -> Result<(), String> {
        for suffix in value.split('$').skip(1) {
            let length = suffix
                .bytes()
                .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
                .count();
            if let Some((_, binding)) = self.get(&suffix[..length]) {
                return Err(format!(
                    "Instance light binding ${} appears outside an immediate supported whole light value field",
                    binding.name
                ));
            }
        }
        Ok(())
    }

    pub fn check_child(
        &self,
        child: &Vmf,
        source_bytes: usize,
        template: bool,
        work: &mut usize,
        control: &Control,
    ) -> Result<(), String> {
        if self.len == 0 && !template {
            return Ok(());
        }
        let scan = source_bytes
            .checked_mul(self.len + 2)
            .and_then(|amount| {
                self.values[..self.len]
                    .iter()
                    .try_fold(amount, |amount, binding| {
                        let binding = binding.as_ref()?;
                        amount
                            .checked_add(binding.name.len())?
                            .checked_add(binding.value.len())
                    })
            })
            .ok_or("Instance light binding scan-work overflow")?;
        charge(work, scan, control)?;
        let mut used = [false; MAX_BINDINGS];
        for (key, value) in child.world.properties.iter() {
            control.checkpoint()?;
            self.unsupported(key)?;
            self.unsupported(value)?;
        }
        for solid in child.world.solids.iter().filter(|solid| !solid.hidden) {
            self.check_materials(solid, control)?;
        }
        for entity in child.entities.iter().filter(|entity| !entity.hidden) {
            control.checkpoint()?;
            let nested = is_instance(entity);
            for (key, value) in entity.properties.iter() {
                self.unsupported(key)?;
                if let Some(name) =
                    map_core::light::value_parameter(entity.classname().unwrap_or(""), key, value)
                {
                    let (index, _) = self.get(name).ok_or_else(|| {
                        format!(
                            "Child light {} is missing immediate binding ${name}",
                            entity.id
                        )
                    })?;
                    used[index] = true;
                } else if nested && replacement_key(key) {
                    if let Some(name) = forwarding_name(value) {
                        let (index, _) = self.get(name).ok_or_else(|| {
                            format!("Child instance {} is missing immediate-parent light binding ${name}", entity.id)
                        })?;
                        used[index] = true;
                    }
                } else {
                    self.unsupported(value)?;
                }
            }
            for solid in entity.solids.iter().filter(|solid| !solid.hidden) {
                self.check_materials(solid, control)?;
            }
        }
        for (index, binding) in self.values[..self.len].iter().enumerate() {
            if !used[index] {
                return Err(format!(
                    "Instance light binding ${} is unused by direct visible child lights or forwarding helpers",
                    binding.unwrap().name
                ));
            }
        }
        Ok(())
    }

    fn check_materials(&self, solid: &Solid, control: &Control) -> Result<(), String> {
        for side in &solid.sides {
            control.checkpoint()?;
            self.unsupported(&side.material)?;
        }
        Ok(())
    }

    fn forwarded<'v>(
        &self,
        key: &str,
        value: &'v str,
    ) -> Result<Option<(&'v str, &'a str)>, String> {
        if !replacement_key(key) {
            return Ok(None);
        }
        let Some(name) = forwarding_name(value) else {
            return Ok(None);
        };
        let (_, binding) = self
            .get(name)
            .ok_or_else(|| format!("Missing immediate-parent light binding ${name}"))?;
        let (token, _) = value.split_once(' ').unwrap();
        token
            .len()
            .checked_add(1)
            .and_then(|bytes| bytes.checked_add(binding.value.len()))
            .filter(|bytes| *bytes <= MAX_FIELD_BYTES)
            .ok_or_else(|| {
                format!("Forwarded instance replacement exceeds {MAX_FIELD_BYTES} bytes")
            })?;
        Ok(Some((token, binding.value)))
    }

    pub fn resolve_helper<'e>(
        &self,
        original: &'e Entity,
        work: &mut usize,
        control: &Control,
    ) -> Result<Cow<'e, Entity>, String> {
        if !has_forwarding(original) {
            return Ok(Cow::Borrowed(original));
        }
        helper_template_admission(original)?;
        let mut bytes = 0_usize;
        for (key, value) in original.properties.iter() {
            control.checkpoint()?;
            let length = match self.forwarded(key, value)? {
                Some((token, literal)) => token.len() + 1 + literal.len(),
                None => value.len(),
            };
            bytes = bytes
                .checked_add(key.len())
                .and_then(|bytes| bytes.checked_add(length))
                .ok_or("Instance forwarded helper property-work overflow")?;
        }
        let amount = bytes
            .checked_mul(2)
            .ok_or("Instance forwarded helper copy-work overflow")?;
        charge(work, amount, control)?;
        let mut entity = Entity::new(original.id, "");
        entity.properties.0.clear();
        entity.properties.0.reserve(original.properties.0.len());
        for (key, value) in original.properties.iter() {
            control.checkpoint()?;
            let value = match self.forwarded(key, value)? {
                Some((token, literal)) => format!("{token} {literal}"),
                None => value.to_owned(),
            };
            entity.properties.0.push((key.to_owned(), value));
        }
        helper_admission(&entity)?;
        Ok(Cow::Owned(entity))
    }

    pub fn resolve<'e>(
        &self,
        original: &'e Entity,
        kind: entities::Kind,
        targets: &map_core::light::Targets<'_>,
        work: &mut usize,
        control: &Control,
    ) -> Result<Cow<'e, Entity>, String> {
        if !has_parameters(original) {
            return Ok(Cow::Borrowed(original));
        }
        let classname = original.classname().unwrap_or("");
        let bytes = original
            .properties
            .iter()
            .try_fold(0_usize, |bytes, (key, value)| {
                let value =
                    if let Some(name) = map_core::light::value_parameter(classname, key, value) {
                        self.get(name).map(|(_, binding)| binding.value)?
                    } else {
                        value
                    };
                bytes.checked_add(key.len())?.checked_add(value.len())
            })
            .ok_or(
                "Instance effective light property-work overflow or missing immediate binding",
            )?;
        let amount = bytes
            .checked_mul(2)
            .ok_or("Instance effective light copy-work overflow")?;
        charge(work, amount, control)?;
        let mut entity = Entity::new(original.id, "");
        entity.properties.0.clear();
        entity.properties.0.reserve(original.properties.0.len());
        for (key, value) in original.properties.iter() {
            control.checkpoint()?;
            let value = if let Some(name) = map_core::light::value_parameter(classname, key, value)
            {
                let (_, binding) = self.get(name).ok_or("Missing immediate light binding")?;
                binding.value
            } else {
                value
            };
            entity.properties.0.push((key.to_owned(), value.to_owned()));
        }
        entities::admit_with_targets(&entity, kind, targets)?;
        Ok(Cow::Owned(entity))
    }
}
