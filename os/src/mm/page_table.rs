//! Implementation of [`PageTableEntry`] and [`PageTable`].

use super::{frame_alloc, FrameTracker, PhysPageNum, StepByOne, VirtAddr, VirtPageNum};
use super::PAGE_SIZE;
use alloc::vec;
use alloc::vec::Vec;
use bitflags::*;

bitflags! {
    /// page table entry flags
    pub struct PTEFlags: u8 {
        /// Valid
        const V = 1 << 0;
        /// Readable
        const R = 1 << 1;
        /// Writable
        const W = 1 << 2;
        /// eXecutable
        const X = 1 << 3;
        /// User
        const U = 1 << 4;
        /// Global
        const G = 1 << 5;
        /// Accessed
        const A = 1 << 6;
        /// Dirty
        const D = 1 << 7;
    }
}

#[derive(Copy, Clone)]
#[repr(C)]
/// page table entry structure
pub struct PageTableEntry {
    /// bits of page table entry
    pub bits: usize,
}

impl PageTableEntry {
    /// Create a new page table entry
    pub fn new(ppn: PhysPageNum, flags: PTEFlags) -> Self {
        PageTableEntry {
            bits: ppn.0 << 10 | flags.bits as usize,
        }
    }
    /// Create an empty page table entry
    pub fn empty() -> Self {
        PageTableEntry { bits: 0 }
    }
    /// Get the physical page number from the page table entry
    pub fn ppn(&self) -> PhysPageNum {
        (self.bits >> 10 & ((1usize << 44) - 1)).into()
    }
    /// Get the flags from the page table entry
    pub fn flags(&self) -> PTEFlags {
        PTEFlags::from_bits(self.bits as u8).unwrap()
    }
    /// The page pointered by page table entry is valid?
    pub fn is_valid(&self) -> bool {
        (self.flags() & PTEFlags::V) != PTEFlags::empty()
    }
    /// The page pointered by page table entry is readable?
    pub fn readable(&self) -> bool {
        (self.flags() & PTEFlags::R) != PTEFlags::empty()
    }
    /// The page pointered by page table entry is writable?
    pub fn writable(&self) -> bool {
        (self.flags() & PTEFlags::W) != PTEFlags::empty()
    }
    /// The page pointered by page table entry is executable?
    pub fn executable(&self) -> bool {
        (self.flags() & PTEFlags::X) != PTEFlags::empty()
    }
}

/// page table structure
pub struct PageTable {
    root_ppn: PhysPageNum,
    frames: Vec<FrameTracker>,
}

