//! Process management syscalls
use crate::task::{change_program_brk, exit_current_and_run_next, suspend_current_and_run_next, current_user_token, with_current_memory_set, current_memory_set};
use crate::mm::{check_user_address, copy_from_user, copy_to_user, VirtAddr, MapPermission};
use crate::syscall::{get_syscall_counter};
use crate::config::PAGE_SIZE;
use crate::timer::get_time_us;
use core::mem;

#[repr(C)]
#[derive(Debug, Default)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(_exit_code: i32) -> ! {
    trace!("kernel: sys_exit");
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

/// YOUR JOB: get time with second and microsecond
/// HINT: You might reimplement it with virtual memory management.
/// HINT: What if [`TimeVal`] is splitted by two pages ?
pub fn sys_get_time(_ts: *mut TimeVal, _tz: usize) -> isize {
    let mut ts = TimeVal::default();
    let us = get_time_us();
    ts.sec = us / 1000000;
    ts.usec = us % 1000000;
    let ts_bytes = unsafe { 
        core::slice::from_raw_parts(&ts as *const TimeVal as *const u8, mem::size_of::<TimeVal>()) 
    };
    let _ = copy_to_user(current_user_token(), ts_bytes, _ts as *mut u8, mem::size_of::<TimeVal>());
    0
}

/// Trace system call implementation
/// 
/// # Arguments
/// * `trace_request` - Type of operation (0: read, 1: write, 2: query counter)
/// * `id` - For read/write: pointer to user space, For query: syscall number
/// * `data` - For write: data to write (as u8), ignored for other operations
/// 
/// # Returns
/// * For read: the byte value at the address, or -1 if error
/// * For write: 0 on success, -1 on error
/// * For query: the syscall counter value
/// * For invalid request: -1
pub fn sys_trace(trace_request: usize, id: usize, data: usize) -> isize {
    trace!("kernel: sys_trace");
    
    let token = current_user_token();
    
    match trace_request {
        0 => {
            // Read operation: id is *const u8, return the byte value
            let user_ptr = id as *const u8;
            
            // Check if address is readable
            if !check_user_address(token, user_ptr, true, false) {
                return -1;
            }
            
            // Read one byte from user space
            let mut kernel_buf = [0u8; 1];
            match copy_from_user(token, user_ptr, &mut kernel_buf, 1) {
                Ok(()) => kernel_buf[0] as isize,
                Err(_) => -1,
            }
        }
        1 => {
            // Write operation: id is *mut u8, write data as u8
            let user_ptr = id as *mut u8;
            let value = data as u8; // Only use the lowest byte
            
            // Check if address is writable
            if !check_user_address(token, user_ptr, false, true) {
                return -1;
            }
            
            // Write one byte to user space
            let kernel_buf = [value];
            match copy_to_user(token, &kernel_buf, user_ptr, 1) {
                Ok(()) => 0,
                Err(_) => -1,
            }
        }
        2 => {
            // Query operation: id is syscall number, return counter value
            get_syscall_counter(id) as isize
        }
        _ => {
            // Invalid request
            -1
        }
    }
}

/// mmap system call implementation
/// 
/// # Arguments
/// * `start` - Virtual address to map (must be page aligned)
/// * `len` - Length in bytes (will be rounded up to page size)
/// * `prot` - Protection flags (bits 0-2: R, W, X)
/// 
/// # Returns
/// * `0` - Success
/// * `-1` - Error
pub fn sys_mmap(start: usize, len: usize, prot: usize) -> isize {
    trace!("kernel: sys_mmap");
    
    // Check if start is page aligned
    if start & (PAGE_SIZE - 1) != 0 {
        return -1;
    }
    
    // Convert prot to MapPermission
    let permission = match MapPermission::from_mmap_prot(prot) {
        Ok(perm) => perm,
        Err(_) => return -1,
    };
    
    // Perform the mapping
    with_current_memory_set(|memory_set| {
        match memory_set.mmap(VirtAddr::from(start), len, permission) {
            Ok(()) => 0,
            Err(_) => -1,
        }
    })
}

/// munmap system call implementation
/// 
/// # Arguments
/// * `start` - Virtual address to unmap (must be page aligned)
/// * `len` - Length in bytes (will be rounded up to page size)
/// 
/// # Returns
/// * `0` - Success
/// * `-1` - Error
pub fn sys_munmap(start: usize, len: usize) -> isize {
    trace!("kernel: sys_munmap");
    
    // Check if start is page aligned
    if start & (PAGE_SIZE - 1) != 0 {
        return -1;
    }
    
    // Get current memory set
    let memory_set = current_memory_set();
    
    // Perform the unmapping
    match memory_set.munmap(VirtAddr::from(start), len) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}
/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel: sys_sbrk");
    if let Some(old_brk) = change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}
