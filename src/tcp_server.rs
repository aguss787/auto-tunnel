use std::collections::HashMap;

use futures_util::join;
use tokio::{io::AsyncWriteExt, net::TcpListener, select};
use tokio_util::sync::CancellationToken;

use crate::{
    message::{Address, DaemonMessage, DaemonResponse, InitMessage, TcpMessage},
    tcp::BufferedMessageStream,
};

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
            let Ok((connection, _)) = listener
                .accept()
                .await
                .inspect_err(|error| tracing::error!(?error, "failed to accept connection"))
            else {
                continue;
            };

            let thread_port_filter = port_filter.clone();
            tokio::spawn(handle_tcp_connection(connection, thread_port_filter));
        }
    }
}

async fn handle_tcp_connection(mut connection: tokio::net::TcpStream, port_filter: PortFilter) {
    let (reader, mut writer) = connection.split();

    let mut reader = BufferedMessageStream::<_, 1024>::new(reader);
    let message = reader
        .read_message::<InitMessage, 256>()
        .await
        .unwrap_or_else(|error| {
            tracing::error!(?error, "failed to read message");
            None
        });

    match message {
        None => {}
        Some(InitMessage::DaemonProcess) => loop {
            let message = reader
                .read_message::<DaemonMessage, 256>()
                .await
                .unwrap_or_else(|error| {
                    tracing::error!(?error, "failed to read daemon message");
                    None
                });

            match message {
                None => break,
                Some(DaemonMessage::GetPorts) => {
                    let response = get_ports(&port_filter)
                        .to_message()
                        .expect("unable to serialize port response");

                    writer
                        .write(&response)
                        .await
                        .inspect_err(|error| {
                            tracing::error!(?error, "failed to write port response")
                        })
                        .ok();
                }
            }
        },
        Some(InitMessage::TcpTunnel(address)) => {
            forward_tcp(reader, writer, address)
                .await
                .inspect_err(|error| tracing::error!(?error, "failed to forward tcp connection"))
                .ok();
        }
    }
}

async fn forward_tcp(
    mut reader: BufferedMessageStream<tokio::net::tcp::ReadHalf<'_>>,
    mut writer: tokio::net::tcp::WriteHalf<'_>,
    address: Address,
) -> Result<(), std::io::Error> {
    let mut target = tokio::net::TcpStream::connect(address.to_socket()).await?;
    let (target_reader, mut target_writer) = target.split();
    let mut target_reader = BufferedMessageStream::new(target_reader);

    let tcp_from_target_cancel = CancellationToken::new();
    let tcp_to_target_cancel = tcp_from_target_cancel.clone();

    let to_target = async move {
        loop {
            tracing::trace!("reading from connection");
            let data = select! {
                _ = tcp_to_target_cancel.cancelled() => { break; }
                data = reader.read_buffer() => { data },
            }?;

            match data {
                Some(data) => target_writer.write_all(data).await?,
                None => break,
            }
        }

        tracing::trace!("routine to target finished");
        tcp_to_target_cancel.cancel();
        Ok::<_, std::io::Error>(())
    };

    let from_target = async move {
        loop {
            let data = select! {
                _ = tcp_from_target_cancel.cancelled() => { break; }
                data = target_reader.read_buffer() => { data },
            }?;

            match data {
                Some(data) => writer.write_all(data).await?,
                None => break,
            }
        }

        tracing::trace!("routine from target finished");
        tcp_from_target_cancel.cancel();
        Ok::<_, std::io::Error>(())
    };

    let (to_target, from_target) = join!(to_target, from_target);
    [to_target, from_target].iter().for_each(|r| {
        if let Err(error) = r {
            tracing::error!(?error, "failed to forward tcp connection");
        }
    });

    Ok(())
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
        .filter_map(|r| {
            if let Err(error) = r {
                tracing::error!(?error, "failed to parse tcp address");
                None
            } else {
                r.ok()
            }
        })
        .map(|a| (a.port(), a))
        .collect::<HashMap<_, _>>();

    DaemonResponse::Ports(tcps.into_values().collect())
}
