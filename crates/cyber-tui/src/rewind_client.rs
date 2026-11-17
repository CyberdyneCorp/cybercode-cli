use super::{Command, Reply, Request};
use crate::{
    model::Session,
    perform::{Msg, snapshot},
};
use cyber_client::Client;
use serde_json::json;

pub async fn perform(client: &Client, request: Request) -> Msg {
    let result = run(&client.at(&request.directory), &request).await;
    Msg::Rewind {
        request,
        result: Box::new(result),
    }
}
async fn load(client: &Client, request: &Request) -> Result<Msg, String> {
    let snapshot = snapshot(client, &request.session).await?;
    let Msg::Snapshot { session, .. } = &snapshot else {
        return Err("Invalid Session snapshot".into());
    };
    let latest = Session::parse(
        &client
            .get(&format!("/sessions/{}", request.session))
            .await
            .map_err(|e| e.to_string())?["data"],
    );
    if latest.id != request.session
        || latest.directory != request.directory
        || latest.seq != session.seq
    {
        return Err("Session changed while loading; refresh".into());
    }
    Ok(snapshot)
}
async fn run(client: &Client, request: &Request) -> Result<Reply, String> {
    let route = format!("/sessions/{}/rewind", request.session);
    match &request.command {
        Command::Recovery(command) => crate::recovery::perform(client, request, command)
            .await
            .map(|reply| Reply::Recovery(Box::new(reply))),
        Command::Load => Ok(Reply::Loaded(Box::new(load(client, request).await?))),
        Command::Preview(selection) => {
            let response = client
                .post(
                    &format!("{route}/preview"),
                    serde_json::to_value(selection).map_err(|e| e.to_string())?,
                )
                .await
                .map_err(|e| e.to_string())?;
            Ok(Reply::Preview(Box::new(
                serde_json::from_value(response["data"].clone())
                    .map_err(|_| "Invalid rewind preview")?,
            )))
        }
        Command::Confirm(confirmation) => {
            let response = client
                .post(
                    &format!("{route}/confirm"),
                    serde_json::to_value(confirmation).map_err(|e| e.to_string())?,
                )
                .await
                .map_err(|e| e.to_string())?;
            let operation = serde_json::from_value(response["data"].clone())
                .map_err(|_| "Invalid rewind acknowledgement")?;
            Ok(Reply::Changed {
                snapshot: Box::new(load(client, request).await?),
                operation,
            })
        }
        Command::Interrupt => {
            client
                .post(
                    &format!("/sessions/{}/interrupt", request.session),
                    json!({}),
                )
                .await
                .map_err(|e| e.to_string())?;
            let response = client
                .get(&format!("/sessions/{}", request.session))
                .await
                .map_err(|e| e.to_string())?;
            Ok(Reply::Interrupted(Box::new(Session::parse(
                &response["data"],
            ))))
        }
    }
}
