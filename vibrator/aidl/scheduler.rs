/*
 * SPDX-FileCopyrightText: The LineageOS Project
 * SPDX-License-Identifier: Apache-2.0
 */

//! One thread that runs delayed tasks, so a vibration does not need a sleeping thread of its own.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

type Task = Box<dyn FnOnce() + Send>;

struct Entry {
    at: Instant,
    seq: u64,
    task: Task,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        (self.at, self.seq) == (other.at, other.seq)
    }
}

impl Eq for Entry {}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

// BinaryHeap is a max-heap: the earliest deadline, then the earliest request, is the greatest.
impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        (other.at, other.seq).cmp(&(self.at, self.seq))
    }
}

pub struct Scheduler {
    sender: Mutex<Sender<(Instant, Task)>>,
}

impl Scheduler {
    pub fn new(name: &str) -> Scheduler {
        let (sender, receiver) = channel::<(Instant, Task)>();
        thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                let mut queue = BinaryHeap::new();
                let mut seq = 0u64;
                loop {
                    let now = Instant::now();
                    while queue.peek().is_some_and(|e: &Entry| e.at <= now) {
                        (queue.pop().unwrap().task)();
                    }
                    let next = match queue.peek() {
                        Some(e) => receiver.recv_timeout(e.at.saturating_duration_since(now)),
                        None => receiver.recv().map_err(|_| RecvTimeoutError::Disconnected),
                    };
                    match next {
                        Ok((at, task)) => {
                            queue.push(Entry { at, seq, task });
                            seq += 1;
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .expect("Failed to start the scheduler thread");
        Scheduler { sender: Mutex::new(sender) }
    }

    pub fn after(&self, delay: Duration, task: impl FnOnce() + Send + 'static) {
        let _ = self.sender.lock().unwrap().send((Instant::now() + delay, Box::new(task)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Arc};

    #[test]
    fn runs_tasks_in_deadline_order() {
        let scheduler = Scheduler::new("test");
        let (tx, rx) = mpsc::channel();
        for (delay, id) in [(60, 3), (20, 1), (40, 2), (20, 4)] {
            let tx = tx.clone();
            scheduler.after(Duration::from_millis(delay), move || tx.send(id).unwrap());
        }
        let order: Vec<i32> = (0..4).map(|_| rx.recv_timeout(Duration::from_secs(2)).unwrap()).collect();
        assert_eq!(order, vec![1, 4, 2, 3]);
    }

    #[test]
    fn does_not_run_early() {
        let scheduler = Scheduler::new("test");
        let start = Instant::now();
        let (tx, rx) = mpsc::channel();
        scheduler.after(Duration::from_millis(50), move || tx.send(Instant::now()).unwrap());
        let ran = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(ran - start >= Duration::from_millis(50));
    }

    #[test]
    fn later_request_with_earlier_deadline_runs_first() {
        let scheduler = Arc::new(Scheduler::new("test"));
        let (tx, rx) = mpsc::channel();
        let tx2 = tx.clone();
        scheduler.after(Duration::from_millis(300), move || tx.send("late").unwrap());
        thread::sleep(Duration::from_millis(20));
        scheduler.after(Duration::from_millis(10), move || tx2.send("early").unwrap());
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), "early");
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), "late");
    }
}
