//! Tasks run side by side on a bounded number of threads.
//!
//! From rucc-kernel, where boots of one suite run at once. A boot is one QEMU with two CPUs, so on
//! a runner with four, one boot at a time leaves half the machine idle. The tasks must not read
//! each other's files.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// One piece of work for [`run`].
pub type Task<'a, T> = Box<dyn FnOnce() -> T + Send + 'a>;

/// How many tasks run at once when nobody says: one for every `cpus_each` host CPUs, and at
/// least one. A boot of a two CPU guest takes two.
#[must_use]
pub fn default_jobs(cpus_each: usize) -> usize {
    std::thread::available_parallelism().map_or(1, |n| (n.get() / cpus_each.max(1)).max(1))
}

/// Run every task, at most `jobs` of them at once, and answer what each returned, in the order of
/// the tasks.
#[must_use]
pub fn run<T: Send>(jobs: usize, tasks: Vec<Task<'_, T>>) -> Vec<T> {
    let count = tasks.len();
    let next = AtomicUsize::new(0);
    let tasks: Vec<Mutex<Option<Task<'_, T>>>> =
        tasks.into_iter().map(|t| Mutex::new(Some(t))).collect();
    let results: Vec<Mutex<Option<T>>> = (0..count).map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..jobs.clamp(1, count.max(1)) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(slot) = tasks.get(i) else { break };
                    let task = slot.lock().ok().and_then(|mut t| t.take());
                    if let (Some(task), Ok(mut result)) = (task, results[i].lock()) {
                        *result = Some(task());
                    }
                }
            });
        }
    });
    results
        .into_iter()
        .map(|r| {
            r.into_inner()
                .ok()
                .flatten()
                .expect("every task ran to the end")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn the_results_come_back_in_the_order_of_the_tasks() {
        let tasks: Vec<Task<'_, usize>> = (0..7_usize)
            .map(|i| -> Task<'_, usize> {
                Box::new(move || {
                    std::thread::sleep(Duration::from_millis(((7 - i) * 5) as u64));
                    i * 10
                })
            })
            .collect();
        assert_eq!(run(3, tasks), vec![0, 10, 20, 30, 40, 50, 60]);
        assert_eq!(run::<u8>(2, Vec::new()), Vec::<u8>::new());
    }

    #[test]
    fn there_is_always_at_least_one_job() {
        assert!(default_jobs(1) >= default_jobs(2));
        assert!(default_jobs(usize::MAX) >= 1);
        assert!(default_jobs(0) >= 1);
    }

    #[test]
    fn two_jobs_run_two_tasks_at_once() {
        let start = Instant::now();
        let tasks: Vec<Task<'_, ()>> = (0..2)
            .map(|_| -> Task<'_, ()> {
                Box::new(|| std::thread::sleep(Duration::from_millis(300)))
            })
            .collect();
        let _ = run(2, tasks);
        assert!(
            start.elapsed() < Duration::from_millis(550),
            "{:?}",
            start.elapsed()
        );
    }
}
