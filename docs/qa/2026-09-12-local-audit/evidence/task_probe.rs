#[path = "../../../../crates/app/src/task.rs"]
#[allow(dead_code)]
mod task;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc};
use std::time::Duration;
fn main() {
    let pool = task::Pool::new(1, 4);
    let (sent, received) = mpsc::channel();
    let task = pool.spawn(move || {sent.send(()).unwrap();}, |_| 42).unwrap();
    received.recv_timeout(Duration::from_secs(2)).unwrap();
    task.cancel();
    println!("cancel_after_notify_is_cancelled={}; result_after_cancel={:?}", task.is_cancelled(), task.poll());
    let pool = task::Pool::new(1, 4);
    let cancelled = pool.spawn(|| {}, |_| 7).unwrap();
    cancelled.cancel();
    std::thread::sleep(Duration::from_millis(100));
    println!("cancelled_terminal_poll={:?}", cancelled.poll());
    let panic_pool = task::Pool::new(1, 4);
    let dying = panic_pool.spawn(|| {}, |_| -> i32 {panic!("controlled audit worker failure")}).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    println!("panic_terminal_poll={:?}; workers_reported={}; running_reported={}; accepts_next={}", dying.poll(), panic_pool.worker_count(), panic_pool.running(), panic_pool.spawn(|| {}, |_| 1).is_ok());
    let drop_pool = task::Pool::new(1,4);
    let saw_cancel = Arc::new(AtomicBool::new(false));
    let observed = saw_cancel.clone();
    let (start, started) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let dropped = drop_pool.spawn(|| {}, move |cancel| {start.send(()).unwrap();released.recv().unwrap();observed.store(cancel.is_cancelled(),Ordering::SeqCst);}).unwrap();
    started.recv().unwrap(); drop(dropped); release.send(()).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    println!("drop_signalled_cancel={}", saw_cancel.load(Ordering::SeqCst));
    let queue_pool = task::Pool::new(2,4);
    let (release_q, released_q) = mpsc::channel();
    let blocked = queue_pool.spawn(||{}, move |_| {released_q.recv().unwrap();}).unwrap();
    let free = queue_pool.spawn(||{}, |_| 2).unwrap();
    println!("free_worker_result={:?}", free.wait_timeout(Duration::from_secs(1)));
    let behind_blocked = queue_pool.spawn(||{}, |_| 3).unwrap();
    println!("third_job_while_other_worker_idle={:?}", behind_blocked.wait_timeout(Duration::from_millis(100)));
    release_q.send(()).unwrap();
    println!("third_job_after_blocked_worker_release={:?}", behind_blocked.wait_timeout(Duration::from_secs(1)));
    let _ = blocked;
}

