mod borrow_state;
mod guards;

use std::{
    cell::UnsafeCell, error::Error, marker::PhantomPinned, pin::Pin, ptr::NonNull, sync::Mutex,
};

use borrow_state::BorrowState;
pub use guards::{GdMut, GdRef, NonAliasingGuard};

#[derive(Debug)]
pub struct GdCell<T> {
    state: Mutex<BorrowState>,
    value: UnsafeCell<T>,
    current_ptr: Mutex<Vec<NonNull<T>>>,
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

    pub fn gd_ref(self: Pin<&Self>) -> Result<GdRef<'_, T>, Box<dyn Error>> {
        self.state.lock().unwrap().increment_shared()?;

        // SAFETY:
        // `increment_shared` succeeded, therefore there cannot currently be any aliasing mutable references.
        unsafe { Ok(GdRef::new(&self.get_ref().state, self.get_value())) }
    }

    pub fn gd_mut(self: Pin<&Self>) -> Result<GdMut<'_, T>, Box<dyn Error>> {
        let mut guard = self.state.lock().unwrap();
        guard.increment_mut()?;
        let count = guard.mut_count();

        // SAFETY:
        // `increment_mut` succeeded, therefore any existing mutable references do not alias, and no new
        // references may be made unless this one is guaranteed not to alias those.
        //
        // This is the case because the only way for a new `GdMut` or `GdRef` to be made after this, then
        // either this guard has to be dropped or `set_non_aliasing` must be called.
        //
        // If this guard is dropped, then we dont need to worry.
        //
        // If `set_non_aliasing` is called, then either a mutable reference from this guard is passed in.
        // In which case, we cannot use this guard again until the resulting non-aliasing guard is dropped.
        //
        // We cannot pass in a different mutable reference, since `set_non_aliasing` ensures any references
        // matches the ones this one would return. And only one mutable reference to the same value can exist
        // since we cannot have any other aliasing mutable references around to pass in.
        unsafe { Ok(GdMut::new(&self.get_ref().state, count, self.get_value())) }
    }

    fn get_value(self: Pin<&Self>) -> NonNull<T> {
        let mut ptr_vec = self.current_ptr.lock().unwrap();

        if ptr_vec.is_empty() {
            ptr_vec.push(NonNull::new(self.value.get()).unwrap())
        }

        *ptr_vec.last().unwrap()
    }

    /// Set the current mutable borrow as not aliasing any other references.
    ///
    /// Will error if there is no current possibly aliasing mutable borrow.
    pub fn set_non_aliasing<'a, 'b>(
        self: Pin<&'a Self>,
        current_ref: &'b mut T,
    ) -> Result<NonAliasingGuard<'b, T>, Box<dyn Error>>
    where
        'a: 'b,
    {
        let mut current_ptr_vec = self.current_ptr.lock().unwrap();
        let current_ptr = *current_ptr_vec.last().unwrap();
        let ptr = NonNull::from(current_ref);

        if current_ptr != ptr {
            // it is likely not unsound for this to happen, but it's unexpected
            return Err("wrong reference passed in".into());
        }

        let mut state_guard = self.state.lock().unwrap();
        state_guard.set_non_aliasing()?;
        current_ptr_vec.push(ptr);
        drop(state_guard);
        drop(current_ptr_vec);

        Ok(NonAliasingGuard::new(
            &self.get_ref().state,
            &self.get_ref().current_ptr,
        ))
    }

    pub fn is_currently_bound(self: Pin<&Self>) -> bool {
        let guard = self.state.lock().unwrap();

        guard.shared_count() > 0 || guard.mut_count() > 0
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
