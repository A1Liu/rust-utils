use crate::alloc_api::*;

type Ptr = *const ();

use os::*;

#[cfg(target_family = "unix")]
mod os {
    use super::*;

    pub unsafe fn map_region(base: Ptr, size: usize) -> Result<Ptr, AllocError> {
        let mut flags = libc::MAP_PRIVATE | libc::MAP_ANON;

        if !base.is_null() {
            flags |= libc::MAP_FIXED;
        }

        let protection = libc::PROT_WRITE | libc::PROT_READ;
        let result = libc::mmap(base as *mut _, size, protection, flags, -1, 0);

        if result == libc::MAP_FAILED {
            return Err(AllocError);
        }

        return Ok(result as Ptr);
    }

    pub unsafe fn unmap_region(base: Ptr, size: usize) -> Result<(), AllocError> {
        let result = libc::munmap(base as *mut _, size);

        if result == 0 {
            return Ok(());
        }

        return Err(AllocError);
    }
}

#[cfg(target_family = "windows")]
mod os {
    use super::*;

    pub unsafe fn map_region(base: Ptr, size: usize) -> Result<Ptr, AllocError> {
        use winapi::um::memoryapi::VirtualAlloc;
        use winapi::um::winnt::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, PVOID};

        let flags = MEM_COMMIT | MEM_RESERVE;
        let protection = PAGE_READWRITE;

        let allocation = VirtualAlloc(base as PVOID, size, flags, protection);

        if allocation.is_null() {
            return Err(AllocError);
        }

        Ok(allocation as Ptr)
    }

    pub unsafe fn unmap_region(base: Ptr, _size: usize) -> Result<(), AllocError> {
        use winapi::shared::minwindef::FALSE;
        use winapi::um::memoryapi::VirtualFree;
        use winapi::um::winnt::{MEM_RELEASE, PVOID};

        let result = VirtualFree(base as PVOID, 0, MEM_RELEASE);

        if result == FALSE {
            return Err(AllocError);
        }

        return Ok(());
    }
}

#[test]
fn mapping() {
    unsafe {
        let ptr = map_region(core::ptr::null(), 4096).unwrap();
        unmap_region(ptr, 4096).unwrap();
    }
}