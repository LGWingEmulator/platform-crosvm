// Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The mmap module provides a safe interface to mmap memory and ensures unmap is called when the
//! mmap object leaves scope.

use std;
use std::fmt::{self, Display};
use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::ptr::null_mut;

use data_model::volatile_memory::*;
use data_model::DataInit;
use libc::{self, c_int};

use crate::errno;

#[derive(Debug)]
pub enum Error {
    /// Requested memory out of range.
    InvalidAddress,
    /// Requested offset is out of range of `libc::off_t`.
    InvalidOffset,
    /// Requested memory range spans past the end of the region.
    InvalidRange(usize, usize),
    /// Couldn't read from the given source.
    ReadFromSource(std::io::Error),
    /// `mmap` returned the given error.
    SystemCallFailed(errno::Error),
    /// Writing to memory failed
    WriteToMemory(std::io::Error),
    /// Reading from memory failed
    ReadFromMemory(std::io::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use self::Error::*;

        match self {
            InvalidAddress => write!(f, "requested memory out of range"),
            InvalidOffset => write!(f, "requested offset is out of range of off_t"),
            InvalidRange(offset, count) => write!(
                f,
                "requested memory range spans past the end of the region: offset={} count={}",
                offset, count,
            ),
            ReadFromSource(e) => write!(f, "failed to read from the given source: {}", e),
            SystemCallFailed(e) => write!(f, "mmap system call failed: {}", e),
            WriteToMemory(e) => write!(f, "failed to write to memory: {}", e),
            ReadFromMemory(e) => write!(f, "failed to read from memory: {}", e),
        }
    }
}

/// Memory access type for anonymous shared memory mapping.
#[derive(Copy, Clone, Eq, PartialEq)]
pub struct Protection(c_int);
impl Protection {
    /// Returns Protection allowing no access.
    #[inline(always)]
    pub fn none() -> Protection {
        Protection(libc::PROT_NONE)
    }

    /// Returns Protection allowing read/write access.
    #[inline(always)]
    pub fn read_write() -> Protection {
        Protection(libc::PROT_READ | libc::PROT_WRITE)
    }

    /// Set read events.
    #[inline(always)]
    pub fn set_read(self) -> Protection {
        Protection(self.0 | libc::PROT_READ)
    }

    /// Set write events.
    #[inline(always)]
    pub fn set_write(self) -> Protection {
        Protection(self.0 | libc::PROT_WRITE)
    }
}

impl From<c_int> for Protection {
    fn from(f: c_int) -> Self {
        Protection(f)
    }
}

impl Into<c_int> for Protection {
    fn into(self) -> c_int {
        self.0
    }
}

/// Wraps an anonymous shared memory mapping in the current process.
#[derive(Debug)]
pub struct MemoryMapping {
    addr: *mut u8,
    size: usize,
}

// Send and Sync aren't automatically inherited for the raw address pointer.
// Accessing that pointer is only done through the stateless interface which
// allows the object to be shared by multiple threads without a decrease in
// safety.
unsafe impl Send for MemoryMapping {}
unsafe impl Sync for MemoryMapping {}

impl MemoryMapping {
    /// Creates an anonymous shared, read/write mapping of `size` bytes.
    ///
    /// # Arguments
    /// * `size` - Size of memory region in bytes.
    pub fn new(size: usize) -> Result<MemoryMapping> {
        Self::new_protection(size, Protection::read_write())
    }

