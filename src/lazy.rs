use std::cell::UnsafeCell;

pub struct ResettableLazy<T, F: Fn() -> T = fn() -> T> {
    computer: F,
    value: UnsafeCell<Option<T>>,
}

impl<T, F: Fn() -> T> ResettableLazy<T, F> {
    pub fn new(computer: F) -> Self {
        Self {
            computer,
            value: UnsafeCell::new(None),
        }
    }
}

impl<T, F: Fn() -> T> ResettableLazy<T, F> {
    pub fn set(&mut self, value: T) {
        *self.value.get_mut() = Some(value);
    }

    pub fn take(&mut self) -> Option<T> {
        std::mem::take(self.value.get_mut())
    }
}

impl<T, F: Fn() -> T> std::ops::Deref for ResettableLazy<T, F> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        if let Some(value) = unsafe { &*self.value.get() } {
            value
        } else {
            &*(unsafe { &mut *self.value.get() }.insert((self.computer)()))
        }
    }
}

impl<T, F: Fn() -> T> std::ops::DerefMut for ResettableLazy<T, F> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        let value = self.value.get_mut();
        value.get_or_insert_with(&self.computer)
    }
}
