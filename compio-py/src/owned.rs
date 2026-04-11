// SPDX-License-Identifier: Apache-2.0 OR MulanPSL-2.0
// Copyright 2025 Fantix King

//! Thread-safe cell with single-owner access control and reference counting.
//!
//! This module provides `OwnedRefCell<T>`, which combines lazy initialization
//! with exclusive ownership semantics. At any given time, at most one thread
//! "owns" the cell and can access its contents. The owning thread can hold
//! multiple references simultaneously through reference counting.
//!
//! This module is inspired by:
//!  * `std::cell::RefCell` for interior mutability and reference counting
//!  * `std::cell::OnceCell` for lazy initialization
//!  * https://github.com/compio-rs/compio/blob/master/compio-runtime/src/runtime/send_wrapper.rs
//!  * https://github.com/thk1/send_wrapper

use std::{
    cell::{Cell, UnsafeCell},
    marker, mem,
    ops::Deref,
    sync::atomic::{AtomicU32, Ordering},
};

use pyo3::{PyErr, exceptions::PyRuntimeError};

/// Sentinel value indicating that no thread currently owns the cell.
const NO_OWNER: u32 = 0;

/// Type alias for the borrow counter.
type BorrowCounter = usize;

/// Value indicating no active borrows exist.
const UNUSED: BorrowCounter = 0;

#[derive(Debug)]
pub enum Error {
    OwnedByOther(u32),
    AlreadyInitialized,
}

impl From<Error> for PyErr {
    fn from(value: Error) -> Self {
        match value {
            Error::OwnedByOther(_) => PyErr::new::<PyRuntimeError, _>(
                "Concurrent access detected: OwnedRefCell is owned by another thread",
            ),
            Error::AlreadyInitialized => {
                PyErr::new::<PyRuntimeError, _>("OwnedRefCell is already initialized")
            }
        }
    }
}

type OwnedResult<T> = Result<T, Error>;

/// A thread-safe cell with single-owner access control.
///
/// `OwnedRefCell<T>` provides synchronized access to an optional value with the
/// following guarantees:
///
/// - **Single ownership**: At most one thread can own the cell at any time
/// - **Reference counting**: The owning thread can create multiple `Ref`
///   instances
/// - **Reentrant control**: Callers can specify whether reentrant access is
///   allowed
/// - **Lazy initialization**: The value can be initialized exactly once after
///   creation
///
/// # Thread Safety
///
/// The cell is `Sync` when `T: Send`. The `owner` atomic ensures exclusive
/// ownership, while the `borrow` counter tracks active references from the
/// owning thread. Since only the owning thread can access `borrow` (protected
/// by the `owner` lock), using a non-atomic `Cell` is sound and more efficient
/// than `AtomicUsize`.
///
/// # Examples
///
/// ```ignore
/// let cell = OwnedRefCell::default();
///
/// // Initialize the cell
/// cell.init(42).unwrap();
///
/// // Get a reference (allows reentrant access)
/// let ref1 = cell.get().unwrap().unwrap();
/// let ref2 = cell.get().unwrap().unwrap(); // OK: reentrant
///
/// // Try to acquire without allowing reentrant access
/// assert!(cell.acquire(false).is_err()); // Fails: already owned by this thread
/// ```
pub struct OwnedRefCell<T> {
    /// The inner storage for the optional value.
    /// Uses `UnsafeCell` to allow interior mutability while maintaining `Sync`
    /// safety through the owner lock.
    inner: UnsafeCell<Option<T>>,

    /// Atomic tracking of which thread (by thread ID) currently owns this cell.
    /// When `NO_OWNER`, the cell is available for acquisition.
    owner: AtomicU32,

    /// Reference counter tracking the number of active `Ref` instances.
    /// This is a `Cell` (not `AtomicUsize`) because only the owning thread can
    /// access it, with access protected by the `owner` atomic lock.
    borrow: Cell<BorrowCounter>,
}

impl<T> OwnedRefCell<T> {
    /// Gets a reference to the inner value without checking ownership.
    ///
    /// # Safety
    ///
    /// This is safe because:
    /// - Reads are protected by the atomic `owner` field
    /// - Modifications only occur when a thread holds ownership via
    ///   `OwnershipGuard`
    /// - The borrow counter ensures the value isn't dropped while references
    ///   exist
    #[inline]
    fn get_unchecked(&self) -> Option<&T> {
        unsafe { &*self.inner.get() }.as_ref()
    }

