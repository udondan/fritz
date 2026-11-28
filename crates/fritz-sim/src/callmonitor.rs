//! A fake call monitor: a TCP listener on loopback that sends scripted
//! lines, like the one of a device on port 1012.

use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::io::AsyncWriteExt as _;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// A running call monitor. It stops when dropped.
#[derive(Debug)]
pub struct CallMonitor {
    address: SocketAddr,
    connections: Arc<AtomicUsize>,
    server: JoinHandle<()>,
}

impl CallMonitor {
    /// Listens on a free port of the loopback interface. Each connection
    /// gets the lines of the next session, each ended with `\r\n` like the
    /// device does, and is closed after them, except the one of the last
    /// session, which stays open. Lines can be anything, like events that
    /// are not valid; sessions can repeat the lines of the one before, like
    /// a device that would send them again.
    ///
    /// # Errors
    ///
    /// The errors of binding the port.
    ///
    /// # Panics
    ///
    /// The server task panics if it cannot accept a connection on loopback.
    pub async fn start(sessions: Vec<Vec<String>>) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let connections = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&connections);
        let server = tokio::spawn(async move {
            let mut open = Vec::new();
            loop {
                let (mut stream, _) = listener.accept().await.expect("accepts on loopback");
                let index = count.fetch_add(1, Ordering::SeqCst);
                let lines = sessions.get(index).cloned().unwrap_or_default();
                for line in lines {
                    let _ = stream.write_all(format!("{line}\r\n").as_bytes()).await;
                }
                if index + 1 >= sessions.len() {
                    open.push(stream);
                }
            }
        });
        Ok(Self {
            address,
            connections,
            server,
        })
    }

    /// The port it listens on.
    pub fn port(&self) -> u16 {
        self.address.port()
    }

    /// How many connections it accepted so far.
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

impl Drop for CallMonitor {
    fn drop(&mut self) {
        self.server.abort();
    }
}
