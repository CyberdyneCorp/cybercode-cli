//! Owner-reviewed rewind. The default is a non-mutating preview; applying requires
//! the exact fingerprint from a prior review, including in non-interactive scripts.
use clap::ValueEnum;
use cyber_client::Client;
use serde_json::json;

use super::sessions::{api, clean, encode, resolve};
use crate::{cli::GlobalArgs, error::CliError, output};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Target {
    Code,
    Conversation,
    Both,
}
impl Target {
    fn name(self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Conversation => "conversation",
            Self::Both => "both",
        }
    }
}

pub fn fingerprint(value: &str) -> Result<String, String> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(value.to_ascii_lowercase())
    } else {
        Err("expected the 64-character confirmation fingerprint from a rewind preview".into())
    }
}

pub async fn run(
    client: &Client,
    session: &str,
    message: &str,
    target: Target,
    confirmation: Option<String>,
    global: &GlobalArgs,
) -> Result<(), CliError> {
    let found = resolve(client, session).await?;
    let id = found["id"]
        .as_str()
        .ok_or_else(|| CliError::runtime("Missing Session identity"))?;
    let directory = found["directory"]
        .as_str()
        .ok_or_else(|| CliError::runtime("Missing Session Location"))?;
    let mut body =
        json!({"message_id":message,"target":target.name(),"expected_directory":directory});
    let applying = confirmation.is_some();
    if let Some(confirmation) = confirmation {
        body["confirmation"] = json!(confirmation);
    }
    let route = if applying { "confirm" } else { "preview" };
    let reply = client
        .post(&format!("/sessions/{}/rewind/{route}", encode(id)), body)
        .await
        .map_err(api)?;
    let data = &reply["data"];
    if output::is_json(global.format) {
        return output::json(data);
    }
    if applying {
        println!(
            "rewound {} before {} ({})",
            clean(id),
            clean(message),
            target.name()
        );
        if matches!(target, Target::Code) {
            println!("The conversation is unchanged.");
        } else {
            println!("Later messages remain on an inactive branch through sessions tree/checkout.");
        }
        return Ok(());
    }
    println!(
        "Rewind preview: {} before {} ({})",
        clean(id),
        clean(message),
        target.name()
    );
    println!("Location: {}", clean(directory));
    if let Some(files) = data["code"]["files"].as_array() {
        for file in files {
            println!(
                "{} {}",
                clean(file["status"].as_str().unwrap_or("changed")),
                clean(file["file"].as_str().unwrap_or(""))
            );
            for line in file["patch"].as_str().unwrap_or("").lines() {
                println!("{}", clean(line));
            }
        }
        for path in data["code"]["skipped"].as_array().into_iter().flatten() {
            println!(
                "excluded from current snapshot: {}",
                clean(path.as_str().unwrap_or(""))
            );
        }
    }
    println!(
        "{}",
        data["warning"]
            .as_str()
            .unwrap_or("External effects are not undone.")
    );
    for effect in data["effects"].as_array().into_iter().flatten() {
        println!(
            "retained tool effect: {} {} ({})",
            clean(effect["call_id"].as_str().unwrap_or("")),
            clean(effect["tool"].as_str().unwrap_or("")),
            effect["status"]
        );
    }
    println!(
        "No files or conversation path changed. Review this preview, then repeat with --confirm {}",
        data["confirmation"].as_str().unwrap_or("")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    #[test]
    fn invalid_targets_and_unreviewed_confirmations_fail_before_startup() {
        for tail in [
            vec!["--target", "everything"],
            vec!["--confirm", "yes"],
            vec!["--confirm", &"x".repeat(64)],
        ] {
            let mut args = vec!["cyber", "sessions", "rewind", "ses_one", "--to", "msg_one"];
            args.extend(tail);
            assert!(crate::cli::Cli::try_parse_from(args).is_err());
        }
        assert_eq!(super::fingerprint(&"A".repeat(64)).unwrap(), "a".repeat(64));
    }
}
