// Copyright (c) 2022 by Rivos Inc.
// Licensed under the Apache License, Version 2.0, see LICENSE for details.
// SPDX-License-Identifier: Apache-2.0

use core::marker::PhantomData;

use crate::page::{Page, PageAddr, PageSize, PhysAddr};

/// `SequentialPages` holds a range of consecutive pages. Each page's address is one page after the
/// previous. This forms a contiguous area of memory suitable for holding an array or other linear
/// data.
pub struct SequentialPages<S: PageSize> {
    addr: u64,
    count: u64,
    phantom: PhantomData<S>,
}

impl<S: PageSize> SequentialPages<S> {
    /// Creates a `SequentialPages` with no pages.
    fn empty() -> Self {
        SequentialPages {
            addr: 0,
            count: 0,
            phantom: PhantomData,
        }
    }

    /// Creates a `SequentialPages` form the passed iterator.
    ///
    /// If the passed pages are not consecutive, an Error will be returned holding an iterator to
    /// the passed in pages so they don't leak.
    pub fn from_pages<T>(pages: T) -> core::result::Result<Self, impl Iterator<Item = Page<S>>>
    where
        T: IntoIterator<Item = Page<S>>,
    {
        let mut page_iter = pages.into_iter();

        let first_page = match page_iter.next() {
            Some(p) => p,
            None => {
                // This is a complicated way of returning an empty iterator that matches the type
                // signature of the other `return Err(...)` statements.
                return Err(Self::empty()
                    .into_iter()
                    .chain(create_dummy_page_once_iter())
                    .chain(page_iter));
            }
        };

        let addr = first_page.addr().bits();

        let mut last_addr = addr;
        let mut seq = Self {
            addr,
            count: 1,
            phantom: PhantomData,
        };
        while let Some(page) = page_iter.next() {
            let this_addr = page.addr().bits();
            let next_addr = match last_addr.checked_add(S::SIZE_BYTES) {
                Some(a) => a,
                None => {
                    return Err(seq
                        .into_iter()
                        .chain(core::iter::once(page))
                        .chain(page_iter))
                }
            };
            if this_addr != next_addr {
                return Err(seq
                    .into_iter()
                    .chain(core::iter::once(page))
                    .chain(page_iter));
            }
            last_addr = this_addr;
            seq.count += 1;
        }

        Ok(seq)
    }

    /// Returns the address of the first page in the sequence(the start of the contiguous memory
    /// region).
    pub fn base(&self) -> u64 {
        self.addr
    }

    /// Returns the length of the contiguous memory region formed by the owned pages.
    pub fn length_bytes(&self) -> u64 {
        // Guaranteed not to overflow by the constructor.
        self.count * S::SIZE_BYTES
    }

    /// Returns `SequentialPages` for the memory range provided.
    /// # Safety
    /// The range's ownership is given to `SequentialPages`, the caller must uniquely own that
    /// memory.
    pub unsafe fn from_mem_range(start: PageAddr<S>, count: u64) -> Self {
        Self {
            addr: start.bits(),
            count,
            phantom: PhantomData,
        }
    }
}

impl<S: PageSize> From<Page<S>> for SequentialPages<S> {
    fn from(p: Page<S>) -> Self {
        Self {
            addr: p.addr().bits(),
            count: 1,
            phantom: PhantomData,
        }
    }
}

/// An iterator of the individual pages previously used to build a `SequentialPages` struct.
/// Used to reclaim the pages from `SequentialPages`, returned from `SequentialPages::into_iter`.
pub struct SeqPageIter<S: PageSize> {
    pages: SequentialPages<S>,
}

impl<S: PageSize> Iterator for SeqPageIter<S> {
    type Item = Page<S>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pages.count == 0 {
            return None;
        }
        let addr = self.pages.addr;
        self.pages.addr += S::SIZE_BYTES;
        self.pages.count -= 1;
        // Safe because `pages` owns the memory, which can be converted to pages because it is owned
        // and aligned.
        unsafe { Some(Page::new(PageAddr::new(PhysAddr::new(addr))?)) }
    }
}

impl<S: PageSize> IntoIterator for SequentialPages<S> {
    type Item = Page<S>;
    type IntoIter = SeqPageIter<S>;
    fn into_iter(self) -> Self::IntoIter {
        SeqPageIter { pages: self }
    }
}

// Helper function that returns a `Once` iterator that actually yields `None`.
fn create_dummy_page_once_iter<S: PageSize>() -> core::iter::Once<Page<S>> {
    let dummy_page = unsafe { Page::new(PageAddr::new(PhysAddr::new(0)).unwrap()) };
    let mut dummy_iter = core::iter::once(dummy_page);
    core::mem::forget(dummy_iter.next());
    dummy_iter
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Page4k;
    use crate::PageSize4k;

    #[test]
    fn create_success() {
        let pages = unsafe {
            // Not safe, but memory won't be touched in the test...
            [
                Page4k::new(PageAddr::new(PhysAddr::new(0x1000)).unwrap()),
                Page4k::new(PageAddr::new(PhysAddr::new(0x2000)).unwrap()),
                Page4k::new(PageAddr::new(PhysAddr::new(0x3000)).unwrap()),
                Page4k::new(PageAddr::new(PhysAddr::new(0x4000)).unwrap()),
            ]
        };

        assert!(SequentialPages::from_pages(pages).is_ok());
    }

    #[test]
    fn create_failure() {
        let pages = unsafe {
            // Not safe, but memory won't be touched in the test...
            [
                Page4k::new(PageAddr::new(PhysAddr::new(0x1000)).unwrap()),
                Page4k::new(PageAddr::new(PhysAddr::new(0x2000)).unwrap()),
                Page4k::new(PageAddr::new(PhysAddr::new(0x4000)).unwrap()),
                Page4k::new(PageAddr::new(PhysAddr::new(0x5000)).unwrap()),
            ]
        };
        let result = SequentialPages::from_pages(pages);
        match result {
            Ok(_) => panic!("didn't fail with non-sequential pages"),
            Err(returned_pages) => {
                assert_eq!(returned_pages.count(), 4);
            }
        }
    }

    #[test]
    fn create_fail_empty() {
        let pages: [Page4k; 0] = [];
        let result = SequentialPages::from_pages(pages);
        match result {
            Ok(_) => panic!("didn't fail with empty pages"),
            Err(mut returned_pages) => assert!(returned_pages.next().is_none()),
        }
    }

    #[test]
    fn from_single() {
        let p = unsafe {
            // Not safe, Just a test.
            Page4k::new(PageAddr::new(PhysAddr::new(0x1000)).unwrap())
        };
        let seq = SequentialPages::from(p);
        let mut pages = seq.into_iter();
        assert_eq!(0x1000, pages.next().unwrap().addr().bits());
        assert!(pages.next().is_none());
    }

    #[test]
    fn unsafe_range() {
        // Not safe, but this is a test
        let seq = unsafe {
            SequentialPages::<PageSize4k>::from_mem_range(
                PageAddr::new(PhysAddr::new(0x1000)).unwrap(),
                4,
            )
        };
        let mut pages = seq.into_iter();
        assert_eq!(0x1000, pages.next().unwrap().addr().bits());
        assert_eq!(0x2000, pages.next().unwrap().addr().bits());
        assert_eq!(0x3000, pages.next().unwrap().addr().bits());
        assert_eq!(0x4000, pages.next().unwrap().addr().bits());
        assert!(pages.next().is_none());
    }
}