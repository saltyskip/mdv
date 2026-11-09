use crate::core::terminal_palette::TerminalPalette;

#[cfg(target_os = "macos")]
#[must_use]
pub fn query_terminal_palette() -> Option<TerminalPalette> {
    use std::{
        io::{self, Write},
        os::fd::AsFd,
        time::{Duration, Instant},
    };

    use rustix::event::{PollFd, PollFlags, Timespec, poll};

    const TIMEOUT: Duration = Duration::from_millis(400);

    crossterm::terminal::enable_raw_mode().ok()?;
    let replies = (|| {
        let mut stdout = io::stdout().lock();
        stdout.write_all(TerminalPalette::QUERY.as_bytes()).ok()?;
        stdout.flush().ok()?;

        let stdin = io::stdin();
        let deadline = Instant::now() + TIMEOUT;
        let mut replies = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !TerminalPalette::replies_complete(&replies) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let timeout = Timespec {
                tv_sec: remaining.as_secs() as _,
                tv_nsec: i64::from(remaining.subsec_nanos()) as _,
            };
            let mut fds = [PollFd::new(&stdin, PollFlags::IN)];
            if poll(&mut fds, Some(&timeout)).ok()? == 0 {
                break;
            }
            let read = rustix::io::read(stdin.as_fd(), &mut buffer).ok()?;
            if read == 0 {
                break;
            }
            replies.extend_from_slice(&buffer[..read]);
        }
        Some(replies)
    })();
    let _ = crossterm::terminal::disable_raw_mode();
    TerminalPalette::from_replies(&replies?)
}

#[cfg(not(target_os = "macos"))]
#[must_use]
pub fn query_terminal_palette() -> Option<TerminalPalette> {
    None
}
