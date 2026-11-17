use super::{Command, Reply};
use crate::{
    recovery::client::{data, segment},
    rewind::Request,
};
use cyber_client::Client;
use serde_json::json;

pub async fn perform(
    client: &Client,
    request: &Request,
    command: &Command,
) -> Result<Reply, String> {
    let base = format!(
        "/sessions/{}/rewind/preparations",
        segment(&request.session)
    );
    match command {
        Command::List => {
            let reply = client
                .get(&format!("{base}?include_cancelled=true"))
                .await
                .map_err(|error| error.to_string())?;
            Ok(Reply::Candidates(data(reply)?))
        }
        Command::Discover(activity) => {
            let reply = client
                .get(&format!("{base}/{}/receipt", segment(activity)))
                .await
                .map_err(|error| error.to_string())?;
            Ok(Reply::Discovered(data(reply)?))
        }
        Command::Review(activity) => {
            let reply = client
                .post(&format!("{base}/{}/review", segment(activity)), json!({}))
                .await
                .map_err(|error| error.to_string())?;
            Ok(Reply::Review(Box::new(data(reply)?)))
        }
        _ => receipt(client, &base, command).await,
    }
}
async fn receipt(client: &Client, base: &str, command: &Command) -> Result<Reply, String> {
    let reply = match command {
        Command::Confirm(activity, confirmation) => {
            client.post(&format!("{base}/{}/confirm", segment(activity)), json!(confirmation)).await
        }
        Command::Lookup(activity, confirmation) | Command::Acknowledge(activity, confirmation) => {
            if matches!(confirmation.request_id.as_str(), "." | "..") {
                return Err("Retained request ID cannot be addressed by this path API; evidence is preserved".into());
            }
            let path = format!("{base}/{}/requests/{}", segment(activity), segment(&confirmation.request_id));
            if matches!(command, Command::Acknowledge(..)) {
                client.post(&format!("{path}/acknowledge"), json!({})).await
            } else {
                client.get(&path).await
            }
        }
        _ => return Err("Expected an exact retained preparation request".into()),
    }.map_err(|error| format!("{error}. Retain the original request; R looks it up without replay"))?;
    Ok(Reply::Receipt(Box::new(data(reply)?)))
}
