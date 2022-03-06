use super::alloc_api::*;
use super::CopyRange;
use alloc::alloc::{Layout, LayoutError};
use core::num::NonZeroUsize;
use core::ops::*;
use core::ptr::NonNull;

#[macro_export]
macro_rules! pod {
    ($elem:expr; $n:expr) => {{
        let n : usize = $n;
        let elem = $elem;

        let mut pod = $crate::Pod::with_capacity(n);
        pod.push_repeat(elem, n);

        pod
    }};

    ($elem:expr ; $n:expr ; $alloc:expr) => {{
        let n : usize = $n;
        let elem = $elem;

        let mut pod = $crate::Pod::with_allocator($alloc);
        pod.push_repeat(elem, n);

        pod
    }};

    ($($e:expr),* $(,)?) => {{
        let data = [ $( $e ),+ ];
        let mut pod = $crate::Pod::with_capacity(data.len());

        for value in data.into_iter() {
            pod.push(value);
        }

        pod
    }};

    ($($e:expr),* $(,)? ; $alloc:expr) => {{
        let data = [ $( $e ),+ ];
        let mut pod = $crate::Pod::with_allocator($alloc);

        pod.reserve(data.len());

        for value in data.into_iter() {
            pod.push(value);
        }

        pod
    }};
}

struct DataInfo {
    size: usize,
    align: usize,
}

// 2 purposes: Prevent monomorphization as much as possible, and allow for using
// the allocator API on stable.
pub struct Pod<T, A = Global>
where
    T: Copy,
    A: Allocator,
{
    raw: RawPod,
    allocator: A,
    phantom: core::marker::PhantomData<T>,
}

unsafe impl<T, A> Sync for Pod<T, A>
where
    T: Copy + Sync,
    A: Allocator,
{
}

unsafe impl<T, A> Send for Pod<T, A>
where
    T: Copy + Send,
    A: Allocator + Send,
{
}

impl<T> Pod<T, Global>
where
    T: Copy,
{
    #[inline(always)]
    pub fn new() -> Self {
        return Self::with_allocator(Global);
    }

    pub fn with_capacity(capacity: usize) -> Self {
        let mut s = Self::new();
        s.raw.realloc(&Global, capacity);

        return s;
    }
}

