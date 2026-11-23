//! Running child processes, with a bounded retry for the transient
//! `ETXTBSY` a test can see when another thread still holds a freshly written
//! fake script open for writing.

use std::io;
use std::process::{Child, Command, Output};
use std::thread;
use std::time::Duration;

/// ETXTBSY is transient: the writer's fd closes within microseconds.
const BUSY_RETRIES: usize = 50;
const BUSY_WAIT: Duration = Duration::from_millis(2);

fn retry<T>(mut run: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut attempts = 0;
    loop {
        match run() {
            Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) && attempts < BUSY_RETRIES => {
                attempts += 1;
                thread::sleep(BUSY_WAIT);
            }
            other => return other,
        }
    }
}

pub fn output(mut cmd: Command) -> io::Result<Output> {
    retry(|| cmd.output())
}

pub fn spawn(mut cmd: Command) -> io::Result<Child> {
    retry(|| cmd.spawn())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_busy_until_the_exec_succeeds() {
        let mut calls = 0;
        let value = retry(|| {
            calls += 1;
            if calls < 3 {
                Err(io::Error::from_raw_os_error(libc::ETXTBSY))
            } else {
                Ok(7)
            }
        })
        .unwrap();
        assert_eq!(value, 7);
        assert_eq!(calls, 3);
    }

    #[test]
    fn gives_up_after_the_bounded_budget() {
        let mut calls = 0;
        let err = retry(|| {
            calls += 1;
            Err::<(), _>(io::Error::from_raw_os_error(libc::ETXTBSY))
        })
        .unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::ETXTBSY));
        assert_eq!(calls, BUSY_RETRIES + 1);
    }
}
