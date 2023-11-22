use thiserror::Error;

/// A type that tracks the state of borrows for a [`GdCell`].
///
/// This state upholds these invariants:
/// - You can only take a shared borrow when there is no aliasing mutable borrow.
/// - You can only take a mutable borrow when there is neither an aliasing mutable borrow, nor a shared
/// borrow.
/// - You can only set a mutable borrow as non-aliasing when an aliasing mutable borrow exists.
/// - You can only unset a mutable borrow as non-aliasing when there is no aliasing mutable borrow and no
/// shared borrows.  
#[derive(Debug, Clone, PartialEq)]
pub struct BorrowState {
    /// The number of `&T` references that are tracked.
    shared_count: usize,
    /// The number of `&mut T` references that are tracked.
    mut_count: usize,
    /// The number of `&mut T` references that cannot be aliased.
    non_aliasing_count: usize,
    /// `true` if the borrow state has reached an erroneous or unreliable state.
    poisoned: bool,
}

impl BorrowState {
    /// Create a new borrow state representing no borrows.
    pub fn new() -> Self {
        Self {
            shared_count: 0,
            mut_count: 0,
            non_aliasing_count: 0,
            poisoned: false,
        }
    }

    /// Returns `true` if there may be an aliasing mutable reference.
    pub fn has_possibly_aliasing(&self) -> bool {
        let count = self.mut_count - self.non_aliasing_count;

        assert!(
            count <= 1,
            "there should never be more than 1 aliasing reference"
        );

        count == 1
    }

    pub fn has_shared_reference(&self) -> bool {
        self.shared_count > 0
    }

    /// Returns the number of tracked shared references.
    ///
    /// Any amount of shared references will prevent [`Self::increment_mut`] from succeeding.
    pub fn shared_count(&self) -> usize {
        self.shared_count
    }

    /// Returns the number of tracked mutable references.
    pub fn mut_count(&self) -> usize {
        self.mut_count
    }

    /// Returns the number of tracked mutable references that are known to not be aliasing.
    ///
    /// This is guaranteed to always either be equal to or one less than [`Self::mut_count`].
    pub fn non_aliasing_count(&self) -> usize {
        self.non_aliasing_count
    }

    /// Returns `true` if the state has reached an erroneous or unreliable state.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Set self as having reached an erroneous or unreliable state.
    ///
    /// Always returns [`BorrowStateErr::Poisoned`].
    fn poison(&mut self, err: impl Into<String>) -> Result<(), BorrowStateErr> {
        self.poisoned = true;

        Err(BorrowStateErr::Poisoned(err.into()))
    }

    fn ensure_not_poisoned(&self) -> Result<(), BorrowStateErr> {
        if self.is_poisoned() {
            return Err(BorrowStateErr::IsPoisoned);
        }

        Ok(())
    }

    fn ensure_can_ref(&self) -> Result<(), BorrowStateErr> {
        self.ensure_not_poisoned()?;

        if self.has_possibly_aliasing() {
            return Err(BorrowStateErr::HasAliasingRef);
        }

        Ok(())
    }

    fn ensure_can_mut_ref(&self) -> Result<(), BorrowStateErr> {
        self.ensure_not_poisoned()?;

        if self.has_possibly_aliasing() {
            return Err(BorrowStateErr::HasAliasingRef);
        }

        if self.shared_count != 0 {
            return Err(BorrowStateErr::HasSharedRef);
        }

        Ok(())
    }

    /// Track a new shared reference.
    ///
    /// Returns the new total number of shared references.
    ///
    /// This fails when:
    /// - There exists a possibly aliasing mutable reference.
    /// - There exist `usize::MAX` shared references.
    pub fn increment_shared(&mut self) -> Result<usize, BorrowStateErr> {
        self.ensure_not_poisoned()?;

        self.ensure_can_ref()?;

        self.shared_count = self
            .shared_count
            .checked_add(1)
            .ok_or("could not increment shared count")?;

        Ok(self.shared_count)
    }