impl<T, A> Pod<T, A>
where
    T: Copy,
    A: Allocator,
{
    pub fn with_allocator(allocator: A) -> Self {
        let info = DataInfo {
            size: core::mem::size_of::<T>(),
            align: core::mem::align_of::<T>(),
        };

        return Self {
            raw: RawPod::new(info),
            allocator,
            phantom: core::marker::PhantomData,
        };
    }

    #[inline(always)]
    pub fn extend_from_slice(&mut self, data: &[T]) {
        let len = data.len();
        self.raw.reserve_additional(&self.allocator, len);

        let ptr = self.raw.ptr(self.raw.length) as *mut T;
        let to_space = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
        to_space.copy_from_slice(data);

        self.raw.length += len;
    }

    pub fn push(&mut self, t: T) {
        self.raw.reserve_additional(&self.allocator, 1);

        let ptr = self.raw.ptr(self.raw.length) as *mut T;
        self.raw.length += 1;

        unsafe { *ptr = t };
    }

    pub fn leak<'b>(self) -> &'b mut [T] {
        let len = self.raw.length;
        let ptr = self.raw.ptr(0) as *mut T;

        core::mem::forget(self);

        return unsafe { core::slice::from_raw_parts_mut(ptr, len) };
    }

    pub fn clear(&mut self) {
        self.raw.length = 0;
    }

    pub fn insert(&mut self, i: usize, value: T) {
        self.raw.reserve_additional(&self.allocator, 1);
        self.raw.length += 1;

        if self.raw.copy_range(i..self.raw.length, i + 1) {
            panic!("invalid position");
        }

        let ptr = self.raw.ptr(i) as *mut T;
        unsafe { *ptr = value };
    }

    pub fn splice(&mut self, range: impl RangeBounds<usize>, values: &[T]) {
        let range = self.raw.translate_range(range);
        let len = values.len();

        let ptr = self.raw.splice_ptr(&self.allocator, range, len) as *mut T;
        let slice = unsafe { core::slice::from_raw_parts_mut(ptr, len) };

        slice.copy_from_slice(values);
    }

    pub fn pop(&mut self) -> Option<T> {
        if self.raw.length == 0 {
            return None;
        }

        let ptr = self.raw.ptr(self.raw.length - 1) as *const T;
        self.raw.length -= 1;

        return Some(unsafe { *ptr });
    }

    pub fn remove(&mut self, i: usize) -> T {
        let value = self[i];

        self.raw.copy_range((i + 1)..self.raw.length, i);
        self.raw.length -= 1;

        return value;
    }

    pub fn push_repeat(&mut self, t: T, repeat: usize) {
        self.raw.reserve_additional(&self.allocator, repeat);

        let ptr = self.raw.ptr(self.raw.length) as *mut T;
        let data = unsafe { core::slice::from_raw_parts_mut(ptr, repeat) };
        data.fill(t);

        self.raw.length += repeat;
    }

    #[inline(always)]
    pub fn capacity(&self) -> usize {
        return self.raw.capacity;
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        return self.raw.length;
    }

    #[inline(always)]
    pub fn reserve(&mut self, additional: usize) {
        self.raw.reserve_additional(&self.allocator, additional);
    }

    #[inline(always)]
    pub fn shrink_to_fit(&mut self) {
        let len = self.raw.length;
        self.raw.realloc(&self.allocator, len);
    }

    fn u_ptr(&self, i: usize) -> NonNull<T> {
        match self.ptr(i) {
            Some(p) => return p,
            None => {
                panic!(
                    "index out of bounds: len={} but index={}",
                    self.raw.length, i
                );
            }
        }
    }

    fn u_slice(&self, r: Range<usize>) -> (*mut T, usize) {
        let (start, end) = (r.start, r.end);
        if !self.raw.range_is_valid(start, end) {
            panic!(
                "slice index out of bounds: len={} but slice index={}..{}",
                self.raw.length, r.start, r.end
            );
        }

        let data = self.raw.ptr(start);
        let len = end - start;

        return (data as *mut T, len);
    }

    #[inline(always)]
    fn uget(&self, i: usize) -> &T {
        let ptr = self.u_ptr(i);

        return unsafe { &*ptr.as_ptr() };
    }

    #[inline(always)]
    fn uget_mut(&mut self, i: usize) -> &mut T {
        let ptr = self.u_ptr(i);

        return unsafe { &mut *ptr.as_ptr() };
    }

    #[inline(always)]
    fn uget_slice(&self, r: Range<usize>) -> &[T] {
        let (ptr, len) = self.u_slice(r);

        return unsafe { core::slice::from_raw_parts(ptr, len) };
    }

    #[inline(always)]
    fn uget_mut_slice(&mut self, r: Range<usize>) -> &mut [T] {
        let (ptr, len) = self.u_slice(r);

        return unsafe { core::slice::from_raw_parts_mut(ptr, len) };
    }

    #[inline(always)]
    pub fn raw_ptr(&self, i: usize) -> Option<*mut T> {
        let data = self.raw.ptr(i);

        return Some(data as *mut T);
    }

    #[inline(always)]
    fn ptr(&self, i: usize) -> Option<NonNull<T>> {
        if i >= self.raw.length {
            return None;
        }

        let data = self.raw.ptr(i);

        return Some(unsafe { NonNull::new_unchecked(data as *mut T) });
    }

    fn slice(&self, r: Range<usize>) -> Option<(*mut T, usize)> {
        let (start, end) = (r.start, r.end);
        if !self.raw.range_is_valid(start, end) {
            return None;
        }

        let data = self.raw.ptr(start);
        let len = end - start;

        return Some((data as *mut T, len));
    }

    #[inline(always)]
    pub fn get(&self, i: usize) -> Option<&T> {
        let ptr = self.ptr(i)?;

        return Some(unsafe { &*ptr.as_ptr() });
    }

    #[inline(always)]
    pub fn get_mut(&mut self, i: usize) -> Option<&mut T> {
        let ptr = self.ptr(i)?;

        return Some(unsafe { &mut *ptr.as_ptr() });
    }

    #[inline(always)]
    pub fn get_slice(&self, r: Range<usize>) -> Option<&[T]> {
        let (ptr, len) = self.slice(r)?;

        return Some(unsafe { core::slice::from_raw_parts(ptr, len) });
    }

    #[inline(always)]
    pub fn get_mut_slice(&mut self, r: Range<usize>) -> Option<&mut [T]> {
        let (ptr, len) = self.slice(r)?;

        return Some(unsafe { core::slice::from_raw_parts_mut(ptr, len) });
    }
}

