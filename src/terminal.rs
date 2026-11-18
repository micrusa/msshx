use crate::approval::Request;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

pub fn run(command: &mut Command, approvals: &Receiver<Request>) -> io::Result<ExitStatus> {
    let mut tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
    let fd = tty.as_raw_fd();
    let mut settings = unsafe { std::mem::zeroed() };
    check(unsafe { libc::tcgetattr(fd, &mut settings) })?;
    let window = window_size(fd)?;
    let (mut master, slave) = open_pty(&settings, &window)?;
    command
        .stdin(slave.try_clone()?)
        .stdout(slave.try_clone()?)
        .stderr(slave);
    unsafe {
        command.pre_exec(|| {
            check(libc::setsid())?;
            check(libc::ioctl(0, libc::TIOCSCTTY, 0))?;
            Ok(())
        });
    }

    let stop = Arc::new(AtomicBool::new(false));
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        signal_hook::flag::register(signal, Arc::clone(&stop))?;
    }
    let mut child = command.spawn()?;
    // Drop the parent's copies of the slave so the master sees EOF when SSH exits.
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    let result = (|| {
        let mut raw = settings;
        unsafe { libc::cfmakeraw(&mut raw) };
        check(unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) })?;
        let _restore = RestoreTerminal(fd, settings);
        relay(&mut tty, &mut master, approvals, window, &stop)
    })();
    if result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait();
    result.and(status)
}

fn relay(
    tty: &mut File,
    master: &mut File,
    approvals: &Receiver<Request>,
    mut window: libc::winsize,
    stop: &AtomicBool,
) -> io::Result<()> {
    let tty_fd = tty.as_raw_fd();
    let master_fd = master.as_raw_fd();
    let mut output = io::stdout().lock();
    let mut prompt: Option<Prompt> = None;
    let mut buf = [0; 8192];

    while !stop.load(Ordering::Relaxed) {
        if let Ok(size) = window_size(tty_fd)
            && (size.ws_row, size.ws_col, size.ws_xpixel, size.ws_ypixel)
                != (
                    window.ws_row,
                    window.ws_col,
                    window.ws_xpixel,
                    window.ws_ypixel,
                )
        {
            check(unsafe { libc::ioctl(master_fd, libc::TIOCSWINSZ, &size) })?;
            window = size;
        }

        if prompt
            .as_ref()
            .is_some_and(|p| Instant::now() >= p.deadline)
        {
            tty.write_all(b"\r\nApproval timed out.\r\n")?;
            let _ = prompt.take().unwrap().request.answer.send(false);
        }
        if prompt.is_none()
            && let Ok(request) = approvals.try_recv()
        {
            check(unsafe { libc::tcflush(tty_fd, libc::TCIFLUSH) })?;
            tty.write_all(b"\x1b[0m\r\n")?;
            tty.write_all(request.message.as_bytes())?;
            prompt = Some(Prompt {
                request,
                input: Vec::new(),
                deadline: Instant::now() + Duration::from_secs(45),
            });
        }

        let mut fds = [
            libc::pollfd {
                fd: tty_fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: master_fd,
                events: if prompt.is_none() { libc::POLLIN } else { 0 },
                revents: 0,
            },
        ];
        if let Err(error) = check(unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, 50) }) {
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }

        if fds[1].revents != 0 {
            if prompt.is_some() {
                return Ok(());
            }
            match master.read(&mut buf) {
                Ok(0) => return Ok(()),
                Ok(count) => {
                    output.write_all(&buf[..count])?;
                    output.flush()?;
                }
                Err(error) if error.raw_os_error() == Some(libc::EIO) => return Ok(()),
                Err(error) => return Err(error),
            }
        }

        if fds[0].revents != 0 {
            let count = tty.read(&mut buf)?;
            if count == 0 {
                break;
            }
            if let Some(active) = &mut prompt {
                if let Some(allowed) = active.read_answer(tty, &buf[..count])? {
                    tty.write_all(b"\r\n")?;
                    let _ = prompt.take().unwrap().request.answer.send(allowed);
                }
            } else {
                master.write_all(&buf[..count])?;
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::Interrupted,
        "SSH terminal closed",
    ))
}

struct Prompt {
    request: Request,
    input: Vec<u8>,
    deadline: Instant,
}

impl Prompt {
    fn read_answer(&mut self, tty: &mut File, bytes: &[u8]) -> io::Result<Option<bool>> {
        for &byte in bytes {
            match byte {
                b'\r' | b'\n' => {
                    let valid = matches!(self.input.as_slice(), b"" | b"y" | b"Y");
                    return Ok(Some(valid));
                }
                3 | 4 => return Ok(Some(false)),
                8 | 127 => {
                    if self.input.pop().is_some() {
                        tty.write_all(b"\x08 \x08")?;
                    }
                }
                b' '..=b'~' if self.input.len() < 32 => {
                    self.input.push(byte);
                    tty.write_all(&[byte])?;
                }
                _ => return Ok(Some(false)),
            }
        }
        Ok(None)
    }
}

fn open_pty(settings: &libc::termios, window: &libc::winsize) -> io::Result<(File, File)> {
    let (mut master, mut slave) = (-1, -1);
    check(unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            settings,
            window,
        )
    })?;
    let (master, slave) = unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
    for file in [&master, &slave] {
        check(unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) })?;
    }
    Ok((master, slave))
}

fn window_size(fd: RawFd) -> io::Result<libc::winsize> {
    let mut size = unsafe { std::mem::zeroed() };
    check(unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut size) })?;
    Ok(size)
}

struct RestoreTerminal(RawFd, libc::termios);

impl Drop for RestoreTerminal {
    fn drop(&mut self) {
        unsafe { libc::tcsetattr(self.0, libc::TCSAFLUSH, &self.1) };
    }
}

fn check(result: libc::c_int) -> io::Result<()> {
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
