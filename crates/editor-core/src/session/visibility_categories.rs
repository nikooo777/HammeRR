use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::{Value, json};

use super::{
    Actor, Session,
    management::ObjectFilter,
    working_region::{Action, Options, options::MAX_EXCLUSIONS},
};

pub const MAX_PAGE: usize = 200;
pub const MAX_QUERY_BYTES: usize = 256;
pub const MAX_CLASS_BYTES: usize = 256;
pub const MAX_CLASSES: usize = 4096;

#[derive(Serialize)]
struct Category {
    id: String,
    label: String,
    filter: Option<ObjectFilter>,
    total: usize,
    excluded: bool,
    source: &'static str,
    available: bool,
    unavailable_reason: Option<String>,
    membership: Option<Value>,
}

impl Category {
    fn new(
        id: String,
        label: String,
        filter: ObjectFilter,
        total: usize,
        exclusions: &[ObjectFilter],
    ) -> Self {
        let excluded = exclusions.contains(&filter);
        Self {
            id,
            label,
            filter: Some(filter),
            total,
            excluded,
            source: "authored",
            available: true,
            unavailable_reason: None,
            membership: None,
        }
    }
}

impl Session {
    pub fn visibility_rules_snapshot(&self) -> String {
        format!(
            "visibility-rules:{}:{}:{}:{}:{}:{}",
            self.buffer_review_scope,
            self.document_generation,
            self.document.revision(),
            self.working_region.query_serial,
            self.definitions_serial(),
            self.definitions_status_serial()
        )
    }

    fn automatic_visibility_categories(&self) -> Result<Vec<Category>, String> {
        let vmf = self.document.vmf();
        let mut classes = BTreeMap::<String, usize>::new();
        let mut points = 0;
        let mut brushes = 0;
        for entity in &vmf.entities {
            if entity.solids.is_empty() {
                points += 1;
            } else {
                brushes += 1;
            }
            let classname = entity.classname().unwrap_or("");
            if classname.len() > MAX_CLASS_BYTES {
                return Err(format!(
                    "Automatic categories require classnames of at most {MAX_CLASS_BYTES} bytes"
                ));
            }
            *classes.entry(classname.to_ascii_lowercase()).or_default() += 1;
            if classes.len() > MAX_CLASSES {
                return Err(format!(
                    "Automatic categories support at most {MAX_CLASSES} distinct entity classes"
                ));
            }
        }
        let exclusions = &self.working_region.options.exclusions;
        let mut categories =
            Vec::with_capacity(classes.len() + 3 + self.definitions().auto_visgroups.len());
        for (kind, label, total) in [
            ("solid", "World solids", vmf.world.solids.len()),
            ("point_entity", "Point entities", points),
            ("brush_entity", "Brush entities", brushes),
        ] {
            if total == 0 {
                continue;
            }
            categories.push(Category::new(
                format!("kind:{kind}"),
                label.into(),
                ObjectFilter {
                    kind: Some(kind.into()),
                    ..ObjectFilter::default()
                },
                total,
                exclusions,
            ));
        }
        for (classname, &total) in &classes {
            let label = if classname.is_empty() {
                "Entities without classname".into()
            } else {
                classname.clone()
            };
            categories.push(Category::new(
                format!("class:{classname}"),
                label,
                ObjectFilter {
                    classname: Some(classname.clone()),
                    exact_class: true,
                    kind: Some("entity".into()),
                    ..ObjectFilter::default()
                },
                total,
                exclusions,
            ));
        }
        for group in &self.definitions().auto_visgroups {
            let classnames = group
                .classes
                .iter()
                .map(|name| name.to_ascii_lowercase())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let total = classnames
                .iter()
                .map(|name| classes.get(name).copied().unwrap_or_default())
                .sum();
            let matching_classes = classnames
                .iter()
                .filter(|name| classes.contains_key(*name))
                .count();
            let filter = ObjectFilter {
                classnames: Some(classnames.clone()),
                kind: Some("entity".into()),
                ..ObjectFilter::default()
            };
            let invalid = filter.validate().err();
            let unavailable_reason = if self.definitions_error().is_some() {
                Some("Entity definitions could not load; reload definitions before changing this FGD category".into())
            } else if self.definitions_serial() == 0 {
                Some("Load entity definitions before changing this FGD category".into())
            } else {
                invalid.clone()
            };
            let parent = group.parent.to_ascii_lowercase();
            let name = group.name.to_ascii_lowercase();
            categories.push(Category {
                id: format!("fgd:{}:{parent}:{name}", parent.len()),
                label: format!("FGD · {} / {}", group.parent, group.name),
                excluded: invalid.is_none() && exclusions.contains(&filter),
                filter: invalid.is_none().then_some(filter),
                total,
                source: "fgd",
                available: unavailable_reason.is_none(),
                unavailable_reason,
                membership: Some(json!({
                    "parent":group.parent,"name":group.name,"class_count":classnames.len(),
                    "matching_classes":matching_classes,
                    "sources":group.sources.iter().map(|source|json!({
                        "source":source.source.as_ref().map(|path|path.display().to_string()),
                        "line":source.line
                    })).collect::<Vec<_>>()
                })),
            });
        }
        Ok(categories)
    }

