//! POSIX shared memory segments that hand frames to a local terminal.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Write as _};

use rustix::fs::Mode;
use rustix::io::Errno;
use rustix::shm;

/// Segments the terminal has not read yet are unlinked once this many newer ones exist.
const MAX_UNREAD_SEGMENTS: usize = 4;

/// Creates uniquely named segments and cleans up the ones a terminal never consumed.
///
/// The terminal unlinks a segment after reading it. If it stops reading (for example after a
/// crash), old segments would otherwise pile up in `/dev/shm`.
pub struct SegmentPool {
    prefix: String,
    next_index: u64,
    written: VecDeque<String>,
}

impl SegmentPool {
    pub fn new(label: &str) -> Self {
        Self {
            prefix: format!("/gpui-tui-{}-{label}", std::process::id()),
            next_index: 0,
            written: VecDeque::new(),
        }
    }

    /// Writes `data` to a fresh segment and returns its name.
    pub fn write(&mut self, data: &[u8]) -> io::Result<String> {
        let name = format!("{}-{}", self.prefix, self.next_index);
        self.next_index += 1;
        write_segment(&name, data)?;

        self.written.push_back(name.clone());
        while self.written.len() > MAX_UNREAD_SEGMENTS {
            if let Some(stale) = self.written.pop_front() {
                unlink_segment(&stale);
            }
        }
        Ok(name)
    }
}

impl Drop for SegmentPool {
    fn drop(&mut self) {
        for name in self.written.drain(..) {
            unlink_segment(&name);
        }
    }
}

/// Creates a new segment containing exactly `data`.
pub fn write_segment(name: &str, data: &[u8]) -> io::Result<()> {
    let fd = shm::open(
        name,
        shm::OFlags::CREATE | shm::OFlags::EXCL | shm::OFlags::RDWR,
        Mode::RUSR | Mode::WUSR,
    )?;
    let mut file = File::from(fd);
    // Linux supports write() on shm objects; a full /dev/shm fails with ENOSPC here instead of
    // a SIGBUS later, as could happen with mmap.
    let result = file.write_all(data);
    if result.is_err() {
        unlink_segment(name);
    }
    result
}

/// Unlinks a segment. A missing segment means the terminal already consumed it.
pub fn unlink_segment(name: &str) {
    match shm::unlink(name) {
        Ok(()) | Err(Errno::NOENT) => {}
        Err(error) => log::warn!("failed to unlink shared memory segment {name}: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment_exists(name: &str) -> bool {
        shm::open(name, shm::OFlags::RDONLY, Mode::empty()).is_ok()
    }

    #[test]
    fn pool_keeps_only_recent_unread_segments() {
        let mut pool = SegmentPool::new("test-recent");
        let names: Vec<String> = (0..6)
            .map(|index| pool.write(&[index; 4]))
            .collect::<io::Result<_>>()
            .unwrap_or_default();
        assert_eq!(names.len(), 6);
        assert!(!segment_exists(&names[0]));
        assert!(!segment_exists(&names[1]));
        assert!(segment_exists(&names[5]));

        drop(pool);
        assert!(names.iter().all(|name| !segment_exists(name)));
    }

    #[test]
    fn already_consumed_segment_is_not_an_error() {
        let mut pool = SegmentPool::new("test-consumed");
        let name = pool.write(b"data").unwrap_or_default();
        unlink_segment(&name);
        drop(pool);
        assert!(!segment_exists(&name));
    }
}
