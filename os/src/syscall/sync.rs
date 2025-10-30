use crate::sync::{Condvar, Mutex, MutexBlocking, MutexSpin, Semaphore};
use crate::task::{block_current_and_run_next, current_process, current_task};
use crate::timer::{add_timer, get_time_ms};
use alloc::sync::Arc;
/// sleep syscall
#[allow(unused)]
pub fn sys_sleep(ms: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_sleep",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let expire_ms = get_time_ms() + ms;
    let task = current_task().unwrap();
    add_timer(expire_ms, task);
    block_current_and_run_next();
    0
}
/// mutex create syscall
pub fn sys_mutex_create(blocking: bool) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_mutex_create",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let mutex: Option<Arc<dyn Mutex>> = if !blocking {
        Some(Arc::new(MutexSpin::new()))
    } else {
        Some(Arc::new(MutexBlocking::new()))
    };
    let mut process_inner = process.inner_exclusive_access();
    if let Some(id) = process_inner
        .mutex_list
        .iter()
        .enumerate()
        .find(|(_, item)| item.is_none())
        .map(|(id, _)| id)
    {
        process_inner.mutex_list[id] = mutex;
        id as isize
    } else {
        process_inner.mutex_list.push(mutex);
        process_inner.mutex_list.len() as isize - 1
    }
}
/// mutex lock syscall
pub fn sys_mutex_lock(mutex_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_mutex_lock",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let task = current_task().unwrap();
    let process = current_process();
    // Pre-check for deadlock using wait-for graph
    {
        let process_inner = process.inner_exclusive_access();
        if process_inner.deadlock_detect_enabled {
            if !check_deadlock_mutex(mutex_id) {
                trace!("Deadlock detected for mutex {}", mutex_id);
                return -0xDEAD;
            }
        }
    }

    // Mark waiting before potentially blocking (only if enabled)
    let mark_waiting_mutex = {
        let process_inner = process.inner_exclusive_access();
        process_inner.deadlock_detect_enabled
    };
    if mark_waiting_mutex {
        let mut task_inner = task.inner_exclusive_access();
        task_inner.waiting_mutex = Some(mutex_id);
    }

    // Perform lock
    let mutex = {
        let process_inner = process.inner_exclusive_access();
        Arc::clone(process_inner.mutex_list[mutex_id].as_ref().unwrap())
    };
    mutex.lock();

    // Clear waiting (if set) and record allocation
    let mut task_inner = task.inner_exclusive_access();
    if mark_waiting_mutex {
        task_inner.waiting_mutex = None;
    }
    if !task_inner.mutex_allocation.contains(&mutex_id) {
        task_inner.mutex_allocation.push(mutex_id);
    }
    drop(task_inner);

    0
}
/// mutex unlock syscall
pub fn sys_mutex_unlock(mutex_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_mutex_unlock",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let task = current_task().unwrap();
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let mutex = Arc::clone(process_inner.mutex_list[mutex_id].as_ref().unwrap());
    drop(process_inner);
    drop(process);
    mutex.unlock();
    
    // Update allocation tracking
    let mut task_inner = task.inner_exclusive_access();
    task_inner.mutex_allocation.retain(|&x| x != mutex_id);
    drop(task_inner);
    
    0
}
/// semaphore create syscall
pub fn sys_semaphore_create(res_count: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_semaphore_create",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let id = if let Some(id) = process_inner
        .semaphore_list
        .iter()
        .enumerate()
        .find(|(_, item)| item.is_none())
        .map(|(id, _)| id)
    {
        process_inner.semaphore_list[id] = Some(Arc::new(Semaphore::new(res_count)));
        id
    } else {
        process_inner
            .semaphore_list
            .push(Some(Arc::new(Semaphore::new(res_count))));
        process_inner.semaphore_list.len() - 1
    };
    id as isize
}
/// semaphore up syscall
pub fn sys_semaphore_up(sem_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_semaphore_up",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let task = current_task().unwrap();
    let process = current_process();
    let sem = {
        let process_inner = process.inner_exclusive_access();
        Arc::clone(process_inner.semaphore_list[sem_id].as_ref().unwrap())
    };
    sem.up();
    
    // Update allocation tracking
    let mut task_inner = task.inner_exclusive_access();
    if let Some(entry) = task_inner.semaphore_allocation.iter_mut().find(|e| e.0 == sem_id) {
        entry.1 -= 1;
        if entry.1 == 0 {
            task_inner.semaphore_allocation.retain(|e| e.0 != sem_id || e.1 != 0);
        }
    }
    drop(task_inner);
    
    0
}
/// semaphore down syscall
pub fn sys_semaphore_down(sem_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_semaphore_down",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let task = current_task().unwrap();
    let process = current_process();

    // Pre-check: if enabled and would block, run cycle detection
    let need_check = {
        let process_inner = process.inner_exclusive_access();
        if !process_inner.deadlock_detect_enabled {
            false
        } else {
            let sem = process_inner.semaphore_list[sem_id].as_ref().unwrap();
            let cnt = sem.inner.exclusive_access().count;
            cnt <= 0
        }
    };
    if need_check {
        if !check_deadlock_semaphore(sem_id) {
            trace!("Deadlock detected for semaphore {}", sem_id);
            return -0xDEAD;
        }
    }

    // Mark waiting before potentially blocking (only if enabled)
    let mark_waiting_semaphore = {
        let process_inner = process.inner_exclusive_access();
        process_inner.deadlock_detect_enabled
    };
    if mark_waiting_semaphore {
        let mut task_inner = task.inner_exclusive_access();
        task_inner.waiting_semaphore = Some(sem_id);
    }

    // Perform down
    let sem = {
        let process_inner = process.inner_exclusive_access();
        Arc::clone(process_inner.semaphore_list[sem_id].as_ref().unwrap())
    };
    sem.down();

    // Clear waiting (if set) and update allocation tracking
    let mut task_inner = task.inner_exclusive_access();
    if mark_waiting_semaphore {
        task_inner.waiting_semaphore = None;
    }
    if let Some(entry) = task_inner.semaphore_allocation.iter_mut().find(|e| e.0 == sem_id) {
        entry.1 += 1;
    } else {
        task_inner.semaphore_allocation.push((sem_id, 1));
    }
    drop(task_inner);

    0
}
/// condvar create syscall
pub fn sys_condvar_create() -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_condvar_create",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let id = if let Some(id) = process_inner
        .condvar_list
        .iter()
        .enumerate()
        .find(|(_, item)| item.is_none())
        .map(|(id, _)| id)
    {
        process_inner.condvar_list[id] = Some(Arc::new(Condvar::new()));
        id
    } else {
        process_inner
            .condvar_list
            .push(Some(Arc::new(Condvar::new())));
        process_inner.condvar_list.len() - 1
    };
    id as isize
}
/// condvar signal syscall
pub fn sys_condvar_signal(condvar_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_condvar_signal",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let condvar = Arc::clone(process_inner.condvar_list[condvar_id].as_ref().unwrap());
    drop(process_inner);
    condvar.signal();
    0
}
/// condvar wait syscall
pub fn sys_condvar_wait(condvar_id: usize, mutex_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_condvar_wait",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let condvar = Arc::clone(process_inner.condvar_list[condvar_id].as_ref().unwrap());
    let mutex = Arc::clone(process_inner.mutex_list[mutex_id].as_ref().unwrap());
    drop(process_inner);
    condvar.wait(mutex);
    0
}
/// enable deadlock detection syscall
pub fn sys_enable_deadlock_detect(enabled: usize) -> isize {
    trace!("kernel: sys_enable_deadlock_detect");
    if enabled != 0 && enabled != 1 {
        return -1;
    }
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    process_inner.deadlock_detect_enabled = enabled == 1;
    if enabled == 1 {
        trace!("Deadlock detection enabled for process {}", process.getpid());
    } else {
        trace!("Deadlock detection disabled for process {}", process.getpid());
    }
    0
}

