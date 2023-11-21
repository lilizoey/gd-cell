#![feature(strict_provenance)]

use std::{
    cell::UnsafeCell,
    marker::{PhantomData, PhantomPinned},
    mem::ManuallyDrop,
    ops::{Deref, DerefMut},
    pin::Pin,
    sync::Mutex,
};

#[derive(Debug)]
struct BorrowState {
    /// The number of `&T` references that are tracked.
    shared_count: usize,
    /// The number of `&mut T` references that are tracked.
    mut_count: usize,
    /// The number of `&mut T` references that cannot be aliased.
    non_aliasing_count: usize,
}

impl BorrowState {
    pub fn new() -> Self {
        Self {
            shared_count: 0,
            mut_count: 0,
            non_aliasing_count: 0,
        }
    }

    pub fn possibly_aliasing_count(&self) -> usize {
        self.mut_count - self.non_aliasing_count
    }

    pub fn may_ref(&self) -> bool {
        self.possibly_aliasing_count() == 0
    }

    pub fn may_mut_ref(&self) -> bool {
        self.possibly_aliasing_count() == 0 && self.shared_count == 0
    }

    pub fn increment_shared(&mut self) -> Result<(), String> {
        if !self.may_ref() {
            return Err(
                "cannot increment shared as there exist possibly aliasing mutable references"
                    .into(),
            );
        }

        self.shared_count = self
            .shared_count
            .checked_add(1)
            .ok_or("could not increment shared count")?;
        Ok(())
    }

    pub fn decrement_shared(&mut self) {
        debug_assert_eq!(self.possibly_aliasing_count(), 0);
        self.shared_count = self.shared_count.checked_sub(1).unwrap();
    }

    pub fn increment_mut(&mut self) -> Result<(), String> {
        if self.possibly_aliasing_count() != 0 {
            return Err(
                "cannot increment mut as there exist possibly aliasing mutable references".into(),
            );
        }

        if self.shared_count != 0 {
            return Err("cannot increment mut as there exist shared references".into());
        }

        self.mut_count = self
            .mut_count
            .checked_add(1)
            .ok_or("could not increment mut count")?;

        Ok(())
    }

    pub fn decrement_mut(&mut self) {
        debug_assert_eq!(
            self.mut_count,
            self.non_aliasing_count + 1,
            "must only decrement the mut counter when the current borrow is accessible"
        );

        self.mut_count = self.mut_count.checked_sub(1).unwrap();
        self.non_aliasing_count = self.mut_count;
    }

    pub fn increment_non_aliasing(&mut self) -> Result<(), String> {
        if self.possibly_aliasing_count() == 0 {
            return Err("cannot set mut reference as non-aliasing when there are no possibly aliasing pointers".into());
        }

        self.non_aliasing_count = self
            .non_aliasing_count
            .checked_add(1)
            .ok_or("could not increment non-aliasing count")?;
        Ok(())
    }

    pub fn decrement_non_aliasing(&mut self) -> Result<(), String> {
        if self.possibly_aliasing_count() > 0 {
            return Err("cannot have more than 1 possibly aliasing pointers".into());
        }

        if self.non_aliasing_count == 0 {
            return Err(
                "cannot mark mut pointer as aliasing when there are no possibly aliasing pointers"
                    .into(),
            );
        }

        self.non_aliasing_count = self
            .non_aliasing_count
            .checked_sub(1)
            .ok_or("could not decrement non-aliasing count")?;
        Ok(())
    }

    pub fn mut_count(&self) -> usize {
        self.mut_count
    }
}

#[derive(Debug)]
pub struct GdCell<T> {
    state: Mutex<BorrowState>,
    value: UnsafeCell<T>,
    current_ptr: Mutex<Vec<*mut T>>,
    _pin: PhantomPinned,
}

impl<T> GdCell<T> {
    pub fn new(value: T) -> Self {
        Self {
            state: Mutex::new(BorrowState::new()),
            value: UnsafeCell::new(value),
            current_ptr: Mutex::new(Vec::new()),
            _pin: PhantomPinned,
        }
    }

