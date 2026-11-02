use std::path::{Path, PathBuf};

use super::loading::Budget;

pub const MAX_AUTO_VIS_CLASSES: usize = 256;
pub const MAX_AUTO_VIS_GROUPS: usize = 256;
pub const MAX_AUTO_VIS_SOURCES: usize = 256;
pub const MAX_METADATA_NAME_BYTES: usize = 256;
pub const MAX_MATERIAL_EXCLUSIONS: usize = 1024;
pub const MAX_VERSION_DECLARATIONS: usize = 256;
pub const HAMMERPLUSPLUS_FGD_VERSION: i32 = 9;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetadataSource {
    pub source: Option<PathBuf>,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterialExclusion {
    pub prefix: String,
    pub source: Option<PathBuf>,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutoVisGroup {
    pub parent: String,
    pub name: String,
    pub classes: Vec<String>,
    pub sources: Vec<MetadataSource>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionDeclaration {
    pub value: i32,
    pub source: Option<PathBuf>,
    pub line: usize,
}

impl VersionDeclaration {
    pub fn diagnostic(&self) -> Option<&'static str> {
        match self.value.cmp(&HAMMERPLUSPLUS_FGD_VERSION) {
            std::cmp::Ordering::Less => Some(
                "Older than the verified Hammer++ FGD resource version 9; native Hammer++ reports an out-of-date resource. This does not select an FGD grammar or certify vendor compatibility.",
            ),
            std::cmp::Ordering::Greater => Some(
                "Newer than the verified Hammer++ FGD resource version 9; native Hammer++ warns about a newer resource. This does not select an FGD grammar or certify vendor compatibility.",
            ),
            std::cmp::Ordering::Equal => None,
        }
    }
}

#[derive(Default)]
pub(super) struct Metadata {
    pub(super) material_exclusions: Vec<MaterialExclusion>,
    pub(super) auto_visgroups: Vec<AutoVisGroup>,
    pub(super) versions: Vec<VersionDeclaration>,
}

impl Metadata {
    pub(super) fn exclusion(
        &mut self,
        mut exclusion: MaterialExclusion,
        source: Option<&Path>,
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        for previous in &self.material_exclusions {
            budget.work(previous.prefix.len() + exclusion.prefix.len() + 1)?;
            if previous.prefix.eq_ignore_ascii_case(&exclusion.prefix) {
                return Ok(());
            }
        }
        if self.material_exclusions.len() >= MAX_MATERIAL_EXCLUSIONS {
            return Err(format!(
                "FGD material exclusion limit of {MAX_MATERIAL_EXCLUSIONS} exceeded"
            ));
        }
        exclusion.source = copy_source(source, budget)?;
        self.material_exclusions.push(exclusion);
        Ok(())
    }

    pub(super) fn group(
        &mut self,
        mut group: AutoVisGroup,
        source: Option<&Path>,
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        group.sources[0].source = copy_source(source, budget)?;
        for previous in &mut self.auto_visgroups {
            budget.work(
                previous.parent.len()
                    + group.parent.len()
                    + previous.name.len()
                    + group.name.len()
                    + 1,
            )?;
            if previous.parent.eq_ignore_ascii_case(&group.parent)
                && previous.name.eq_ignore_ascii_case(&group.name)
            {
                if previous.sources.len().saturating_add(group.sources.len()) > MAX_AUTO_VIS_SOURCES
                {
                    return Err(format!(
                        "FGD auto-vis source declaration limit of {MAX_AUTO_VIS_SOURCES} exceeded"
                    ));
                }
                for name in group.classes {
                    append_class(&mut previous.classes, name, budget)?;
                }
                previous.sources.extend(group.sources);
                return Ok(());
            }
        }
        if self.auto_visgroups.len() >= MAX_AUTO_VIS_GROUPS {
            return Err(format!(
                "FGD auto-vis group limit of {MAX_AUTO_VIS_GROUPS} exceeded"
            ));
        }
        self.auto_visgroups.push(group);
        Ok(())
    }

    pub(super) fn version(
        &mut self,
        mut version: VersionDeclaration,
        source: Option<&Path>,
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        if self.versions.len() >= MAX_VERSION_DECLARATIONS {
            return Err(format!(
                "FGD version declaration limit of {MAX_VERSION_DECLARATIONS} exceeded"
            ));
        }
        version.source = copy_source(source, budget)?;
        self.versions.push(version);
        Ok(())
    }
}

pub(super) fn append_class(
    classes: &mut Vec<String>,
    name: String,
    budget: &mut Budget<'_>,
) -> Result<(), String> {
    for previous in classes.iter() {
        budget.work(previous.len() + name.len() + 1)?;
        if previous.eq_ignore_ascii_case(&name) {
            return Ok(());
        }
    }
    if classes.len() >= MAX_AUTO_VIS_CLASSES {
        return Err(format!(
            "FGD auto-vis class limit of {MAX_AUTO_VIS_CLASSES} exceeded"
        ));
    }
    classes.push(name);
    Ok(())
}

fn copy_source(source: Option<&Path>, budget: &mut Budget<'_>) -> Result<Option<PathBuf>, String> {
    if let Some(source) = source {
        budget.metadata(&source.to_string_lossy())?;
    }
    Ok(source.map(Path::to_path_buf))
}

pub(super) fn material_excluded(exclusions: &[MaterialExclusion], name: &str) -> bool {
    let Some((directory, _)) = name.rsplit_once('/') else {
        return false;
    };
    exclusions.iter().any(|exclusion| {
        !exclusion.prefix.is_empty()
            && directory
                .get(..exclusion.prefix.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(&exclusion.prefix))
    })
}
