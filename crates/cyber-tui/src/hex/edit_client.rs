use super::*;
use cyber_client::Client;

pub async fn perform(
    client: &Client,
    session: &Session,
    document: Document,
    command: Command,
    operation_id: String,
) -> Result<Reply, String> {
    let base = format!("/files/binary/documents/{}", document.id);
    if let Command::Copy { offset, length } = command {
        return copy(client, session, document, &base, offset, length).await;
    }
    if matches!(command, Command::Reconcile) {
        let response = client
            .at(&session.directory)
            .get(&format!("{base}?session_id={}", session.id))
            .await
            .map_err(|e| e.to_string())?;
        if response["data"]["pending_operation"].is_null() {
            return Ok(Reply {
                admitted: false,
                clipboard: None,
                status: response["data"].clone(),
                review: None,
                operation: None,
            });
        }
    }

    let mut body = json!({"session_id":session.id,"revision":document.revision});
    let (route, review) = match command {
        Command::Copy { .. } => unreachable!("copy returned above"),
        Command::Prepare => ("prepare", None),
        Command::Patch {
            offset,
            delete,
            hex,
        } => {
            if delete > 4096 {
                return Err("Patch exceeds bounded selection limit".into());
            }
            let expected = if delete == 0 {
                String::new()
            } else {
                let loaded = super::super::page(client, session, document.clone(), offset).await?;
                let bytes = loaded
                    .bytes
                    .get(..delete as usize)
                    .ok_or("Short expected-byte read")?;
                bytes.iter().map(|b| format!("{b:02x}")).collect()
            };
            body["operation_id"] = json!(operation_id);
            body["splices"] = json!([{"offset":offset.to_string(),"delete_length":delete.to_string(),"expected_hex":expected,"insert_hex":hex}]);
            ("patches", None)
        }
        Command::History(action) => {
            body["operation_id"] = json!(operation_id);
            body["action"] = json!(action);
            ("history", None)
        }
        Command::Review { restore, start } => {
            if !restore {
                body["start"] = json!(start);
                body["limit"] = json!(128);
            }
            (
                if restore { "restore-review" } else { "review" },
                Some(restore),
            )
        }
        Command::Save {
            fingerprint,
            restore,
        } => {
            body =
                json!({"session_id":session.id,"operation_id":operation_id,"review":fingerprint});
            (if restore { "restore" } else { "save" }, None)
        }
        Command::Reconcile => {
            // Reconciliation never starts another native exchange.
            body = json!({"session_id":session.id});
            ("reconcile", None)
        }
    };
    let response = client
        .at(&session.directory)
        .post(&format!("{base}/{route}"), body)
        .await
        .map_err(|e| e.to_string())?;
    let data = &response["data"];
    let review = review
        .map(|restore| {
            serde_json::from_value(data.clone())
                .map(|review| (review, restore))
                .map_err(|_| "Invalid binary review".to_owned())
        })
        .transpose()?;
    let status = if route == "prepare" {
        data.clone()
    } else if data.get("status").is_some() {
        data["status"].clone()
    } else {
        client
            .at(&session.directory)
            .get(&format!("{base}?session_id={}", session.id))
            .await
            .map_err(|e| e.to_string())?["data"]
            .clone()
    };
    Ok(Reply {
        admitted: route == "prepare",
        clipboard: None,
        status,
        review,
        operation: data.get("operation").cloned(),
    })
}

async fn copy(
    client: &Client,
    session: &Session,
    document: Document,
    base: &str,
    offset: u64,
    length: u64,
) -> Result<Reply, String> {
    if length == 0 || length > 4096 {
        return Err("Copy requires a selection of 1–4096 bytes".into());
    }
    let loaded = super::super::page(client, session, document, offset).await?;
    let bytes = loaded
        .bytes
        .get(..length as usize)
        .ok_or("Short selection read")?;
    let hex = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let response = client
        .at(&session.directory)
        .get(&format!("{base}?session_id={}", session.id))
        .await
        .map_err(|e| e.to_string())?;
    if response["data"]["revision"] != loaded.document.revision {
        return Err("Selection revision changed".into());
    }
    Ok(Reply {
        admitted: false,
        status: response["data"].clone(),
        clipboard: Some(hex),
        review: None,
        operation: None,
    })
}
