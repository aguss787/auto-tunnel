/// Client module
///
/// This module contains the client implementation for the auto-tunneling daemon.
/// The client is responsible for connecting to the server and creating tunnels
/// based on the server's specifications.
use std::{
    collections::{HashMap, HashSet},
    io::{Read, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use websocket::stream::sync::Splittable;

use crate::{
    backoff::Backoff,
    message::{Address, ByteMessage, DaemonMessage, DaemonResponse, InitMessage},
    utils::WebSocketResult,
};

pub struct Client {
    tunnels: HashMap<u16, Tunnel>,
    port_offset: u16,
}

impl Client {
    pub fn new() -> Self {
        Self {
            tunnels: Default::default(),
            port_offset: 0,
        }
    }

    pub fn set_port_offset(mut self, port_offset: u16) -> Self {
        self.port_offset = port_offset;
        self
    }

    pub fn run(mut self, server_addr: &str) -> std::io::Result<()> {
        let mut daemon_connection = websocket::client::ClientBuilder::new(server_addr)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?
            .connect_insecure()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::NotConnected, e))?;

        tracing::debug!("initializing ports");
        daemon_connection
            .send_message(&websocket::Message::binary(
                InitMessage::DaemonProcess.to_vec()?,
            ))
            .into_io_result()?;

        loop {
            daemon_connection
                .send_message(&websocket::Message::binary(
                    DaemonMessage::GetPorts.to_vec()?,
                ))
                .into_io_result()?;

            tracing::debug!("refreshing ports list");
            let message = daemon_connection.recv_message().into_io_result()?;
            let message = match message {
                websocket::OwnedMessage::Binary(b) => b,
                _ => panic!("unexpected message type from server"),
            };
            let message = DaemonResponse::from_vec(message)?;

            let addresses = match message {
                DaemonResponse::Ports(addresses) => addresses,
                _ => panic!("unexpected message response from server"),
            };

            self.remove_old_tunnels(&addresses);
            self.create_missing_tunnel(&server_addr, &addresses);

            std::thread::sleep(std::time::Duration::from_secs(5));
        }
    }

    fn remove_old_tunnels(&mut self, addresses: &[Address]) {
        let available_addresses = addresses.iter().collect::<HashSet<_>>();

        self.tunnels
            .retain(|_, value| available_addresses.contains(value.address()));
    }

    fn create_missing_tunnel(&mut self, server_addr: &str, addresses: &[Address]) {
        addresses.iter().for_each(|address| {
            if self
                .tunnels
                .get(&address.port())
                .map(|t| t.address() != address)
                .unwrap_or(false)
            {
                tracing::warn!(
                    "Skipping tunnel to {address} because tunnel with exists port exists"
                );
            }

            let host_port = address.port() + self.port_offset;
            self.tunnels.entry(address.port()).or_insert_with(|| {
                Tunnel::new(server_addr.to_string(), address.clone(), host_port)
            });
        });
    }
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

struct Tunnel {
    handler: Option<std::thread::JoinHandle<std::io::Result<()>>>,
    stop_flag: Arc<AtomicBool>,
    address: Address,
}

impl Tunnel {
    fn new(server_address: String, address: Address, host_port: u16) -> Self {
        tracing::info!("initializing tunnel to {address}");
        let stop_flag = Arc::new(AtomicBool::new(false));

        let thread_stop_flag = stop_flag.clone();
        let thread_address = address.clone();
        let handler = std::thread::spawn(move || {
            start_tcp_tunnel(
                server_address.clone(),
                host_port,
                thread_address,
                thread_stop_flag,
            )
            .inspect_err(|error| {
                tracing::error!(?error, "failed to start tcp tunnel to {server_address}")
            })
            .ok();

            Ok(())
        });

        Self {
            handler: Some(handler),
            stop_flag,
            address,
        }
    }

    fn address(&self) -> &Address {
        &self.address
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        tracing::info!("stopping tunnel to {}", self.address());
        self.stop_flag.store(true, Ordering::Relaxed);

        if let Some(handler) = self.handler.take() {
            handler
                .join()
                .inspect_err(|error| tracing::error!(?error, "failed to join tunnel thread"))
                .ok()
                .unwrap_or(Ok(()))
                .inspect_err(|error| tracing::error!(?error, "tunnel thread returns error"))
                .ok();
        }
    }
}

fn start_tcp_tunnel(
    server_addr: String,
    host_port: u16,
    address: Address,
    stop_flag: Arc<AtomicBool>,
) -> std::io::Result<()> {
    tracing::info!("starting tcp listener on 0.0.0.0:{host_port} -> {address}");

    let tcp_listener = std::net::TcpListener::bind(("0.0.0.0", host_port))?;
    tcp_listener.set_nonblocking(true)?;

    let mut idle_sleep = Backoff::new();
    for connection in tcp_listener.incoming() {
        let tcp_stream = match connection {
            Ok(tcp_stream) => tcp_stream,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if stop_flag.load(Ordering::Relaxed) {
                    break;
                }

                idle_sleep.sleep();
                continue;
            }
            Err(error) => {
                tracing::error!(?error, "error on tcp listener connection");
                return Err(error);
            }
        };

        idle_sleep.reset();

        let peer_addr = tcp_stream.peer_addr()?;
        tracing::info!("[{peer_addr}] tunneling port {address}");

        let mut websocket_client = websocket::ClientBuilder::new(&server_addr)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?
            .connect_insecure()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::NotConnected, e))?;

        let init_message = InitMessage::TcpTunnel(address.clone()).to_vec().unwrap();
        websocket_client
            .send_message(&websocket::Message::binary(init_message))
            .unwrap();

        std::thread::spawn(|| tunnel_tcp_stream(tcp_stream, websocket_client));
    }

    Ok(())
}

