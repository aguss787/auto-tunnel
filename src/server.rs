/// Server module
///
/// This module contains the server implementation for the auto-tunneling daemon.
/// The server is responsible for handling incoming websocket connections and
/// forwarding the requests to the appropriate target.
use std::{
    collections::HashMap,
    io::{Read, Write},
};

use futures_util::sink::SinkExt;
use futures_util::{future::join_all, stream::StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{accept_async, tungstenite::Message, WebSocketStream};
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
    pub async fn run(self, addr: (&str, u16)) -> Result<(), std::io::Error> {
        let mut port_filter = self.port_filter.unwrap_or(PortFilter::Blacklist(vec![]));
        if port_filter.is_blacklist() {
            port_filter.add_port([addr.1].into_iter());
        }

        let listener = TcpListener::bind(addr).await?;
        tracing::trace!("listener started");
        loop {
            let Ok((connection, _)) = listener.accept().await.inspect_err(|error| {
                tracing::error!(?error, "failed to accept websocket connection")
            }) else {
                continue;
            };

            let thread_port_filter = port_filter.clone();
            tokio::spawn(handle_connection(connection, thread_port_filter));
        }
    }
}

async fn handle_connection(connection: TcpStream, port_filter: PortFilter) {
    tracing::trace!("handling new connection");
    let peer_addr = connection.peer_addr().expect("unable to get peer addr");
    tracing::info!("[{peer_addr}] handling websocket connection");

    let mut connection = accept_async(connection)
        .await
        .expect("unable to accept connection");

    let init_message = connection
        .next()
        .await
        .expect("no init message")
        .expect("failed to receive init message");

    let init_message = match init_message {
        Message::Binary(body) => body,
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
        InitMessage::DaemonProcess => {
            handle_daemon_process(connection, peer_addr, &port_filter).await
        }
        InitMessage::TcpTunnel(port) => handle_tcp_tunnel(connection, peer_addr, port).await,
    }
    .inspect_err(|error| {
        tracing::error!(?error, "[{peer_addr}] failed to handle init request");
    })
    .ok();
}

async fn handle_daemon_process(
    mut connection: WebSocketStream<TcpStream>,
    peer_addr: std::net::SocketAddr,
    port_filter: &PortFilter,
) -> Result<(), std::io::Error> {
    tracing::info!("[{peer_addr}] starting daemon thread");

    loop {
        let message = match connection.next().await {
            None => {
                tracing::debug!("[{peer_addr}] daemon websocket reader closed");
                break;
            }
            Some(Err(error)) => {
                tracing::error!(?error, "[{peer_addr}] error on daemon websocket reader");
                break;
            }
            Some(Ok(message)) => message,
        };

        tracing::debug!("[{peer_addr}] received daemon message");

        let message = match message {
            Message::Binary(b) => DaemonMessage::from_vec(b),
            Message::Ping(b) => {
                connection.send(Message::Pong(b)).await.into_io_result()?;
                continue;
            }
            _ => {
                connection
                    .send(Message::Binary(
                        DaemonResponse::BadRequest(String::from("unknown format")).to_vec()?,
                    ))
                    .await
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
            .send(Message::binary(reply.to_vec()?))
            .await
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

async fn handle_tcp_tunnel(
    connection: WebSocketStream<TcpStream>,
    peer_addr: std::net::SocketAddr,
    addr: Address,
) -> Result<(), std::io::Error> {
    tracing::info!("[{peer_addr}] starting tcp tunnel");
    let tcp_client = std::net::TcpStream::connect(addr)?;

    let (mut tcp_reader, mut tcp_writer) = tcp_client.split()?;
    let (mut websocket_writer, mut websocket_reader) = connection.split();

    let websocket_reader_thread = tokio::spawn(async move {
        loop {
            let message = match websocket_reader.next().await {
                None => {
                    tracing::debug!("[{peer_addr}] tcp tunnel websocket reader closed");
                    break;
                }
                Some(Err(error)) => {
                    tracing::error!(?error, "[{peer_addr}] error on tcp tunnel websocket reader");
                    break;
                }
                Some(Ok(message)) => message,
            };

            match message {
                Message::Binary(b) => {
                    tcp_writer.write_all(&b)?;
                }
                _ => {
                    tracing::warn!("[{peer_addr}] received non binary data during tcp tunnel");
                }
            };
        }

        Ok::<(), std::io::Error>(())
    });

    let websocket_writer_thread = tokio::spawn(async move {
        let mut buffer = [0u8; 32768];
        loop {
            let len = tcp_reader.read(&mut buffer)?;
            tracing::debug!(len, "[{peer_addr}] tcp tunnel target response");
            if len == 0 {
                break;
            }

            websocket_writer
                .send(Message::binary(&buffer[..len]))
                .await
                .into_io_result()?;
        }

        websocket_writer.close().await.into_io_result()?;
        Ok::<(), std::io::Error>(())
    });

    let result = join_all([websocket_writer_thread, websocket_reader_thread])
        .await
        .into_iter()
        .filter_map(|r| {
            r.inspect_err(|error| tracing::error!(?error, "[{peer_addr}] thread join error"))
                .ok()
        })
        .collect();

    tracing::debug!("[{peer_addr}] tcp tunnel closed");

    result
}
