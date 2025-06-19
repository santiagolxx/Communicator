use communicator::{Activity, Assets, DiscordRpcClient, Timestamps};
use std::time::SystemTime;
use tokio::time;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let client = DiscordRpcClient::new("1305247641252397059");

    // Esperar un momento para la conexión
    time::sleep(time::Duration::from_secs(1)).await;

    // Crear actividad
    let activity = Activity {
        details: Some("Developing a Rust app".to_string()),
        state: Some("Coding".to_string()),
        timestamps: Some(Timestamps {
            start: Some(
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)?
                    .as_secs(),
            ),
            end: None,
        }),
        assets: Some(Assets {
            large_image: Some("rust".to_string()),
            large_text: Some("Rust Programming".to_string()),
            small_image: Some("vscode".to_string()),
            small_text: Some("VS Code".to_string()),
        }),
        party: None,
        secrets: None,
        instance: Some(false),
    };

    // Establecer actividad
    client.set_activity(activity).await?;

    // Mantener la aplicación corriendo
    loop {
        time::sleep(time::Duration::from_secs(10)).await;
    }
}
