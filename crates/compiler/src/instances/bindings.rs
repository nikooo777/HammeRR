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

fn parse_replacement<'a>(key: &str, value: &'a str) -> Result<Option<Binding<'a>>, String> {
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
    if literal.trim().is_empty() || literal.contains('$') {
        return Err("Instance light bindings require nonempty literal values; recursive bindings and ancestor forwarding are unsupported".into());
    }
    field(value)?;
    field(literal)?;
    Ok(Some(Binding {
        name,
        value: literal,
    }))
}

pub(super) fn replacement(key: &str, value: &str) -> Result<(), String> {
    parse_replacement(key, value).map(|_| ())
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
        let mut result = Self {
            values: [None; MAX_BINDINGS],
            len: 0,
        };
        for (key, value) in helper
            .properties
            .iter()
            .filter(|(key, _)| replacement_key(key))
        {
            let Some(binding) = parse_replacement(key, value)? else {
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
                } else if !(nested && replacement_key(key)) {
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
                    "Instance light binding ${} is unused by direct visible child lights",
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

#[cfg(test)]
mod tests {
    use super::*;

    fn helper(value: &str) -> Entity {
        let mut helper = Entity::new(100, "func_instance");
        helper.properties.set("file", "child.vmf");
        helper.properties.set("replace01", value);
        helper
    }

    fn light(value: &str) -> Entity {
        let mut light = Entity::new(20, "light");
        light.properties.set("origin", "0 0 96");
        light.properties.set("_light", value);
        light
    }

    #[test]
    fn syntax_keeps_empty_compatibility_fields_and_checks_canonical_literal_slots() {
        for key in ["replace", "replace11", "Replace01", "replace_custom"] {
            let mut entity = helper("");
            entity.properties.remove("replace01");
            entity.properties.set(key, " \t");
            helper_admission(&entity).unwrap();
            entity.properties.set(key, "$lamp 255 128 32 300");
            assert!(helper_admission(&entity).is_err(), "{key}");
        }
        for (key, value) in [
            ("replace10", "$lamp 255 128 32 300"),
            ("replace01", "$_Lamp2  255 128 32 300"),
        ] {
            let mut entity = helper("");
            entity.properties.set(key, value);
            helper_admission(&entity).unwrap();
        }
        for value in [
            "$lamp",
            "$lamp\t255 128 32 300",
            "$lamp $other",
            "$lamp 1 $other 3 4",
            "$lamp ",
            "$1lamp 255",
        ] {
            assert!(helper_admission(&helper(value)).is_err(), "{value:?}");
        }
        let mut duplicate = helper("$lamp 255 128 32 300");
        duplicate.properties.set("replace02", "$LAMP 1 2 3 4");
        assert!(
            helper_admission(&duplicate)
                .unwrap_err()
                .contains("repeats light binding")
        );
        let full = format!("$lamp {}", "1".repeat(MAX_FIELD_BYTES - "$lamp ".len()));
        helper_admission(&helper(&full)).unwrap();
        assert!(helper_admission(&helper(&(full + "1"))).is_err());
    }

    #[test]
    fn attenuation_binding_preserves_absent_optional_properties_and_original_template() {
        let mut original = light("255 128 32 300");
        original.properties.set("_constant_attn", "$constant");
        let before = original.clone();
        for value in ["0", "0.5"] {
            let owner = helper(&format!("$constant {value}"));
            let bindings = Bindings::new(&owner).unwrap();
            let mut work = 0;
            let effective = bindings
                .resolve(
                    &original,
                    entities::Kind::Point,
                    &map_core::light::Targets::new(),
                    &mut work,
                    &Control::default(),
                )
                .unwrap();
            assert_eq!(effective.properties.get("_constant_attn"), Some(value));
            for key in [
                "_linear_attn",
                "_quadratic_attn",
                "_distance",
                "_fifty_percent_distance",
                "_zero_percent_distance",
            ] {
                assert_eq!(effective.properties.get(key), None);
            }
            assert_eq!(original, before);
        }
    }

    #[test]
    fn literal_fast_path_is_borrowed_and_binding_work_is_reserved_before_scan_copy_and_validation()
    {
        let mut child = Document::empty().into_vmf();
        child.entities.push(light("255 128 32 300"));
        let empty = helper("");
        let literal = Bindings::new(&empty).unwrap();
        let mut work = usize::MAX;
        literal
            .check_child(&child, usize::MAX, false, &mut work, &Control::default())
            .unwrap();
        assert!(matches!(
            literal
                .resolve(
                    &child.entities[0],
                    entities::Kind::Point,
                    &map_core::light::Targets::new(),
                    &mut work,
                    &Control::default()
                )
                .unwrap(),
            Cow::Borrowed(_)
        ));
        assert_eq!(work, usize::MAX);

        let bound = helper("$lamp invalid numeric light");
        let bindings = Bindings::new(&bound).unwrap();
        child.entities[0].properties.set("_light", "$lamp");
        let before = child.clone();
        for exhausted in [MAX_GEOMETRY_WORK, usize::MAX] {
            let mut work = exhausted;
            let error = bindings
                .check_child(&child, 1, true, &mut work, &Control::default())
                .unwrap_err();
            assert!(error.contains("geometry-work"));
            assert_eq!(work, exhausted);
            let error = bindings
                .resolve(
                    &child.entities[0],
                    entities::Kind::Point,
                    &map_core::light::Targets::new(),
                    &mut work,
                    &Control::default(),
                )
                .unwrap_err();
            assert!(error.contains("geometry-work"));
            assert_eq!(work, exhausted);
        }
        let control = Control::default();
        control.cancel().unwrap();
        let mut work = 0;
        assert!(
            bindings
                .check_child(&child, 1, true, &mut work, &control)
                .unwrap_err()
                .contains("cancelled")
        );
        assert!(
            bindings
                .resolve(
                    &child.entities[0],
                    entities::Kind::Point,
                    &map_core::light::Targets::new(),
                    &mut work,
                    &control
                )
                .unwrap_err()
                .contains("cancelled")
        );
        assert_eq!(work, 0);
        assert_eq!(child, before);
    }
}
