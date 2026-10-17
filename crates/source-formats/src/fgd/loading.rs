use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::Path;

use super::{Choice, Class, Helper, Io, Property};

pub(super) const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;
const MAX_FILES: usize = 256;
const MAX_TOKENS: usize = 2_000_000;
const MAX_CLASSES: usize = 16_384;
pub(super) const MAX_FIELD_BYTES: usize = 1024 * 1024;
const MAX_MEMBERS: usize = 4_000_000;
const MAX_COPIED_BYTES: usize = 512 * 1024 * 1024;
const MAX_WORK: usize = 512 * 1024 * 1024;
const READ_BYTES: usize = 64 * 1024;

pub(super) struct Budget<'a> {
    check: &'a mut dyn FnMut() -> Result<(), String>,
    files: usize,
    bytes: usize,
    tokens: usize,
    classes: usize,
    members: usize,
    copied: usize,
    work: usize,
    next_scan: usize,
}

fn charge(value: &mut usize, amount: usize, maximum: usize, label: &str) -> Result<(), String> {
    *value = value
        .checked_add(amount)
        .filter(|value| *value <= maximum)
        .ok_or_else(|| format!("FGD {label} limit exceeded ({maximum})"))?;
    Ok(())
}

impl<'a> Budget<'a> {
    pub(super) fn new(check: &'a mut dyn FnMut() -> Result<(), String>) -> Self {
        Self {
            check,
            files: 0,
            bytes: 0,
            tokens: 0,
            classes: 0,
            members: 0,
            copied: 0,
            work: 0,
            next_scan: 0,
        }
    }

    pub(super) fn poll(&mut self) -> Result<(), String> {
        (self.check)()
    }

    pub(super) fn scan(&mut self, offset: usize) -> Result<(), String> {
        if offset >= self.next_scan {
            self.poll()?;
            self.next_scan = offset.saturating_add(4096);
        }
        Ok(())
    }

    pub(super) fn start_scan(&mut self) -> Result<(), String> {
        self.next_scan = 0;
        self.poll()
    }

    pub(super) fn token(&mut self) -> Result<(), String> {
        self.poll()?;
        charge(&mut self.tokens, 1, MAX_TOKENS, "token count")
    }

    pub(super) fn class(&mut self) -> Result<(), String> {
        self.poll()?;
        charge(&mut self.classes, 1, MAX_CLASSES, "class declaration count")
    }

    pub(super) fn work(&mut self, amount: usize) -> Result<(), String> {
        self.poll()?;
        charge(&mut self.work, amount, MAX_WORK, "inheritance work")
    }

    fn member(&mut self) -> Result<(), String> {
        self.poll()?;
        charge(&mut self.members, 1, MAX_MEMBERS, "copied member count")
    }

    fn text(&mut self, text: &str) -> Result<(), String> {
        charge(
            &mut self.copied,
            text.len(),
            MAX_COPIED_BYTES,
            "copied definition bytes",
        )
    }

    pub(super) fn property(&mut self, property: &Property) -> Result<(), String> {
        self.member()?;
        for text in [
            &property.name,
            &property.kind,
            &property.label,
            &property.description,
        ] {
            self.text(text)?;
        }
        if let Some(default) = &property.default {
            self.text(default)?;
        }
        for choice in &property.choices {
            self.choice(choice)?;
        }
        Ok(())
    }

    pub(super) fn choice(&mut self, choice: &Choice) -> Result<(), String> {
        self.member()?;
        self.text(&choice.value)?;
        self.text(&choice.label)
    }

    pub(super) fn io(&mut self, io: &Io) -> Result<(), String> {
        self.member()?;
        for text in [&io.name, &io.kind, &io.description] {
            self.text(text)?;
        }
        Ok(())
    }

    pub(super) fn helper(&mut self, helper: &Helper) -> Result<(), String> {
        self.member()?;
        self.text(&helper.name)?;
        for argument in &helper.arguments {
            self.member()?;
            self.text(argument)?;
        }
        Ok(())
    }

    pub(super) fn copy_class(&mut self, class: &Class) -> Result<(), String> {
        self.member()?;
        self.text(&class.name)?;
        self.text(&class.description)?;
        for base in &class.bases {
            self.member()?;
            self.text(base)?;
        }
        for property in &class.properties {
            self.property(property)?;
        }
        for io in class.inputs.iter().chain(&class.outputs) {
            self.io(io)?;
        }
        for helper in &class.helpers {
            self.helper(helper)?;
        }
        Ok(())
    }

