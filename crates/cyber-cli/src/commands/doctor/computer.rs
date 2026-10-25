//! Inspect the running server's native identity without requesting OS consent.
use super::{Check, Status, check};
use crate::context::Context;
use serde_json::Value;
pub(super) async fn inspect(ctx: &Context) -> Check {
    let settings = match ctx
        .global_config()
        .map_err(|error| error.message)
        .and_then(|config| cyber_core::config::ComputerSettings::from_global(&config))
    {
        Ok(settings) => settings,
        Err(error) => return check("computer", Status::Warn, error),
    };
    if !settings.enabled {
        return check(
            "computer",
            Status::Pass,
            "disabled (global features.computer_use is off)",
        );
    }
    let Some(registration) = cyber_app::read_registration(&ctx.paths) else {
        return check(
            "computer",
            Status::Warn,
            "enabled; start the server and run `cyber computer status` to inspect its OS consent",
        );
    };
    let password = match cyber_app::password(&ctx.paths) {
        Ok(password) => password,
        Err(error) => return check("computer", Status::Warn, error),
    };
    let client = cyber_client::Client::http(&registration.url, Some(password));
    match client.raw("GET", "/tools/computer/health", None, &[]).await {
        Ok((200, body)) => report(&body["data"]),
        _ => check(
            "computer",
            Status::Warn,
            "native health unavailable; restart the server and run `cyber computer status`",
        ),
    }
}
fn report(value: &Value) -> Check {
    if value["enabled"] != true {
        return check(
            "computer",
            Status::Warn,
            "enabled in configuration but disabled in the running server; restart it",
        );
    }
    if value["health"]["supported"] != true {
        return check(
            "computer",
            Status::Warn,
            format!(
                "unsupported platform/version; requires {}",
                value["health"]["minimum_version"]
                    .as_str()
                    .unwrap_or("macOS 14+")
            ),
        );
    }
    if value["health"]["screen_capture"] != true || value["health"]["accessibility"] != true {
        return check(
            "computer",
            Status::Warn,
            format!(
                "OS consent missing for {}; inspect `cyber computer status`, then explicitly run `cyber computer setup --yes`",
                value["health"]["binary"]
                    .as_str()
                    .unwrap_or("the running server binary")
            ),
        );
    }
    check(
        "computer",
        Status::Pass,
        "native OS permissions available; an attached Session and explicit owner target grant are still required",
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn health_reports_missing_consent_and_unsupported_versions_without_prompts() {
        let ready = json!({"enabled":true,"health":{"supported":true,"screen_capture":true,"accessibility":true}});
        assert_eq!(report(&ready).status, Status::Pass);
        let missing = json!({"enabled":true,"health":{"supported":true,"screen_capture":false,"accessibility":true,"binary":"fixture-server"}});
        assert_eq!(report(&missing).status, Status::Warn);
        assert!(report(&missing).detail.contains("setup --yes"));
        assert_eq!(
            report(
                &json!({"enabled":true,"health":{"supported":false,"minimum_version":"macOS 14"}})
            )
            .status,
            Status::Warn
        );
    }
}