    /// Creates an anonymous shared mapping of `size` bytes with `prot` protection.
    ///
    /// # Arguments
    /// * `size` - Size of memory region in bytes.
    /// * `prot` - Protection (e.g. readable/writable) of the memory region.
    pub fn new_protection(size: usize, prot: Protection) -> Result<MemoryMapping> {
        // This is safe because we are creating an anonymous mapping in a place not already used by
        // any other area in this process.
        let addr = unsafe {
            libc::mmap(
                null_mut(),
                size,
                prot.into(),
                libc::MAP_ANONYMOUS | libc::MAP_SHARED | libc::MAP_NORESERVE,
                -1,
                0,
            )
        };
        if addr == libc::MAP_FAILED {
            return Err(Error::SystemCallFailed(errno::Error::last()));
        }
        // This is safe because we call madvise with a valid address and size, and we check the
        // return value. We only warn about an error because failure here is not fatal to the mmap.
        if unsafe { libc::madvise(addr, size, libc::MADV_DONTDUMP) } == -1 {
            warn!(
                "failed madvise(MADV_DONTDUMP) on mmap: {}",
                errno::Error::last()
            );
        }
        Ok(MemoryMapping {
            addr: addr as *mut u8,
            size,
        })
    }

    /// Maps the first `size` bytes of the given `fd`.
    ///
    /// # Arguments
    /// * `fd` - File descriptor to mmap from.
    /// * `size` - Size of memory region in bytes.
    pub fn from_fd(fd: &AsRawFd, size: usize) -> Result<MemoryMapping> {
        MemoryMapping::from_fd_offset(fd, size, 0)
    }

    /// Maps the `size` bytes starting at `offset` bytes of the given `fd`.
    ///
    /// # Arguments
    /// * `fd` - File descriptor to mmap from.
    /// * `size` - Size of memory region in bytes.
    /// * `offset` - Offset in bytes from the beginning of `fd` to start the mmap.
    pub fn from_fd_offset(fd: &AsRawFd, size: usize, offset: usize) -> Result<MemoryMapping> {
        if offset > libc::off_t::max_value() as usize {
            return Err(Error::InvalidOffset);
        }
        // This is safe because we are creating a mapping in a place not already used by any other
        // area in this process.
        let addr = unsafe {
            libc::mmap(
                null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                offset as libc::off_t,
            )
        };
        if addr == libc::MAP_FAILED {
            return Err(Error::SystemCallFailed(errno::Error::last()));
        }
        // This is safe because we call madvise with a valid address and size, and we check the
        // return value. We only warn about an error because failure here is not fatal to the mmap.
        if unsafe { libc::madvise(addr, size, libc::MADV_DONTDUMP) } == -1 {
            warn!(
                "failed madvise(MADV_DONTDUMP) on mmap: {}",
                errno::Error::last()
            );
        }
        Ok(MemoryMapping {
            addr: addr as *mut u8,
            size,
        })
    }

    /// Returns a pointer to the begining of the memory region.  Should only be
    /// used for passing this region to ioctls for setting guest memory.
    pub fn as_ptr(&self) -> *mut u8 {
        self.addr
    }

    /// Returns the size of the memory region in bytes.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Writes a slice to the memory region at the specified offset.
    /// Returns the number of bytes written.  The number of bytes written can
    /// be less than the length of the slice if there isn't enough room in the
    /// memory region.
    ///
    /// # Examples
    /// * Write a slice at offset 256.
    ///
    /// ```
    /// #   use sys_util::MemoryMapping;
    /// #   let mut mem_map = MemoryMapping::new(1024).unwrap();
    ///     let res = mem_map.write_slice(&[1,2,3,4,5], 256);
    ///     assert!(res.is_ok());
    ///     assert_eq!(res.unwrap(), 5);
    /// ```
    pub fn write_slice(&self, buf: &[u8], offset: usize) -> Result<usize> {
        if offset >= self.size {
            return Err(Error::InvalidAddress);
        }
        unsafe {
            // Guest memory can't strictly be modeled as a slice because it is
            // volatile.  Writing to it with what compiles down to a memcpy
            // won't hurt anything as long as we get the bounds checks right.
            let mut slice: &mut [u8] = &mut self.as_mut_slice()[offset..];
            Ok(slice.write(buf).map_err(Error::WriteToMemory)?)
        }
    }

