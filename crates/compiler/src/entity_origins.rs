use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Origin {
    Root {
        entity: u32,
    },
    Instance {
        instance: u32,
        file: String,
        entity: u32,
        expanded_entity: u32,
        #[serde(serialize_with = "crate::instances::serialize_occurrence")]
        occurrence: Arc<crate::instances::OccurrencePath>,
    },
}

impl Origin {
    pub(crate) fn capture(
        entity: u32,
        instances: &crate::instances::Prepared<'_>,
        bytes: &mut usize,
        limit: usize,
    ) -> Result<Self, String> {
        let Some((instance, file, original)) = instances.entity_origin(entity) else {
            return Ok(Self::Root { entity });
        };
        let occurrence = instances
            .entity_occurrence(entity)
            .ok_or("Expanded entity source has no retained occurrence path")?;
        *bytes = bytes
            .checked_add(file.len())
            .filter(|bytes| *bytes <= limit)
            .ok_or("Captured entity origins exceed their storage budget")?;
        let mut path = String::new();
        path.try_reserve_exact(file.len())
            .map_err(|_| "Captured entity source path cannot be allocated")?;
        path.push_str(file);
        Ok(Self::Instance {
            instance,
            file: path,
            entity: original,
            expanded_entity: entity,
            occurrence: Arc::clone(occurrence),
        })
    }

    pub fn source_bytes(&self) -> usize {
        match self {
            Self::Root { .. } => 0,
            Self::Instance {
                file, occurrence, ..
            } => file.len().saturating_add(occurrence.response_bytes()),
        }
    }

    pub fn response_copy_bytes(&self) -> usize {
        match self {
            Self::Root { .. } => 1024,
            Self::Instance {
                file, occurrence, ..
            } => file
                .len()
                .saturating_mul(8)
                .saturating_add(occurrence.response_copy_bytes())
                .saturating_add(1024),
        }
    }

    pub fn try_clone(&self) -> Result<Self, String> {
        match self {
            Self::Root { entity } => Ok(Self::Root { entity: *entity }),
            Self::Instance {
                instance,
                file,
                entity,
                expanded_entity,
                occurrence,
            } => {
                let mut path = String::new();
                path.try_reserve_exact(file.len())
                    .map_err(|_| "Captured entity source path cannot be allocated")?;
                path.push_str(file);
                Ok(Self::Instance {
                    instance: *instance,
                    file: path,
                    entity: *entity,
                    expanded_entity: *expanded_entity,
                    occurrence: Arc::clone(occurrence),
                })
            }
        }
    }

    pub fn label(&self, compact: bool) -> String {
        match self {
            Self::Root { entity } => {
                if compact {
                    format!("entity:{entity}")
                } else {
                    format!("root entity:{entity}")
                }
            }
            Self::Instance {
                instance,
                file,
                entity,
                expanded_entity,
                occurrence,
            } => {
                if compact {
                    format!("instance:{instance} / entity:{entity}")
                } else {
                    format!(
                        "outer helper entity:{instance}\nOccurrence: {}\nLeaf file: {file}\nOriginal entity:{entity} · expanded entity:{expanded_entity} (this build)",
                        occurrence.label()
                    )
                }
            }
        }
    }

    pub fn route_label(&self) -> String {
        match self {
            Self::Root { .. } => self.label(true),
            Self::Instance {
                entity, occurrence, ..
            } => {
                format!("{} / entity:{entity}", occurrence.label())
            }
        }
    }
}
