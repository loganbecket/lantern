//! A pool of decode threads that always work on the photo nearest the
//! viewport first.
//!
//! GTK creates cells for a good stretch above and below the visible area, so
//! the queue is ordered by distance from the current scroll position rather
//! than by arrival. Cells that scroll away cancel their request before a
//! thread picks it up, so fast scrolling does not pile up wasted work.
//!
//! Each photo is loaded in two steps: a cheap embedded preview from the
//! file header, then the real thumbnail. All pending previews go before any
//! full decode, so on slow storage every cell shows something recognizable
//! quickly and sharpens as bandwidth allows; on a local disk previews cost
//! a few milliseconds and nobody notices the detour.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use crate::decode::{self, Decoded};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Kind {
    /// The tiny thumbnail embedded in the file header.
    Preview,
    /// A real decode at `target` pixels.
    Full,
}

pub struct Job {
    pub path: PathBuf,
    pub kind: Kind,
    pub target: u32,
    /// Index of the photo in the grid, used to order the queue.
    pub position: u32,
    pub cancel: Arc<AtomicBool>,
}

pub struct Reply {
    pub path: PathBuf,
    pub kind: Kind,
    pub target: u32,
    pub result: Result<Decoded, String>,
}

#[derive(Clone)]
pub struct Pool {
    queue: Arc<(Mutex<Vec<Job>>, Condvar)>,
    /// Index of the first photo currently on screen.
    focus: Arc<AtomicU32>,
}

impl Pool {
    pub fn new(replies: async_channel::Sender<Reply>) -> Self {
        let queue = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
        let focus = Arc::new(AtomicU32::new(0));
        let threads = thread::available_parallelism().map_or(4, |n| n.get()).saturating_sub(1).max(2);

        for _ in 0..threads {
            let queue = queue.clone();
            let focus = focus.clone();
            let replies = replies.clone();
            thread::spawn(move || worker(&queue, &focus, &replies));
        }

        Self { queue, focus }
    }

    pub fn submit(&self, job: Job) {
        let (jobs, ready) = &*self.queue;
        jobs.lock().unwrap().push(job);
        ready.notify_one();
    }

    pub fn set_focus(&self, position: u32) {
        self.focus.store(position, Ordering::Relaxed);
    }
}

fn worker(queue: &(Mutex<Vec<Job>>, Condvar), focus: &AtomicU32, replies: &async_channel::Sender<Reply>) {
    let (jobs, ready) = queue;
    loop {
        let job = {
            let mut jobs = jobs.lock().unwrap();
            // Drop canceled work without ever starting it.
            jobs.retain(|j| !j.cancel.load(Ordering::Relaxed));
            while jobs.is_empty() {
                jobs = ready.wait(jobs).unwrap();
                jobs.retain(|j| !j.cancel.load(Ordering::Relaxed));
            }
            // The queue stays small (a few hundred at most), so a scan beats
            // keeping it sorted while the focus keeps moving.
            let focus = focus.load(Ordering::Relaxed) as i64;
            let nearest = jobs
                .iter()
                .enumerate()
                .min_by_key(|(_, j)| (j.kind, (j.position as i64 - focus).abs()))
                .map(|(i, _)| i)
                .unwrap();
            jobs.swap_remove(nearest)
        };

        let result = match job.kind {
            Kind::Preview => decode::preview(&job.path).ok_or_else(|| "no embedded preview".to_string()),
            Kind::Full => decode::decode(&job.path, job.target, &job.cancel),
        };
        if job.cancel.load(Ordering::Relaxed) {
            continue;
        }
        let reply = Reply { path: job.path, kind: job.kind, target: job.target, result };
        if replies.send_blocking(reply).is_err() {
            return;
        }
    }
}
