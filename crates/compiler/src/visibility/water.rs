use source_formats::bsp::{Visibility as ParsedVisibility, VisibilityKind, contents};

use super::*;

impl Visibility {
    pub(crate) fn water_fog_tests(
        &mut self,
        tree: &Tree,
        regions: &[i16],
        control: &Control,
    ) -> Result<Vec<bool>, String> {
        control.checkpoint()?;
        if regions.len() != tree.nodes.len() || self.clusters.len() != tree.nodes.len() {
            return Err("Water visibility disagrees with world leaves".into());
        }
        let mut work = Work::Local(
            MAX_VISIBILITY_WORK
                .checked_sub(self.work)
                .ok_or(WORK_LIMIT)?,
        );
        work.consume(
            tree.nodes
                .len()
                .checked_mul(2)
                .and_then(|work| work.checked_add(self.count))
                .ok_or(WORK_LIMIT)?,
        )?;
        let parsed = ParsedVisibility::parse(&self.data).map_err(|error| error.to_string())?;
        if parsed.clusters() != self.count {
            return Err("Water visibility disagrees with emitted clusters".into());
        }
        let mut sources = vec![false; self.count];
        for (index, node) in tree.nodes.iter().enumerate() {
            control.checkpoint()?;
            if regions[index] < 0 || node.contents & contents::SLIME != 0 {
                continue;
            }
            if let Some(cluster) = self.clusters[index] {
                sources[cluster] = true;
            }
        }
        let mut visible = vec![0; self.count.div_ceil(8)];
        for (cluster, source) in sources.into_iter().enumerate() {
            if !source {
                continue;
            }
            control.checkpoint()?;
            work.consume(visible.len().checked_mul(2).ok_or(WORK_LIMIT)?)?;
            let row = parsed
                .row(cluster, VisibilityKind::Visible)
                .map_err(|error| error.to_string())?;
            for (destination, source) in visible.iter_mut().zip(row) {
                *destination |= source;
            }
        }
        let mut tests = vec![false; tree.nodes.len()];
        for (index, node) in tree.nodes.iter().enumerate() {
            control.checkpoint()?;
            if regions[index] < 0
                && node.contents & (contents::SOLID | contents::WATER | contents::SLIME) == 0
            {
                tests[index] = self.clusters[index].is_some_and(|cluster| bit(&visible, cluster));
            }
        }
        self.work = MAX_VISIBILITY_WORK - work.remaining();
        Ok(tests)
    }
}
