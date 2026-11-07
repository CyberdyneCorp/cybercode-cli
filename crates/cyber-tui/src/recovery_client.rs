use super::{Command, Reply};
use crate::{model::Session, rewind::Request};
use cyber_client::Client;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

fn segment(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}
fn data<T: DeserializeOwned>(reply: Value) -> Result<T, String> {
    serde_json::from_value(reply["data"].clone())
        .map_err(|error| format!("Invalid recovery reply: {error}"))
}
pub async fn perform(
    client: &Client,
    request: &Request,
    command: &Command,
) -> Result<Reply, String> {
    let session = Session::parse(
        &client
            .get(&format!("/sessions/{}", segment(&request.session)))
            .await
            .map_err(|error| error.to_string())?["data"],
    );
    if session.id != request.session || session.directory != request.directory {
        return Err("Recovery Session/Location changed; refresh without replay".into());
    }
    let base = format!("/sessions/{}/rewind/recovery", segment(&request.session));
    match command {
        Command::List => {
            let journals: Vec<cyber_server::runtime::SnapshotRecovery> =
                data(client.get(&base).await.map_err(|error| error.to_string())?)?;
            if journals.len() > 4096 {
                return Err("Recovery journal limit exceeded".into());
            }
            Ok(Reply::Journals(journals))
        }
        Command::Discover(journal) => {
            let requests: Vec<cyber_server::runtime::SnapshotRecoveryConfirmation> = data(
                client
                    .get(&format!("{base}/requests?journal_id={}", segment(journal)))
                    .await
                    .map_err(|error| error.to_string())?,
            )?;
            if requests.len() > 4096 {
                return Err("Recovery request limit exceeded".into());
            }
            Ok(Reply::Requests(requests))
        }
        Command::Review(selection) => Ok(Reply::Review(Box::new(data(
            client
                .post(&format!("{base}/review"), json!(selection))
                .await
                .map_err(|error| error.to_string())?,
        )?))),
        Command::Confirm(confirmation) => Ok(Reply::Outcome(Box::new(data(
            client
                .post(&format!("{base}/confirm"), json!(confirmation))
                .await
                .map_err(|error| {
                    format!(
                        "{error}. Retain request {:?}; R looks it up without replay",
                        confirmation.request_id
                    )
                })?,
        )?))),
        Command::Lookup(confirmation) => {
            if matches!(confirmation.request_id.as_str(), "." | "..") {
                return Err("This request ID is not addressable through the path lookup API; retained evidence is unchanged".into());
            }
            Ok(Reply::Outcome(Box::new(data(
                client
                    .get(&format!(
                        "{base}/requests/{}",
                        segment(&confirmation.request_id)
                    ))
                    .await
                    .map_err(|error| error.to_string())?,
            )?)))
        }
        Command::Acknowledge(confirmation) => Ok(Reply::Outcome(Box::new(data(
            client
                .post(
                    &format!(
                        "{base}/requests/{}/acknowledge",
                        segment(&confirmation.request_id)
                    ),
                    json!({}),
                )
                .await
                .map_err(|error| error.to_string())?,
        )?))),
    }
}
