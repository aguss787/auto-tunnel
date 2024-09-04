use std::io::Error;

use tokio::io::AsyncReadExt;

use crate::message::TcpMessage;

pub struct BufferedMessageStream<S, const N: usize = 1024> {
    stream: S,
    buffer_ptr: usize,
    buffer_end_ptr: usize,
    buffer: [u8; N],
}

impl<S, const N: usize> BufferedMessageStream<S, N> {
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            buffer_ptr: 0,
            buffer_end_ptr: 0,
            buffer: [0; N],
        }
    }
}

impl<S: AsyncReadExt + Unpin> BufferedMessageStream<S> {
    pub async fn read_message<M: TcpMessage>(&mut self) -> Result<Option<M>, Error> {
        let mut current_message = Vec::<u8>::new();

        loop {
            if self.buffer_ptr >= self.buffer_end_ptr {
                self.buffer_end_ptr = self.stream.read_buf(&mut &mut self.buffer[..]).await?;
                self.buffer_ptr = 0;

                if self.buffer_end_ptr == 0 {
                    break;
                }
            }

            current_message.push(self.buffer[self.buffer_ptr]);
            self.buffer_ptr += 1;

            if current_message.last_chunk::<2>() == Some(&[0, 0]) {
                current_message.pop();
                current_message.pop();
                break;
            }
        }

        if current_message.is_empty() {
            return Ok(None);
        }

        Ok(Some(M::from_bytes(current_message)?))
    }

    pub async fn read_buffer(&mut self) -> Result<Option<&[u8]>, Error> {
        if self.buffer_ptr < self.buffer_end_ptr {
            let left = self.buffer_ptr;
            let right = self.buffer_end_ptr;
            self.buffer_ptr = 0;
            self.buffer_end_ptr = 0;
            return Ok(Some(&self.buffer[left..right]));
        }

        let len = self.stream.read(&mut self.buffer).await?;
        if len == 0 {
            return Ok(None);
        }

        return Ok(Some(&self.buffer[..len]));
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Deref;

    use super::*;

    #[derive(Debug, PartialEq)]
    struct Data(Vec<u8>);

    impl Deref for Data {
        type Target = [u8];
        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl TcpMessage for Data {
        fn from_bytes(raw: Vec<u8>) -> Result<Self, Error> {
            Ok(Self(raw))
        }

        fn to_bytes(&self) -> Result<Vec<u8>, Error> {
            Ok(self.0.clone())
        }
    }

    #[tokio::test]
    async fn read_single_message() {
        let mut stream = BufferedMessageStream::<&[u8], 1024>::new(b"hello world");
        assert_eq!(
            stream.read_message::<Data>().await.unwrap(),
            Some(Data(Vec::from(b"hello world")))
        );
    }

    #[tokio::test]
    async fn read_multiple_message() {
        let mut stream = BufferedMessageStream::<&[u8], 1024>::new(b"hello world\0\0goodbye world");
        assert_eq!(
            stream.read_message::<Data>().await.unwrap(),
            Some(Data(Vec::from(b"hello world")))
        );
        assert_eq!(
            stream.read_message::<Data>().await.unwrap(),
            Some(Data(Vec::from(b"goodbye world")))
        );
    }

    #[tokio::test]
    async fn read_buffer_empty() {
        let mut stream = BufferedMessageStream::<&[u8], 1024>::new(b"hello world");
        assert_eq!(
            stream.read_buffer().await.unwrap(),
            Some(&b"hello world"[..])
        );
    }

    #[tokio::test]
    async fn read_buffer_after_message() {
        let mut stream = BufferedMessageStream::<&[u8], 1024>::new(b"hello world\0\0something");
        assert_eq!(
            stream.read_message::<Data>().await.unwrap(),
            Some(Data(Vec::from(b"hello world")))
        );
        assert_eq!(stream.read_buffer().await.unwrap(), Some(&b"something"[..]));
    }

    #[tokio::test]
    async fn read_buffer_until_empty() {
        let mut stream = BufferedMessageStream::<&[u8], 1024>::new(b"hello world");
        assert_eq!(
            stream.read_buffer().await.unwrap(),
            Some(&b"hello world"[..])
        );
        assert_eq!(stream.read_buffer().await.unwrap(), None);
    }
}
