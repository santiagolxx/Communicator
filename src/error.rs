use std;

#[derive(Debug)]
pub enum DiscordRpcError {
    ConnectionFailed(String),
    IoError(std::io::Error),
    SerializationError(serde_json::Error),
    HandshakeError(String),
    NotConnected,
}

impl std::fmt::Display for DiscordRpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            DiscordRpcError::ConnectionFailed(msg) => write!(f, "Connection failed: {}", msg),
            DiscordRpcError::IoError(e) => write!(f, "IO error: {}", e),
            DiscordRpcError::SerializationError(e) => write!(f, "Serialization error: {}", e),
            DiscordRpcError::HandshakeError(msg) => write!(f, "Handshake error: {}", msg),
            DiscordRpcError::NotConnected => write!(f, "Not connected to Discord"),
        }
    }
}

impl std::error::Error for DiscordRpcError {}

impl From<std::io::Error> for DiscordRpcError {
    fn from(error: std::io::Error) -> Self {
        DiscordRpcError::IoError(error)
    }
}

impl From<serde_json::Error> for DiscordRpcError {
    fn from(error: serde_json::Error) -> Self {
        DiscordRpcError::SerializationError(error)
    }
}