    pub fn visibility_categories(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Value, String> {
        if !(1..=MAX_PAGE).contains(&limit) {
            return Err(format!(
                "Visibility category page limit must be between 1 and {MAX_PAGE}"
            ));
        }
        if query.len() > MAX_QUERY_BYTES {
            return Err(format!(
                "Visibility category search must be at most {MAX_QUERY_BYTES} bytes"
            ));
        }
        let search = query.to_ascii_lowercase();
        let categories = self
            .automatic_visibility_categories()?
            .into_iter()
            .filter(|category| {
                category.id.contains(&search)
                    || category.label.to_ascii_lowercase().contains(&search)
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "snapshot":self.visibility_rules_snapshot(),
            "query":query,
            "total":categories.len(),
            "offset":offset,
            "limit":limit,
            "next_offset":(offset.saturating_add(limit)<categories.len()).then_some(offset.saturating_add(limit)),
            "categories":categories.into_iter().skip(offset).take(limit).collect::<Vec<_>>(),
            "exclusion_count":self.working_region.options.exclusions.len(),
            "fgd":{"available":self.definitions_serial()>0 && self.definitions_error().is_none(),
                "definitions_serial":self.definitions_serial(),"definitions_status_serial":self.definitions_status_serial(),
                "groups":self.definitions().auto_visgroups.len(),
                "unavailable_reason":self.definitions_error().map(str::to_owned).or_else(||(self.definitions_serial()==0).then(||"Load entity definitions to use FGD categories".into()))},
            "limits":{"page":MAX_PAGE,"query_bytes":MAX_QUERY_BYTES,"classname_bytes":MAX_CLASS_BYTES,"classes":MAX_CLASSES,"exclusions":MAX_EXCLUSIONS,
                "class_set_members":source_formats::fgd::MAX_AUTO_VIS_CLASSES},
            "policy":"Categories count committed authored root objects, including saved-hidden objects, by root kind, exact entity classname and FGD AutoVisGroup literal membership. FGD groups use one canonical exact-class-set exclusion each, including groups with no current matches. Class sets have no wildcard or inheritance expansion. Child brushes and expanded instance contents are not separate categories. The excluded flag means this exact category exclusion is present; other inclusion, exclusion, spatial and saved/editor visibility rules may still hide overlapping objects. Saved rules retain literal membership across FGD changes. Category changes preserve source, history and builds."
        }))
    }

    pub(super) fn check_visibility_rules_snapshot(&self, snapshot: &str) -> Result<(), String> {
        self.ensure_not_closing()?;
        if snapshot != self.visibility_rules_snapshot() {
            return Err(
                "Visibility rules, map or entity definitions changed; inspect visibility again"
                    .into(),
            );
        }
        Ok(())
    }

    fn configure_visibility_rules(
        &mut self,
        actor: Actor,
        options: Options,
    ) -> Result<Value, String> {
        let snapshot = self.working_region_stamp();
        self.set_working_region(actor, Action::Configure { options }, &snapshot)?;
        Ok(json!({
            "snapshot":self.visibility_rules_snapshot(),
            "working_region":self.working_region_summary()
        }))
    }

    pub fn set_visibility_category(
        &mut self,
        actor: Actor,
        id: &str,
        excluded: bool,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.check_visibility_rules_snapshot(snapshot)?;
        let category = self
            .automatic_visibility_categories()?
            .into_iter()
            .find(|category| category.id == id)
            .ok_or("Choose an existing automatic visibility category")?;
        if !category.available {
            return Err(category
                .unavailable_reason
                .unwrap_or_else(|| "This visibility category is unavailable".into()));
        }
        let filter = category
            .filter
            .ok_or("This visibility category has no usable literal class members")?;
        let mut options = self.working_region.options.clone();
        if excluded {
            if !options.exclusions.contains(&filter) {
                options.exclusions.push(filter);
            }
        } else {
            options.exclusions.retain(|entry| entry != &filter);
        }
        self.configure_visibility_rules(actor, options)
    }

    pub fn exclude_visibility_filter(
        &mut self,
        actor: Actor,
        filter: ObjectFilter,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.check_visibility_rules_snapshot(snapshot)?;
        filter.validate()?;
        let mut options = self.working_region.options.clone();
        options.filter = None;
        if !options.exclusions.contains(&filter) {
            options.exclusions.push(filter);
        }
        self.configure_visibility_rules(actor, options)
    }

    pub fn remove_visibility_exclusion(
        &mut self,
        actor: Actor,
        index: usize,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.check_visibility_rules_snapshot(snapshot)?;
        let mut options = self.working_region.options.clone();
        if index >= options.exclusions.len() {
            return Err("Choose an existing visibility exclusion".into());
        }
        options.exclusions.remove(index);
        self.configure_visibility_rules(actor, options)
    }
}
