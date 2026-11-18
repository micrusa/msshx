use base64::{Engine, engine::general_purpose::STANDARD};
use std::env;
use std::fs::File;
use std::io;
use std::net::TcpListener;
use std::num::NonZeroU16;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tiny_http::{Method, Request, Response, ResponseBox, Server};

pub fn serve(server: Server, control_socket: &Path) {
    for mut request in server.incoming_requests() {
        let response = handle(&mut request, control_socket).unwrap_or_else(|error| {
            Response::from_string(format!("{error}\n"))
                .with_status_code(400)
                .boxed()
        });
        let _ = request.respond(response);
    }
}

fn handle(request: &mut Request, control_socket: &Path) -> io::Result<ResponseBox> {
    let value = request
        .headers()
        .iter()
        .find(|header| header.field.equiv("X-Msshx-Value"))
        .and_then(|header| STANDARD.decode(header.value.as_str()).ok())
        .filter(|value| !value.is_empty() && !value.contains(&0))
        .ok_or_else(|| io::Error::other("missing or invalid argument"))?;
    match (request.method(), request.url()) {
        (&Method::Get, "/from-client") => {
            let path = client_path(value)?;
            let metadata = path.metadata()?;
            if !metadata.is_file() {
                return Err(io::Error::other("expected a regular file"));
            }
            let length = usize::try_from(metadata.len()).map_err(io::Error::other)?;
            let file = File::open(path)?;
            Ok(Response::new(200.into(), vec![], file, Some(length), None).boxed())
        }
        (&Method::Put, "/to-client") => {
            let destination = client_path(value)?;
            let parent = destination
                .parent()
                .ok_or_else(|| io::Error::other("invalid destination"))?;
            let mut file = tempfile::NamedTempFile::new_in(parent)?;
            let bytes = io::copy(request.as_reader(), &mut file)?;
            if request
                .body_length()
                .is_some_and(|length| length as u64 != bytes)
            {
                return Err(io::Error::other("incomplete upload"));
            }
            file.persist(destination)?;
            Ok(Response::empty(204).boxed())
        }
        (&Method::Post, "/open") => {
            let url = std::str::from_utf8(&value).map_err(io::Error::other)?;
            let forwarded = forwarded_url(control_socket, url)?;
            let url = forwarded.as_deref().unwrap_or(url);
            let opener = if cfg!(target_os = "macos") {
                "open"
            } else {
                "xdg-open"
            };
            let mut child = Command::new(opener)
                .arg(url)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            Ok(Response::empty(204).boxed())
        }
        (&Method::Post, "/forward") => forward(control_socket, &value),
        _ => Err(io::Error::other("unknown operation")),
    }
}

fn client_path(value: Vec<u8>) -> io::Result<PathBuf> {
    let path = PathBuf::from(std::ffi::OsString::from_vec(value));
    if let Ok(relative) = path.strip_prefix("~") {
        let home = env::var_os("HOME").ok_or_else(|| io::Error::other("client HOME is unset"))?;
        return Ok(Path::new(&home).join(relative));
    }
    Ok(env::current_dir()?.join(path))
}

fn forwarded_url(socket: &Path, url: &str) -> io::Result<Option<String>> {
    let (scheme, address, default_port, explicit_scheme) =
        if let Some(address) = url.strip_prefix("https://") {
            ("https", address, 443, true)
        } else if let Some(address) = url.strip_prefix("http://") {
            ("http", address, 80, true)
        } else {
            ("http", url, 80, false)
        };
    let end = address.find(['/', '?', '#']).unwrap_or(address.len());
    let (authority, suffix) = address.split_at(end);
    if explicit_scheme && authority.contains('@') {
        return Ok(None);
    }
    for host in ["127.0.0.1", "localhost", "[::1]"] {
        let Some(port) = authority.strip_prefix(host) else {
            continue;
        };
        let remote = if let Some(port) = port.strip_prefix(':') {
            port.parse::<NonZeroU16>()
                .map_err(|_| io::Error::other("copen expects a port from 1 to 65535"))?
                .get()
        } else if port.is_empty() {
            default_port
        } else {
            continue;
        };

        // OpenSSH requires a local port number. Retry if the released port is taken.
        let mut result = Err(io::Error::other("could not allocate a local forward"));
        for _ in 0..3 {
            let local = TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port();
            result = add_forward(socket, local, host, remote)
                .map(|()| Some(format!("{scheme}://127.0.0.1:{local}{suffix}")));
            if result.is_ok() {
                break;
            }
        }
        return result;
    }
    if explicit_scheme {
        Ok(None)
    } else {
        Err(io::Error::other(
            "copen expects an HTTP(S) URL or loopback address",
        ))
    }
}

fn forward(socket: &Path, value: &[u8]) -> io::Result<ResponseBox> {
    let value = std::str::from_utf8(value).map_err(io::Error::other)?;
    let (local, remote) = value.split_once(':').unwrap_or((value, value));
    let port = |value: &str| {
        value
            .parse::<NonZeroU16>()
            .map_err(|_| io::Error::other("expected PORT or LOCAL_PORT:REMOTE_PORT (1–65535)"))
    };
    let (local, remote) = (port(local)?, port(remote)?);
    add_forward(socket, local.get(), "127.0.0.1", remote.get())?;
    Ok(Response::from_string(format!(
        "Forwarding client 127.0.0.1:{local} to remote 127.0.0.1:{remote}\n"
    ))
    .boxed())
}

fn add_forward(socket: &Path, local: u16, host: &str, remote: u16) -> io::Result<()> {
    let spec = format!("127.0.0.1:{local}:{host}:{remote}");
    let output = crate::control(socket)
        .args([
            "-o",
            "ExitOnForwardFailure=yes",
            "-O",
            "forward",
            "-L",
            &spec,
            "msshx.invalid",
        ])
        .output()?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "could not forward port {local}: {}",
            message.trim()
        )));
    }
    Ok(())
}