    /// Reads to a slice from the memory region at the specified offset.
    /// Returns the number of bytes read.  The number of bytes read can
    /// be less than the length of the slice if there isn't enough room in the
    /// memory region.
    ///
    /// # Examples
    /// * Read a slice of size 16 at offset 256.
    ///
    /// ```
    /// #   use sys_util::MemoryMapping;
    /// #   let mut mem_map = MemoryMapping::new(1024).unwrap();
    ///     let buf = &mut [0u8; 16];
    ///     let res = mem_map.read_slice(buf, 256);
    ///     assert!(res.is_ok());
    ///     assert_eq!(res.unwrap(), 16);
    /// ```
    pub fn read_slice(&self, mut buf: &mut [u8], offset: usize) -> Result<usize> {
        if offset >= self.size {
            return Err(Error::InvalidAddress);
        }
        unsafe {
            // Guest memory can't strictly be modeled as a slice because it is
            // volatile.  Writing to it with what compiles down to a memcpy
            // won't hurt anything as long as we get the bounds checks right.
            let slice: &[u8] = &self.as_slice()[offset..];
            Ok(buf.write(slice).map_err(Error::ReadFromMemory)?)
        }
    }

    /// Writes an object to the memory region at the specified offset.
    /// Returns Ok(()) if the object fits, or Err if it extends past the end.
    ///
    /// # Examples
    /// * Write a u64 at offset 16.
    ///
    /// ```
    /// #   use sys_util::MemoryMapping;
    /// #   let mut mem_map = MemoryMapping::new(1024).unwrap();
    ///     let res = mem_map.write_obj(55u64, 16);
    ///     assert!(res.is_ok());
    /// ```
    pub fn write_obj<T: DataInit>(&self, val: T, offset: usize) -> Result<()> {
        unsafe {
            // Guest memory can't strictly be modeled as a slice because it is
            // volatile.  Writing to it with what compiles down to a memcpy
            // won't hurt anything as long as we get the bounds checks right.
            self.range_end(offset, std::mem::size_of::<T>())?;
            std::ptr::write_volatile(&mut self.as_mut_slice()[offset..] as *mut _ as *mut T, val);
            Ok(())
        }
    }

    /// Reads on object from the memory region at the given offset.
    /// Reading from a volatile area isn't strictly safe as it could change
    /// mid-read.  However, as long as the type T is plain old data and can
    /// handle random initialization, everything will be OK.
    ///
    /// # Examples
    /// * Read a u64 written to offset 32.
    ///
    /// ```
    /// #   use sys_util::MemoryMapping;
    /// #   let mut mem_map = MemoryMapping::new(1024).unwrap();
    ///     let res = mem_map.write_obj(55u64, 32);
    ///     assert!(res.is_ok());
    ///     let num: u64 = mem_map.read_obj(32).unwrap();
    ///     assert_eq!(55, num);
    /// ```
    pub fn read_obj<T: DataInit>(&self, offset: usize) -> Result<T> {
        self.range_end(offset, std::mem::size_of::<T>())?;
        unsafe {
            // This is safe because by definition Copy types can have their bits
            // set arbitrarily and still be valid.
            Ok(std::ptr::read_volatile(
                &self.as_slice()[offset..] as *const _ as *const T,
            ))
        }
    }

