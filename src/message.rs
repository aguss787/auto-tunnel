use std::{fmt::Display, net::ToSocketAddrs};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_binary::binary_stream::Endian;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InitMessage {
    DaemonProcess,
    TcpTunnel(Address),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DaemonMessage {
    GetPorts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DaemonResponse {
    BadRequest(String),
    Ports(Vec<Address>),
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address {
    host: String,
    port: u16,
}

impl Address {
    pub fn new<T: Into<String>>(host: T, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
        }
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

impl ToSocketAddrs for Address {
    type Iter = std::vec::IntoIter<std::net::SocketAddr>;

    fn to_socket_addrs(&self) -> std::io::Result<Self::Iter> {
        (self.host(), self.port()).to_socket_addrs()
    }
}

pub trait ByteMessage {
    fn from_vec(raw: Vec<u8>) -> Result<Self, std::io::Error>
    where
        Self: Sized;
    fn to_vec(&self) -> Result<Vec<u8>, std::io::Error>;
}

impl<T> ByteMessage for T
where
    T: Serialize + DeserializeOwned,
{
    fn from_vec(raw: Vec<u8>) -> Result<Self, std::io::Error> {
        serde_binary::from_vec::<Self>(raw, Endian::Big)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{e:?}")))
    }

    fn to_vec(&self) -> Result<Vec<u8>, std::io::Error> {
        serde_binary::to_vec(self, Endian::Big)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serde_init_message() {
        for message in [
            InitMessage::DaemonProcess,
            InitMessage::TcpTunnel(Address::new("127.0.0.1", 80)),
        ] {
            assert_eq!(
                InitMessage::from_vec(message.to_vec().unwrap()).unwrap(),
                message,
            )
        }
    }

    #[test]
    fn test_serde_daemon_message() {
        for message in [DaemonMessage::GetPorts] {
            assert_eq!(
                DaemonMessage::from_vec(message.to_vec().unwrap()).unwrap(),
                message,
            )
        }
    }

    #[test]
    fn test_serde_daemon_response() {
        for message in [
            DaemonResponse::BadRequest(String::from("test")),
            DaemonResponse::Ports(vec![]),
        ] {
            assert_eq!(
                DaemonResponse::from_vec(message.to_vec().unwrap()).unwrap(),
                message,
            )
        }
    }

    #[test]
    fn test_serde_address() {
        let address = Address::new("127.0.0.1", 80);

        assert_eq!(
            Address::from_vec(address.to_vec().unwrap()).unwrap(),
            address
        )
    }
}