    /// Untrack an existing shared reference.
    ///
    /// Returns the new total number of shared references.
    ///
    /// This fails when:
    /// - There are currently no tracked shared references.
    pub fn decrement_shared(&mut self) -> Result<usize, BorrowStateErr> {
        self.ensure_not_poisoned()?;

        if self.shared_count == 0 {
            return Err(BorrowStateErr::NoSharedRef);
        }

        if self.has_possibly_aliasing() {
            self.poison("shared reference tracked while aliasing mutable reference exists")?;
        }

        // We know `shared_count` isn't 0.
        self.shared_count -= 1;

        Ok(self.shared_count)
    }

    /// Track a new mutable reference.
    ///
    /// Returns the new total number of mutable references.
    ///
    /// This fails when:
    /// - There exists a possibly aliasing mutable reference.
    /// - There exists a shared reference.
    /// - There are `usize::MAX` tracked mutable references.
    ///
    /// Any amount of shared references will prevent [`Self::increment_non_aliasing`] from succeeding.
    pub fn increment_mut(&mut self) -> Result<usize, BorrowStateErr> {
        self.ensure_not_poisoned()?;

        self.ensure_can_mut_ref()?;

        self.mut_count = self
            .mut_count
            .checked_add(1)
            .ok_or("could not increment mut count")?;

        Ok(self.mut_count)
    }

    /// Untrack an existing mutable reference.
    ///
    /// Returns the new total number of mutable references.
    ///
    /// This fails when:
    /// - There are currently no mutable references.
    /// - There is a mutable reference, but it's guaranteed to be non-aliasing.
    pub fn decrement_mut(&mut self) -> Result<usize, BorrowStateErr> {
        self.ensure_not_poisoned()?;

        if self.mut_count == 0 {
            return Err(BorrowStateErr::NoMutRef);
        }

        if self.mut_count == self.non_aliasing_count {
            return Err(BorrowStateErr::IsNonAliasing);
        }

        if self.mut_count - 1 != self.non_aliasing_count {
            self.poison("`non_aliasing_count` does not fit its invariant")?;
        }

        // We know `mut_count` isn't 0.
        self.mut_count -= 1;

        Ok(self.mut_count)
    }

    /// Set the current mutable reference as non-aliasing.
    ///
    /// Returns the new total of non-aliasing mutable references.
    ///
    /// Fails when:
    /// - There is no current
    pub fn set_non_aliasing(&mut self) -> Result<usize, BorrowStateErr> {
        if !self.has_possibly_aliasing() {
            return Err(BorrowStateErr::NoAliasingRef);
        }

        self.non_aliasing_count = self
            .non_aliasing_count
            .checked_add(1)
            .ok_or("could not increment non-aliasing count")?;

        Ok(self.non_aliasing_count)
    }

    pub fn unset_non_aliasing(&mut self) -> Result<usize, BorrowStateErr> {
        if self.has_possibly_aliasing() {
            return Err(BorrowStateErr::HasAliasingRef);
        }

        if self.shared_count() > 0 {
            return Err(BorrowStateErr::HasSharedRef);
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

        Ok(self.non_aliasing_count)
    }
}

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum BorrowStateErr {
    #[error("expected a tracked shared reference")]
    NoSharedRef,
    #[error("expected no tracked shared references")]
    HasSharedRef,
    #[error("expected a tracked mutable reference")]
    NoMutRef,
    #[error("expected no tracked mutable references")]
    HasMutRef,
    #[error("expected the current borrow to not be a non-aliasing reference")]
    IsNonAliasing,
    #[error("expected a tracked non-aliasing mutable reference")]
    NoAliasingRef,
    #[error("expected no tracked non-aliasing mutable references")]
    HasAliasingRef,
    #[error("borrow state is poisoned and cannot continue")]
    IsPoisoned,
    #[error("borrow state encountered an unexpected state and was poisoned: {0}")]
    Poisoned(String),
    #[error("{0}")]
    Custom(String),
}

impl<'a> From<&'a str> for BorrowStateErr {
    fn from(value: &'a str) -> Self {
        Self::Custom(value.into())
    }
}

impl From<String> for BorrowStateErr {
    fn from(value: String) -> Self {
        Self::Custom(value)
    }
}

#[cfg(all(test, not(miri)))]
mod test {
    use super::*;
    use proptest::{collection::vec, prelude::*};
    use proptest_derive::Arbitrary;