    pub fn gd_ref(self: Pin<&Self>) -> Result<GdRef<'_, T>, String> {
        self.state.lock().unwrap().increment_shared()?;

        Ok(GdRef {
            state: &self.get_ref().state,
            value: self.get_value(),
        })
    }

    pub fn gd_mut(self: Pin<&Self>) -> Result<GdMut<'_, T>, String> {
        let mut guard = self.state.lock().unwrap();
        guard.increment_mut()?;
        let count = guard.mut_count();

        Ok(GdMut {
            state: &self.get_ref().state,
            count,
            value: self.get_value(),
        })
    }

    fn get_value(self: Pin<&Self>) -> *mut T {
        let mut ptr_vec = self.current_ptr.lock().unwrap();

        if ptr_vec.is_empty() {
            ptr_vec.push(self.value.get())
        }

        *ptr_vec.last().unwrap()
    }

    /// Set the current mutable borrow as not aliasing any other references.
    ///
    /// Will error if there is no current possibly aliasing mutable borrow.
    ///
    /// # Safety
    ///
    /// The current mutable borrow, and any derived references, must *not* be accessed between this call, and
    /// a future call to `Self::unset_non_aliasing()`.
    pub fn set_non_aliasing<'a, 'b>(
        self: Pin<&'a Self>,
        current_ref: &'b mut T,
    ) -> Result<NonAliasingGuard<'b, T>, String>
    where
        'a: 'b,
    {
        let mut current_ptr_vec = self.current_ptr.lock().unwrap();
        let current_ptr = *current_ptr_vec.last().unwrap();
        let ptr = current_ref as *mut T;

        if current_ptr != ptr {
            // it is likely not unsound for this to happen, but it's unexpected
            return Err("wrong reference passed in".into());
        }

        let mut state_guard = self.state.lock().unwrap();
        let result = state_guard.increment_non_aliasing();
        current_ptr_vec.push(ptr);
        drop(state_guard);
        drop(current_ptr_vec);

        result.map(|_| NonAliasingGuard {
            state: &self.get_ref().state,
            current_ptr: &self.get_ref().current_ptr,
        })
    }
}

#[derive(Debug)]
pub struct NonAliasingGuard<'a, T> {
    state: &'a Mutex<BorrowState>,
    current_ptr: &'a Mutex<Vec<*mut T>>,
}

impl<'a, T> Drop for NonAliasingGuard<'a, T> {
    fn drop(&mut self) {
        let Self { state, current_ptr } = self;
        let mut state_guard = state.lock().unwrap();
        let mut ptr_guard = current_ptr.lock().unwrap();
        state_guard.decrement_non_aliasing().unwrap();
        let ptr = ptr_guard.pop().unwrap();
        drop(state_guard);
        drop(ptr_guard);
    }
}

#[derive(Debug)]
pub struct GdRef<'a, T> {
    state: &'a Mutex<BorrowState>,
    value: *mut T,
}

impl<'a, T> Deref for GdRef<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.value }
    }
}

impl<'a, T> Drop for GdRef<'a, T> {
    fn drop(&mut self) {
        self.state.lock().unwrap().decrement_shared()
    }
}

#[derive(Debug)]
pub struct GdMut<'a, T> {
    state: &'a Mutex<BorrowState>,
    count: usize,
    value: *mut T,
}

impl<'a, T> Deref for GdMut<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        let count = self.state.lock().unwrap().mut_count();
        assert_eq!(
            self.count, count,
            "attempted to access the non-current mutable borrow"
        );
        unsafe { &*self.value }
    }
}

impl<'a, T> DerefMut for GdMut<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        let count = self.state.lock().unwrap().mut_count();
        assert_eq!(
            self.count, count,
            "attempted to access the non-current mutable borrow"
        );
        unsafe { &mut *self.value }
    }
}

impl<'a, T> Drop for GdMut<'a, T> {
    fn drop(&mut self) {
        self.state.lock().unwrap().decrement_mut()
    }
}

#[cfg(test)]
mod test {
    use std::pin::pin;

    use super::*;

