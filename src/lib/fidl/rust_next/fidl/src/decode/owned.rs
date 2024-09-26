// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::fmt;
use core::marker::PhantomData;
use core::mem::forget;
use core::ops::{Deref, DerefMut};
use core::ptr::NonNull;

use crate::Chunk;

/// An owned value in borrowed backing memory.
pub struct Owned<'buf, T: ?Sized> {
    ptr: NonNull<T>,
    _phantom: PhantomData<&'buf mut [Chunk]>,
}

impl<T: ?Sized> Drop for Owned<'_, T> {
    fn drop(&mut self) {
        unsafe {
            self.ptr.as_ptr().drop_in_place();
        }
    }
}

impl<T: ?Sized> Owned<'_, T> {
    /// Returns an `Owned` of the given pointer.
    ///
    /// # Safety
    ///
    /// `new_unchecked` takes ownership of the pointed-to value. It must point
    /// to a valid value that is not aliased.
    pub unsafe fn new_unchecked(ptr: *mut T) -> Self {
        Self { ptr: unsafe { NonNull::new_unchecked(ptr) }, _phantom: PhantomData }
    }

    /// Returns the owned value.
    pub fn take(self) -> T
    where
        T: Sized,
    {
        unsafe { self.into_raw().read() }
    }

    /// Consumes the `Owned`, returning its pointer to the owned value.
    pub fn into_raw(self) -> *mut T {
        let result = self.ptr.as_ptr();
        forget(self);
        result
    }
}

impl<T: ?Sized> Deref for Owned<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { self.ptr.as_ref() }
    }
}

impl<T: ?Sized> DerefMut for Owned<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.ptr.as_mut() }
    }
}

impl<T: fmt::Debug + ?Sized> fmt::Debug for Owned<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.deref().fmt(f)
    }
}
