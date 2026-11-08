//! Running the jobs of a harness.
//!
//! A harness has long jobs: a world build, an image, a boot, a test suite. [`job`] runs one of
//! them on this machine or on another one over ssh, with a timeout and a log file. [`pool`] runs a
//! list of tasks on a bounded number of threads and gives the results back in order. [`run_all`]
//! puts the two together.
//!
//! This is the first version. It does not choose hosts. Each job names the host it runs on, and
//! the multi-machine scheduler that would place jobs, copy their inputs and retry them elsewhere
//! is not written yet.

pub mod job;
pub mod pool;

pub use job::{Host, Job, Outcome};

/// Run every job, at most `jobs` at once, and give back the outcomes in the order of the jobs.
#[must_use]
pub fn run_all(jobs: usize, list: &[Job]) -> Vec<Outcome> {
    let tasks: Vec<pool::Task<'_, Outcome>> = list
        .iter()
        .map(|job| -> pool::Task<'_, Outcome> { Box::new(move || job::run(job)) })
        .collect();
    pool::run(jobs, tasks)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn every_job_runs_and_the_outcomes_keep_their_order() {
        let list: Vec<Job> = (0..5)
            .map(|i| {
                let code = format!("exit {i}");
                Job::new(&format!("j{i}"), Host::Local, &["sh", "-c", &code])
            })
            .collect();
        let outcomes = run_all(3, &list);
        let exits: Vec<Option<i32>> = outcomes.iter().map(|o| o.exit).collect();
        assert_eq!(exits, [Some(0), Some(1), Some(2), Some(3), Some(4)]);
        assert_eq!(outcomes[4].name, "j4");
    }
}
