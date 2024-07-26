pub trait WebSocketResult {
    type OkResult;

    fn into_io_result(self) -> std::io::Result<Self::OkResult>;
}

impl<T> WebSocketResult for Result<T, websocket::WebSocketError> {
    type OkResult = T;

    fn into_io_result(self) -> std::io::Result<Self::OkResult> {
        self.map_err(|e| std::io::Error::new(std::io::ErrorKind::ConnectionAborted, e))
    }
}

