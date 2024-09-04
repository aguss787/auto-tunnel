/// Server module
///
/// This module contains the server implementation for the auto-tunneling daemon.
/// The server is responsible for handling incoming websocket connections and
/// forwarding the requests to the appropriate target.
use std::{
    collections::HashMap,
    io::{Read, Write},
};

use websocket::stream::sync::Splittable;

use crate::{
    message::{Address, ByteMessage, DaemonMessage, DaemonResponse, InitMessage},
    utils::WebSocketResult,
};

pub struct Server {
    port_filter: Option<PortFilter>,
}

impl Server {
    pub fn new() -> Self {
        Server { port_filter: None }
    }

    pub fn with_whitelist(ports: Vec<u16>) -> Self {
        Server {
            port_filter: Some(PortFilter::Whitelist(ports)),
        }
    }

    pub fn with_blacklist(ports: Vec<u16>) -> Self {
        Server {
            port_filter: Some(PortFilter::Blacklist(ports)),
        }
    }
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

impl Server {
    pub fn run<T: std::net::ToSocketAddrs>(self, addr: T) -> Result<(), std::io::Error> {
        let addrs = addr.to_socket_addrs()?.collect::<Vec<_>>();
        let ws_server = websocket::server::sync::Server::bind(&addr)?;

        let connections = ws_server
            .inspect(|c| {
                if let Err(error) = c {
                    tracing::error!(?error, "failed to get websocket connection");
                }
            })
            .filter_map(Result::ok);

        let mut port_filter = self.port_filter.unwrap_or(PortFilter::Blacklist(vec![]));
        if port_filter.is_blacklist() {
            port_filter.add_port(addrs.iter().map(|a| a.port()));
        }

        for connection in connections {
            let Ok(connection) = connection.accept().map_err(|(_, error)| {
                tracing::error!(?error, "failed to accept websocket connection")
            }) else {
                continue;
            };

            let thread_port_filter = port_filter.clone();
            std::thread::spawn(move || handle_connection(connection, &thread_port_filter));
        }

        Ok(())
    }
}

fn handle_connection(
    mut connection: websocket::sync::Client<std::net::TcpStream>,
    port_filter: &PortFilter,
) {
    let peer_addr = connection
        .stream_ref()
        .peer_addr()
        .expect("unable to get peer addr");

    tracing::info!("[{peer_addr}] handling websocket connection");

    connection
        .stream_ref()
        .set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .expect("unable to set read timeout");

    let init_message = connection
        .recv_message()
        .expect("failed to receive init message");

    let init_message = match init_message {
        websocket::OwnedMessage::Binary(body) => body,
        _ => {
            tracing::info!("[{peer_addr}] received invalid init message");
            return;
        }
    };

    let Ok(init_message) = InitMessage::from_vec(init_message) else {
        tracing::info!("[{peer_addr}] unable to parse init message");
        return;
    };

    match init_message {
        InitMessage::DaemonProcess => handle_daemon_process(connection, port_filter),
        InitMessage::TcpTunnel(port) => handle_tcp_tunnel(connection, port),
    }
    .inspect_err(|error| {
        tracing::error!(?error, "[{peer_addr}] failed to handle init request");
    })
    .ok();
}

fn handle_daemon_process(
    mut connection: websocket::sync::Client<std::net::TcpStream>,
    port_filter: &PortFilter,
) -> Result<(), std::io::Error> {
    let peer_addr = connection.stream_ref().peer_addr()?;
    tracing::info!("[{peer_addr}] starting daemon thread");

    loop {
        let message = match connection.recv_message() {
            Ok(message) => message,
            Err(websocket::WebSocketError::NoDataAvailable) => {
                tracing::debug!("[{peer_addr}] daemon websocket reader closed");
                break;
            }
            Err(websocket::WebSocketError::IoError(error))
                if error.kind() == std::io::ErrorKind::WouldBlock =>
            {
                tracing::debug!(
                    "[{peer_addr}] daemon websocket reader read timeout, closing connection..."
                );
                break;
            }
            Err(error) => {
                tracing::error!(?error, "[{peer_addr}] error on daemon websocket reader");
                break;
            }
        };

        tracing::debug!("[{peer_addr}] received daemon message");

        let message = match message {
            websocket::OwnedMessage::Binary(b) => DaemonMessage::from_vec(b),
            websocket::OwnedMessage::Ping(b) => {
                connection
                    .send_message(&websocket::Message::pong(b))
                    .into_io_result()?;
                continue;
            }
            _ => {
                connection
                    .send_message(&websocket::Message::binary(
                        DaemonResponse::BadRequest(String::from("unknown format")).to_vec()?,
                    ))
                    .into_io_result()?;
                continue;
            }
        };

        let reply = match message {
            Err(e) => DaemonResponse::BadRequest(e.to_string()),
            Ok(message) => match message {
                DaemonMessage::GetPorts => {
                    tracing::debug!("[{peer_addr}] daemon handler receive GetPorts");
                    get_ports(port_filter)
                }
            },
        };

        connection
            .send_message(&websocket::Message::binary(reply.to_vec()?))
            .into_io_result()?;
    }

    tracing::info!("[{peer_addr}] daemon thread stopped");
    Ok(())
}

#[derive(Debug, Clone)]
enum PortFilter {
    Whitelist(Vec<u16>),
    Blacklist(Vec<u16>),
}

impl PortFilter {
    fn allow(&self, port: u16) -> bool {
        match self {
            PortFilter::Whitelist(ports) => ports.contains(&port),
            PortFilter::Blacklist(ports) => !ports.contains(&port),
        }
    }

