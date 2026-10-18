//! `git` settings for generated commit text (`vcs-integration` → Commit and PR text
//! generation). Other `git` keys are read by their own owners.

use serde_json::Value;

/// `git.co_author`: the `Name <email>` added as a `Co-authored-by` trailer. Unset by
/// default, so generated commits are attributed to the user alone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitSettings {
    pub co_author: Option<String>,
}

impl GitSettings {
    pub fn from_config(config: &Value) -> Result<Self, String> {
        let Some(git) = config.get("git") else {
            return Ok(Self::default());
        };
        let git = git.as_object().ok_or("git: expected an object")?;
        let co_author = match git.get("co_author") {
            None | Some(Value::Null) => None,
            Some(Value::String(author)) if is_identity(author) => Some(author.trim().to_owned()),
            Some(_) => {
                return Err("git.co_author: expected one line of the form \"Name <email>\"".into());
            }
        };
        Ok(Self { co_author })
    }

    /// The trailer line, when a co-author is configured.
    pub fn trailer(&self) -> Option<String> {
        self.co_author
            .as_ref()
            .map(|author| format!("Co-authored-by: {author}"))
    }
}

/// `Name <local@domain>` on one line.
fn is_identity(author: &str) -> bool {
    let author = author.trim();
    let Some((name, email)) = author
        .strip_suffix('>')
        .and_then(|rest| rest.split_once(" <"))
    else {
        return false;
    };
    !name.trim().is_empty()
        && email.contains('@')
        && !email.contains(['<', '>', ' '])
        && !author.contains(['\n', '\r'])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn co_author_is_unset_by_default_and_validated_when_set() {
        assert_eq!(
            GitSettings::from_config(&json!({})).unwrap().trailer(),
            None
        );
        let set = GitSettings::from_config(
            &json!({"git":{"co_author":"Cyber Code <bot@cyber.dev>","protected_branches":["main"]}}),
        )
        .unwrap();
        assert_eq!(
            set.trailer().as_deref(),
            Some("Co-authored-by: Cyber Code <bot@cyber.dev>")
        );
        for bad in [
            json!({"git":{"co_author":"bot@cyber.dev"}}),
            json!({"git":{"co_author":"Bot <bot>"}}),
            json!({"git":{"co_author":"Bot <a@b>\nSigned-off-by: X <x@y>"}}),
            json!({"git":{"co_author":true}}),
            json!({"git":"yes"}),
        ] {
            assert!(GitSettings::from_config(&bad).is_err(), "{bad}");
        }
    }
}