/// Assume that it won't oom when creating/mapping.
impl PageTable {
    /// Create a new page table
    pub fn new() -> Self {
        let frame = frame_alloc().unwrap();
        PageTable {
            root_ppn: frame.ppn,
            frames: vec![frame],
        }
    }
    /// Temporarily used to get arguments from user space.
    pub fn from_token(satp: usize) -> Self {
        Self {
            root_ppn: PhysPageNum::from(satp & ((1usize << 44) - 1)),
            frames: Vec::new(),
        }
    }
    /// Find PageTableEntry by VirtPageNum, create a frame for a 4KB page table if not exist
    fn find_pte_create(&mut self, vpn: VirtPageNum) -> Option<&mut PageTableEntry> {
        let idxs = vpn.indexes();
        let mut ppn = self.root_ppn;
        let mut result: Option<&mut PageTableEntry> = None;
        for (i, idx) in idxs.iter().enumerate() {
            let pte = &mut ppn.get_pte_array()[*idx];
            if i == 2 {
                result = Some(pte);
                break;
            }
            if !pte.is_valid() {
                let frame = frame_alloc().unwrap();
                *pte = PageTableEntry::new(frame.ppn, PTEFlags::V);
                self.frames.push(frame);
            }
            ppn = pte.ppn();
        }
        result
    }
    /// Find PageTableEntry by VirtPageNum
    fn find_pte(&self, vpn: VirtPageNum) -> Option<&mut PageTableEntry> {
        let idxs = vpn.indexes();
        let mut ppn = self.root_ppn;
        let mut result: Option<&mut PageTableEntry> = None;
        for (i, idx) in idxs.iter().enumerate() {
            let pte = &mut ppn.get_pte_array()[*idx];
            if i == 2 {
                result = Some(pte);
                break;
            }
            if !pte.is_valid() {
                return None;
            }
            ppn = pte.ppn();
        }
        result
    }
    /// set the map between virtual page number and physical page number
    #[allow(unused)]
    pub fn map(&mut self, vpn: VirtPageNum, ppn: PhysPageNum, flags: PTEFlags) {
        let pte = self.find_pte_create(vpn).unwrap();
        assert!(!pte.is_valid(), "vpn {:?} is mapped before mapping", vpn);
        *pte = PageTableEntry::new(ppn, flags | PTEFlags::V);
    }
    /// remove the map between virtual page number and physical page number
    #[allow(unused)]
    pub fn unmap(&mut self, vpn: VirtPageNum) {
        let pte = self.find_pte(vpn).unwrap();
        assert!(pte.is_valid(), "vpn {:?} is invalid before unmapping", vpn);
        *pte = PageTableEntry::empty();
    }
    /// get the page table entry from the virtual page number
    pub fn translate(&self, vpn: VirtPageNum) -> Option<PageTableEntry> {
        self.find_pte(vpn).map(|pte| *pte)
    }
    /// get the token from the page table
    pub fn token(&self) -> usize {
        8usize << 60 | self.root_ppn.0
    }
}

/// Translate&Copy a ptr[u8] array with LENGTH len to a mutable u8 Vec through page table
pub fn translated_byte_buffer(token: usize, ptr: *const u8, len: usize) -> Vec<&'static mut [u8]> {
    let page_table = PageTable::from_token(token);
    let mut start = ptr as usize;
    let end = start + len;
    let mut v = Vec::new();
    while start < end {
        let start_va = VirtAddr::from(start);
        let mut vpn = start_va.floor();
        let ppn = page_table.translate(vpn).unwrap().ppn();
        vpn.step();
        let mut end_va: VirtAddr = vpn.into();
        end_va = end_va.min(VirtAddr::from(end));
        if end_va.page_offset() == 0 {
            v.push(&mut ppn.get_bytes_array()[start_va.page_offset()..]);
        } else {
            v.push(&mut ppn.get_bytes_array()[start_va.page_offset()..end_va.page_offset()]);
        }
        start = end_va.into();
    }
    v
}

/// Copy data from user space to kernel space
/// 
/// # Arguments
/// * `token` - User space page table token
/// * `user_ptr` - Pointer to user space data
/// * `kernel_buf` - Buffer in kernel space to copy data to
/// * `len` - Number of bytes to copy
/// 
/// # Returns
/// * `Ok(())` - Success
/// * `Err(usize)` - Error with the number of bytes successfully copied
pub fn copy_from_user(token: usize, user_ptr: *const u8, kernel_buf: &mut [u8], len: usize) -> Result<(), usize> {
    if len == 0 {
        return Ok(());
    }
    
    if len > kernel_buf.len() {
        return Err(0);
    }
    
    let page_table = PageTable::from_token(token);
    let mut copied = 0;
    let mut start = user_ptr as usize;
    let end = start + len;
    let mut kernel_offset = 0;
    
    while start < end && kernel_offset < kernel_buf.len() {
        let start_va = VirtAddr::from(start);
        let mut vpn = start_va.floor();
        
        // Check if the page is valid and readable
        let pte = match page_table.translate(vpn) {
            Some(pte) if pte.is_valid() && pte.readable() => pte,
            _ => return Err(copied), // Page not accessible
        };
        
        let ppn = pte.ppn();
        vpn.step();
        let mut end_va: VirtAddr = vpn.into();
        end_va = end_va.min(VirtAddr::from(end));
        
        let page_start_offset = start_va.page_offset();
        let page_end_offset = end_va.page_offset();
        let page_len = if page_end_offset == 0 {
            PAGE_SIZE - page_start_offset
        } else {
            page_end_offset - page_start_offset
        };
        
        let copy_len = page_len.min(kernel_buf.len() - kernel_offset);
        
        // Perform the actual copy
        unsafe {
            let user_data = &ppn.get_bytes_array()[page_start_offset..page_start_offset + copy_len];
            kernel_buf[kernel_offset..kernel_offset + copy_len].copy_from_slice(user_data);
        }
        
        copied += copy_len;
        kernel_offset += copy_len;
        start += copy_len;
    }
    
    if copied == len {
        Ok(())
    } else {
        Err(copied)
    }
}

