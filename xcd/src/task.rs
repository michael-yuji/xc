// Copyright (c) 2023 Yan Ka, Chiu.
// All rights reserved.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions
// are met:
// 1. Redistributions of source code must retain the above copyright
//    notice, this list of conditions, and the following disclaimer,
//    without modification, immediately at the beginning of the file.
// 2. The name of the author may not be used to endorse or promote products
//    derived from this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE AUTHOR AND CONTRIBUTORS ``AS IS'' AND
// ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
// IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
// ARE DISCLAIMED. IN NO EVENT SHALL THE AUTHOR OR CONTRIBUTORS BE LIABLE FOR
// ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
// DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
// OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
// HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
// LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY
// OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
// SUCH DAMAGE.
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;
use tokio::sync::watch::{channel, Receiver, Sender};
use tokio::sync::Notify;

pub trait FromId<C, K: Hash + Eq + Clone> {
    fn from_id(context: C, k: &K) -> (Self, TaskStatus)
    where
        Self: Sized;
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum TaskStatus {
    Fault(String),
    Completed,
    InProgress,
}

#[derive(Debug)]
pub struct Task<K: Hash + Eq + Clone, V> {
    #[allow(unused)]
    id: K,
    pub last_state: V,
    state: TaskStatus,
    pub notify: Arc<Notify>,
}

impl<K: Hash + Eq + Clone, V> Task<K, V> {
    pub fn from_id<C>(context: C, id: &K) -> Task<K, V>
    where
        V: FromId<C, K>,
    {
        let (value, state) = V::from_id(context, id);
        Task {
            id: id.clone(),
            last_state: value,
            state,
            notify: Arc::new(Notify::new()),
        }
    }

    pub fn has_failed(&self) -> bool {
        matches!(&self.state, TaskStatus::Fault(_))
    }

    pub fn fault(&self) -> Option<String> {
        if let TaskStatus::Fault(reason) = &self.state {
            Some(reason.to_string())
        } else {
            None
        }
    }

    pub fn is_completed(&self) -> bool {
        self.state == TaskStatus::Completed
    }
}

// Wrapper around an computation that notify its waiters when the computation
// is completed or faulted
pub struct TaskHandle<K: Hash + Eq + Clone, T>(Sender<Task<K, T>>);

impl<K: Hash + Eq + Clone, T> TaskHandle<K, T> {
    pub fn use_try<F, A>(&mut self, func: F) -> Result<A, ()>
    where
        T: Clone,
        F: Fn(&mut T) -> Result<A, String>,
    {
        let mut s = self.0.borrow().last_state.clone();
        match func(&mut s) {
            Err(err) => {
                self.0.send_modify(|task| {
                    task.last_state = s;
                    task.state = TaskStatus::Fault(err);
                    task.notify.notify_waiters()
                });
                Err(())
            }
            Ok(res) => {
                self.0.send_modify(|task| {
                    task.last_state = s;
                });
                Ok(res)
            }
        }
    }

    pub fn set_completed(&mut self) {
        self.0.send_modify(|task| {
            task.state = TaskStatus::Completed;
            task.notify.notify_waiters()
        });
    }

    pub fn set_faulted(&mut self, reason: &str) {
        self.0.send_modify(|task| {
            task.state = TaskStatus::Fault(reason.to_string());
            task.notify.notify_waiters()
        });
    }
    pub fn is_completed(&self) -> bool {
        self.0.borrow().state == TaskStatus::Completed
    }
}

/// Context: all variables needed in the workers
#[derive(Debug)]
pub struct NotificationStore<C, K: Hash + Eq + Clone, V: FromId<C, K>> {
    context: C,
    store: HashMap<K, Receiver<Task<K, V>>>,
}

impl<C: Clone, K: Hash + Eq + Clone, V: FromId<C, K>> NotificationStore<C, K, V> {
    pub fn new(context: C) -> NotificationStore<C, K, V> {
        NotificationStore {
            context,
            store: HashMap::new(),
        }
    }

    pub fn get(&mut self, id: &K) -> Option<Receiver<Task<K, V>>> {
        self.store.get(id).cloned()
    }

    pub fn register(&mut self, id: &K) -> (TaskHandle<K, V>, Receiver<Task<K, V>>) {
        let (sender, rx) = channel(Task::from_id(self.context.clone(), id));
        self.store.insert(id.clone(), rx.clone());
        (TaskHandle(sender), rx)
    }
}