    /// Attempts to acquire ownership of this cell for the current thread.
    ///
    /// This method acquires an `OwnershipGuard` which increments the reference
    /// count. When all `OwnershipGuard` instances are dropped, ownership is
    /// released.
    ///
    /// # Parameters
    ///
    /// - `reentrant`: If `true`, allows the current owner to acquire again
    ///   (increments ref count). If `false`, returns `Err` if the cell is
    ///   already owned.
    ///
    /// # Returns
    ///
    /// - `Ok(OwnershipGuard)` if ownership was acquired or reacquired
    /// - `Err(thread_id)` if another thread owns the cell, or if
    ///   `reentrant=false` and this thread already owns it
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // First acquisition
    /// let guard1 = cell.acquire(true)?; // OK
    ///
    /// // Reentrant acquisition with reentrant=true
    /// let guard2 = cell.acquire(true)?; // OK
    ///
    /// // Reentrant acquisition with reentrant=false
    /// let guard3 = cell.acquire(false); // Err: already owned
    /// ```
    #[inline]
    pub fn acquire(&self, reentrant: bool) -> OwnedResult<OwnershipGuard<'_>> {
        OwnershipGuard::new(&self.owner, &self.borrow, reentrant)
    }

    /// Gets a reference to the value in the cell, if it exists.
    ///
    /// This is a convenience method that acquires ownership (allowing reentrant
    /// access) and returns a reference to the value if it has been
    /// initialized.
    ///
    /// # Returns
    ///
    /// - `Ok(Some(Ref))` if the cell contains a value and access was granted
    /// - `Ok(None)` if the cell is empty but access was granted
    /// - `Err(thread_id)` if another thread owns the cell
    #[inline]
    pub fn get(&self) -> Result<Option<Ref<'_, T>>, Error> {
        let b = self.acquire(true)?;
        Ok(self.get_unchecked().map(|value| Ref { value, _borrow: b }))
    }

    /// Initializes the cell with a value.
    ///
    /// The calling thread must acquire ownership (or already own it) before
    /// initializing. This method allows reentrant access.
    ///
    /// # Panics
    ///
    /// Panics if the cell already contains a value.
    ///
    /// # Returns
    ///
    /// - `Ok(())` if initialization succeeds
    /// - `Err(thread_id)` if another thread owns the cell
    #[inline]
    pub fn init(&self, value: T) -> OwnedResult<()> {
        self.try_init(value).map_err(|e| match e {
            Error::AlreadyInitialized => panic!("Attempted to set twice"),
            other => other,
        })
    }

    #[inline]
    pub fn try_init(&self, value: T) -> OwnedResult<()> {
        let _guard = self.acquire(true)?;
        let slot = unsafe { &mut *self.inner.get() };
        if slot.is_some() {
            Err(Error::AlreadyInitialized)
        } else {
            slot.replace(value);
            Ok(())
        }
    }

    /// Takes the value out of the cell, leaving it empty.
    ///
    /// This consumes the cell via mutable reference, ensuring exclusive access.
    ///
    /// # Returns
    ///
    /// The value if present, or `None` if the cell was empty.
    #[inline]
    pub fn take(&mut self) -> Option<T> {
        mem::take(self).inner.into_inner()
    }
}

impl<T> Default for OwnedRefCell<T> {
    /// Creates an empty `OwnedRefCell` with no owner and no value.
    fn default() -> Self {
        Self {
            inner: UnsafeCell::new(None),
            owner: AtomicU32::new(NO_OWNER),
            borrow: Cell::new(UNUSED),
        }
    }
}

/// `OwnedRefCell` is `Sync` when `T` is `Send` because:
///
/// - The `owner` atomic ensures exclusive ownership (only one thread at a time)
/// - The `borrow` counter is only accessed by the owning thread
/// - The value can be sent between threads when ownership is transferred
/// - After initialization, the value is immutable (only read through shared
///   references)
// FIXME: since compio-rs/compio#660, `Proactor` is no longer `Send`, so we
// tentatively allow any `T` to be accessible from multiple threads, leaving
// undefined behavior if `T` is not `Send`. A proper fix would be to explicitly
// pin the thread ID by the runtime when it is known to be safe. See also #6.
// unsafe impl<T> Sync for OwnedRefCell<T> where T: Send {}
unsafe impl<T> Sync for OwnedRefCell<T> {}
unsafe impl<T> Send for OwnedRefCell<T> {}