    pub(super) fn read(&mut self, path: &Path) -> Result<Vec<u8>, String> {
        self.poll()?;
        charge(&mut self.files, 1, MAX_FILES, "source file count")?;
        let before = std::fs::metadata(path).map_err(|error| error.to_string())?;
        if !before.is_file() || before.len() > MAX_FILE_BYTES as u64 {
            return Err(format!(
                "FGD source must be a regular file of at most {MAX_FILE_BYTES} bytes"
            ));
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
        }
        let mut file: File = options.open(path).map_err(|error| error.to_string())?;
        let opened = file.metadata().map_err(|error| error.to_string())?;
        if !opened.is_file()
            || opened.len() != before.len()
            || opened.modified().ok() != before.modified().ok()
        {
            return Err("FGD source changed while opening".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if (before.dev(), before.ino()) != (opened.dev(), opened.ino()) {
                return Err("FGD source was replaced while opening".into());
            }
        }
        let mut bytes = Vec::new();
        let mut buffer = [0u8; READ_BYTES];
        loop {
            self.poll()?;
            let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
            if count == 0 {
                break;
            }
            if bytes.len().saturating_add(count) > MAX_FILE_BYTES
                || bytes.len().saturating_add(count) > opened.len() as usize
            {
                return Err("FGD source grew while reading".into());
            }
            charge(
                &mut self.bytes,
                count,
                MAX_TOTAL_BYTES,
                "aggregate source bytes",
            )?;
            bytes.extend_from_slice(&buffer[..count]);
        }
        let after = file.metadata().map_err(|error| error.to_string())?;
        if bytes.len() as u64 != opened.len()
            || after.len() != opened.len()
            || after.modified().ok() != opened.modified().ok()
        {
            return Err("FGD source changed while reading".into());
        }
        self.poll()?;
        Ok(bytes)
    }
}

pub(super) fn helper_bytes(helper: &Helper) -> usize {
    helper.name.len()
        + helper
            .arguments
            .iter()
            .map(|argument| argument.len() + 1)
            .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fgd::{Catalog, Parser};

    #[test]
    fn source_token_declaration_and_inheritance_budgets_reject_at_the_boundary() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("small.fgd");
        std::fs::write(&path, "ab").unwrap();
        let mut check = || Ok(());
        let mut budget = Budget::new(&mut check);
        budget.files = MAX_FILES;
        assert!(
            budget
                .read(&path)
                .unwrap_err()
                .contains("source file count")
        );
        budget.files = 0;
        budget.bytes = MAX_TOTAL_BYTES - 1;
        assert!(
            budget
                .read(&path)
                .unwrap_err()
                .contains("aggregate source bytes")
        );
        budget.tokens = MAX_TOKENS;
        assert!(
            Parser::new("@PointClass = thing []", &mut budget)
                .err()
                .unwrap()
                .contains("token count")
        );
        budget.tokens = 0;
        budget.classes = MAX_CLASSES;
        assert!(
            Parser::new("@PointClass = thing []", &mut budget)
                .unwrap()
                .parse()
                .err()
                .unwrap()
                .contains("class declaration")
        );
        let catalog = Catalog::parse("@PointClass = thing [ p(string) ]").unwrap();
        budget.members = MAX_MEMBERS;
        assert!(
            budget
                .copy_class(catalog.class("thing").unwrap())
                .unwrap_err()
                .contains("member count")
        );
        budget.members = 0;
        budget.copied = MAX_COPIED_BYTES;
        assert!(
            budget
                .copy_class(catalog.class("thing").unwrap())
                .unwrap_err()
                .contains("definition bytes")
        );
        budget.work = MAX_WORK;
        let mut items = vec!["existing".to_owned()];
        assert!(
            crate::fgd::upsert(
                &mut items,
                "replacement".into(),
                String::as_str,
                &mut budget
            )
            .unwrap_err()
            .contains("inheritance work")
        );
        assert_eq!(items, ["existing"]);
    }

    #[test]
    fn escaped_quotes_obey_the_field_limit_before_token_materialization() {
        let input = format!(
            "@PointClass = thing : \"{}\" []",
            "\\\"".repeat(MAX_FIELD_BYTES + 1)
        );
        assert!(
            Catalog::parse(&input)
                .unwrap_err()
                .contains("field byte limit")
        );
    }
}