pub struct PodIter<T, A>
where
    T: Copy,
    A: Allocator,
{
    pod: Pod<T, A>,
    index: usize,
}

impl<T, A> Iterator for PodIter<T, A>
where
    T: Copy,
    A: Allocator,
{
    type Item = T;

    fn next(&mut self) -> Option<T> {
        let index = self.index;
        self.index += 1;

        let value = self.pod.get(index)?;

        return Some(*value);
    }
}

impl<T, A> IntoIterator for Pod<T, A>
where
    T: Copy,
    A: Allocator,
{
    type IntoIter = PodIter<T, A>;
    type Item = T;

    #[inline(always)]
    fn into_iter(self) -> Self::IntoIter {
        return PodIter {
            pod: self,
            index: 0,
        };
    }
}

impl<T, A> Drop for Pod<T, A>
where
    T: Copy,
    A: Allocator,
{
    #[inline(always)]
    fn drop(&mut self) {
        self.raw.realloc(&self.allocator, 0)
    }
}

impl<T, A> Clone for Pod<T, A>
where
    T: Copy,
    A: Allocator + Clone,
{
    fn clone(&self) -> Self {
        let mut other = Pod::with_allocator(self.allocator.clone());
        other.reserve(self.raw.length);
        other.raw.length = self.raw.length;

        other.copy_from_slice(&*self);

        return other;
    }
}

impl<T, A> core::fmt::Debug for Pod<T, A>
where
    T: Copy + core::fmt::Debug,
    A: Allocator,
{
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        return f.debug_list().entries(self.iter()).finish();
    }
}

impl<T, E, A, B> core::cmp::PartialEq<Pod<E, B>> for Pod<T, A>
where
    T: Copy + core::cmp::PartialEq<E>,
    A: Allocator,
    E: Copy,
    B: Allocator,
{
    #[inline(always)]
    fn eq(&self, other: &Pod<E, B>) -> bool {
        return self.deref() == other.deref();
    }
}

impl<T, A> Deref for Pod<T, A>
where
    T: Copy,
    A: Allocator,
{
    type Target = [T];

    #[inline(always)]
    fn deref(&self) -> &[T] {
        return self.uget_slice(0..self.raw.length);
    }
}

impl<T, A> DerefMut for Pod<T, A>
where
    T: Copy,
    A: Allocator,
{
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut [T] {
        return self.uget_mut_slice(0..self.raw.length);
    }
}

impl<T, A> Index<u32> for Pod<T, A>
where
    T: Copy,
    A: Allocator,
{
    type Output = T;

    #[inline(always)]
    fn index(&self, i: u32) -> &T {
        return self.uget(i as usize);
    }
}

impl<T, A> IndexMut<u32> for Pod<T, A>
where
    T: Copy,
    A: Allocator,
{
    #[inline(always)]
    fn index_mut(&mut self, i: u32) -> &mut T {
        return self.uget_mut(i as usize);
    }
}

impl<T, A> Index<usize> for Pod<T, A>
where
    T: Copy,
    A: Allocator,
{
    type Output = T;

    #[inline(always)]
    fn index(&self, i: usize) -> &T {
        return self.uget(i);
    }
}

impl<T, A> IndexMut<usize> for Pod<T, A>
where
    T: Copy,
    A: Allocator,
{
    #[inline(always)]
    fn index_mut(&mut self, i: usize) -> &mut T {
        return self.uget_mut(i);
    }
}