    /// Reads data from a readable object like a File and writes it to guest memory.
    ///
    /// # Arguments
    /// * `mem_offset` - Begin writing memory at this offset.
    /// * `src` - Read from `src` to memory.
    /// * `count` - Read `count` bytes from `src` to memory.
    ///
    /// # Examples
    ///
    /// * Read bytes from /dev/urandom
    ///
    /// ```
    /// # use sys_util::MemoryMapping;
    /// # use std::fs::File;
    /// # use std::path::Path;
    /// # fn test_read_random() -> Result<u32, ()> {
    /// #     let mut mem_map = MemoryMapping::new(1024).unwrap();
    ///       let mut file = File::open(Path::new("/dev/urandom")).map_err(|_| ())?;
    ///       mem_map.read_to_memory(32, &mut file, 128).map_err(|_| ())?;
    ///       let rand_val: u32 =  mem_map.read_obj(40).map_err(|_| ())?;
    /// #     Ok(rand_val)
    /// # }
    /// ```
    pub fn read_to_memory<F>(&self, mem_offset: usize, src: &mut F, count: usize) -> Result<()>
    where
        F: Read,
    {
        let mem_end = self
            .range_end(mem_offset, count)
            .map_err(|_| Error::InvalidRange(mem_offset, count))?;
        unsafe {
            // It is safe to overwrite the volatile memory.  Acessing the guest
            // memory as a mutable slice is OK because nothing assumes another
            // thread won't change what is loaded.
            let dst = &mut self.as_mut_slice()[mem_offset..mem_end];
            src.read_exact(dst).map_err(Error::ReadFromSource)?;
        }
        Ok(())
    }

    /// Writes data from memory to a writable object.
    ///
    /// # Arguments
    /// * `mem_offset` - Begin reading memory from this offset.
    /// * `dst` - Write from memory to `dst`.
    /// * `count` - Read `count` bytes from memory to `src`.
    ///
    /// # Examples
    ///
    /// * Write 128 bytes to /dev/null
    ///
    /// ```
    /// # use sys_util::MemoryMapping;
    /// # use std::fs::File;
    /// # use std::path::Path;
    /// # fn test_write_null() -> Result<(), ()> {
    /// #     let mut mem_map = MemoryMapping::new(1024).unwrap();
    ///       let mut file = File::open(Path::new("/dev/null")).map_err(|_| ())?;
    ///       mem_map.write_from_memory(32, &mut file, 128).map_err(|_| ())?;
    /// #     Ok(())
    /// # }
    /// ```
    pub fn write_from_memory<F>(&self, mem_offset: usize, dst: &mut F, count: usize) -> Result<()>
    where
        F: Write,
    {
        let mem_end = self
            .range_end(mem_offset, count)
            .map_err(|_| Error::InvalidRange(mem_offset, count))?;
        unsafe {
            // It is safe to read from volatile memory.  Acessing the guest
            // memory as a slice is OK because nothing assumes another thread
            // won't change what is loaded.
            let src = &self.as_mut_slice()[mem_offset..mem_end];
            dst.write_all(src).map_err(Error::ReadFromSource)?;
        }
        Ok(())
    }

    /// Uses madvise to tell the kernel to remove the specified range.  Subsequent reads
    /// to the pages in the range will return zero bytes.
    pub fn remove_range(&self, mem_offset: usize, count: usize) -> Result<()> {
        self.range_end(mem_offset, count)
            .map_err(|_| Error::InvalidRange(mem_offset, count))?;
        let ret = unsafe {
            // madvising away the region is the same as the guest changing it.
            // Next time it is read, it may return zero pages.
            libc::madvise(
                (self.addr as usize + mem_offset) as *mut _,
                count,
                libc::MADV_REMOVE,
            )
        };
        if ret < 0 {
            Err(Error::InvalidRange(mem_offset, count))
        } else {
            Ok(())
        }
    }

    unsafe fn as_slice(&self) -> &[u8] {
        // This is safe because we mapped the area at addr ourselves, so this slice will not
        // overflow. However, it is possible to alias.
        std::slice::from_raw_parts(self.addr, self.size)
    }

    // TODO(dgreid) - refactor this so the mut from non-mut isn't necessary (bug: 938767)
    #[allow(clippy::mut_from_ref)]
    unsafe fn as_mut_slice(&self) -> &mut [u8] {
        // This is safe because we mapped the area at addr ourselves, so this slice will not
        // overflow. However, it is possible to alias.
        std::slice::from_raw_parts_mut(self.addr, self.size)
    }

