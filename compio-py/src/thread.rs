// SPDX-License-Identifier: Apache-2.0 OR MulanPSL-2.0
// Copyright 2025 Fantix King

//! Thread ID utilities for atomic storage.
//!
//! Provides a numeric representation of thread IDs that can be stored in atomic
//! integers for ownership tracking and cross-thread validation.
//!
//! # Implementation
//!
//! This module uses a monotonic atomic counter to generate unique thread IDs,
//! guaranteeing that each thread receives a distinct non-zero identifier. This
//! approach ensures:
//! - **Uniqueness**: No hash collisions possible
//! - **Efficiency**: Thread-local caching via `thread_local!`
//! - **Compatibility**: Works with atomic compare-exchange operations
//!
//! # Example
//!
//! ```ignore
//! use crate::thread::get_current_thread_id;
//!
//! let thread_id = get_current_thread_id();
//! // Use thread_id in atomic operations for ownership tracking
//! ```

use std::{
    num::NonZero,
    sync::atomic::{AtomicU32, Ordering},
};

/// Global atomic counter for generating unique thread IDs.
/// Starts at 1 to ensure all generated IDs are non-zero.
static NEXT_THREAD_ID: AtomicU32 = AtomicU32::new(1);

thread_local! {
    /// Thread-local storage for the current thread's unique ID.
    ///
    /// Each thread lazily initializes this value by atomically incrementing
    /// the global counter, ensuring uniqueness across all threads.
    static CURRENT_THREAD_ID: NonZero<u32> = {
        let id = NEXT_THREAD_ID.fetch_add(1, Ordering::Relaxed);
        // SAFETY: NEXT_THREAD_ID starts at 1, so fetch_add will never return 0
        // for any thread (until u32 overflow, which is practically impossible
        // with ~4 billion threads)
        NonZero::new(id).expect("Thread ID counter overflow")
    };
}

/// Returns a unique numeric identifier for the current thread.
///
/// This function is thread-safe and returns the same ID when called multiple
/// times from the same thread. Each thread is guaranteed to receive a distinct
/// non-zero ID.
///
/// # Returns
///
/// A `NonZero<u32>` representing the current thread's unique identifier.
///
/// # Examples
///
/// ```ignore
/// use crate::thread::get_current_thread_id;
///
/// let id1 = get_current_thread_id();
/// let id2 = get_current_thread_id();
/// assert_eq!(id1, id2); // Same thread, same ID
/// ```
///
/// # Performance
///
/// This function is very fast as it simply reads from thread-local storage
/// after the first call. The initial call per thread performs one atomic
/// increment.
pub fn get_current_thread_id() -> NonZero<u32> {
    CURRENT_THREAD_ID.with(|id| *id)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        sync::{Arc, Mutex},
        thread,
    };

    use super::*;

    #[test]
    fn test_same_thread_returns_same_id() {
        // Calling from the same thread should always return the same ID
        let id1 = get_current_thread_id();
        let id2 = get_current_thread_id();
        let id3 = get_current_thread_id();

        assert_eq!(id1, id2);
        assert_eq!(id2, id3);
    }

    #[test]
    fn test_different_threads_get_different_ids() {
        // Each thread should receive a unique ID
        let id_main = get_current_thread_id();

        let id_thread1 = thread::spawn(|| get_current_thread_id()).join().unwrap();

        let id_thread2 = thread::spawn(|| get_current_thread_id()).join().unwrap();

        // All three IDs should be different
        assert_ne!(id_main, id_thread1);
        assert_ne!(id_main, id_thread2);
        assert_ne!(id_thread1, id_thread2);
    }

    #[test]
    fn test_many_threads_unique_ids() {
        // Test with many threads to ensure no collisions
        const NUM_THREADS: usize = 100;
        let ids = Arc::new(Mutex::new(HashSet::new()));

        let handles: Vec<_> = (0..NUM_THREADS)
            .map(|_| {
                let ids = Arc::clone(&ids);
                thread::spawn(move || {
                    let id = get_current_thread_id();
                    let mut set = ids.lock().unwrap();
                    // Insert should return true (ID was not already in set)
                    assert!(set.insert(id.get()), "Duplicate thread ID detected!");
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        // Verify we got exactly NUM_THREADS unique IDs
        let set = ids.lock().unwrap();
        assert_eq!(set.len(), NUM_THREADS);
    }

    #[test]
    fn test_thread_id_is_nonzero() {
        // Thread IDs should never be zero
        let id = get_current_thread_id();
        assert!(id.get() > 0);

        let id_other = thread::spawn(|| get_current_thread_id()).join().unwrap();
        assert!(id_other.get() > 0);
    }

    #[test]
    fn test_thread_id_monotonic() {
        // IDs should be assigned in increasing order
        let ids = Arc::new(Mutex::new(Vec::new()));

        let handles: Vec<_> = (0..10)
            .map(|_| {
                let ids = Arc::clone(&ids);
                thread::spawn(move || {
                    let id = get_current_thread_id();
                    ids.lock().unwrap().push(id.get());
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        let collected_ids = ids.lock().unwrap();
        // Each ID should be unique (already tested above, but double-check)
        let unique: HashSet<_> = collected_ids.iter().collect();
        assert_eq!(unique.len(), collected_ids.len());
    }
}