impl<T, A> Index<CopyRange> for Pod<T, A>
where
    T: Copy,
    A: Allocator,
{
    type Output = [T];

    #[inline(always)]
    fn index(&self, i: CopyRange) -> &[T] {
        return self.uget_slice(i.start..i.end);
    }
}

impl<T, A> IndexMut<CopyRange> for Pod<T, A>
where
    T: Copy,
    A: Allocator,
{
    #[inline(always)]
    fn index_mut(&mut self, i: CopyRange) -> &mut [T] {
        return self.uget_mut_slice(i.start..i.end);
    }
}

impl<T, A> Index<RangeTo<usize>> for Pod<T, A>
where
    T: Copy,
    A: Allocator,
{
    type Output = [T];

    #[inline(always)]
    fn index(&self, i: RangeTo<usize>) -> &[T] {
        return self.uget_slice(0..i.end);
    }
}

impl<T, A> IndexMut<RangeTo<usize>> for Pod<T, A>
where
    T: Copy,
    A: Allocator,
{
    #[inline(always)]
    fn index_mut(&mut self, i: RangeTo<usize>) -> &mut [T] {
        return self.uget_mut_slice(0..i.end);
    }
}

impl<T, A> Index<RangeFrom<usize>> for Pod<T, A>
where
    T: Copy,
    A: Allocator,
{
    type Output = [T];

    #[inline(always)]
    fn index(&self, i: RangeFrom<usize>) -> &[T] {
        return self.uget_slice(i.start..self.raw.length);
    }
}

impl<T, A> IndexMut<RangeFrom<usize>> for Pod<T, A>
where
    T: Copy,
    A: Allocator,
{
    #[inline(always)]
    fn index_mut(&mut self, i: RangeFrom<usize>) -> &mut [T] {
        return self.uget_mut_slice(i.start..self.raw.length);
    }
}

impl<T, A> Index<RangeFull> for Pod<T, A>
where
    T: Copy,
    A: Allocator,
{
    type Output = [T];

    #[inline(always)]
    fn index(&self, i: RangeFull) -> &[T] {
        return self.uget_slice(0..self.raw.length);
    }
}

impl<T, A> IndexMut<RangeFull> for Pod<T, A>
where
    T: Copy,
    A: Allocator,
{
    #[inline(always)]
    fn index_mut(&mut self, i: RangeFull) -> &mut [T] {
        return self.uget_mut_slice(0..self.raw.length);
    }
}

impl<T, A> Index<Range<usize>> for Pod<T, A>
where
    T: Copy,
    A: Allocator,
{
    type Output = [T];

    #[inline(always)]
    fn index(&self, i: Range<usize>) -> &[T] {
        return self.uget_slice(i);
    }
}

impl<T, A> IndexMut<Range<usize>> for Pod<T, A>
where
    T: Copy,
    A: Allocator,
{
    #[inline(always)]
    fn index_mut(&mut self, i: Range<usize>) -> &mut [T] {
        return self.uget_mut_slice(i);
    }
}

// ----------------------------------------------------------------------------
//
//                               POD ARRAY UTILS
//
// ----------------------------------------------------------------------------

struct RawPod {
    data: NonNull<u8>,
    info: DataInfo,
    length: usize,
    capacity: usize,
}

impl RawPod {
    fn new(info: DataInfo) -> Self {
        // We use the same trick that std::vec::Vec uses
        return Self {
            data: NonNull::dangling(),
            info,
            length: 0,
            capacity: 0,
        };
    }

    #[inline(always)]
    fn range_is_valid(&self, start: usize, end: usize) -> bool {
        return start <= end && end <= self.length;
    }

    fn translate_range(&self, range: impl RangeBounds<usize>) -> Range<usize> {
        let start = match range.start_bound() {
            Bound::Included(s) => *s,
            Bound::Excluded(s) => *s + 1,
            Bound::Unbounded => 0,
        };

        let end = match range.end_bound() {
            Bound::Included(e) => *e + 1,
            Bound::Excluded(e) => *e,
            Bound::Unbounded => self.length,
        };

        return start..end;
    }

