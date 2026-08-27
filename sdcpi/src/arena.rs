//! Where the records of the two tables live.
//!
//! A choice state, a history and an expansion are created once and then read by
//! every worker for the rest of the run. Holding them in reference-counted
//! pointers made the search **slower** with more workers: a process with a few
//! hundred records has a few hundred counters, and sixteen cores incrementing
//! and decrementing the same few words tens of millions of times a second spend
//! their time moving cache lines rather than searching. Reference counting also
//! costs an allocation per node of the frontier, and the frontier is copied at
//! every expansion.
//!
//! So the records are owned here, by one arena that outlives every reference
//! into it, and the search passes plain shared references, which are `Copy`,
//! carry no counter and touch no memory when they are handed around.
//!
//! The arena grows by pushing a box on a list. A box does not move when the list
//! that holds it grows, so a reference into one stays valid, and nothing is ever
//! removed. That is the invariant the single unsafe block below rests on, and it
//! is why the lifetime of the reference is the lifetime of the arena rather than
//! the lifetime of the lock that guards the list.

use std::sync::Mutex;

pub struct Arena<T> {
    owned: Mutex<Vec<Box<T>>>,
}

impl<T> Default for Arena<T> {
    fn default() -> Arena<T> {
        Arena::new()
    }
}

impl<T> Arena<T> {
    pub fn new() -> Arena<T> {
        Arena {
            owned: Mutex::new(Vec::new()),
        }
    }

    /// Takes ownership of `value` and hands back a reference that lives as long
    /// as the arena does.
    pub fn alloc(&self, value: T) -> &T {
        let boxed = Box::new(value);
        // SAFETY: the pointee of a box does not move when the vector that holds
        // the box grows or reallocates, and nothing is ever removed from
        // `owned`, so the address stays valid until the arena itself is dropped.
        // The returned reference borrows the arena, so the compiler will not let
        // it outlive that point. The lock guards the vector, not the pointee.
        let stable: *const T = &*boxed;
        self.owned.lock().unwrap().push(boxed);
        unsafe { &*stable }
    }

    pub fn len(&self) -> usize {
        self.owned.lock().unwrap().len()
    }
}
