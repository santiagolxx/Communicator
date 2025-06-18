use communicator::{Activity, Assets, DiscordRpcClient, Timestamps};
use std::time::{SystemTime, UNIX_EPOCH};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client_id = "1305247641252397059";
    let mut client = DiscordRpcClient::new(client_id);

    client.connect()?;

    let activity = Activity {
        details: Some("Developing a Rust app".to_string()),
        state: Some("Coding".to_string()),
        timestamps: Some(Timestamps {
            start: Some(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()),
            end: None,
        }),
        assets: Some(Assets {
            large_image: Some("logo".to_string()), // if you have uploaded at discord dev portal, same with small_img
            large_text: Some("the best launcher on the world".to_string()),
            small_image: Some("rust".to_string()),
            small_text: Some("Rust Programming Language".to_string()),
        }),
        party: None,
        secrets: None,
        instance: Some(false),
    };

    client.set_activity(&activity)?;

    // Mantener la actividad activa
    std::thread::sleep(std::time::Duration::from_secs(30));

    client.clear_activity()?;
    Ok(())
}
