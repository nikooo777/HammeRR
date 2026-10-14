use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use super::{Control, DirectedPortal, VISIBILITY_EPSILON, Work, budget, set};

pub(super) struct Graph<'a> {
    pub portals: &'a [DirectedPortal<'a>],
    pub edges: &'a [Vec<usize>],
    pub control: &'a Control,
    pub detailed: bool,
}

pub(super) struct Row {
    pub source: usize,
    pub clusters: Vec<u8>,
    pub fronts: Option<Vec<u8>>,
}

struct Scratch {
    eligible: Vec<bool>,
    reached: Vec<bool>,
    pending: Vec<usize>,
}

impl Graph<'_> {
    fn scratch(&self) -> Scratch {
        Scratch {
            eligible: vec![false; self.portals.len()],
            reached: vec![false; self.edges.len()],
            pending: Vec::new(),
        }
    }

    fn row(&self, index: usize, scratch: &mut Scratch, work: &mut Work) -> Result<Row, String> {
        self.control.checkpoint()?;
        let source = &self.portals[index];
        let mut fronts = self
            .detailed
            .then(|| vec![0; self.portals.len().div_ceil(8)]);
        for (index, target) in self.portals.iter().enumerate() {
            self.control.checkpoint()?;
            work.consume(source.winding.0.len() + target.winding.0.len())?;
            let eligible = target
                .winding
                .0
                .iter()
                .any(|&point| source.plane.distance_to(point) >= -VISIBILITY_EPSILON)
                && source
                    .winding
                    .0
                    .iter()
                    .any(|&point| target.plane.distance_to(point) <= VISIBILITY_EPSILON);
            scratch.eligible[index] = eligible;
            if eligible && let Some(fronts) = &mut fronts {
                set(fronts, index);
            }
        }
        let mut clusters = vec![0; self.edges.len().div_ceil(8)];
        scratch.reached.fill(false);
        scratch.reached[source.to] = true;
        scratch.pending.push(source.to);
        while let Some(cluster) = scratch.pending.pop() {
            self.control.checkpoint()?;
            set(&mut clusters, cluster);
            work.consume(self.edges[cluster].len())?;
            for &index in &self.edges[cluster] {
                let target = &self.portals[index];
                if scratch.eligible[index] && !scratch.reached[target.to] {
                    scratch.reached[target.to] = true;
                    scratch.pending.push(target.to);
                }
            }
        }
        Ok(Row {
            source: index,
            clusters,
            fronts,
        })
    }

    pub fn build(&self, workers: usize, work: &mut Work) -> Result<Vec<Row>, String> {
        if workers == 1 {
            let mut scratch = self.scratch();
            return (0..self.portals.len())
                .map(|index| self.row(index, &mut scratch, work))
                .collect();
        }
        let remaining = Arc::new(budget::Budget::new(work.remaining(), self.control.clone()));
        *work = Work::Shared(budget::Lease::new(remaining.clone()));
        let cursor = AtomicUsize::new(0);
        let mut rows = std::thread::scope(|scope| {
            let handles = (0..workers)
                .map(|_| {
                    let remaining = remaining.clone();
                    let cursor = &cursor;
                    scope.spawn(move || self.worker(cursor, remaining))
                })
                .collect::<Vec<_>>();
            let mut rows = Vec::with_capacity(self.portals.len());
            let mut error = None;
            for handle in handles {
                match handle
                    .join()
                    .unwrap_or_else(|_| Err("Base visibility worker panicked".into()))
                {
                    Ok(group) => rows.extend(group),
                    Err(message) => {
                        error.get_or_insert(message);
                    }
                }
            }
            error.map_or(Ok(rows), Err)
        })?;
        rows.sort_unstable_by_key(|row| row.source);
        Ok(rows)
    }

    fn worker(
        &self,
        cursor: &AtomicUsize,
        remaining: Arc<budget::Budget>,
    ) -> Result<Vec<Row>, String> {
        let mut work = Work::Shared(budget::Lease::new(remaining));
        let mut scratch = self.scratch();
        let mut rows = Vec::new();
        loop {
            self.control.checkpoint()?;
            let index = cursor.fetch_add(1, Ordering::Relaxed);
            if index >= self.portals.len() {
                break;
            }
            rows.push(self.row(index, &mut scratch, &mut work)?);
        }
        Ok(rows)
    }
}
