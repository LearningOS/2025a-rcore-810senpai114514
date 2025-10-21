//! Process management syscalls
//!
use alloc::sync::Arc;
use core::mem;

use crate::{
    fs::{open_file, OpenFlags},
    config::PAGE_SIZE,
    mm::{check_user_address, copy_from_user, copy_to_user, translated_refmut, translated_str, VirtAddr, MapPermission},
    syscall::get_syscall_counter,
    task::{
        add_task, current_task, current_user_token, exit_current_and_run_next,
        suspend_current_and_run_next, with_current_memory_set, TaskControlBlock,
    },
    timer::get_time_us,
};

#[repr(C)]
#[derive(Debug, Default)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

pub fn sys_exit(exit_code: i32) -> ! {
    trace!("kernel:pid[{}] sys_exit", current_task().unwrap().pid.0);
    exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

pub fn sys_yield() -> isize {
    //trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

pub fn sys_getpid() -> isize {
    trace!("kernel: sys_getpid pid:{}", current_task().unwrap().pid.0);
    current_task().unwrap().pid.0 as isize
}

pub fn sys_fork() -> isize {
    trace!("kernel:pid[{}] sys_fork", current_task().unwrap().pid.0);
    let current_task = current_task().unwrap();
    let new_task = current_task.fork();
    let new_pid = new_task.pid.0;
    // modify trap context of new_task, because it returns immediately after switching
    let trap_cx = new_task.inner_exclusive_access().get_trap_cx();
    // we do not have to move to next instruction since we have done it before
    // for child process, fork returns 0
    trap_cx.x[10] = 0;
    // add new task to scheduler
    add_task(new_task);
    new_pid as isize
}

pub fn sys_exec(path: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_exec", current_task().unwrap().pid.0);
    let token = current_user_token();
    let path = translated_str(token, path);
    if let Some(app_inode) = open_file(path.as_str(), OpenFlags::RDONLY) {
        let all_data = app_inode.read_all();
        let task = current_task().unwrap();
        task.exec(all_data.as_slice());
        0
    } else {
        -1
    }
}

/// If there is not a child process whose pid is same as given, return -1.
/// Else if there is a child process but it is still running, return -2.
pub fn sys_waitpid(pid: isize, exit_code_ptr: *mut i32) -> isize {
    //trace!("kernel: sys_waitpid");
    let task = current_task().unwrap();
    // find a child process

    // ---- access current PCB exclusively
    let mut inner = task.inner_exclusive_access();
    if !inner
        .children
        .iter()
        .any(|p| pid == -1 || pid as usize == p.getpid())
    {
        return -1;
        // ---- release current PCB
    }
    let pair = inner.children.iter().enumerate().find(|(_, p)| {
        // ++++ temporarily access child PCB exclusively
        p.inner_exclusive_access().is_zombie() && (pid == -1 || pid as usize == p.getpid())
        // ++++ release child PCB
    });
    if let Some((idx, _)) = pair {
        let child = inner.children.remove(idx);
        // confirm that child will be deallocated after being removed from children list
        assert_eq!(Arc::strong_count(&child), 1);
        let found_pid = child.getpid();
        // ++++ temporarily access child PCB exclusively
        let exit_code = child.inner_exclusive_access().exit_code;
        // ++++ release child PCB
        *translated_refmut(inner.memory_set.token(), exit_code_ptr) = exit_code;
        found_pid as isize
    } else {
        -2
    }
    // ---- release current PCB automatically
}

/// get time with second and microsecond
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
    
    // Perform the unmapping
    with_current_memory_set(|memory_set| {
        match memory_set.munmap(VirtAddr::from(start), len) {
            Ok(()) => 0,
            Err(_) => -1,
        }
    })
}

/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel:pid[{}] sys_sbrk", current_task().unwrap().pid.0);
    if let Some(old_brk) = current_task().unwrap().change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}

/// Spawn a new process to execute the target program
/// 
/// # Arguments
/// * `path` - Pointer to the program name in user space
/// 
/// # Returns
/// * On success: child process PID
/// * On failure: -1 (invalid filename or other error)
pub fn sys_spawn(path: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_spawn", current_task().unwrap().pid.0);
    
    let token = current_user_token();
    let path = translated_str(token, path);
    
    // Try to open the file from the file system
    if let Some(app_inode) = open_file(path.as_str(), OpenFlags::RDONLY) {
        let all_data = app_inode.read_all();
        
        // Create a new task control block for the child process
        let child_task = Arc::new(TaskControlBlock::new(all_data.as_slice()));
        let child_pid = child_task.pid.0;
        
        // Set up the parent-child relationship
        let current_task = current_task().unwrap();
        {
            let mut current_inner = current_task.inner_exclusive_access();
            current_inner.children.push(child_task.clone());
        }
        
        {
            let mut child_inner = child_task.inner_exclusive_access();
            child_inner.parent = Some(Arc::downgrade(&current_task));
        }
        
        // Add the child task to the scheduler
        add_task(child_task);
        
        child_pid as isize
    } else {
        // Invalid filename
        -1
    }
}

/// Set task priority for stride scheduling
/// 
/// # Arguments
/// * `prio` - Process priority (must be >= 2)
/// 
/// # Returns
/// * On success: the priority value
/// * On failure: -1 (invalid priority)
pub fn sys_set_priority(prio: isize) -> isize {
    trace!("kernel:pid[{}] sys_set_priority", current_task().unwrap().pid.0);
    
    if prio < 2 {
        return -1;
    }
    
    let current_task = current_task().unwrap();
    let mut inner = current_task.inner_exclusive_access();
    
    if inner.set_priority(prio as usize) {
        prio
    } else {
        -1
    }
}
