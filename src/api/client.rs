//! Client commands for sign-in and device enrollment.

use super::*;

/// Send a client request to the local server; a refusal becomes its sentence.
pub(super) async fn send(request: reqwest::RequestBuilder) -> Result<reqwest::Response, Error> {
    let answer = (request.send().await).map_err(other("Reaching the Razor server"))?;
    if answer.status().is_success() {
        return Ok(answer);
    }
    let refusal: Value = answer.json().await.unwrap_or_default();
    let sentence = refusal["error"]
        .as_str()
        .unwrap_or("The server refused the request.");
    Err(Error::Refused(sentence.into()))
}

/// `razor login`: the ChatGPT sign-in runs in the server; each instruction is printed as it comes.
pub async fn login(client: &reqwest::Client, key: &str) -> Result<(), Error> {
    let mut answer = send(client.post(format!("{LOCAL}/v1/login")).bearer_auth(key)).await?;
    // `pending` holds at most one line of the answer.
    let mut pending = Vec::new();
    while let Some(chunk) = (answer.chunk().await).map_err(other("Reading the sign-in"))? {
        pending.extend_from_slice(&chunk);
        while let Some(newline) = pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = pending.drain(..=newline).collect();
            match serde_json::from_slice(&line).map_err(other("Reading the sign-in"))? {
                Login::Show { text } => println!("{text}"),
                Login::Done => return Ok(()),
                Login::Failed { error } => return Err(Error::Refused(error)),
            }
        }
    }
    Err(Error::Refused(
        "The Razor server went away before the sign-in ended.".into(),
    ))
}

/// `razor devices`: the enrolled devices' names, one a line.
pub async fn devices(client: &reqwest::Client, key: &str) -> Result<String, Error> {
    let answer = send(client.get(format!("{LOCAL}/v1/devices")).bearer_auth(key)).await?;
    let listed: Vec<Value> = (answer.json().await).map_err(other("Reading the device list"))?;
    if listed.is_empty() {
        return Ok("No device is enrolled; razor devices add <name> enrolls one.".into());
    }
    let names = listed.iter().filter_map(|d| d["name"].as_str());
    Ok(names.collect::<Vec<_>>().join("\n"))
}

/// `razor devices add <name> [--address <host:port>] [--key <key>]`: what to print. A device
/// with an address is another server, paired with one key: without `--key` this makes it, and
/// the text ends with the command that pairs this server, at `own`, on the other side.
pub async fn enroll(
    client: &reqwest::Client,
    key: &str,
    name: &str,
    address: Option<&str>,
    given: Option<&str>,
    own: Option<&str>,
) -> Result<String, Error> {
    if address.is_some() && own.is_none() {
        return Err(Error::Refused(
            "This server has no server.address, so the other could not answer it; set it, restart Razor, then pair.".into(),
        ));
    };
    let request = client.post(format!("{LOCAL}/v1/devices")).bearer_auth(key);
    let body = json!({ "name": name, "address": address, "key": given });
    let device: Value =
        (send(request.json(&body)).await?.json().await).map_err(other("Reading the new key"))?;
    let key = device["key"].as_str().unwrap_or_default();
    let pairing = match device["pairing"].as_str() {
        Some(line) => format!("To pair the phone app, paste this line into it:\n{line}"),
        None => format!(
            "To pair a phone, set server.address to this computer's tailnet name and port, restart Razor, then remove {name} and add it again."
        ),
    };
    Ok(match (address, given) {
        (None, _) => format!("{name} is enrolled. Its key, shown only now:\n{key}\n{pairing}"),
        (Some(_), Some(_)) => format!("{name} is paired."),
        (Some(_), None) => format!(
            "{name} is paired. On {name}, run this, with the name {name} will call this server:\nrazor devices add <name> --address {} --key {key}",
            own.unwrap_or_default()
        ),
    })
}

/// How `razor devices` is used.
pub const DEVICES_USE: &str = "Use razor login, razor devices, razor devices add <name> [--address <host:port>] [--key <key>], or razor devices remove <name>.";

/// `razor devices remove <name>`: what to print once the key no longer works.
pub async fn revoke(client: &reqwest::Client, key: &str, name: &str) -> Result<String, Error> {
    let request = client.delete(format!("{LOCAL}/v1/devices/{name}"));
    send(request.bearer_auth(key)).await?;
    Ok(format!("{name} is removed; its key no longer works."))
}
