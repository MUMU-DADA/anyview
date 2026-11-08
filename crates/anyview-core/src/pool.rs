//! 后台线程池：按优先级取任务，同优先级先进先出（ARCHITECTURE"后台任务、缓存与预加载"）。
//! 任务开始前由任务自己检查是否已过期（代次、是否仍在预加载范围内），过期就直接返回。
//! 预加载、缩略图这些不急的任务最多同时占"线程数 − 1"个线程（只有一个线程时可以占它）：总留一个给当前图片和
//! 目录扫描。正在解的任务停不下来，不留的话，打开、跳转之后要等一张预读的图片解完才轮到要看的那张。

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    /// 当前图片。
    Current = 0,
    /// 目录扫描。
    Scan = 1,
    /// 预加载相邻图片。
    Prefetch = 2,
    /// 选择图片窗口的缩略图（看得见的几张）。
    Thumbnail = 3,
}

impl Priority {
    /// 不急的任务：不占留给当前图片、目录扫描的那个线程。
    fn is_background(self) -> bool {
        self >= Priority::Prefetch
    }
}

type Task = Box<dyn FnOnce() + Send + 'static>;

struct Job {
    priority: Priority,
    seq: u64,
    task: Task,
}

impl PartialEq for Job {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.seq == other.seq
    }
}

impl Eq for Job {}

impl PartialOrd for Job {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Job {
    // BinaryHeap 先弹出"最大"的：优先级数值小的、序号小的排在前面。
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .priority
            .cmp(&self.priority)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}

#[derive(Default)]
struct Queue {
    jobs: BinaryHeap<Job>,
    next_seq: u64,
    shutdown: bool,
    /// 正在做的不急的任务有几个。
    background: usize,
}

pub struct Pool {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    workers: Vec<thread::JoinHandle<()>>,
}

impl Pool {
    pub fn new(threads: usize) -> Self {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let threads = threads.max(1);
        let background_limit = (threads - 1).max(1);
        let workers = (0..threads)
            .map(|i| {
                let queue = Arc::clone(&queue);
                thread::Builder::new()
                    .name(format!("anyview-worker-{i}"))
                    .spawn(move || worker_loop(&queue, background_limit))
                    .expect("failed to spawn anyview worker thread")
            })
            .collect();
        Self { queue, workers }
    }

    pub fn submit(&self, priority: Priority, task: impl FnOnce() + Send + 'static) {
        let (lock, ready) = &*self.queue;
        let mut queue = lock.lock().unwrap_or_else(|p| p.into_inner());
        let seq = queue.next_seq;
        queue.next_seq += 1;
        queue.jobs.push(Job {
            priority,
            seq,
            task: Box::new(task),
        });
        ready.notify_one();
    }

    /// 后台线程数（测试用）。
    #[cfg(test)]
    pub fn threads(&self) -> usize {
        self.workers.len()
    }

    /// 排着队、还没开始的任务数（测试用）。
    #[cfg(test)]
    pub fn queued(&self) -> usize {
        self.queue.0.lock().unwrap_or_else(|p| p.into_inner()).jobs.len()
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        {
            let (lock, ready) = &*self.queue;
            let mut queue = lock.lock().unwrap_or_else(|p| p.into_inner());
            queue.shutdown = true;
            queue.jobs.clear();
            ready.notify_all();
        }
        let current = thread::current().id();
        for worker in self.workers.drain(..) {
            // 最后一个引用可能在工作线程里释放，不能 join 自己。
            if worker.thread().id() != current {
                let _ = worker.join();
            }
        }
    }
}

fn worker_loop(queue: &(Mutex<Queue>, Condvar), background_limit: usize) {
    let (lock, ready) = queue;
    loop {
        let job = {
            let mut q = lock.lock().unwrap_or_else(|p| p.into_inner());
            loop {
                if q.shutdown {
                    return;
                }
                // 排在最前面的是不急的任务、而它们已经占满了能占的线程：等（有任务做完、来了急的任务时再看）。
                let runnable = q
                    .jobs
                    .peek()
                    .is_some_and(|job| !job.priority.is_background() || q.background < background_limit);
                if runnable && let Some(job) = q.jobs.pop() {
                    if job.priority.is_background() {
                        q.background += 1;
                    }
                    break job;
                }
                q = ready.wait(q).unwrap_or_else(|p| p.into_inner());
            }
        };
        let background = job.priority.is_background();
        // 任务里出错（panic）只结束这个任务，线程留下来接着干活，后台线程不会越用越少。
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job.task));
        if background {
            let mut q = lock.lock().unwrap_or_else(|p| p.into_inner());
            q.background -= 1;
            // 等着的线程里可能有在等这个空位的。
            ready.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn runs_higher_priority_first() {
        let pool = Pool::new(1);
        let (tx, rx) = mpsc::channel();
        // 先占住唯一的线程，再排入三个任务。
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        pool.submit(Priority::Current, move || {
            let _ = gate_rx.recv();
        });
        for (priority, tag) in [
            (Priority::Prefetch, "prefetch"),
            (Priority::Scan, "scan"),
            (Priority::Current, "current"),
        ] {
            let tx = tx.clone();
            pool.submit(priority, move || tx.send(tag).unwrap());
        }
        gate_tx.send(()).unwrap();
        let order: Vec<&str> = (0..3)
            .map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap())
            .collect();
        assert_eq!(order, ["current", "scan", "prefetch"]);
    }

    /// 预读、缩略图最多占"线程数 − 1"个线程：两个都在解很慢的图片时，当前图片不用等它们。
    #[test]
    fn background_jobs_leave_a_thread_for_the_current_image() {
        let pool = Pool::new(2);
        let (tx, rx) = mpsc::channel();
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let gate = Arc::new(Mutex::new(gate_rx));
        for tag in ["prefetch 1", "prefetch 2"] {
            let (tx, gate) = (tx.clone(), Arc::clone(&gate));
            pool.submit(Priority::Prefetch, move || {
                tx.send(tag).unwrap();
                let _ = gate.lock().unwrap().recv();
            });
        }
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "prefetch 1");
        let current = tx.clone();
        pool.submit(Priority::Current, move || current.send("current").unwrap());
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "current", "第二个预读不占留下的线程");
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err(), "第一个还没做完时第二个不开始");
        gate_tx.send(()).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "prefetch 2", "空出来以后接着做");
        gate_tx.send(()).unwrap();
        // 只有一个线程时不急的任务也能用它。
        let single = Pool::new(1);
        let done = tx.clone();
        single.submit(Priority::Thumbnail, move || done.send("thumbnail").unwrap());
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "thumbnail");
    }
}
