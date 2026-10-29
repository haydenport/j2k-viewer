//! Background tile decoding. The main thread publishes the complete, prioritised list of tiles it
//! wants; workers take from the front. Publishing again replaces everything that has not started,
//! so stale requests (after a pan or zoom) are dropped instead of clogging the queue.

use std::collections::{HashSet, VecDeque};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use decoder::{Gray8Buf, J2kFile, TileDecoder};

use crate::view::{tile_region, TileKey};

pub struct Job {
    pub image: usize,
    pub key: TileKey,
    pub file: Arc<J2kFile>,
}

pub struct Decoded {
    pub image: usize,
    pub key: TileKey,
    pub result: Result<Gray8Buf, String>,
    /// Wall time the decode took on its worker, in milliseconds.
    pub ms: f64,
}

#[derive(Default)]
struct Queue {
    pending: VecDeque<Job>,
    /// Queued or in flight. Entries leave when the main thread collects the result.
    requested: HashSet<(usize, TileKey)>,
    shutdown: bool,
}

pub type Waker = Arc<dyn Fn() + Send + Sync>;

pub struct Scheduler {
    shared: Arc<(Mutex<Queue>, Condvar)>,
    results: Receiver<Decoded>,
    threads: Vec<JoinHandle<()>>,
    waker: Waker,
}

impl Scheduler {
    /// `wake` is called from a worker thread whenever a result is ready.
    pub fn new(workers: usize, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let shared = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let (tx, results) = channel();
        let wake: Waker = Arc::new(wake);
        let threads = (0..workers.max(1))
            .map(|i| {
                let (shared, tx, wake) = (shared.clone(), tx.clone(), wake.clone());
                std::thread::Builder::new()
                    .name(format!("tile-decode-{i}"))
                    .spawn(move || worker(&shared, &tx, &*wake))
                    .expect("spawn decode worker")
            })
            .collect();
        Self {
            shared,
            results,
            threads,
            waker: wake,
        }
    }

    /// The callback that asks the UI thread to redraw; other background work (such as the image
    /// info scan) uses it to announce its result.
    pub fn waker(&self) -> Waker {
        self.waker.clone()
    }

    /// Replace everything not yet started with `wanted` (highest priority first). Tiles already in
    /// flight are not queued again.
    pub fn set_wanted(&self, wanted: Vec<Job>) {
        let (lock, cv) = &*self.shared;
        let mut q = lock.lock().unwrap();
        let dropped: Vec<_> = q.pending.drain(..).map(|j| (j.image, j.key)).collect();
        for k in dropped {
            q.requested.remove(&k);
        }
        for job in wanted {
            if q.requested.insert((job.image, job.key)) {
                q.pending.push_back(job);
            }
        }
        cv.notify_all();
    }

    /// Next finished tile, if any.
    pub fn try_recv(&self) -> Option<Decoded> {
        let d = self.results.try_recv().ok()?;
        self.shared
            .0
            .lock()
            .unwrap()
            .requested
            .remove(&(d.image, d.key));
        Some(d)
    }

    /// Tiles queued or being decoded (not yet collected).
    pub fn outstanding(&self) -> usize {
        self.shared.0.lock().unwrap().requested.len()
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        {
            let (lock, cv) = &*self.shared;
            let mut q = lock.lock().unwrap();
            q.shutdown = true;
            q.pending.clear();
            cv.notify_all();
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

fn worker(
    shared: &(Mutex<Queue>, Condvar),
    tx: &std::sync::mpsc::Sender<Decoded>,
    wake: &dyn Fn(),
) {
    let (lock, cv) = shared;
    loop {
        let job = {
            let mut q = lock.lock().unwrap();
            loop {
                if q.shutdown {
                    return;
                }
                if let Some(j) = q.pending.pop_front() {
                    break j;
                }
                q = cv.wait(q).unwrap();
            }
        };
        let t = std::time::Instant::now();
        let result = match tile_region(job.file.info(), job.key) {
            Some(region) => job.file.decode(region, job.key.level),
            None => Err(format!("tile {:?} outside image", job.key)),
        };
        let ms = t.elapsed().as_secs_f64() * 1e3;
        if tx
            .send(Decoded {
                image: job.image,
                key: job.key,
                result,
                ms,
            })
            .is_err()
        {
            return;
        }
        wake();
    }
}
