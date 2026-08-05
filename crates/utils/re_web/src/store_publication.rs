//! Web-only, generation-aware identities for future Store publication.
//!
//! This module defines identity and allocation primitives only.
//! It does not change `StoreId` lookup, Store publication, subscribers, queries, or swap behavior.
//!
//! Raw integers cannot be promoted into a [`StoreGeneration`]:
//!
//! ```compile_fail
//! use re_web::store_publication::StoreGeneration;
//! let _generation = StoreGeneration::new(1);
//! ```
//!
//! The allocator core and its seed are not part of the production API:
//!
//! ```compile_fail
//! use re_web::store_publication::StoreGenerationAllocatorCore;
//! ```

use std::fmt;
use std::hash::Hash;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};

use re_log_types::StoreId;

/// One nonzero, Wasm-module-lifetime Store publication generation.
///
/// Production code can only obtain this value from [`allocate_store_publication_identity`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StoreGeneration(NonZeroU64);

impl StoreGeneration {
    /// Returns the generation as a nonzero integer.
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Debug for StoreGeneration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("StoreGeneration(<opaque>)")
    }
}

/// The indivisible identity of one published generation of a Store.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StorePublicationIdentity {
    store_id: StoreId,
    generation: StoreGeneration,
}

impl StorePublicationIdentity {
    /// Returns the existing logical Store identity.
    pub fn store_id(&self) -> &StoreId {
        &self.store_id
    }

    /// Returns this publication's never-reused generation.
    pub const fn generation(&self) -> StoreGeneration {
        self.generation
    }

    /// Creates a fixed target that retains the complete publication identity.
    pub fn fixed_target(&self) -> StoreGenerationId {
        StoreGenerationId {
            store_id: self.store_id.clone(),
            generation: self.generation,
        }
    }
}

impl fmt::Debug for StorePublicationIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("StorePublicationIdentity(<opaque>)")
    }
}

/// A fixed command/result target that cannot fall back to a bare [`StoreId`].
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StoreGenerationId {
    store_id: StoreId,
    generation: StoreGeneration,
}

impl StoreGenerationId {
    /// Returns the existing logical Store identity.
    pub fn store_id(&self) -> &StoreId {
        &self.store_id
    }

    /// Returns the exact publication generation.
    pub const fn generation(&self) -> StoreGeneration {
        self.generation
    }

    /// Checks the complete Store and generation identity.
    pub fn matches_publication(&self, publication: &StorePublicationIdentity) -> bool {
        self.store_id == publication.store_id && self.generation == publication.generation
    }
}

impl fmt::Debug for StoreGenerationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("StoreGenerationId(<opaque>)")
    }
}

/// Failure to allocate a fresh Store publication generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreGenerationAllocationError {
    /// Every nonzero `u64` generation has already been allocated.
    Exhausted,
}

impl fmt::Display for StoreGenerationAllocationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Store publication generations are exhausted")
    }
}

impl std::error::Error for StoreGenerationAllocationError {}

/// Allocates the only production source of Store publication generations.
///
/// The allocator belongs to the Wasm module, so Viewer close, restart, and Store removal do not
/// reset or refund its sequence.
pub fn allocate_store_publication_identity(
    store_id: StoreId,
) -> Result<StorePublicationIdentity, StoreGenerationAllocationError> {
    STORE_GENERATION_ALLOCATOR.allocate_publication(store_id)
}

struct StoreGenerationAllocatorCore {
    /// The next generation, with zero permanently representing exhaustion.
    next: AtomicU64,
}

impl StoreGenerationAllocatorCore {
    const fn module_lifetime() -> Self {
        Self {
            next: AtomicU64::new(1),
        }
    }

    #[cfg(test)]
    const fn seeded_for_test(next: NonZeroU64) -> Self {
        Self {
            next: AtomicU64::new(next.get()),
        }
    }

    fn allocate_generation(&self) -> Result<StoreGeneration, StoreGenerationAllocationError> {
        let mut current = self.next.load(Ordering::Relaxed);
        loop {
            let Some(generation) = NonZeroU64::new(current) else {
                return Err(StoreGenerationAllocationError::Exhausted);
            };
            let next = current.checked_add(1).unwrap_or(0);
            match self.next.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Ok(StoreGeneration(generation)),
                Err(observed) => current = observed,
            }
        }
    }

    fn allocate_publication(
        &self,
        store_id: StoreId,
    ) -> Result<StorePublicationIdentity, StoreGenerationAllocationError> {
        let generation = self.allocate_generation()?;
        Ok(StorePublicationIdentity {
            store_id,
            generation,
        })
    }
}

