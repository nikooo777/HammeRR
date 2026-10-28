use super::{Session, Thumbnail, ThumbnailStyle};
use crate::prefabs::RegistrySnapshot;
use serde_json::{Value, json};
use source_formats::vtf::RgbaImage;
use std::sync::{Arc, Weak};

pub const MAX_GALLERY_ENTRIES: usize = 8;

pub struct GalleryEntry {
    pub name: String,
    pub bytes: u64,
    pub thumbnail: Result<Thumbnail, String>,
}

pub struct Gallery {
    pub report: Value,
    pub entries: Vec<GalleryEntry>,
    pub review: GalleryReview,
}

pub struct GalleryReview {
    snapshot: RegistrySnapshot,
    query: String,
    offset: usize,
    limit: usize,
    style: ThumbnailStyle,
    epoch: (u64, u64),
    successful: Vec<(String, Weak<RgbaImage>)>,
}

impl Session {
    pub fn render_prefab_gallery(
        &mut self,
        query: &str,
        offset: usize,
        limit: usize,
        style: ThumbnailStyle,
        stamp: &str,
    ) -> Result<Gallery, String> {
        if !(1..=MAX_GALLERY_ENTRIES).contains(&limit) {
            return Err(format!(
                "Prefab gallery limit must be 1–{MAX_GALLERY_ENTRIES}"
            ));
        }
        let snapshot = self.checked_prefabs(stamp)?;
        let query = query.to_lowercase();
        let matching = snapshot
            .entries
            .iter()
            .filter(|entry| entry.name.to_lowercase().contains(&query))
            .collect::<Vec<_>>();
        let total = matching.len();
        let names = matching
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|entry| (entry.name.clone(), entry.bytes))
            .collect::<Vec<_>>();
        let entries = names
            .into_iter()
            .map(|(name, bytes)| {
                let thumbnail = self.prefab_thumbnail_with_style(&name, false, style);
                GalleryEntry {
                    name,
                    bytes,
                    thumbnail,
                }
            })
            .collect::<Vec<_>>();
        let review = GalleryReview {
            snapshot,
            query,
            offset,
            limit,
            style,
            epoch: (self.content_epoch, self.definitions_serial),
            successful: Vec::new(),
        };
        self.finish_prefab_gallery(entries, review, total, stamp)
    }

    fn finish_prefab_gallery(
        &self,
        mut entries: Vec<GalleryEntry>,
        mut review: GalleryReview,
        total: usize,
        stamp: &str,
    ) -> Result<Gallery, String> {
        let style = review.style;
        for entry in &mut entries {
            if let Some(cached) = entry
                .thumbnail
                .as_ref()
                .ok()
                .map(|thumbnail| thumbnail.report["cached"].clone())
            {
                let checked = self.cached_prefab_thumbnail_with_style(&entry.name,style)
                    .and_then(|thumbnail| {
                        thumbnail.ok_or_else(|| "Prefab source or thumbnail resources changed while rendering the page; render again".into())
                    });
                entry.thumbnail = checked.map(|mut thumbnail| {
                    thumbnail.report["cached"] = cached;
                    thumbnail
                });
            }
        }
        self.checked_prefabs(stamp)?;
        self.prefabs
            .binding
            .authority()
            .check(review.snapshot.revision, &review.snapshot.stamp)?;
        if review.epoch != (self.content_epoch, self.definitions_serial) {
            return Err("Prefab gallery content settings changed; render the page again".into());
        }
        review.successful = entries
            .iter()
            .filter_map(|entry| {
                entry
                    .thumbnail
                    .as_ref()
                    .ok()
                    .map(|thumbnail| (entry.name.clone(), Arc::downgrade(&thumbnail.image)))
            })
            .collect();
        let (query, offset, limit) = (&review.query, review.offset, review.limit);
        let report = json!({
            "query":query,"offset":offset,"limit":limit,"total":total,
            "next_offset":(offset.saturating_add(limit)<total).then_some(offset.saturating_add(limit)),
            "style":style.label(),"registry_revision":review.snapshot.revision,
            "max_images":MAX_GALLERY_ENTRIES,
            "entries":entries.iter().map(|entry| {
                let mut result = match &entry.thumbnail {
                    Ok(thumbnail)=>thumbnail.report.clone(),
                    Err(error)=>json!({"name":entry.name,"style":style.label(),"error":error}),
                };
                result["bytes"] = json!(entry.bytes);
                result
            }).collect::<Vec<_>>(),
        });
        Ok(Gallery {
            report,
            entries,
            review,
        })
    }

    pub fn validate_prefab_gallery(
        &self,
        review: &GalleryReview,
        query: &str,
        offset: usize,
        limit: usize,
        style: ThumbnailStyle,
    ) -> Result<(), String> {
        if review.query != query.to_lowercase()
            || review.offset != offset
            || review.limit != limit
            || review.style != style
            || review.epoch != (self.content_epoch, self.definitions_serial)
        {
            return Err(
                "Prefab gallery page or content settings changed; render the page again".into(),
            );
        }
        self.prefabs
            .binding
            .authority()
            .check(review.snapshot.revision, &review.snapshot.stamp)?;
        for (name, expected) in &review.successful {
            let current = self
                .cached_prefab_thumbnail_with_style(name, style)?
                .ok_or(
                    "Prefab gallery source or thumbnail resources changed; render the page again",
                )?;
            if !expected
                .upgrade()
                .is_some_and(|expected| Arc::ptr_eq(&current.image, &expected))
            {
                return Err("Prefab gallery image changed; render the page again".into());
            }
        }
        self.prefabs
            .binding
            .authority()
            .check(review.snapshot.revision, &review.snapshot.stamp)
    }
}