fn check_deadlock_mutex(requesting_mutex_id: usize) -> bool {
    // Build wait-for graph among threads in the current process for mutexes
    let task = current_task().unwrap();
    let (tasks, current_tid) = {
        let process = task.process.upgrade().unwrap();
        let inner = process.inner_exclusive_access();
        let mut v = alloc::vec::Vec::new();
        for (tid, t) in inner.tasks.iter().enumerate() {
            if let Some(tcb) = t {
                v.push((tid, Arc::clone(tcb)));
            }
        }
        let ti = task.inner_exclusive_access();
        (v, ti.res.as_ref().unwrap().tid)
    };

    // Helper: who holds a mutex
    let holders_of = |mid: usize, list: &[(usize, Arc<crate::task::TaskControlBlock>)]| -> alloc::vec::Vec<usize> {
        let mut ret = alloc::vec::Vec::new();
        for (tid, tcb) in list.iter() {
            let ti = tcb.inner_exclusive_access();
            if ti.mutex_allocation.contains(&mid) {
                ret.push(*tid);
            }
        }
        ret
    };

    // adjacency list by tid index
    let mut adj: alloc::vec::Vec<alloc::vec::Vec<usize>> = alloc::vec::Vec::new();
    let max_tid = tasks.iter().map(|(tid, _)| *tid).max().unwrap_or(0);
    adj.resize(max_tid + 1, alloc::vec::Vec::new());

    // existing waiting edges
    for (tid, tcb) in tasks.iter() {
        let ti = tcb.inner_exclusive_access();
        if let Some(wait_m) = ti.waiting_mutex {
            let hs = holders_of(wait_m, &tasks);
            for h in hs { adj[*tid].push(h); }
        }
    }

    // simulate current request edge
    for h in holders_of(requesting_mutex_id, &tasks) {
        adj[current_tid].push(h);
    }

    // detect cycle
    let mut visited = alloc::vec::Vec::new();
    let mut in_stack = alloc::vec::Vec::new();
    visited.resize(max_tid + 1, false);
    in_stack.resize(max_tid + 1, false);
    fn dfs(u: usize, adj: &alloc::vec::Vec<alloc::vec::Vec<usize>>, vis: &mut [bool], st: &mut [bool]) -> bool {
        if st[u] { return true; }
        if vis[u] { return false; }
        vis[u] = true;
        st[u] = true;
        for &v in &adj[u] {
            if dfs(v, adj, vis, st) { return true; }
        }
        st[u] = false;
        false
    }
    for (tid, _) in tasks.iter() {
        if dfs(*tid, &adj, &mut visited, &mut in_stack) {
            return false; // cycle found → would deadlock
        }
    }
    true
}

