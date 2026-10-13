use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use super::{
    Control, DirectedPortal, Work,
    budget::{Budget, Lease},
    flow, set,
};

pub(super) struct Graph<'a> {
    pub portals: &'a [DirectedPortal<'a>],
    pub edges: &'a [Vec<usize>],
    pub fronts: &'a [Vec<u8>],
    pub bounds: &'a [Vec<u8>],
    pub control: &'a Control,
}

struct Row {
    cluster: usize,
    bytes: Vec<u8>,
    fallbacks: usize,
}

impl Graph<'_> {
    pub fn refine(&self, cluster: usize, row: &mut [u8], work: &mut Work) -> Result<usize, String> {
        self.control.checkpoint()?;
        let mut fallbacks = 0;
        for &index in &self.edges[cluster] {
            flow::refine(
                &self.portals[index],
                self.portals,
                self.edges,
                &self.fronts[index],
                row,
                flow::Context {
                    work,
                    fallbacks: &mut fallbacks,
                    control: self.control,
                    possible: &self.bounds[index],
                    bounds: self.bounds,
                },
            )?;
        }
        Ok(fallbacks)
    }

    pub fn parallel(
        &self,
        output: &mut [Vec<u8>],
        workers: usize,
        remaining: Arc<Budget>,
    ) -> Result<usize, String> {
        let cursor = AtomicUsize::new(0);
        let width = output[0].len();
        let groups = std::thread::scope(|scope| {
            let handles = (0..workers)
                .map(|_| {
                    let remaining = remaining.clone();
                    let cursor = &cursor;
                    scope.spawn(move || self.worker(cursor, width, remaining))
                })
                .collect::<Vec<_>>();
            let mut groups = Vec::new();
            let mut error = None;
            for handle in handles {
                match handle
                    .join()
                    .unwrap_or_else(|_| Err("Visibility worker panicked".into()))
                {
                    Ok(rows) => groups.push(rows),
                    Err(message) => {
                        error.get_or_insert(message);
                    }
                }
            }
            error.map_or(Ok(groups), Err)
        })?;
        let mut fallbacks = 0;
        for group in groups {
            for row in group {
                output[row.cluster] = row.bytes;
                fallbacks += row.fallbacks;
            }
        }
        Ok(fallbacks)
    }

    fn worker(
        &self,
        cursor: &AtomicUsize,
        width: usize,
        remaining: Arc<Budget>,
    ) -> Result<Vec<Row>, String> {
        let mut work = Work::Shared(Lease::new(remaining));
        let mut rows = Vec::new();
        loop {
            self.control.checkpoint()?;
            let cluster = cursor.fetch_add(1, Ordering::Relaxed);
            if cluster >= self.edges.len() {
                break;
            }
            let mut bytes = vec![0; width];
            set(&mut bytes, cluster);
            let fallbacks = self.refine(cluster, &mut bytes, &mut work)?;
            rows.push(Row {
                cluster,
                bytes,
                fallbacks,
            });
        }
        Ok(rows)
    }
}
