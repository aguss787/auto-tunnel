use std::fmt::Display;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitMessage {
    DaemonProcess,
    TcpTunnel(Address),
}

impl TcpMessage for InitMessage {
    fn from_bytes(raw: &[u8]) -> Result<Self, std::io::Error>
    where
        Self: Sized,
    {
        let message_type = raw.get(0).ok_or_else(invalid_message_length)?;
        match message_type {
            1 => Ok(Self::DaemonProcess),
            2 => {
                let address =
                    Address::from_bytes(raw.get(1..).ok_or_else(invalid_message_length)?)?;
                Ok(Self::TcpTunnel(address))
            }
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid message type",
            )),
        }
    }

    fn to_bytes_body(&self) -> Result<Vec<u8>, std::io::Error> {
        match self {
            Self::DaemonProcess => Ok(vec![1]),
            Self::TcpTunnel(address) => {
                let mut data = address.to_bytes_body()?;
                data.insert(0, 2);
                Ok(data)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonMessage {
    GetPorts,
}

impl TcpMessage for DaemonMessage {
    fn from_bytes(raw: &[u8]) -> Result<Self, std::io::Error> {
        let message_type = raw.get(0).ok_or_else(invalid_message_length)?;

        match message_type {
            1 => Ok(Self::GetPorts),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid message type",
            )),
        }
    }

    fn to_bytes_body(&self) -> Result<Vec<u8>, std::io::Error> {
        Ok(vec![1])
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DaemonResponse {
    BadRequest(String),
    Ports(Vec<Address>),
}

impl TcpMessage for DaemonResponse {
    fn from_bytes(raw: &[u8]) -> Result<Self, std::io::Error>
    where
        Self: Sized,
    {
        serde_json::from_slice(&raw)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{e:?}")))
    }

    fn to_bytes_body(&self) -> Result<Vec<u8>, std::io::Error> {
        serde_json::to_vec(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e:?}")))
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address {
    host: String,
    port: u16,
}

impl TcpMessage for Address {
    fn to_bytes_body(&self) -> Result<Vec<u8>, std::io::Error> {
        let host = self.host.as_bytes();
        let host_len = host.len() as u8;
        let port = self.port.to_be_bytes();

        let mut data = Vec::with_capacity(1 + host.len() + 2);
        data.push(host_len);
        data.extend_from_slice(host);
        data.extend_from_slice(&port);

        Ok(data)
    }

    fn from_bytes(raw: &[u8]) -> Result<Self, std::io::Error>
    where
        Self: Sized,
    {
        let host_len = *(raw.get(0).ok_or_else(invalid_message_length)?) as usize;

        let host = raw
            .get(1..1 + host_len)
            .ok_or_else(invalid_message_length)?;
        let host = String::from_utf8(host.to_vec())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{e:?}")))?;

        let port = u16::from_be_bytes(
            raw.get(1 + host_len..)
                .ok_or_else(invalid_message_length)?
                .try_into()
                .map_err(|_| invalid_message_length())?,
        );

        Ok(Self { host, port })
    }
}

impl Address {
    pub fn new<T: Into<String>>(host: T, port: u16) -> std::io::Result<Self> {
        let host = host.into();
        if host.len() > 255 {
            panic!("host name is too long");
        }

        Ok(Self { host, port })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn to_socket(&self) -> (&str, u16) {
        (&self.host, self.port)
    }
}

impl Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

pub trait TcpMessage {
    fn from_bytes(raw: &[u8]) -> Result<Self, std::io::Error>
    where
        Self: Sized;
    fn to_bytes_body(&self) -> Result<Vec<u8>, std::io::Error>;

    fn to_message(&self) -> Result<Vec<u8>, std::io::Error> {
        let mut data = self.to_bytes_body()?;
        data.extend(b"\0\0");
        Ok(data)
    }
}

fn invalid_message_length() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid message length")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serde_init_message() {
        for message in [
            InitMessage::DaemonProcess,
            InitMessage::TcpTunnel(Address::new("127.0.0.1", 80).unwrap()),
        ] {
            assert_eq!(
                InitMessage::from_bytes(&message.to_bytes_body().unwrap()).unwrap(),
                message,
            )
        }
    }

    #[test]
    fn test_serde_daemon_message() {
        for message in [DaemonMessage::GetPorts] {
            assert_eq!(
                DaemonMessage::from_bytes(&message.to_bytes_body().unwrap()).unwrap(),
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
                DaemonResponse::from_bytes(&message.to_bytes_body().unwrap()).unwrap(),
                message,
            )
        }
    }

    #[test]
    fn test_serde_address() {
        let address = Address::new("127.0.0.1", 80).unwrap();

        assert_eq!(
            Address::from_bytes(&address.to_bytes_body().unwrap()).unwrap(),
            address
        )
    }
}