    fn is_blacklist(&self) -> bool {
        match self {
            PortFilter::Whitelist(_) => false,
            PortFilter::Blacklist(_) => true,
        }
    }

    fn add_port(&mut self, ports: impl Iterator<Item = u16>) -> &mut Self {
        match self {
            PortFilter::Whitelist(v) | PortFilter::Blacklist(v) => v.extend(ports),
        };

        self
    }
}

fn get_ports(filter: &PortFilter) -> DaemonResponse {
    let tcp6s = procfs::net::tcp6().unwrap();
    let tcp4s = procfs::net::tcp().unwrap();
    let tcps = tcp4s
        .into_iter()
        .chain(tcp6s)
        .inspect(|tcp_entry| tracing::trace!(?tcp_entry, "found tcp port"))
        .filter(|t| t.state == procfs::net::TcpState::Listen)
        .filter(|t| t.local_address.port() >= 1024)
        .filter(|t| filter.allow(t.local_address.port()))
        .inspect(|tcp_entry| tracing::trace!(?tcp_entry, "forwarding tcp port"))
        .map(|t| Address::new(t.local_address.ip().to_string(), t.local_address.port()))
        .map(|a| (a.port(), a))
        .collect::<HashMap<_, _>>();

    DaemonResponse::Ports(tcps.into_values().collect())
}

fn handle_tcp_tunnel(
    websocket_connetion: websocket::sync::Client<std::net::TcpStream>,
    addr: Address,
) -> Result<(), std::io::Error> {
    let peer_addr = websocket_connetion.stream_ref().peer_addr()?;
    tracing::info!("[{peer_addr}] starting tcp tunnel");
    let tcp_client = std::net::TcpStream::connect(addr)?;

    websocket_connetion.stream_ref().set_read_timeout(None)?;

    let (mut tcp_reader, mut tcp_writer) = tcp_client.split()?;
    let (mut websocket_reader, mut websocket_writer) = websocket_connetion.split()?;

    let websocket_reader_thread = std::thread::spawn(move || {
        loop {
            let message = match websocket_reader.recv_message() {
                Ok(message) => message,
                Err(websocket::WebSocketError::NoDataAvailable) => {
                    tracing::debug!("[{peer_addr}] tcp tunnel websocket reader closed");
                    break;
                }
                Err(websocket::WebSocketError::IoError(error))
                    if error.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    tracing::debug!(
                        "[{peer_addr}] tcp tunnel websocket reader read timeout, ignoring..."
                    );
                    continue;
                }
                Err(error) => {
                    tracing::error!(?error, "[{peer_addr}] error on tcp tunnel websocket reader");
                    break;
                }
            };

            match message {
                websocket::OwnedMessage::Binary(b) => {
                    tcp_writer.write_all(&b)?;
                }
                _ => {
                    tracing::warn!("[{peer_addr}] received non binary data during tcp tunnel");
                }
            };
        }

        Ok::<(), std::io::Error>(())
    });

    let websocket_writer_thread = std::thread::spawn(move || {
        let mut buffer = [0u8; 32768];
        loop {
            let len = tcp_reader.read(&mut buffer)?;
            tracing::debug!(len, "[{peer_addr}] tcp tunnel target response");
            if len == 0 {
                break;
            }

            websocket_writer
                .send_message(&websocket::Message::binary(&buffer[..len]))
                .into_io_result()?;
        }

        websocket_writer.shutdown_all()?;
        Ok::<(), std::io::Error>(())
    });

    let result = [
        websocket_writer_thread.join(),
        websocket_reader_thread.join(),
    ]
    .into_iter()
    .filter_map(|r| {
        r.inspect_err(|error| tracing::error!(?error, "[{peer_addr}] thread join error"))
            .ok()
    })
    .collect();

    tracing::debug!("[{peer_addr}] tcp tunnel closed");

    result
}
