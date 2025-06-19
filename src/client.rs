use crate::error::DiscordRpcError;
use crate::models::Activity;
use serde_json::{Value, json};
use std::pin::Pin;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::time;
use tracing::{debug, error, info, instrument, warn};

// Importaciones específicas de plataforma
#[cfg(unix)]
use tokio::net::UnixStream;

#[cfg(windows)]
use tokio::net::windows::named_pipe::ClientOptions;

// Definición del tipo de stream asíncrono
trait AsyncReadWrite: AsyncRead + AsyncWrite + Send + Sync + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Sync + Unpin> AsyncReadWrite for T {}

type AsyncStream = Pin<Box<dyn AsyncReadWrite>>;

pub struct DiscordRpcClient {
    client_id: String,
    command_tx: mpsc::Sender<ClientCommand>,
}

#[derive(Debug)]
enum ClientCommand {
    SetActivity(Activity),
    ClearActivity,
    Disconnect,
}

// Mensajes para el servidor
#[derive(Debug)]
enum ServerMessage {
    Payload(Value),
    Close,
}

// Eventos de heartbeat
#[derive(Debug)]
enum HeartbeatEvent {
    Beat,
    Ack,
    Stop,
}

// Comandos para la tarea de escritura
#[derive(Debug)]
enum WriteCommand {
    SetActivity(Activity),
    ClearActivity,
    Heartbeat,
    Disconnect,
}

impl DiscordRpcClient {
    pub fn new(client_id: &str) -> Self {
        let (command_tx, command_rx) = mpsc::channel(32);
        let client = Self {
            client_id: client_id.to_string(),
            command_tx,
        };

        // Iniciar la tarea de fondo
        tokio::spawn(Self::connection_task(client.client_id.clone(), command_rx));

        client
    }

    pub async fn set_activity(&self, activity: Activity) -> Result<(), DiscordRpcError> {
        self.command_tx
            .send(ClientCommand::SetActivity(activity))
            .await
            .map_err(|_| DiscordRpcError::NotConnected)
    }

    pub async fn clear_activity(&self) -> Result<(), DiscordRpcError> {
        self.command_tx
            .send(ClientCommand::ClearActivity)
            .await
            .map_err(|_| DiscordRpcError::NotConnected)
    }

    pub async fn disconnect(&self) -> Result<(), DiscordRpcError> {
        self.command_tx
            .send(ClientCommand::Disconnect)
            .await
            .map_err(|_| DiscordRpcError::NotConnected)
    }

    #[instrument(skip(command_rx))]
    async fn connection_task(client_id: String, mut command_rx: mpsc::Receiver<ClientCommand>) {
        info!("Starting Discord RPC connection task");

        loop {
            match Self::connect_and_run(&client_id, &mut command_rx).await {
                Ok(_) => info!("Connection closed cleanly"),
                Err(e) => warn!("Connection error: {}", e),
            }

            // Esperar antes de reconectar
            time::sleep(Duration::from_secs(5)).await;
        }
    }

    #[instrument(skip(command_rx))]
    async fn connect_and_run(
        client_id: &str,
        command_rx: &mut mpsc::Receiver<ClientCommand>,
    ) -> Result<(), DiscordRpcError> {
        let mut stream = Self::connect().await?;
        let heartbeat_interval = Self::perform_handshake(&mut stream, client_id).await?;

        // Iniciar tareas de background
        let (write_tx, write_rx) = mpsc::channel(32);
        let (server_tx, mut server_rx) = mpsc::channel(32);
        let (heartbeat_tx, mut heartbeat_rx) = mpsc::channel(32);

        // Split del stream en lectura y escritura
        let (read_half, write_half) = tokio::io::split(stream);

        let read_handle = tokio::spawn(Self::read_task(read_half, server_tx, heartbeat_tx.clone()));

        // Pasar client_id a la tarea de escritura
        let client_id_str = client_id.to_string();
        let write_handle = tokio::spawn(Self::write_task(write_half, write_rx, client_id_str));

        let heartbeat_handle =
            tokio::spawn(Self::heartbeat_task(heartbeat_interval, write_tx.clone()));

        // Loop principal para manejar comandos
        loop {
            tokio::select! {
                Some(cmd) = command_rx.recv() => {
                    match cmd {
                        ClientCommand::SetActivity(activity) => {
                            if let Err(e) = write_tx.send(WriteCommand::SetActivity(activity)).await {
                                error!("Failed to send activity: {}", e);
                            }
                        }
                        ClientCommand::ClearActivity => {
                            if let Err(e) = write_tx.send(WriteCommand::ClearActivity).await {
                                error!("Failed to send clear activity: {}", e);
                            }
                        }
                        ClientCommand::Disconnect => {
                            info!("Disconnecting by user request");
                            break;
                        }
                    }
                }
                Some(msg) = server_rx.recv() => {
                    debug!("Received message from server: {:?}", msg);
                }
                Some(heartbeat) = heartbeat_rx.recv() => {
                    match heartbeat {
                        HeartbeatEvent::Beat => {
                            // Enviar heartbeat a través de la tarea de escritura
                            if let Err(e) = write_tx.send(WriteCommand::Heartbeat).await {
                                error!("Failed to send heartbeat: {}", e);
                            }
                        }
                        _ => {}
                    }
                }
                else => {
                    warn!("All channels closed, exiting");
                    break;
                }
            }
        }

        // Limpiar tareas
        heartbeat_tx.send(HeartbeatEvent::Stop).await.ok();
        write_tx.send(WriteCommand::Disconnect).await.ok();

        read_handle.abort();
        write_handle.abort();
        heartbeat_handle.abort();

        Ok(())
    }