fn check_deadlock_semaphore(requesting_sem_id: usize) -> bool {
    // Build wait-for graph among threads in the current process for semaphores
    let task = current_task().unwrap();
    let (tasks, current_tid) = {
        let process = task.process.upgrade().unwrap();
        let inner = process.inner_exclusive_access();
        let mut v = alloc::vec::Vec::new();
        for (tid, t) in inner.tasks.iter().enumerate() {
            if let Some(tcb) = t {
                v.push((tid, Arc::clone(tcb)));
            }
        }
        let ti = task.inner_exclusive_access();
        (v, ti.res.as_ref().unwrap().tid)
    };

    let holders_of = |sid: usize, list: &[(usize, Arc<crate::task::TaskControlBlock>)]| -> alloc::vec::Vec<usize> {
        let mut ret = alloc::vec::Vec::new();
        for (tid, tcb) in list.iter() {
            let ti = tcb.inner_exclusive_access();
            if let Some((_, cnt)) = ti.semaphore_allocation.iter().find(|(id, _)| *id == sid) {
                if *cnt > 0 { ret.push(*tid); }
            }
        }
        ret
    };

    let mut adj: alloc::vec::Vec<alloc::vec::Vec<usize>> = alloc::vec::Vec::new();
    let max_tid = tasks.iter().map(|(tid, _)| *tid).max().unwrap_or(0);
    adj.resize(max_tid + 1, alloc::vec::Vec::new());

    // existing waiting edges
    for (tid, tcb) in tasks.iter() {
        let ti = tcb.inner_exclusive_access();
        if let Some(wait_s) = ti.waiting_semaphore {
            let hs = holders_of(wait_s, &tasks);
            for h in hs { adj[*tid].push(h); }
        }
    }

    // simulate current request edge
    for h in holders_of(requesting_sem_id, &tasks) {
        adj[current_tid].push(h);
    }

    // detect cycle
    let mut visited = alloc::vec::Vec::new();
    let mut in_stack = alloc::vec::Vec::new();
    visited.resize(max_tid + 1, false);
    in_stack.resize(max_tid + 1, false);
    fn dfs(u: usize, adj: &alloc::vec::Vec<alloc::vec::Vec<usize>>, vis: &mut [bool], st: &mut [bool]) -> bool {
        if st[u] { return true; }
        if vis[u] { return false; }
        vis[u] = true;
        st[u] = true;
        for &v in &adj[u] {
            if dfs(v, adj, vis, st) { return true; }
        }
        st[u] = false;
        false
    }
    for (tid, _) in tasks.iter() {
        if dfs(*tid, &adj, &mut visited, &mut in_stack) {
            return false; // cycle found → would deadlock
        }
    }
    true
}