    #[derive(Arbitrary, Debug, Clone, Copy, PartialEq, Eq)]
    enum Operation {
        IncShared,
        DecShared,
        IncMut,
        DecMut,
        SetNoAlias,
        UnsetNoAlias,
    }

    impl Operation {
        fn execute(&self, state: &mut BorrowState) -> Result<(), BorrowStateErr> {
            use Operation as Op;

            let result = match self {
                Op::IncShared => state.increment_shared(),
                Op::DecShared => state.decrement_shared(),
                Op::IncMut => state.increment_mut(),
                Op::DecMut => state.decrement_mut(),
                Op::SetNoAlias => state.set_non_aliasing(),
                Op::UnsetNoAlias => state.unset_non_aliasing(),
            };

            result.map(|_| ())
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct OperationExecutor {
        vec: Vec<Operation>,
    }

    impl OperationExecutor {
        fn execute_all(&self, state: &mut BorrowState) {
            for op in self.vec.iter() {
                _ = op.execute(state);
            }
        }

        fn remove_shared_inc_dec_pairs(mut self) -> Self {
            loop {
                let mut inc_index = None;
                let mut just_saw_inc = false;

                for (i, op) in self.vec.iter().enumerate() {
                    match op {
                        Operation::IncShared => just_saw_inc = true,
                        Operation::DecShared if just_saw_inc => {
                            inc_index = Some(i - 1);
                            break;
                        }
                        _ => just_saw_inc = false,
                    }
                }

                match inc_index {
                    Some(i) => {
                        self.vec.remove(i + 1);
                        self.vec.remove(i);
                    }
                    None => break,
                }
            }

            self
        }
    }

    impl From<Vec<Operation>> for OperationExecutor {
        fn from(vec: Vec<Operation>) -> Self {
            Self { vec }
        }
    }

    prop_compose! {
        fn arbitrary_ops(max_len: usize)(len in 0..max_len)(operations in vec(any::<Operation>(), len)) -> Vec<Operation> {
            operations
        }
    }

    proptest! {
        #[test]
        fn operations_do_only_whats_expected_or_nothing(operations in arbitrary_ops(50)) {
            use Operation as Op;
            let mut state = BorrowState::new();
            for op in operations {
                let expected_on_success = match op {
                    Op::IncShared => |mut original: BorrowState| {
                        original.shared_count += 1;
                        original
                    },
                    Op::DecShared => |mut original: BorrowState| {
                        original.shared_count -= 1;
                        original
                    },
                    Op::IncMut => |mut original: BorrowState| {
                        original.mut_count += 1;
                        original
                    },
                    Op::DecMut => |mut original: BorrowState| {
                        original.mut_count -= 1;
                        original
                    },
                    Op::SetNoAlias => |mut original: BorrowState| {
                        original.non_aliasing_count += 1;
                        original
                    },
                    Op::UnsetNoAlias => |mut original: BorrowState| {
                        original.non_aliasing_count -= 1;
                        original
                    },
                };

                let original = state.clone();
                if op.execute(&mut state).is_ok() {
                    assert_eq!(state, expected_on_success(original));
                } else {
                    assert_eq!(state, original);
                }
            }
        }
    }

    proptest! {
        #[test]
        fn no_poison(operations in arbitrary_ops(50)) {
            let mut state = BorrowState::new();
            for op in operations {
                if let Err(err) = op.execute(&mut state) {
                    assert_ne!(err, BorrowStateErr::IsPoisoned);
                    assert!(!matches!(err, BorrowStateErr::Poisoned(_)));
                }

                assert!(!state.is_poisoned());
            }
        }
    }

    proptest! {
        #[test]
        fn no_shared_and_mut(operations in arbitrary_ops(50)) {
            let mut state = BorrowState::new();
            for op in operations {
                _ = op.execute(&mut state);
                if state.has_shared_reference() {
                    assert!(!state.has_possibly_aliasing())
                }
            }
        }
    }

    proptest! {
        #[test]
        fn can_borrow_shared_when_borrowed_shared(operations in arbitrary_ops(50)) {
            let mut state = BorrowState::new();

            for op in operations {
                _ = op.execute(&mut state);
                if state.has_shared_reference() {
                    assert!(state.increment_shared().is_ok());
                    assert!(state.decrement_shared().is_ok());
                }
            }
        }
    }

    proptest! {
        #[test]
        fn cannot_borrow_shared_when_borrowed_aliasing(operations in arbitrary_ops(50)) {
            let mut state = BorrowState::new();

            for op in operations {
                _ = op.execute(&mut state);
                if state.has_possibly_aliasing() {
                    assert!(state.increment_shared().is_err());
                }
            }
        }
    }

    proptest! {
        #[test]
        fn can_borrow_shared_when_not_borrowed_aliasing(operations in arbitrary_ops(50)) {
            let mut state = BorrowState::new();

            for op in operations {
                _ = op.execute(&mut state);
                if !state.has_possibly_aliasing() {
                    assert!(state.increment_shared().is_ok());
                    assert!(state.decrement_shared().is_ok());
                }
            }
        }
    }

    proptest! {
        #[test]
        fn can_borrow_mut_when_no_shared_and_no_aliasing(operations in arbitrary_ops(50)) {
            let mut state = BorrowState::new();

            for op in operations {
                _ = op.execute(&mut state);
                if !state.has_possibly_aliasing() && !state.has_shared_reference() {
                    assert!(state.increment_mut().is_ok());
                    assert!(state.decrement_mut().is_ok());
                }
            }
        }
    }

    proptest! {
        #[test]
        fn cannot_borrow_mut_when_shared(operations in arbitrary_ops(50)) {
            let mut state = BorrowState::new();

            for op in operations {
                _ = op.execute(&mut state);
                if state.has_shared_reference() {
                    assert!(state.increment_mut().is_err());
                }
            }
        }
    }

    proptest! {
        #[test]
        fn cannot_borrow_mut_when_has_aliasing(operations in arbitrary_ops(50)) {
            let mut state = BorrowState::new();

            for op in operations {
                _ = op.execute(&mut state);
                if state.has_possibly_aliasing() {
                    assert!(state.increment_mut().is_err());
                }
            }
        }
    }

    proptest! {
        #[test]
        fn can_set_nonaliasing_when_aliasing(operations in arbitrary_ops(50)) {
            let mut state = BorrowState::new();

            for op in operations {
                _ = op.execute(&mut state);
                if state.has_possibly_aliasing() {
                    assert!(state.set_non_aliasing().is_ok());
                    assert!(state.unset_non_aliasing().is_ok());
                }
            }
        }
    }

    proptest! {
        #[test]
        fn cannot_set_nonaliasing_when_shared(operations in arbitrary_ops(50)) {
            let mut state = BorrowState::new();

            for op in operations {
                _ = op.execute(&mut state);
                if state.has_shared_reference() {
                    assert!(state.set_non_aliasing().is_err());
                }
            }
        }
    }

    proptest! {
        #[test]
        fn cannot_set_nonaliasing_when_nonaliasing(operations in arbitrary_ops(50)) {
            let mut state = BorrowState::new();

            for op in operations {
                _ = op.execute(&mut state);
                if !state.has_possibly_aliasing() {
                    assert!(state.set_non_aliasing().is_err());
                }
            }
        }
    }

    proptest! {
        #[test]
        fn remove_shared_inc_dec_pairs_is_noop(operations in arbitrary_ops(50)) {
            let mut state_all = BorrowState::new();
            let executor_all = OperationExecutor::from(operations);
            executor_all.execute_all(&mut state_all);

            let mut state_no_shared_pairs = BorrowState::new();
            let executor_no_shared_pairs = executor_all.clone().remove_shared_inc_dec_pairs();
            executor_no_shared_pairs.execute_all(&mut state_no_shared_pairs);

            assert_eq!(state_all, state_no_shared_pairs);
        }
    }

    #[test]
    fn poisoned_unset_shared_ref() {
        let mut state = BorrowState::new();
        assert!(!state.is_poisoned());

        _ = state.increment_mut();
        assert!(!state.is_poisoned());
        _ = state.set_non_aliasing();
        assert!(!state.is_poisoned());
        _ = state.increment_shared();
        assert!(!state.is_poisoned());
        _ = state.unset_non_aliasing();
        assert!(!state.is_poisoned());
        _ = state.increment_shared();
        assert!(!state.is_poisoned());
        _ = state.decrement_shared();
        assert!(!state.is_poisoned());
    }
}
