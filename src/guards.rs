use std::{
    ops::{Deref, DerefMut},
    ptr::NonNull,
    sync::Mutex,
};

use crate::borrow_state::BorrowState;

#[derive(Debug)]
pub struct NonAliasingGuard<'a, T> {
    state: &'a Mutex<BorrowState>,
    current_ptr: &'a Mutex<Vec<NonNull<T>>>,
}

impl<'a, T> NonAliasingGuard<'a, T> {
    pub fn new(state: &'a Mutex<BorrowState>, current_ptr: &'a Mutex<Vec<NonNull<T>>>) -> Self {
        Self { state, current_ptr }
    }
}

impl<'a, T> Drop for NonAliasingGuard<'a, T> {
    fn drop(&mut self) {
        let Self { state, current_ptr } = self;
        let mut state_guard = state.lock().unwrap();
        let mut ptr_guard = current_ptr.lock().unwrap();
        state_guard.unset_non_aliasing().unwrap();
        ptr_guard.pop().unwrap();
        drop(state_guard);
        drop(ptr_guard);
    }
}

#[derive(Debug)]
pub struct GdRef<'a, T> {
    state: &'a Mutex<BorrowState>,
    value: NonNull<T>,
}

impl<'a, T> GdRef<'a, T> {
    /// Create a new `GdRef` guard which can be immutably dereferenced.
    ///
    /// # Safety
    ///
    /// The value behind the `value` pointer must be accessible for as long as the guard is not dropped.
    /// And there must also be no mutable references made to the value for as long as this guard exists, nor
    /// can this alias any existing mutable references.
    pub unsafe fn new(state: &'a Mutex<BorrowState>, value: NonNull<T>) -> Self {
        Self { state, value }
    }
}

impl<'a, T> Deref for GdRef<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { self.value.as_ref() }
    }
}

impl<'a, T> Drop for GdRef<'a, T> {
    fn drop(&mut self) {
        self.state.lock().unwrap().decrement_shared().unwrap();
    }
}

#[derive(Debug)]
pub struct GdMut<'a, T> {
    state: &'a Mutex<BorrowState>,
    count: usize,
    value: NonNull<T>,
}

impl<'a, T> GdMut<'a, T> {
    /// Create a new `GdMut` guard which can be mutably dereferenced.
    ///
    /// # Safety
    ///
    /// The value behind the `value` pointer must be accessible for as long as the guard is not dropped.
    /// And there must not be any other references or mutable references to this value for as long as this
    /// guard exists, unless:
    /// 1. It is know that this guard cannot be used to make a new reference when those references exist.
    /// 2. Any new references to the same value must be derived from the same `value` pointer.
    pub unsafe fn new(state: &'a Mutex<BorrowState>, count: usize, value: NonNull<T>) -> Self {
        Self {
            state,
            count,
            value,
        }
    }
}

impl<'a, T> Deref for GdMut<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        let count = self.state.lock().unwrap().mut_count();
        // This is just a best-effort error check. It should never be triggered.
        assert_eq!(
            self.count, count,
            "attempted to access the non-current mutable borrow. **this is a bug, please report it**"
        );
        unsafe { self.value.as_ref() }
    }
}

impl<'a, T> DerefMut for GdMut<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        let count = self.state.lock().unwrap().mut_count();
        // This is just a best-effort error check. It should never be triggered.
        assert_eq!(
            self.count, count,
            "attempted to access the non-current mutable borrow. **this is a bug, please report it**"
        );
        unsafe { self.value.as_mut() }
    }
}

impl<'a, T> Drop for GdMut<'a, T> {
    fn drop(&mut self) {
        self.state.lock().unwrap().decrement_mut().unwrap();
    }
}