    #[test]
    fn prevent_mut_mut() {
        const VAL: i32 = -451431556;
        let cell = pin!(GdCell::new(VAL));
        let cell = cell.into_ref();
        let guard1 = cell.gd_mut().unwrap();
        let guard2 = cell.gd_mut();

        assert_eq!(*guard1, VAL);
        assert!(guard2.is_err());
        std::mem::drop(guard1);
    }

    #[test]
    fn prevent_mut_shared() {
        const VAL: i32 = 13512;
        let cell = pin!(GdCell::new(VAL));
        let cell = cell.into_ref();
        let guard1 = cell.gd_mut().unwrap();
        let guard2 = cell.gd_ref();

        assert_eq!(*guard1, VAL);
        assert!(guard2.is_err());
        std::mem::drop(guard1);
    }

    #[test]
    fn prevent_shared_mut() {
        const VAL: i32 = 99;
        let cell = pin!(GdCell::new(VAL));
        let cell = cell.into_ref();
        let guard1 = cell.gd_ref().unwrap();
        let guard2 = cell.gd_mut();

        assert_eq!(*guard1, VAL);
        assert!(guard2.is_err());
        std::mem::drop(guard1);
    }

    #[test]
    fn allow_shared_shared() {
        const VAL: i32 = 10;
        let cell = pin!(GdCell::new(VAL));
        let cell = cell.into_ref();
        let guard1 = cell.gd_ref().unwrap();
        let guard2 = cell.gd_ref().unwrap();

        assert_eq!(*guard1, VAL);
        assert_eq!(*guard2, VAL);
        std::mem::drop(guard1);
    }

    #[test]
    fn allow_non_aliasing_mut_mut() {
        const VAL: i32 = 23456;
        let cell = pin!(GdCell::new(VAL));
        let cell = cell.into_ref();

        let mut guard1 = cell.gd_mut().unwrap();
        let mut1 = &mut *guard1;
        assert_eq!(*mut1, VAL);
        *mut1 = VAL + 50;

        let no_alias_guard = cell.set_non_aliasing(mut1).unwrap();

        let mut guard2 = cell.gd_mut().unwrap();
        let mut2 = &mut *guard2;
        assert_eq!(*mut2, VAL + 50);
        *mut2 = VAL - 30;
        drop(guard2);

        drop(no_alias_guard);

        assert_eq!(*mut1, VAL - 30);
        *mut1 = VAL - 5;

        drop(guard1);

        let guard3 = cell.gd_ref().unwrap();
        assert_eq!(*guard3, VAL - 5);
    }

    #[test]
    fn prevent_mut_mut_without_non_aliasing() {
        const VAL: i32 = 23456;
        let cell = pin!(GdCell::new(VAL));
        let cell = cell.into_ref();

        let mut guard1 = cell.gd_mut().unwrap();
        let mut1 = &mut *guard1;
        assert_eq!(*mut1, VAL);
        *mut1 = VAL + 50;

        // let no_alias_guard = cell.set_non_aliasing(mut1).unwrap();

        cell.gd_mut()
            .expect_err("reference may be aliasing so should be prevented");

        drop(guard1);
    }

    #[test]
    fn different_non_aliasing() {
        const VAL1: i32 = 23456;
        const VAL2: i32 = 11111;
        let cell1 = pin!(GdCell::new(VAL1));
        let cell1 = cell1.into_ref();
        let cell2 = pin!(GdCell::new(VAL2));
        let cell2 = cell2.into_ref();

        let mut guard1 = cell1.gd_mut().unwrap();
        let mut1 = &mut *guard1;

        assert_eq!(*mut1, VAL1);
        *mut1 = VAL1 + 10;

        let mut guard2 = cell2.gd_mut().unwrap();
        let mut2 = &mut *guard2;

        assert_eq!(*mut2, VAL2);
        *mut2 = VAL2 + 10;

        let no_alias_guard = cell1
            .set_non_aliasing(mut2)
            .expect_err("should not allow different references");

        drop(no_alias_guard);

        drop(guard1);
        drop(guard2);
    }
}
