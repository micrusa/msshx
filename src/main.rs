mod approval;
mod client;
mod shell;
mod terminal;

use std::env;
use std::ffi::OsString;
use std::io::{self, IsTerminal, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let result = if io::stdin().is_terminal() && interactive(&args) {
        run(&args)
    } else {
        Err(Command::new("ssh").args(&args).exec().into())
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("msshx: {error}");
            ExitCode::FAILURE
        }
    }
}

fn interactive(args: &[OsString]) -> bool {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let Some(arg) = arg.to_str() else {
            return false;
        };
        if arg == "--" {
            return args.len() == 1;
        }
        if arg == "-" {
            return false;
        }
        let Some(options) = arg.strip_prefix('-') else {
            return args.len() == 0;
        };
        for (index, option) in options.char_indices() {
            match option {
                'f' | 'G' | 'M' | 'N' | 'n' | 'O' | 'Q' | 'S' | 's' | 'T' | 'V' | 'W' => {
                    return false;
                }
                'B' | 'b' | 'c' | 'D' | 'E' | 'e' | 'F' | 'I' | 'i' | 'J' | 'L' | 'l' | 'm'
                | 'o' | 'P' | 'p' | 'R' | 'w' => {
                    if index + 1 == options.len() && args.next().is_none() {
                        return false;
                    }
                    break;
                }
                '4' | '6' | 'A' | 'a' | 'C' | 'g' | 'K' | 'k' | 'q' | 't' | 'v' | 'X' | 'x'
                | 'Y' | 'y' => {}
                _ => return false,
            }
        }
    }
    false
}

fn run(args: &[OsString]) -> Result<u8, Box<dyn std::error::Error + Send + Sync>> {
    let config = shell::local_config();
    let mut random = [0; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
    let token: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let directory = tempfile::Builder::new()
        .prefix("msshx-")
        .tempdir_in("/tmp")?;
    let local = directory.path().join("client.sock");
    let server = tiny_http::Server::http_unix(&local)?;
    let mut session = Session {
        socket: directory.path().join("ctl"),
        remote: String::new(),
    };
    let status = session
        .ssh()
        .args(["-o", "ControlMaster=yes", "-o", "ControlPersist=no", "-fN"])
        .args(args)
        .status()?;
    if !status.success() {
        return Ok(exit_code(status));
    }

    let output = session
        .remote_command("umask 077; mktemp -d /tmp/msshx.XXXXXXXX")?
        .stdin(std::process::Stdio::null())
        .output()?;
    let remote = std::str::from_utf8(&output.stdout)?.trim();
    if !output.status.success()
        || !remote.strip_prefix("/tmp/msshx.").is_some_and(|suffix| {
            suffix.len() == 8 && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
    {
        return Err("could not create a private directory on the SSH server".into());
    }
    session.remote = remote.to_owned();
    let mut setup = session
        .remote_command(&format!("umask 077; cat > {remote}/token"))?
        .stdin(Stdio::piped())
        .spawn()?;
    let written = setup.stdin.take().unwrap().write_all(token.as_bytes());
    if !setup.wait()?.success() || written.is_err() {
        return Err("could not provision the remote socket token".into());
    }

    let forward = format!("{remote}/client.sock:{}", local.display());
    let status = control(&session.socket)
        .args(["-O", "forward", "-R", &forward, "msshx.invalid"])
        .status()?;
    if !status.success() {
        return Ok(exit_code(status));
    }

    let socket = session.socket.clone();
    let (approver, approvals) = approval::channel();
    std::thread::spawn(move || {
        client::serve(server, &socket, &format!("Bearer {token}"), &approver)
    });
    let command = shell::remote_command(&session.remote, &config);
    let mut ssh = session.ssh();
    ssh.args(["-o", "ControlMaster=no", "-t"])
        .args(args)
        .arg(command);
    let status = terminal::run(&mut ssh, &approvals)?;
    Ok(exit_code(status))
}

fn exit_code(status: std::process::ExitStatus) -> u8 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(1) as u8
}

fn control(socket: &Path) -> Command {
    let mut command = Command::new("ssh");
    command.arg("-S").arg(socket).args(["-F", "/dev/null"]);
    command
}

struct Session {
    socket: PathBuf,
    remote: String,
}

impl Session {
    fn ssh(&self) -> Command {
        let mut command = Command::new("ssh");
        command.arg("-S").arg(&self.socket);
        command
    }

    fn remote_command(&self, script: &str) -> io::Result<Command> {
        // Check the master before issuing standalone cleanup or setup commands.
        if !control(&self.socket)
            .args(["-O", "check", "msshx.invalid"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?
            .success()
        {
            return Err(io::Error::other("SSH control connection is unavailable"));
        }
        let mut command = control(&self.socket);
        command.args(["-T", "msshx.invalid", script]);
        Ok(command)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        use std::process::Stdio;
        if !self.remote.is_empty() {
            let script = format!("rm -f {0}/client.sock {0}/token; rmdir {0}", self.remote);
            if let Ok(mut command) = self.remote_command(&script) {
                let _ = command
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
        if self.socket.exists() {
            let _ = control(&self.socket)
                .args(["-O", "exit", "msshx.invalid"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}