    #[inline(always)]
    fn ptr(&self, i: usize) -> *mut u8 {
        return unsafe { self.data.as_ptr().add(self.info.size * i) };
    }

    fn copy_range(&mut self, range: Range<usize>, to: usize) -> bool {
        let (start, end) = (range.start, range.end);

        if !self.range_is_valid(start, end) {
            return true;
        }

        let src = self.ptr(start);
        let dest = self.ptr(to);
        let copy_len = end - start;

        // Shift everything down to fill in that spot.
        unsafe { core::ptr::copy(src, dest, self.info.size * copy_len) };

        return false;
    }

    #[inline(always)]
    fn reserve_additional(&mut self, alloc: &dyn Allocator, additional: usize) {
        return self.reserve_total(alloc, self.length + additional);
    }

    fn reserve_total(&mut self, alloc: &dyn Allocator, needed: usize) {
        if needed <= self.capacity {
            return;
        }

        let new_capacity = core::cmp::max(needed, self.capacity * 3 / 2);
        self.realloc(alloc, new_capacity);
    }

    fn splice_ptr(&mut self, alloc: &dyn Allocator, range: Range<usize>, len: usize) -> *mut u8 {
        let (start, end) = (range.start, range.end);

        if !self.range_is_valid(start, end) {
            panic!("invalid range");
        }

        let copy_target = start + len;
        let range_to_copy = end..self.length;
        let final_len = copy_target + range_to_copy.len();
        self.reserve_total(alloc, final_len);

        self.copy_range(range_to_copy, copy_target);
        self.length = final_len;

        return self.ptr(start);
    }

    fn realloc(&mut self, alloc: &dyn Allocator, elem_capacity: usize) {
        match self.try_realloc(alloc, elem_capacity) {
            Ok(()) => {}
            Err(e) => {
                panic!("failed to get layout for reallocation");
            }
        }
    }

    fn try_realloc(&mut self, alloc: &dyn Allocator, elem_capacity: usize) -> Result<(), ()> {
        let (size, align) = (self.info.size, self.info.align);
        let get_info = move |mut data: NonNull<[u8]>| -> (NonNull<u8>, usize) {
            let data = unsafe { data.as_mut() };
            let capacity = data.len() / size;
            let data = unsafe { NonNull::new_unchecked(data.as_mut_ptr()) };

            return (data, capacity);
        };

        // We use the same trick that std::vec::Vec uses
        let (data, capacity) = match (size * self.capacity, size * elem_capacity) {
            (x, y) if x == y => return Ok(()),
            (0, 0) => {
                self.capacity = elem_capacity;
                return Ok(());
            }

            (prev_size, 0) => {
                let layout = Layout::from_size_align(prev_size, align).map_err(|_| ())?;
                unsafe { alloc.deallocate(self.data, layout) };

                (NonNull::dangling(), elem_capacity)
            }

            (0, new_size) => {
                let layout = Layout::from_size_align(new_size, align).map_err(|_| ())?;
                let data = alloc.allocate(layout).map_err(|_| ())?;

                get_info(data)
            }

            (prev_size, new_size) => {
                let prev_layout = Layout::from_size_align(prev_size, align).map_err(|_| ())?;
                let new_layout = Layout::from_size_align(new_size, align).map_err(|_| ())?;

                let result = unsafe {
                    if new_size > prev_size {
                        alloc.grow(self.data, prev_layout, new_layout)
                    } else {
                        alloc.shrink(self.data, prev_layout, new_layout)
                    }
                };

                let data = result.map_err(|_| ())?;

                get_info(data)
            }
        };

        self.data = data;
        self.length = core::cmp::min(self.length, elem_capacity);
        self.capacity = elem_capacity;

        return Ok(());
    }

    fn with_capacity(info: DataInfo, alloc: &dyn Allocator, capacity: usize) -> Self {
        // We use the same trick that std::vec::Vec uses
        let mut s = Self::new(info);
        s.realloc(alloc, capacity);

        return s;
    }
}