static STORE_GENERATION_ALLOCATOR: StoreGenerationAllocatorCore =
    StoreGenerationAllocatorCore::module_lifetime();

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;
    use std::thread;

    use super::*;

    fn seed(value: u64) -> NonZeroU64 {
        NonZeroU64::new(value).unwrap()
    }

    fn store(recording_id: &str) -> StoreId {
        StoreId::recording("store-publication-test", recording_id)
    }

    #[test]
    fn sequential_allocations_are_nonzero_unique_and_monotonic() {
        let allocator = StoreGenerationAllocatorCore::seeded_for_test(seed(1));
        let generations = (0..4)
            .map(|_| allocator.allocate_generation().unwrap().get())
            .collect::<Vec<_>>();
        assert_eq!(generations, [1, 2, 3, 4]);
    }

    #[test]
    fn concurrent_allocations_are_globally_unique() {
        const THREADS: usize = 8;
        const PER_THREAD: usize = 256;

        let allocator = Arc::new(StoreGenerationAllocatorCore::seeded_for_test(seed(1)));
        let workers = (0..THREADS)
            .map(|worker_index| {
                let allocator = Arc::clone(&allocator);
                thread::Builder::new()
                    .name(format!("store-generation-{worker_index}"))
                    .spawn(move || {
                        (0..PER_THREAD)
                            .map(|_| allocator.allocate_generation().unwrap().get())
                            .collect::<Vec<_>>()
                    })
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let generations = workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap())
            .collect::<BTreeSet<_>>();

        assert_eq!(generations.len(), THREADS * PER_THREAD);
        assert_eq!(generations.first(), Some(&1));
        assert_eq!(
            generations.last(),
            Some(&u64::try_from(THREADS * PER_THREAD).unwrap())
        );
    }

    #[test]
    fn same_store_reopen_gets_a_fresh_complete_identity() {
        let allocator = StoreGenerationAllocatorCore::seeded_for_test(seed(7));
        let store_id = store("same-store");
        let old = allocator.allocate_publication(store_id.clone()).unwrap();
        let reopened = allocator.allocate_publication(store_id).unwrap();

        assert_eq!(old.store_id(), reopened.store_id());
        assert_ne!(old.generation(), reopened.generation());
        assert_ne!(old, reopened);
        assert!(!old.fixed_target().matches_publication(&reopened));
    }

    #[test]
    fn simulated_viewer_restart_does_not_reset_the_allocator() {
        fn viewer_session(store_id: &StoreId) -> StorePublicationIdentity {
            allocate_store_publication_identity(store_id.clone()).unwrap()
        }

        let store_id = store("production-global-restart");
        let before_restart = viewer_session(&store_id);
        let old_target = before_restart.fixed_target();
        drop(before_restart);
        let after_restart = viewer_session(&store_id);

        assert!(after_restart.generation() > old_target.generation());
        assert!(!old_target.matches_publication(&after_restart));
    }

    #[test]
    fn maximum_generation_is_allocated_once_then_exhaustion_is_permanent() {
        let allocator = StoreGenerationAllocatorCore::seeded_for_test(NonZeroU64::MAX);
        assert_eq!(allocator.allocate_generation().unwrap().get(), u64::MAX);
        for _ in 0..3 {
            assert_eq!(
                allocator.allocate_generation(),
                Err(StoreGenerationAllocationError::Exhausted)
            );
        }
    }

    #[test]
    fn complete_identity_distinguishes_store_and_generation_axes() {
        let store_a = store("a");
        let store_b = store("b");
        let generation_seven_a = StoreGenerationAllocatorCore::seeded_for_test(seed(7))
            .allocate_publication(store_a.clone())
            .unwrap();
        let generation_seven_b = StoreGenerationAllocatorCore::seeded_for_test(seed(7))
            .allocate_publication(store_b)
            .unwrap();
        let generation_eight_a = StoreGenerationAllocatorCore::seeded_for_test(seed(8))
            .allocate_publication(store_a)
            .unwrap();

        assert_ne!(generation_seven_a, generation_seven_b);
        assert_ne!(generation_seven_a, generation_eight_a);
        assert_eq!(
            generation_seven_a.generation(),
            generation_seven_b.generation()
        );
    }

    #[test]
    fn stale_fixed_targets_and_results_do_not_resolve_to_reopened_store() {
        let allocator = StoreGenerationAllocatorCore::seeded_for_test(seed(1));
        let store_id = store("reopened");
        let old = allocator.allocate_publication(store_id.clone()).unwrap();
        let reopened = allocator.allocate_publication(store_id).unwrap();
        let old_target = old.fixed_target();
        let new_target = reopened.fixed_target();
        let mut results = BTreeMap::new();
        results.insert(old_target.clone(), "old result");

        assert_eq!(results.get(&old_target), Some(&"old result"));
        assert_eq!(results.get(&new_target), None);
        assert!(!old_target.matches_publication(&reopened));
        assert!(new_target.matches_publication(&reopened));
    }

    #[test]
    fn diagnostics_use_existing_store_debug_and_fixed_errors() {
        let secret_application = "secret-app?token=application-secret";
        let secret_recording = "secret-recording?token=recording-secret";
        let identity = StoreGenerationAllocatorCore::seeded_for_test(seed(42))
            .allocate_publication(StoreId::recording(secret_application, secret_recording))
            .unwrap();
        let diagnostics = [
            format!("{:?}", identity.generation()),
            format!("{identity:?}"),
            format!("{:?}", identity.fixed_target()),
        ];
        assert_eq!(diagnostics[0], "StoreGeneration(<opaque>)");
        assert_eq!(diagnostics[1], "StorePublicationIdentity(<opaque>)");
        assert_eq!(diagnostics[2], "StoreGenerationId(<opaque>)");
        for debug in diagnostics {
            assert!(!debug.contains(secret_application));
            assert!(!debug.contains(secret_recording));
            assert!(!debug.contains("42"));
        }
        assert_eq!(
            StoreGenerationAllocationError::Exhausted.to_string(),
            "Store publication generations are exhausted"
        );
    }
}
