//! Reaping of short-lived child processes spawned by the tray daemon.
//!
//! The daemon outlives every GUI window it opens, so it must wait on the
//! children it spawns; otherwise each closed window stays as a zombie until
//! the daemon exits.

use std::process::Child;

/// Children spawned by the daemon that have not been reaped yet.
#[derive(Default)]
pub struct Children {
    children: Vec<Child>,
}

impl Children {
    /// Remember a child so [`Children::reap`] can collect it later.
    pub fn track(&mut self, child: Child) {
        self.children.push(child);
    }

    /// Collect children that have exited and return how many were reaped.
    /// Running children stay tracked.
    pub fn reap(&mut self) -> usize {
        let mut reaped = 0;
        self.children.retain_mut(|child| match child.try_wait() {
            Ok(Some(_)) => {
                reaped += 1;
                false
            }
            Ok(None) | Err(_) => true,
        });
        reaped
    }

    pub fn len(&self) -> usize {
        self.children.len()
    }

    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }
}