fn tunnel_tcp_stream(
    tcp_stream: std::net::TcpStream,
    websocket_client: websocket::sync::Client<std::net::TcpStream>,
) {
    let peer_addr = tcp_stream.peer_addr().unwrap();
    let (mut tcp_reader, mut tcp_writter) = tcp_stream.split().unwrap();
    let (mut websocket_reader, mut websocket_writer) = websocket_client.split().unwrap();

    let mut idle_sleep = Backoff::new();
    let tcp_reader_thread = std::thread::spawn(move || {
        let mut buffer = [0u8; 32768];
        loop {
            let len = match tcp_reader.read(&mut buffer) {
                Ok(len) => len,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    tracing::debug!("[{peer_addr}] non-blocking return from tcp reader");
                    idle_sleep.sleep();
                    continue;
                }
                Err(error) => {
                    tracing::error!(?error, "[{peer_addr}] error on client tcp reader");
                    break;
                }
            };

            if len == 0 {
                break;
            }

            idle_sleep.reset();

            if let Err(error) =
                websocket_writer.send_message(&websocket::Message::binary(&buffer[..len]))
            {
                tracing::error!(?error, "[{peer_addr}] failed to forward tcp request");
                break;
            }
        }
    });

    let tcp_writer_thread = std::thread::spawn(move || {
        loop {
            let message = match websocket_reader.recv_message() {
                Ok(message) => message,
                Err(websocket::WebSocketError::NoDataAvailable) => {
                    tracing::debug!("[{peer_addr}] client websocket reader closed");
                    break;
                }
                Err(e) => {
                    tracing::error!("[{peer_addr}] error on client websocket reader: {e}");
                    break;
                }
            };

            let response = match message {
                websocket::OwnedMessage::Binary(b) => b,
                _ => {
                    tracing::error!(
                        "[{peer_addr}] received non binary data from websocket, closing tunnel"
                    );
                    break;
                }
            };

            if let Err(error) = tcp_writter.write_all(&response) {
                tracing::error!(?error, "[{peer_addr}] failed to return tcp response");
                break;
            }
        }

        if let Err(error) = tcp_writter.shutdown(std::net::Shutdown::Both) {
            match error.kind() {
                std::io::ErrorKind::NotConnected => {}
                _ => tracing::error!(?error, "[{peer_addr}] failed to shutdown tcp connection"),
            }
        }
    });

    tcp_writer_thread.join().unwrap();
    tcp_reader_thread.join().unwrap();

    tracing::info!("[{peer_addr}] tunnel closed");
}