    // Check that offset+count is valid and return the sum.
    fn range_end(&self, offset: usize, count: usize) -> Result<usize> {
        let mem_end = offset.checked_add(count).ok_or(Error::InvalidAddress)?;
        if mem_end > self.size() {
            return Err(Error::InvalidAddress);
        }
        Ok(mem_end)
    }
}

impl VolatileMemory for MemoryMapping {
    fn get_slice(&self, offset: u64, count: u64) -> VolatileMemoryResult<VolatileSlice> {
        let mem_end = calc_offset(offset, count)?;
        if mem_end > self.size as u64 {
            return Err(VolatileMemoryError::OutOfBounds { addr: mem_end });
        }

        // Safe because we checked that offset + count was within our range and we only ever hand
        // out volatile accessors.
        Ok(unsafe { VolatileSlice::new((self.addr as usize + offset as usize) as *mut _, count) })
    }
}

impl Drop for MemoryMapping {
    fn drop(&mut self) {
        // This is safe because we mmap the area at addr ourselves, and nobody
        // else is holding a reference to it.
        unsafe {
            libc::munmap(self.addr as *mut libc::c_void, self.size);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use data_model::{VolatileMemory, VolatileMemoryError};
    use std::os::unix::io::FromRawFd;

    #[test]
    fn basic_map() {
        let m = MemoryMapping::new(1024).unwrap();
        assert_eq!(1024, m.size());
    }

    #[test]
    fn map_invalid_size() {
        let res = MemoryMapping::new(0).unwrap_err();
        if let Error::SystemCallFailed(e) = res {
            assert_eq!(e.errno(), libc::EINVAL);
        } else {
            panic!("unexpected error: {}", res);
        }
    }

    #[test]
    fn map_invalid_fd() {
        let fd = unsafe { std::fs::File::from_raw_fd(-1) };
        let res = MemoryMapping::from_fd(&fd, 1024).unwrap_err();
        if let Error::SystemCallFailed(e) = res {
            assert_eq!(e.errno(), libc::EBADF);
        } else {
            panic!("unexpected error: {}", res);
        }
    }

    #[test]
    fn test_write_past_end() {
        let m = MemoryMapping::new(5).unwrap();
        let res = m.write_slice(&[1, 2, 3, 4, 5, 6], 0);
        assert!(res.is_ok());
        assert_eq!(res.unwrap(), 5);
    }

    #[test]
    fn slice_size() {
        let m = MemoryMapping::new(5).unwrap();
        let s = m.get_slice(2, 3).unwrap();
        assert_eq!(s.size(), 3);
    }

    #[test]
    fn slice_addr() {
        let m = MemoryMapping::new(5).unwrap();
        let s = m.get_slice(2, 3).unwrap();
        assert_eq!(s.as_ptr(), unsafe { m.as_ptr().offset(2) });
    }

    #[test]
    fn slice_store() {
        let m = MemoryMapping::new(5).unwrap();
        let r = m.get_ref(2).unwrap();
        r.store(9u16);
        assert_eq!(m.read_obj::<u16>(2).unwrap(), 9);
    }

    #[test]
    fn slice_overflow_error() {
        let m = MemoryMapping::new(5).unwrap();
        let res = m.get_slice(std::u64::MAX, 3).unwrap_err();
        assert_eq!(
            res,
            VolatileMemoryError::Overflow {
                base: std::u64::MAX,
                offset: 3,
            }
        );
    }
    #[test]
    fn slice_oob_error() {
        let m = MemoryMapping::new(5).unwrap();
        let res = m.get_slice(3, 3).unwrap_err();
        assert_eq!(res, VolatileMemoryError::OutOfBounds { addr: 6 });
    }

    #[test]
    fn from_fd_offset_invalid() {
        let fd = unsafe { std::fs::File::from_raw_fd(-1) };
        let res = MemoryMapping::from_fd_offset(&fd, 4096, (libc::off_t::max_value() as usize) + 1)
            .unwrap_err();
        match res {
            Error::InvalidOffset => {}
            e => panic!("unexpected error: {}", e),
        }
    }
}
