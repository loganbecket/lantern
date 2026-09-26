//! A pool of decode threads fed by a last-in-first-out queue.
//!
//! Newest requests are the cells the user is looking at right now, so they go
//! first. Cells that scroll away cancel their request before a thread picks
//! it up, so fast scrolling does not pile up wasted work.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use crate::decode::{self, Decoded};

pub struct Job {
    pub path: PathBuf,
    pub target: u32,
    pub cancel: Arc<AtomicBool>,
}

pub struct Reply {
    pub path: PathBuf,
    pub target: u32,
    pub result: Result<Decoded, String>,
}

#[derive(Clone)]
pub struct Pool {
    queue: Arc<(Mutex<Vec<Job>>, Condvar)>,
}

impl Pool {
    pub fn new(replies: async_channel::Sender<Reply>) -> Self {
        let queue = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
        let threads = thread::available_parallelism().map_or(4, |n| n.get()).saturating_sub(1).max(2);

        for _ in 0..threads {
            let queue = queue.clone();
            let replies = replies.clone();
            thread::spawn(move || worker(&queue, &replies));
        }

        Self { queue }
    }

    pub fn submit(&self, job: Job) {
        let (jobs, ready) = &*self.queue;
        jobs.lock().unwrap().push(job);
        ready.notify_one();
    }
}

fn worker(queue: &(Mutex<Vec<Job>>, Condvar), replies: &async_channel::Sender<Reply>) {
    let (jobs, ready) = queue;
    loop {
        let job = {
            let mut jobs = jobs.lock().unwrap();
            while jobs.is_empty() {
                jobs = ready.wait(jobs).unwrap();
            }
            jobs.pop().unwrap()
        };

        if job.cancel.load(Ordering::Relaxed) {
            continue;
        }
        let result = decode::decode(&job.path, job.target);
        if job.cancel.load(Ordering::Relaxed) {
            continue;
        }
        if replies.send_blocking(Reply { path: job.path, target: job.target, result }).is_err() {
            return;
        }
    }
}
