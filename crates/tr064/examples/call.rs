//! Calls an action and prints its output arguments.
//!
//! ```sh
//! TR064_USERNAME=user TR064_PASSWORD=secret \
//!     cargo run -p tr064 --example call -- DeviceInfo1 GetInfo
//! cargo run -p tr064 --example call -- Hosts1 GetGenericHostEntry NewIndex=0
//! ```
//!
//! The service is a type or id, or a short form like `DeviceInfo1` or
//! `DeviceInfo:1`. Input arguments are given as `Name=value`.
//!
//! Actions can change the configuration of the device; this example calls
//! whatever it is told to.

mod common;

use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    let [service, action, inputs @ ..] = args.as_slice() else {
        return Err("usage: call <service> <action> [Name=value ...]".into());
    };
    let inputs = inputs
        .iter()
        .map(|input| {
            input
                .split_once('=')
                .ok_or(format!("{input} is not Name=value"))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let client = common::client()?;
    let description = client.description().await?;
    let service = description.service(service)?;
    match client.call(service, action, &inputs).await {
        Ok(outputs) => {
            for (name, value) in outputs.iter() {
                println!("{name} = {value}");
            }
            Ok(())
        }
        Err(error) => match error.upnp_code() {
            // The standard codes for wrong input (402), wrong state (501)
            // and an index beyond the end of a list (713).
            Some(402 | 501 | 713) => Err(format!("the device refused the input: {error}").into()),
            _ => Err(error.into()),
        },
    }
}
