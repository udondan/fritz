//! Lists the services of a device and the actions of each service.
//!
//! ```sh
//! TR064_URL=http://192.168.178.1:49000 cargo run -p tr064 --example services
//! ```
//!
//! The descriptions are usually readable without credentials.

mod common;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = common::client()?;
    let description = client.description().await?;
    for device in description.devices() {
        println!(
            "{} ({})",
            device.friendly_name.as_deref().unwrap_or("unnamed device"),
            device.device_type
        );
    }
    for service in description.services() {
        println!("\n{}", service.service_id);
        let scpd = client.scpd(service).await?;
        for action in &scpd.actions {
            let inputs: Vec<&str> = action.inputs().map(|a| a.name.as_str()).collect();
            let outputs: Vec<&str> = action.outputs().map(|a| a.name.as_str()).collect();
            println!(
                "  {}({}) -> ({})",
                action.name,
                inputs.join(", "),
                outputs.join(", ")
            );
        }
    }
    Ok(())
}
