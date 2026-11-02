use super::{Bindings, Command, KeyInput, MAX_SEQUENCE_STROKES, Resolution};

impl Bindings {
    pub fn validated(mut self) -> Result<Self, String> {
        for command in Command::ALL {
            for binding in self.slots_mut(command).iter_mut().flatten() {
                binding.validate(command)?;
            }
        }
        let assigned = Command::ALL
            .into_iter()
            .flat_map(|command| {
                self.slots(command)
                    .iter()
                    .flatten()
                    .map(move |binding| (command, binding))
            })
            .collect::<Vec<_>>();
        for (index, (command, binding)) in assigned.iter().enumerate() {
            for (other_command, other) in assigned.iter().skip(index + 1) {
                let ignore_shift = command.is_camera() || other_command.is_camera();
                let prefix = binding
                    .strokes()
                    .iter()
                    .zip(other.strokes())
                    .all(|(a, b)| a.overlaps(b, ignore_shift));
                if prefix {
                    let conflict = if binding.strokes().len() == other.strokes().len() {
                        "is assigned more than once"
                    } else {
                        "conflicts with a sequence prefix"
                    };
                    return Err(format!(
                        "{} {conflict}: {} and {}",
                        binding.label(),
                        command.label(),
                        other_command.label()
                    ));
                }
            }
        }
        Ok(self)
    }

    pub fn resolve_sequence(&self, inputs: &[KeyInput]) -> Result<Resolution, String> {
        if !(1..=MAX_SEQUENCE_STROKES).contains(&inputs.len()) {
            return Err(format!(
                "Shortcut resolution requires one or {MAX_SEQUENCE_STROKES} strokes"
            ));
        }
        let inputs = inputs
            .iter()
            .map(KeyInput::validated)
            .collect::<Result<Vec<_>, _>>()?;
        if inputs.len() == MAX_SEQUENCE_STROKES && inputs[1].reserved_control().is_some() {
            self.resolve_inputs(&inputs[1..])?;
        }
        if inputs.len() == MAX_SEQUENCE_STROKES
            && self.resolve_inputs(&inputs[..1])? != Resolution::Prefix
        {
            return Ok(Resolution::None);
        }
        self.resolve_inputs(&inputs)
    }

    fn resolve_inputs(&self, inputs: &[KeyInput]) -> Result<Resolution, String> {
        let mut matches = Vec::new();
        for command in Command::ALL {
            for binding in self.slots(command).iter().flatten() {
                let strokes = binding.strokes();
                if inputs.len() <= strokes.len()
                    && strokes
                        .iter()
                        .zip(inputs)
                        .all(|(chord, input)| chord.matches(input, command.is_camera()))
                {
                    matches.push((command, binding));
                }
            }
        }
        let Some((first_command, first)) = matches.first() else {
            return Ok(Resolution::None);
        };
        if let Some(reserved) = inputs.last().and_then(KeyInput::reserved_control) {
            return Err(format!(
                "Shortcut input is ambiguous with reserved {reserved}; change overlapping bindings"
            ));
        }
        let complete = first.strokes().len() == inputs.len();
        if matches.iter().skip(1).any(|(_, binding)| {
            complete
                || binding.strokes().len() == inputs.len()
                || first
                    .strokes()
                    .iter()
                    .zip(binding.strokes())
                    .take(inputs.len())
                    .any(|(a, b)| a.physical != b.physical)
        }) {
            return Err(format!(
                "Shortcut input is ambiguous between {}; change overlapping bindings",
                matches
                    .iter()
                    .map(|(command, _)| command.label())
                    .collect::<Vec<_>>()
                    .join(" and ")
            ));
        }
        if complete {
            Ok(Resolution::Command(*first_command))
        } else {
            Ok(Resolution::Prefix)
        }
    }
}
