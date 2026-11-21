//! One connection worker sleeps until a socket, its output queue or a
//! deadline needs attention. Only that worker consumes socket readiness.

use std::{
    io,
    net::{TcpStream, UdpSocket},
    sync::Arc,
    time::Duration,
};

use polling::{Event, Events, Poller};

pub(crate) struct SocketWait {
    poller: Arc<Poller>,
    tcp: Option<TcpStream>,
    udp: Option<UdpSocket>,
    events: Events,
}

impl SocketWait {
    #[allow(unsafe_code)]
    pub(crate) fn new(
        poller: Arc<Poller>,
        tcp: Option<&TcpStream>,
        udp: Option<&UdpSocket>,
    ) -> io::Result<Self> {
        let wait = Self {
            poller,
            tcp: tcp.map(TcpStream::try_clone).transpose()?,
            udp: udp.map(UdpSocket::try_clone).transpose()?,
            events: Events::new(),
        };
        // SAFETY: each registered handle belongs to this value. Drop deletes
        // the registrations before closing either handle, including after an
        // error registering the second socket. The original sockets can still
        // be read and written normally by the connection worker.
        unsafe {
            if let Some(tcp) = &wait.tcp {
                wait.poller.add(tcp, Event::readable(0))?;
            }
            if let Some(udp) = &wait.udp {
                wait.poller.add(udp, Event::readable(1))?;
            }
        }
        Ok(wait)
    }

    pub(crate) fn wait(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        self.events.clear();
        self.poller.wait(&mut self.events, timeout)?;
        for event in self.events.iter() {
            match event.key {
                0 => self.poller.modify(self.tcp.as_ref().unwrap(), Event::readable(0))?,
                1 => self.poller.modify(self.udp.as_ref().unwrap(), Event::readable(1))?,
                _ => {}
            }
        }
        Ok(())
    }
}

impl Drop for SocketWait {
    fn drop(&mut self) {
        if let Some(tcp) = &self.tcp {
            let _ = self.poller.delete(tcp);
        }
        if let Some(udp) = &self.udp {
            let _ = self.poller.delete(udp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::sync::mpsc;

    // Exercise both the no-data shutdown signal and re-arming after a
    // packet. A signal issued just before wait must not be lost.
    #[test]
    fn queue_and_repeated_datagrams_wake_the_connection_worker() {
        let poller = Arc::new(Poller::new().unwrap());
        let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        udp.set_nonblocking(true).unwrap();
        let address = udp.local_addr().unwrap();
        let mut wait = SocketWait::new(poller.clone(), None, Some(&udp)).unwrap();
        let (ack, received) = mpsc::channel();

        poller.notify().unwrap();
        let worker = std::thread::spawn(move || {
            wait.wait(None).unwrap();
            ack.send(0).unwrap();
            for seq in [1, 2] {
                let mut data = [0; 1];
                loop {
                    match udp.recv(&mut data) {
                        Ok(1) => break,
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => wait.wait(None).unwrap(),
                        r => panic!("expected a datagram: {r:?}"),
                    }
                }
                assert_eq!(data, [seq]);
                ack.send(seq).unwrap();
            }
        });
        let sender = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let limit = Duration::from_secs(5);
        assert_eq!(received.recv_timeout(limit).unwrap(), 0);
        for seq in [1, 2] {
            sender.send_to(&[seq], address).unwrap();
            assert_eq!(received.recv_timeout(limit).unwrap(), seq);
        }
        worker.join().unwrap();
    }
}