    async fn connect() -> Result<AsyncStream, DiscordRpcError> {
        #[cfg(unix)]
        return Self::connect_unix().await;

        #[cfg(windows)]
        return Self::connect_windows().await;
    }

    #[cfg(unix)]
    async fn connect_unix() -> Result<AsyncStream, DiscordRpcError> {
        let possible_paths =
            vec![std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string())]; //Se pueden agregar mas paths si es necesario xd

        for base_path in possible_paths {
            for i in 0..10 {
                let socket_path = format!("{}/discord-ipc-{}", base_path, i);
                match UnixStream::connect(&socket_path).await {
                    Ok(stream) => {
                        info!("Connected to Unix socket: {}", socket_path);
                        return Ok(Box::pin(stream));
                    }
                    Err(e) => {
                        debug!("Connection failed to {}: {}", socket_path, e);
                        continue;
                    }
                }
            }
        }

        Err(DiscordRpcError::ConnectionFailed(
            "Could not find Discord IPC socket".to_string(),
        ))
    }

    #[cfg(windows)]
    async fn connect_windows() -> Result<AsyncStream, DiscordRpcError> {
        for i in 0..10 {
            let pipe_name = format!(r"\\.\pipe\discord-ipc-{}", i);
            match ClientOptions::new().open(&pipe_name) {
                Ok(pipe) => {
                    info!("Connected to named pipe: {}", pipe_name);
                    return Ok(Box::pin(pipe));
                }
                Err(e) => {
                    debug!("Connection failed to {}: {}", pipe_name, e);
                    continue;
                }
            }
        }

        Err(DiscordRpcError::ConnectionFailed(
            "Could not find Discord named pipe".to_string(),
        ))
    }

    async fn perform_handshake(
        stream: &mut AsyncStream,
        client_id: &str,
    ) -> Result<u64, DiscordRpcError> {
        let handshake = json!({
            "v": 1,
            "client_id": client_id
        });

        Self::write_message(stream, 0, &handshake).await?;
        let response = Self::read_message(stream).await?;

        // Nueva lógica para manejar diferentes formatos de respuesta
        if response.get("evt").is_some() && response["evt"] == "READY" {
            info!("Handshake successful (legacy format)");
            return Self::extract_heartbeat_interval(&response);
        }

        if response.get("cmd").is_some() && response["cmd"] == "DISPATCH" {
            info!("Handshake successful (new format)");
            return Self::extract_heartbeat_interval(&response);
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

    // Función auxiliar para extraer el intervalo de heartbeat
    fn extract_heartbeat_interval(response: &Value) -> Result<u64, DiscordRpcError> {
        // Intenta obtener el intervalo de 3 formas diferentes
        let interval = response
            .get("data")
            .and_then(|d| d.get("heartbeat_interval"))
            .or_else(|| response.get("heartbeat_interval")) // Nueva ubicación posible
            .and_then(|hi| hi.as_u64());

        match interval {
            Some(interval) => Ok(interval),
            None => {
                warn!(
                    "Handshake missing heartbeat_interval. Response: {}",
                    response
                );
                // Usar valor por defecto de 45 segundos (45,000 ms)
                warn!("Using default heartbeat interval: 45000 ms");
                Ok(45000)
            }
        }
    }

    async fn write_message(
        stream: &mut (impl AsyncWrite + Unpin),
        opcode: u32,
        data: &Value,
    ) -> Result<(), DiscordRpcError> {
        let json_data = serde_json::to_string(data)?;
        let length = json_data.len() as u32;

        let mut buffer = Vec::new();
        buffer.extend_from_slice(&opcode.to_le_bytes());
        buffer.extend_from_slice(&length.to_le_bytes());
        buffer.extend_from_slice(json_data.as_bytes());

        stream.write_all(&buffer).await?;
        stream.flush().await?;

        debug!("Sent message: opcode={}, length={}", opcode, length);
        Ok(())
    }

    async fn read_message(stream: &mut (impl AsyncRead + Unpin)) -> Result<Value, DiscordRpcError> {
        let mut header = [0u8; 8];
        stream.read_exact(&mut header).await?;

        let opcode = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);

        let mut data = vec![0u8; length as usize];
        stream.read_exact(&mut data).await?;

        let value: Value = serde_json::from_slice(&data)?;
        debug!("Received message: opcode={}, length={}", opcode, length);
        Ok(value)
    }

    fn generate_nonce() -> String {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            .to_string()
    }

    #[instrument(skip(stream, server_tx, heartbeat_tx))]
    async fn read_task(
        mut stream: tokio::io::ReadHalf<impl AsyncReadWrite>,
        server_tx: mpsc::Sender<ServerMessage>,
        heartbeat_tx: mpsc::Sender<HeartbeatEvent>,
    ) {
        info!("Starting read task");
        loop {
            match Self::read_message(&mut stream).await {
                Ok(msg) => {
                    // Manejar heartbeats
                    if let Some(opcode) = msg.get("op") {
                        if let Some(1) = opcode.as_u64() {
                            heartbeat_tx.send(HeartbeatEvent::Ack).await.ok();
                            continue;
                        }
                    }

                    // Enviar mensaje al loop principal
                    if let Err(e) = server_tx.send(ServerMessage::Payload(msg)).await {
                        error!("Failed to send message to main loop: {}", e);
                        break;
                    }
                }
                Err(e) => {
                    error!("Read error: {}", e);
                    break;
                }
            }
        }
        info!("Read task exiting");
    }

    #[instrument(skip(stream, write_rx))]
    async fn write_task(
        mut stream: tokio::io::WriteHalf<impl AsyncReadWrite>,
        mut write_rx: mpsc::Receiver<WriteCommand>,
        client_id: String, // Nuevo parámetro
    ) {
        info!("Starting write task");

        while let Some(cmd) = write_rx.recv().await {
            match cmd {
                WriteCommand::SetActivity(activity) => {
                    let message = json!({
                        "cmd": "SET_ACTIVITY",
                        "nonce": Self::generate_nonce(),
                        "args": {
                            "pid": std::process::id(),
                            "activity": activity,
                            "client_id": client_id  // Campo requerido por Discord
                        }
                    });

                    if let Err(e) = Self::write_message(&mut stream, 1, &message).await {
                        error!("Failed to write activity: {}", e);
                        break;
                    }
                }
                WriteCommand::ClearActivity => {
                    let message = json!({
                        "cmd": "SET_ACTIVITY",
                        "nonce": Self::generate_nonce(),
                        "args": {
                            "pid": std::process::id(),
                            "activity": null,
                            "client_id": client_id  // Campo requerido por Discord
                        }
                    });

                    if let Err(e) = Self::write_message(&mut stream, 1, &message).await {
                        error!("Failed to write clear activity: {}", e);
                        break;
                    }
                }
                WriteCommand::Heartbeat => {
                    let message = json!({
                        "op": 1,
                        "d": null
                    });

                    if let Err(e) = Self::write_message(&mut stream, 1, &message).await {
                        error!("Failed to write heartbeat: {}", e);
                        break;
                    }
                }
                WriteCommand::Disconnect => {
                    info!("Write task received disconnect command");
                    break;
                }
            }
        }
        info!("Write task exiting");
    }

    #[instrument(skip(write_tx))]
    async fn heartbeat_task(interval_ms: u64, write_tx: mpsc::Sender<WriteCommand>) {
        info!("Starting heartbeat task with interval: {}ms", interval_ms);
        let mut interval = time::interval(Duration::from_millis(interval_ms));

        loop {
            interval.tick().await;

            if let Err(e) = write_tx.send(WriteCommand::Heartbeat).await {
                error!("Failed to send heartbeat: {}", e);
                break;
            }
        }
        info!("Heartbeat task exiting");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_client_creation() {
        let client = DiscordRpcClient::new("test_client_id");
        assert_eq!(client.client_id, "test_client_id");
    }
}
