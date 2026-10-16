use serde::Serialize;

use super::Session;

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct MapSizeReport {
    pub authored: [i32; 2],
    pub bounds: [i32; 2],
    pub source: Option<String>,
    pub line: usize,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct DefinitionsReport {
    pub classes: usize,
    pub sources: Vec<String>,
    pub error: Option<String>,
    pub map_size: Option<MapSizeReport>,
}

impl DefinitionsReport {
    pub fn status(&self) -> String {
        let mut status = format!("Loaded FGD catalog: {} classes", self.classes);
        match &self.map_size {
            Some(size) => {
                status += &format!(
                    " · FGD map size: {} to {} (authored {}, {}) · {}:{} · Informational only",
                    size.bounds[0],
                    size.bounds[1],
                    size.authored[0],
                    size.authored[1],
                    size.source.as_deref().unwrap_or("Inline FGD"),
                    size.line,
                );
            }
            None => status += " · No effective @mapsize declaration",
        }
        if let Some(error) = &self.error {
            status += &format!(" · {error}");
        }
        status
    }
}

impl Session {
    pub fn entity_definitions_report(&self) -> DefinitionsReport {
        DefinitionsReport {
            classes: self.definitions.classes().count(),
            sources: self
                .definitions
                .sources
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
            error: self.definitions_error.clone(),
            map_size: self
                .definitions
                .map_size
                .as_ref()
                .map(|size| MapSizeReport {
                    authored: size.authored,
                    bounds: size.bounds,
                    source: size.source.as_ref().map(|path| path.display().to_string()),
                    line: size.line,
                }),
        }
    }
}
