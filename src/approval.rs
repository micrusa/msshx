use std::fmt::Debug;
use std::io;
use std::sync::mpsc::{self, Receiver, Sender};

pub struct Approver(Sender<Request>);

pub struct Request {
    pub message: String,
    pub answer: Sender<bool>,
}

pub fn channel() -> (Approver, Receiver<Request>) {
    let (sender, receiver) = mpsc::channel();
    (Approver(sender), receiver)
}

impl Approver {
    pub fn confirm(&self, action: &str, target: &(impl Debug + ?Sized)) -> io::Result<()> {
        let (answer, response) = mpsc::channel();
        let request = Request {
            message: format!("mSSHx: {action}\r\n{target:?}\r\n[Y/n] "),
            answer,
        };
        if self.0.send(request).is_ok() && response.recv().unwrap_or(false) {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "client approval declined or unavailable",
            ))
        }
    }
}
