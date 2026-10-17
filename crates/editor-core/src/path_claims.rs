use source_formats::file_publication::BoundDestination;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Source,
    Bsp,
    Pointfile,
}

struct Record {
    id: u64,
    owner: u64,
    kind: Kind,
    destination: BoundDestination,
}

#[derive(Default)]
struct State {
    serial: u64,
    records: Vec<Record>,
}

#[derive(Clone, Default)]
pub(crate) struct Authority(Arc<Mutex<State>>);

pub(crate) struct Binding {
    authority: Authority,
    owner: u64,
    pub source: Option<Claim>,
}

impl Default for Binding {
    fn default() -> Self {
        Self {
            authority: Authority::default(),
            owner: NEXT_OWNER.fetch_add(1, Ordering::Relaxed),
            source: None,
        }
    }
}

impl Binding {
    pub fn attach(&mut self, authority: Authority) {
        assert!(self.source.is_none());
        self.authority = authority;
    }

    pub fn source(&self, destination: BoundDestination) -> Result<Claim, String> {
        let mut claims = self
            .authority
            .reserve(self.owner, vec![(Kind::Source, destination)])?;
        Ok(claims.remove(0))
    }

    pub fn outputs(
        &self,
        bsp: BoundDestination,
        pointfile: BoundDestination,
    ) -> Result<OutputClaims, String> {
        let mut claims = self.authority.reserve(
            self.owner,
            vec![(Kind::Bsp, bsp), (Kind::Pointfile, pointfile)],
        )?;
        let pointfile = claims.pop().expect("pointfile reservation exists");
        let bsp = claims.pop().expect("BSP reservation exists");
        Ok(OutputClaims { bsp, pointfile })
    }
}

impl Authority {
    fn reserve(
        &self,
        owner: u64,
        destinations: Vec<(Kind, BoundDestination)>,
    ) -> Result<Vec<Claim>, String> {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        for (index, (kind, destination)) in destinations.iter().enumerate() {
            if destinations[..index]
                .iter()
                .any(|(_, other)| overlaps(destination, other))
            {
                return Err(format!(
                    "Output destinations alias each other: {}",
                    destination.authored_path().display()
                ));
            }
            for record in &state.records {
                if record.owner == owner && record.kind == *kind {
                    continue;
                }
                if overlaps(destination, &record.destination) {
                    return Err(format!(
                        "{} conflicts with an owned {}: {}",
                        destination.authored_path().display(),
                        if record.kind == Kind::Source {
                            "source map"
                        } else {
                            "build output"
                        },
                        record.destination.authored_path().display(),
                    ));
                }
            }
        }
        let mut claims = Vec::with_capacity(destinations.len());
        for (kind, destination) in destinations {
            state.serial += 1;
            let id = state.serial;
            state.records.push(Record {
                id,
                owner,
                kind,
                destination,
            });
            claims.push(Claim(Arc::new(Lease {
                authority: self.clone(),
                id,
            })));
        }
        Ok(claims)
    }
}

fn overlaps(a: &BoundDestination, b: &BoundDestination) -> bool {
    a.physical_path() == b.physical_path()
        || a.claim_slots()
            .iter()
            .any(|slot| b.claim_slots().contains(slot))
        || a.file_identities()
            .iter()
            .any(|identity| b.file_identities().contains(identity))
}

struct Lease {
    authority: Authority,
    id: u64,
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut state = self
            .authority
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.records.retain(|record| record.id != self.id);
    }
}

#[derive(Clone)]
pub(crate) struct Claim(Arc<Lease>);

impl Claim {
    pub fn destination(&self) -> BoundDestination {
        let state = self
            .0
            .authority
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state
            .records
            .iter()
            .find(|record| record.id == self.0.id)
            .expect("live claim has a retained destination")
            .destination
            .clone()
    }

    pub fn published(&self, destination: BoundDestination) {
        let mut state = self
            .0
            .authority
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state
            .records
            .iter_mut()
            .find(|record| record.id == self.0.id)
            .expect("live claim has a retained destination")
            .destination = destination;
    }
}

pub(crate) struct OutputClaims {
    pub bsp: Claim,
    pub pointfile: Claim,
}
