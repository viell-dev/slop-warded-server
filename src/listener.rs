//! Binding the sockets before anything is served.

use crate::Error;
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    io,
    net::{SocketAddr, TcpListener},
};

/// Connections the operating system may hold before they are accepted.
const BACKLOG: i32 = 1024;

pub(crate) fn tcp(address: SocketAddr, listener: &'static str) -> Result<TcpListener, Error> {
    let bind = || {
        let socket = socket(address, Type::STREAM, Protocol::TCP)?;
        // Lets a restarted server bind while its old connections wait to expire. On Windows the
        // option instead lets another program take the port.
        #[cfg(unix)]
        socket.set_reuse_address(true)?;
        socket.bind(&address.into())?;
        socket.listen(BACKLOG)?;
        Ok(socket.into())
    };
    bind().map_err(|source| Error::Bind {
        listener,
        address,
        source,
    })
}

#[cfg(feature = "http3")]
pub(crate) fn udp(address: SocketAddr) -> Result<std::net::UdpSocket, Error> {
    let bind = || {
        let socket = socket(address, Type::DGRAM, Protocol::UDP)?;
        socket.bind(&address.into())?;
        Ok(socket.into())
    };
    bind().map_err(|source| Error::Bind {
        listener: "HTTP/3 over UDP",
        address,
        source,
    })
}

fn socket(address: SocketAddr, kind: Type, protocol: Protocol) -> io::Result<Socket> {
    let socket = Socket::new(Domain::for_address(address), kind, Some(protocol))?;
    if address.is_ipv6() {
        // `[::]` then accepts IPv4 too on every platform, not only where that is the default.
        socket.set_only_v6(false)?;
    }
    socket.set_nonblocking(true)?;
    Ok(socket)
}
