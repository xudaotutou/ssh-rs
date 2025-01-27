use std::io;
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::ops::{Deref, DerefMut};
use crate::error::{SshError, SshResult};
use crate::slog::log;
use crate::timeout::Timeout;


pub struct Client {
    pub(crate) stream: TcpStream,
    pub(crate) sequence: Sequence,
    pub(crate) timeout: Timeout
}

#[derive(Clone)]
pub(crate) struct Sequence {
    pub(crate) client_sequence_num: u32,
    pub(crate) server_sequence_num: u32
}

impl Sequence {

    pub(crate) fn client_auto_increment(&mut self) {
        if self.client_sequence_num == u32::MAX {
            self.client_sequence_num = 0;
        }
        self.client_sequence_num += 1;
    }

    pub(crate) fn server_auto_increment(&mut self) {
        if self.server_sequence_num == u32::MAX {
            self.server_sequence_num = 0;
        }
        self.server_sequence_num += 1;
    }
}

impl Client {
    pub(crate) fn connect<A: ToSocketAddrs>(addr: A) -> SshResult<Client> {
        match TcpStream::connect(addr) {
            Ok(stream) => {
                // default nonblocking
                stream.set_nonblocking(true).unwrap();
                Ok(
                    Client{
                        stream,
                        sequence: Sequence {
                            client_sequence_num: 0,
                            server_sequence_num: 0
                        },
                        timeout: Timeout::new()
                    }
                )
            }
            Err(e) => Err(SshError::from(e))
        }
    }


    pub(crate) fn close(&mut self) -> Result<(), SshError> {
        match self.stream.shutdown(Shutdown::Both) {
            Ok(o) => Ok(o),
            Err(e) => Err(SshError::from(e))
        }
    }

    pub(crate) fn is_would_block(e: &io::Error) -> bool {
        e.kind() == io::ErrorKind::WouldBlock
    }

}

impl Deref for Client {
    type Target = TcpStream;

    fn deref(&self) -> &Self::Target {
        &self.stream
    }
}

impl DerefMut for Client {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.stream
    }
}



static mut CLIENT: Option<Client> = None;

pub(crate) fn connect<A: ToSocketAddrs>(addr: A) -> Result<(), SshError> {
    unsafe {
        let client = Client::connect(addr)?;
        CLIENT = Some(client);
        Ok(())
    }
}

pub(crate) fn default() -> SshResult<&'static mut Client> {
    unsafe {
        match &mut CLIENT {
            None => {
                log::error!("Client null pointer");
                Err(SshError::from("Client null pointer"))
            }
            Some(v) => {
                Ok(v)
            }
        }
    }
}
