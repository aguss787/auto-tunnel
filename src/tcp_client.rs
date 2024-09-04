use std::collections::{HashMap, HashSet};

use tokio::{io::AsyncWriteExt, join, net::TcpStream, select};
use tokio_util::sync::CancellationToken;

use crate::message::{Address, DaemonMessage, DaemonResponse, InitMessage, TcpMessage};

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

    pub async fn run(mut self, server_addr: &str, dry_run: bool) -> std::io::Result<()> {
        let mut stream = TcpStream::connect(server_addr).await?;
        let (stream_reader, mut stream_writer) = stream.split();
        let mut stream_reader = crate::tcp::BufferedMessageStream::<_, 1024>::new(stream_reader);

        stream_writer
            .write(&InitMessage::DaemonProcess.to_bytes()?)
            .await?;

        loop {
            tracing::debug!("requesting ports from server");
            stream_writer
                .write(&DaemonMessage::GetPorts.to_bytes()?)
                .await?;

            let response = stream_reader.read_message::<DaemonResponse>().await?;
            match response {
                None => {
                    break;
                }
                Some(DaemonResponse::BadRequest(message)) => {
                    tracing::error!(?message, "server responded with error");
                    break;
                }
                Some(DaemonResponse::Ports(addresses)) => {
                    self.remove_old_tunnels(&addresses);
                    self.create_missing_tunnel(server_addr, &addresses, dry_run);
                }
            }

            tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
        }

        Ok(())
    }

    fn remove_old_tunnels(&mut self, addresses: &[Address]) {
        let available_addresses = addresses.iter().collect::<HashSet<_>>();

        self.tunnels
            .retain(|_, value| available_addresses.contains(value.address()));
    }

    fn create_missing_tunnel(&mut self, server_addr: &str, addresses: &[Address], dry_run: bool) {
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
                Tunnel::new(server_addr.to_string(), address.clone(), host_port, dry_run)
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
    cancelation_token: CancellationToken,
    address: Address,
}

impl Tunnel {
    fn new(server_address: String, address: Address, host_port: u16, dry_run: bool) -> Self {
        tracing::info!("initializing tunnel to {address}");
        let cancelation_token = CancellationToken::new();

        let thread_cancelation_token = cancelation_token.clone();
        let thread_address = address.clone();
        if !dry_run {
            tokio::spawn(start_tcp_tunnel(
                server_address.clone(),
                host_port,
                thread_address,
                thread_cancelation_token,
            ));
        }

        Self {
            cancelation_token,
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
        self.cancelation_token.cancel();
    }
}

async fn start_tcp_tunnel(
    server_addr: String,
    host_port: u16,
    address: Address,
    cancelation_token: CancellationToken,
) -> std::io::Result<()> {
    tracing::info!("starting tcp listener on 0.0.0.0:{host_port} -> {address}");
    let tcp_listener = tokio::net::TcpListener::bind(("0.0.0.0", host_port)).await?;

    loop {
        let (connection, peer_addr) = select! {
            _ = cancelation_token.cancelled() => {
                break;
            }
            Ok((connection, addr)) = tcp_listener.accept() => {
                ( connection, addr )
            }
        };

        tracing::info!("[{peer_addr}] tunneling port {address}");
        let mut tunnel = TcpStream::connect(&server_addr).await?;
        tunnel
            .write(&InitMessage::TcpTunnel(address.clone()).to_bytes()?)
            .await?;

        tokio::spawn(tunnel_tcp_stream(connection, tunnel));
    }

    todo!()
}

async fn tunnel_tcp_stream(
    mut connection: TcpStream,
    mut tunnel: TcpStream,
) -> std::io::Result<()> {
    let (connection_reader, mut connection_writer) = connection.split();
    let mut connection_reader =
        crate::tcp::BufferedMessageStream::<_, 1024>::new(connection_reader);

    let (tunnel_reader, mut tunnel_writer) = tunnel.split();
    let mut tunnel_reader = crate::tcp::BufferedMessageStream::<_, 1024>::new(tunnel_reader);

    let to_tunnel = async move {
        loop {
            let data = connection_reader.read_buffer().await?;

            match data {
                Some(data) => tunnel_writer.write_all(data).await?,
                None => break,
            }
        }

        Ok::<_, std::io::Error>(())
    };

    let from_tunnel = async move {
        loop {
            let data = tunnel_reader.read_buffer().await?;

            match data {
                Some(data) => connection_writer.write_all(data).await?,
                None => break,
            }
        }

        Ok::<_, std::io::Error>(())
    };

    let (to_tunnel, from_tunnel) = join!(to_tunnel, from_tunnel);
    [to_tunnel, from_tunnel].iter().for_each(|r| {
        if let Err(error) = r {
            tracing::error!(?error, "failed to forward tcp connection");
        }
    });

    Ok(())
}