/// A reference to a value inside an `OwnedRefCell`.
///
/// This type derefs to `T` and holds an `OwnershipGuard` to maintain the
/// reference count. When dropped, the reference count is decremented, and
/// ownership may be released if this was the last reference.
///
/// `Ref` is neither `Send` nor `Sync` because it must be dropped on the same
/// thread that created it to maintain correct ownership tracking.
pub struct Ref<'a, T> {
    /// Reference to the value stored in the cell.
    value: &'a T,
    /// Ownership guard that tracks this reference in the counter.
    _borrow: OwnershipGuard<'a>,
}

impl<T> Deref for Ref<'_, T> {
    type Target = T;

    /// Dereferences to the inner value.
    #[inline]
    fn deref(&self) -> &Self::Target {
        self.value
    }
}

/// RAII guard that manages ownership and reference counting for an
/// `OwnedRefCell`.
///
/// Each `OwnershipGuard` increments the borrow counter when created and
/// decrements it when dropped. Ownership is released (reset to `NO_OWNER`) only
/// when the reference count reaches zero.
///
/// `OwnershipGuard` is neither `Send` nor `Sync` (via `PhantomData<*const ()>`)
/// because it must be dropped on the same thread that created it to maintain
/// correct ownership tracking and borrow counting.
pub struct OwnershipGuard<'a> {
    /// Reference to the atomic owner field tracking which thread owns the cell.
    owner: &'a AtomicU32,
    /// Reference to the non-atomic borrow counter (protected by the owner
    /// lock).
    borrow: &'a Cell<BorrowCounter>,
    /// Marker type to make this `!Send` and `!Sync`.
    /// Using `*const ()` because raw pointers are neither `Send` nor `Sync`.
    _phantom: marker::PhantomData<*const ()>,
}

impl<'a> OwnershipGuard<'a> {
    /// Attempts to create a new `OwnershipGuard` by acquiring or reacquiring
    /// ownership.
    ///
    /// This method uses a compare-and-swap loop to atomically manage the owner
    /// field.
    ///
    /// # Parameters
    ///
    /// - `owner`: The atomic owner field to acquire
    /// - `borrow`: The borrow counter to increment
    /// - `reentrant`: Whether to allow reentrant access from the same thread
    ///
    /// # Returns
    ///
    /// - `Ok(OwnershipGuard)` if ownership was acquired or reacquired
    /// - `Err(thread_id)` if another thread owns the cell, or if
    ///   `reentrant=false` and this thread already owns it
    #[inline]
    fn new(
        owner: &'a AtomicU32,
        borrow: &'a Cell<BorrowCounter>,
        reentrant: bool,
    ) -> OwnedResult<OwnershipGuard<'a>> {
        // Get the current thread's ID to claim ownership
        let new = crate::thread::get_current_thread_id().get();

        // Load the current owner with Acquire ordering to synchronize with the Release
        // store in Drop, ensuring we see all modifications from the previous owner
        let mut old = owner.load(Ordering::Acquire);

        // Spin-loop attempting to acquire ownership if the cell is unowned
        while old == NO_OWNER {
            // Attempt to atomically swap NO_OWNER with our thread ID
            // Success ordering: AcqRel
            //   - Acquire: synchronize with previous Release store (though NO_OWNER means
            //     none)
            //   - Release: publish our ownership to other threads
            // Failure ordering: Acquire
            //   - Synchronize with concurrent modifications
            match owner.compare_exchange_weak(NO_OWNER, new, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => {
                    // Successfully acquired ownership - increment borrow count and return
                    return Ok(Self::new_unchecked(owner, borrow));
                }
                Err(current) => {
                    // CAS failed, another thread modified owner - retry with updated value
                    old = current;
                }
            }
        }

        // The cell has an owner - check if it's us (reentrant) or another thread
        if reentrant && old == new {
            // Reentrant access allowed and we're the owner - increment borrow count
            Ok(Self::new_unchecked(owner, borrow))
        } else {
            // Either reentrant=false, or a different thread owns it
            Err(Error::OwnedByOther(old))
        }
    }

    /// Creates a new `OwnershipGuard` and increments the borrow counter.
    ///
    /// # Safety
    ///
    /// This must only be called after successfully acquiring ownership (either
    /// via CAS or reentrant check). The caller must ensure that only the
    /// owning thread calls this.
    ///
    /// # Note on `Cell` safety
    ///
    /// Using `Cell::get()` and `Cell::set()` is safe here because:
    /// - Only the owning thread can call this (protected by the `owner` lock)
    /// - The `OwnershipGuard` is `!Send`, so it can't be transferred to another
    ///   thread
    /// - This avoids the overhead of atomic operations while maintaining safety
    #[inline]
    fn new_unchecked(owner: &'a AtomicU32, borrow: &'a Cell<BorrowCounter>) -> OwnershipGuard<'a> {
        // Increment the borrow count
        // Safe because only the owning thread can access this
        borrow.set(borrow.get() + 1);
        OwnershipGuard {
            owner,
            borrow,
            _phantom: marker::PhantomData,
        }
    }
}