/// Copy data from kernel space to user space
/// 
/// # Arguments
/// * `token` - User space page table token
/// * `kernel_buf` - Buffer in kernel space to copy data from
/// * `user_ptr` - Pointer to user space data
/// * `len` - Number of bytes to copy
/// 
/// # Returns
/// * `Ok(())` - Success
/// * `Err(usize)` - Error with the number of bytes successfully copied
pub fn copy_to_user(token: usize, kernel_buf: &[u8], user_ptr: *mut u8, len: usize) -> Result<(), usize> {
    if len == 0 {
        return Ok(());
    }
    
    if len > kernel_buf.len() {
        return Err(0);
    }
    
    let page_table = PageTable::from_token(token);
    let mut copied = 0;
    let mut start = user_ptr as usize;
    let end = start + len;
    let mut kernel_offset = 0;
    
    while start < end && kernel_offset < kernel_buf.len() {
        let start_va = VirtAddr::from(start);
        let mut vpn = start_va.floor();
        
        // Check if the page is valid and writable
        let pte = match page_table.translate(vpn) {
            Some(pte) if pte.is_valid() && pte.writable() => pte,
            _ => return Err(copied), // Page not accessible
        };
        
        let ppn = pte.ppn();
        vpn.step();
        let mut end_va: VirtAddr = vpn.into();
        end_va = end_va.min(VirtAddr::from(end));
        
        let page_start_offset = start_va.page_offset();
        let page_end_offset = end_va.page_offset();
        let page_len = if page_end_offset == 0 {
            PAGE_SIZE - page_start_offset
        } else {
            page_end_offset - page_start_offset
        };
        
        let copy_len = page_len.min(kernel_buf.len() - kernel_offset);
        
        // Perform the actual copy
        unsafe {
            let user_data = &mut ppn.get_bytes_array()[page_start_offset..page_start_offset + copy_len];
            user_data.copy_from_slice(&kernel_buf[kernel_offset..kernel_offset + copy_len]);
        }
        
        copied += copy_len;
        kernel_offset += copy_len;
        start += copy_len;
    }
    
    if copied == len {
        Ok(())
    } else {
        Err(copied)
    }
}

/// Check if a user space address is valid and accessible
/// 
/// # Arguments
/// * `token` - User space page table token
/// * `ptr` - Pointer to check
/// * `readable` - Whether the address needs to be readable
/// * `writable` - Whether the address needs to be writable
/// 
/// # Returns
/// * `true` - Address is valid and accessible
/// * `false` - Address is invalid or not accessible
pub fn check_user_address(token: usize, ptr: *const u8, readable: bool, writable: bool) -> bool {
    let page_table = PageTable::from_token(token);
    let va = VirtAddr::from(ptr as usize);
    let vpn = va.floor();
    
    // Check if the page is mapped and valid
    let pte = match page_table.translate(vpn) {
        Some(pte) if pte.is_valid() => pte,
        _ => return false,
    };
    
    // Check permissions
    if readable && !pte.readable() {
        return false;
    }
    if writable && !pte.writable() {
        return false;
    }
    
    true
}