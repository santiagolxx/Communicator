use crate::error::DiscordRpcError;
use crate::models::Activity;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::net::UnixStream;

#[cfg(windows)]
use std::fs::File;
#[cfg(windows)]
use std::io::{BufReader, BufWriter};

#[cfg(windows)]
struct NamedPipeConnection {
    reader: BufReader<File>,
    writer: BufWriter<File>,
}

pub struct DiscordRpcClient {
    #[cfg(unix)]
    stream: Option<UnixStream>,
    #[cfg(windows)]
    connection: Option<NamedPipeConnection>,
    client_id: String,
    connected: bool,
}

impl DiscordRpcClient {
    pub fn new(client_id: &str) -> Self {
        Self {
            #[cfg(unix)]
            stream: None,
            #[cfg(windows)]
            connection: None,
            client_id: client_id.to_string(),
            connected: false,
        }
    }

    pub fn connect(&mut self) -> Result<(), DiscordRpcError> {
        #[cfg(unix)]
        return self.connect_unix();

        #[cfg(windows)]
        return self.connect_windows();
    }

    #[cfg(unix)]
    fn connect_unix(&mut self) -> Result<(), DiscordRpcError> {
        let possible_paths = vec![
            std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string()),
            "/tmp".to_string(),
            std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string()),
        ];

        for base_path in possible_paths {
            for i in 0..10 {
                let socket_path = format!("{}/discord-ipc-{}", base_path, i);
                match UnixStream::connect(&socket_path) {
                    Ok(stream) => {
                        self.stream = Some(stream);
                        return self.perform_handshake();
                    }
                    Err(_) => continue,
                }
            }
        }

        Err(DiscordRpcError::ConnectionFailed(
            "Could not find Discord IPC socket".to_string(),
        ))
    }

    #[cfg(windows)]
    fn connect_windows(&mut self) -> Result<(), DiscordRpcError> {
        use std::fs::OpenOptions;

        for i in 0..10 {
            let pipe_name = format!(r"\\.\pipe\discord-ipc-{}", i);
            match OpenOptions::new().read(true).write(true).open(&pipe_name) {
                Ok(file) => {
                    let reader = BufReader::new(file.try_clone()?);
                    let writer = BufWriter::new(file);
                    self.connection = Some(NamedPipeConnection { reader, writer });
                    return self.perform_handshake();
                }
                Err(_) => continue,
            }
        }

        Err(DiscordRpcError::ConnectionFailed(
            "Could not find Discord named pipe".to_string(),
        ))
    }

    fn perform_handshake(&mut self) -> Result<(), DiscordRpcError> {
        let handshake = json!({
            "v": 1,
            "client_id": self.client_id
        });

        self.write_message(0, &handshake)?;
        let response = self.read_message()?;

        if let Some(evt) = response.get("evt") {
            if evt.as_str() == Some("READY") {
                self.connected = true;
                return Ok(());
            }
        }

        if let Some(evt) = response.get("evt") {
            if evt.as_str() == Some("ERROR") {
                let message = response
                    .get("data")
                    .and_then(|d| d.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("Unknown error");
                return Err(DiscordRpcError::HandshakeError(message.to_string()));
            }
        }

        Err(DiscordRpcError::HandshakeError(format!(
            "Unexpected handshake response: {}",
            response
        )))
    }

    pub fn set_activity(&mut self, activity: &Activity) -> Result<(), DiscordRpcError> {
        if !self.connected {
            return Err(DiscordRpcError::NotConnected);
        }

        let message = json!({
            "cmd": "SET_ACTIVITY",
            "nonce": self.generate_nonce(),
            "args": {
                "pid": std::process::id(),
                "activity": activity
            }
        });

        self.write_message(1, &message)?;
        let response = self.read_message()?;

        if let Some(evt) = response.get("evt") {
            if evt.as_str() == Some("ERROR") {
                let error_msg = response
                    .get("data")
                    .and_then(|d| d.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("Unknown activity error");
                return Err(DiscordRpcError::HandshakeError(error_msg.to_string()));
            }
        }

        Ok(())
    }

    pub fn clear_activity(&mut self) -> Result<(), DiscordRpcError> {
        if !self.connected {
            return Err(DiscordRpcError::NotConnected);
        }

        let message = json!({
            "cmd": "SET_ACTIVITY",
            "nonce": self.generate_nonce(),
            "args": {
                "pid": std::process::id(),
                "activity": null
            }
        });

        self.write_message(1, &message)?;
        let _ = self.read_message()?;

        Ok(())
    }

    fn write_message(&mut self, opcode: u32, data: &Value) -> Result<(), DiscordRpcError> {
        let json_data = serde_json::to_string(data)?;
        let length = json_data.len() as u32;

        let mut buffer = Vec::new();
        buffer.extend_from_slice(&opcode.to_le_bytes());
        buffer.extend_from_slice(&length.to_le_bytes());
        buffer.extend_from_slice(json_data.as_bytes());

        #[cfg(unix)]
        if let Some(ref mut stream) = self.stream {
            stream.write_all(&buffer)?;
            stream.flush()?;
        }

        #[cfg(windows)]
        if let Some(ref mut conn) = self.connection {
            conn.writer.write_all(&buffer)?;
            conn.writer.flush()?;
        }

        Ok(())
    }

    fn read_message(&mut self) -> Result<Value, DiscordRpcError> {
        let mut header = [0u8; 8];

        #[cfg(unix)]
        if let Some(ref mut stream) = self.stream {
            stream.read_exact(&mut header)?;
        }

        #[cfg(windows)]
        if let Some(ref mut conn) = self.connection {
            conn.reader.read_exact(&mut header)?;
        }

        let opcode = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);

        let mut data = vec![0u8; length as usize];

        #[cfg(unix)]
        if let Some(ref mut stream) = self.stream {
            stream.read_exact(&mut data)?;
        }

        #[cfg(windows)]
        if let Some(ref mut conn) = self.connection {
            conn.reader.read_exact(&mut data)?;
        }

        let value: Value = serde_json::from_slice(&data)?;
        Ok(value)
    }

    fn generate_nonce(&self) -> String {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            .to_string()
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let client = DiscordRpcClient::new("test_client_id");
        assert!(!client.is_connected());
        assert_eq!(client.client_id, "test_client_id");
    }
}