impl Drop for OwnershipGuard<'_> {
    /// Decrements the reference count when the guard is dropped.
    ///
    /// If the count reaches zero, releases ownership by resetting the owner
    /// field to `NO_OWNER`. Uses Release ordering to ensure all
    /// modifications made while holding ownership are visible to the next
    /// thread that acquires it.
    #[inline]
    fn drop(&mut self) {
        // Decrement the borrow count
        // Safe because this can only run on the owning thread (OwnershipGuard is !Send)
        let new_borrow = self.borrow.get() - 1;
        self.borrow.set(new_borrow);

        // If this was the last reference, release ownership
        if new_borrow == UNUSED {
            // Release ordering ensures all modifications we made are visible to
            // the next thread that acquires ownership (via their Acquire load)
            self.owner.store(NO_OWNER, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, thread};

    use super::*;

    #[test]
    fn test_init_and_get() {
        let cell = OwnedRefCell::default();

        // Cell starts empty
        assert!(cell.get().unwrap().is_none());

        // Initialize the cell
        cell.init(42).unwrap();

        // Can get the value
        let value = cell.get().unwrap();
        assert_eq!(*value.unwrap(), 42);
    }

    #[test]
    #[should_panic(expected = "Attempted to set twice")]
    fn test_init_twice_panics() {
        let cell = OwnedRefCell::default();
        cell.init(42).unwrap();
        cell.init(43).unwrap(); // Should panic
    }

    #[test]
    fn test_take() {
        let mut cell = OwnedRefCell::default();
        cell.init(42).unwrap();

        let value = cell.take();
        assert_eq!(value, Some(42));

        // Cell is now empty
        assert_eq!(cell.take(), None);
    }

    #[test]
    fn test_reentrant_access_allowed() {
        let cell = OwnedRefCell::default();
        cell.init(42).unwrap();

        // First acquisition
        let ref1 = cell.get().unwrap().unwrap();
        assert_eq!(*ref1, 42);

        // Reentrant acquisition (should succeed with reentrant=true)
        let ref2 = cell.get().unwrap().unwrap();
        assert_eq!(*ref2, 42);

        // Both references can coexist
        assert_eq!(*ref1, *ref2);
    }

    #[test]
    fn test_reentrant_access_denied() {
        let cell = OwnedRefCell::default();
        cell.init(42).unwrap();

        // First acquisition with reentrant=true
        let _guard1 = cell.acquire(true).unwrap();

        // Second acquisition with reentrant=false should fail
        let result = cell.acquire(false);
        assert!(result.is_err());
    }

    #[test]
    fn test_acquire_and_release() {
        let cell = OwnedRefCell::<i32>::default();

        // Acquire ownership
        {
            let _guard = cell.acquire(true).unwrap();
            // Ownership is held here
        }
        // Ownership released after guard drops

        // Can acquire again
        let _guard2 = cell.acquire(true).unwrap();
    }

    #[test]
    fn test_multiple_guards_ref_counting() {
        let cell = OwnedRefCell::default();
        cell.init(42).unwrap();

        // Create multiple guards (all should increment ref count)
        let guard1 = cell.acquire(true).unwrap();
        let guard2 = cell.acquire(true).unwrap();
        let guard3 = cell.acquire(true).unwrap();

        // All guards exist simultaneously
        drop(guard1);
        // Cell still owned (ref count = 2)

        drop(guard2);
        // Cell still owned (ref count = 1)

        drop(guard3);
        // Cell now released (ref count = 0)

        // Can acquire again after all guards dropped
        let _guard4 = cell.acquire(true).unwrap();
    }

    #[test]
    fn test_ref_holds_ownership() {
        let cell = OwnedRefCell::default();
        cell.init(42).unwrap();

        let ref1 = cell.get().unwrap().unwrap();
        let ref2 = cell.get().unwrap().unwrap();

        // Both Refs hold ownership guards
        assert_eq!(*ref1, 42);
        assert_eq!(*ref2, 42);

        drop(ref1);
        // ref2 still holds ownership
        assert_eq!(*ref2, 42);
    }

    #[test]
    fn test_cross_thread_ownership() {
        let cell = Arc::new(OwnedRefCell::default());
        cell.init(42).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));

        let cell_clone = Arc::clone(&cell);
        let barrier_clone = barrier.clone();
        let handle = thread::spawn(move || {
            barrier_clone.wait(); // Synchronize with main thread
            // Try to acquire from another thread
            let result = cell_clone.acquire(true);
            result.is_err() // Should fail if main thread holds ownership
        });

        // Main thread holds ownership
        let _guard = cell.acquire(true).unwrap();
        barrier.wait(); // Let other thread attempt to acquire

        // Other thread should fail to acquire
        let other_thread_failed = handle.join().unwrap();
        assert!(other_thread_failed);
    }

    #[test]
    fn test_cross_thread_transfer() {
        let cell = Arc::new(OwnedRefCell::default());
        cell.init(42).unwrap();

        // Main thread acquires and releases
        {
            let value = cell.get().unwrap().unwrap();
            assert_eq!(*value, 42);
        } // Ownership released

        let cell_clone = Arc::clone(&cell);
        let handle = thread::spawn(move || {
            // Other thread can now acquire
            let value = cell_clone.get().unwrap().unwrap();
            *value
        });

        let result = handle.join().unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn test_get_empty_cell() {
        let cell = OwnedRefCell::<i32>::default();

        let result = cell.get().unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_acquire_from_different_thread() {
        use std::sync::Barrier;

        let cell = Arc::new(OwnedRefCell::default());
        cell.init(100).unwrap();

        // Barrier to synchronize: wait for both threads to be ready
        let barrier = Arc::new(Barrier::new(2));

        let cell_clone = Arc::clone(&cell);
        let barrier_clone = Arc::clone(&barrier);

        let handle = thread::spawn(move || {
            // Acquire in thread
            let _guard = cell_clone.acquire(true).unwrap();
            // Signal that we've acquired ownership
            barrier_clone.wait();
            // Wait again before releasing
            barrier_clone.wait();
        });

        // Wait for other thread to acquire ownership
        barrier.wait();

        // Main thread should fail to acquire while other thread holds it
        let result = cell.acquire(true);
        assert!(result.is_err());

        // Signal the other thread to release
        barrier.wait();

        // Wait for thread to complete
        handle.join().unwrap();

        // Now main thread can acquire
        let _guard = cell.acquire(true).unwrap();
    }

    #[test]
    fn test_sequential_init_from_different_threads() {
        let cell = Arc::new(OwnedRefCell::default());

        // First thread initializes
        let cell_clone = Arc::clone(&cell);
        let handle = thread::spawn(move || cell_clone.init(42));

        // Wait for first thread to complete
        let result1 = handle.join().unwrap();
        assert!(result1.is_ok());

        // Second thread tries to init - should fail because already initialized
        let cell_clone2 = Arc::clone(&cell);
        let handle2 = thread::spawn(move || cell_clone2.init(100));

        // Should fail (or panic if it acquires ownership and sees value already set)
        let result2 = handle2.join();
        // Either the thread panicked (join returns Err) or init returned an error
        let failed = result2.is_err() || (result2.is_ok() && result2.unwrap().is_err());
        assert!(failed);

        // Cell should have first value
        let value = *cell.get().unwrap().unwrap();
        assert_eq!(value, 42);
    }

    #[test]
    fn test_default_creates_empty_cell() {
        let cell = OwnedRefCell::<String>::default();
        assert!(cell.get().unwrap().is_none());
    }

    #[test]
    fn test_deref_ref() {
        let cell = OwnedRefCell::default();
        cell.init(String::from("hello")).unwrap();

        let value = cell.get().unwrap().unwrap();
        // Test Deref works
        assert_eq!(value.len(), 5);
        assert_eq!(&*value, "hello");
    }

    #[test]
    fn test_ownership_guard_prevents_concurrent_access() {
        let cell = Arc::new(OwnedRefCell::default());
        cell.init(0).unwrap();

        let cell_clone = Arc::clone(&cell);

        // Hold ownership guard in main thread
        let guard = cell.acquire(true).unwrap();

        let handle = thread::spawn(move || {
            // Try to get value from another thread
            cell_clone.get().is_err()
        });

        let failed = handle.join().unwrap();
        assert!(failed); // Should fail

        drop(guard); // Release ownership
    }

    #[test]
    fn test_reentrant_flag_semantics() {
        let cell = OwnedRefCell::default();
        cell.init(42).unwrap();

        // Acquire with reentrant=true
        let _guard1 = cell.acquire(true).unwrap();

        // Can acquire again with reentrant=true
        let result = cell.acquire(true);
        assert!(result.is_ok());

        drop(result);

        // Cannot acquire with reentrant=false
        let result = cell.acquire(false);
        assert!(result.is_err());
    }
}
