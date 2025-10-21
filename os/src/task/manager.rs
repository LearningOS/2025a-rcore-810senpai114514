//!Implementation of [`TaskManager`]
use super::TaskControlBlock;
use crate::sync::UPSafeCell;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use lazy_static::*;
///A array of `TaskControlBlock` that is thread-safe
pub struct TaskManager {
    ready_queue: VecDeque<Arc<TaskControlBlock>>,
}

/// A stride scheduler.
impl TaskManager {
    ///Creat an empty TaskManager
    pub fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
        }
    }
    /// Add process back to ready queue
    pub fn add(&mut self, task: Arc<TaskControlBlock>) {
        self.ready_queue.push_back(task);
    }
    /// Take a process out of the ready queue using stride scheduling
    /// Find the process with minimum stride and update its stride
    pub fn fetch(&mut self) -> Option<Arc<TaskControlBlock>> {
        if self.ready_queue.is_empty() {
            return None;
        }
        
        // Find the process with minimum stride
        let mut min_stride = usize::MAX;
        let mut min_index = 0;
        
        for (i, task) in self.ready_queue.iter().enumerate() {
            let inner = task.inner_exclusive_access();
            if inner.stride < min_stride {
                min_stride = inner.stride;
                min_index = i;
            }
        }
        
        // Remove and return the task with minimum stride
        let selected_task = self.ready_queue.remove(min_index).unwrap();
        
        // Update the stride of the selected task
        {
            let mut inner = selected_task.inner_exclusive_access();
            inner.update_stride();
        }
        
        Some(selected_task)
    }
    
    /// Execute a closure with mutable access to current task's memory set
    pub fn with_current_memory_set<F, R>(&self, f: F) -> R 
    where 
        F: FnOnce(&mut crate::mm::MemorySet) -> R,
    {
        if let Some(current_task) = crate::task::current_task() {
            let mut inner = current_task.inner_exclusive_access();
            f(&mut inner.memory_set)
        } else {
            panic!("No current task");
        }
    }
}

lazy_static! {
    /// TASK_MANAGER instance through lazy_static!
    pub static ref TASK_MANAGER: UPSafeCell<TaskManager> =
        unsafe { UPSafeCell::new(TaskManager::new()) };
}

/// Add process to ready queue
pub fn add_task(task: Arc<TaskControlBlock>) {
    //trace!("kernel: TaskManager::add_task");
    TASK_MANAGER.exclusive_access().add(task);
}

/// Take a process out of the ready queue
pub fn fetch_task() -> Option<Arc<TaskControlBlock>> {
    //trace!("kernel: TaskManager::fetch_task");
    TASK_MANAGER.exclusive_access().fetch()
}
