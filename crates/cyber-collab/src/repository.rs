use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Repository {
    pub host: String,
    pub path: String,
    pub endpoint: Option<Endpoint>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Endpoint {
    pub protocol: String,
    pub port: u16,
}
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[error("RepositoryMismatchError: expected an unambiguous HTTPS or SSH Git repository URL")]
pub struct InvalidRepository;

impl Repository {
    /// Canonical identity only, never proof of membership or Git permissions. User
    /// information is removed and nondefault transport/ports remain distinct.
    pub fn parse(raw: &str) -> Result<Self, InvalidRepository> {
        if raw.len() > 4096 || raw.chars().any(char::is_control) {
            return Err(InvalidRepository);
        }
        let bytes = raw.as_bytes();
        if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\')
        {
            return Err(InvalidRepository);
        }
        let url = if raw.contains("://") {
            Url::parse(raw).map_err(|_| InvalidRepository)?
        } else {
            let (authority, path) = raw.rsplit_once(':').ok_or(InvalidRepository)?;
            if authority.contains('/')
                || path.contains(':')
                || path.contains('\\')
                || path.chars().any(char::is_whitespace)
            {
                return Err(InvalidRepository);
            }
            Url::parse(&format!(
                "ssh://{authority}/{}",
                path.trim_start_matches('/')
            ))
            .map_err(|_| InvalidRepository)?
        };
        let default = match url.scheme() {
            "https" => 443,
            "ssh" => 22,
            _ => return Err(InvalidRepository),
        };
        if url.query().is_some() || url.fragment().is_some() {
            return Err(InvalidRepository);
        }
        let host = url
            .host_str()
            .ok_or(InvalidRepository)?
            .to_ascii_lowercase();
        let mut path = url.path().trim_matches('/').to_owned();
        if host == "github.com" {
            path.make_ascii_lowercase();
        }
        if let Some(stripped) = path.strip_suffix(".git") {
            path = stripped.to_owned();
        }
        let encoded = path.to_ascii_lowercase();
        if path.is_empty()
            || path.contains('\\')
            || path
                .split('/')
                .any(|s| s.is_empty() || s == "." || s == "..")
            || ["%2f", "%5c", "%00"].iter().any(|s| encoded.contains(s))
        {
            return Err(InvalidRepository);
        }
        if host == "github.com" && path.split('/').count() != 2 {
            return Err(InvalidRepository);
        }
        let endpoint = url.port().filter(|p| *p != default).map(|port| Endpoint {
            protocol: url.scheme().to_owned(),
            port,
        });
        Ok(Self {
            host,
            path,
            endpoint,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn github_transports_and_credentials_share_one_identity() {
        let expected = Repository::parse("git@github.com:CyberdyneCorp/cybercode-cli.git").unwrap();
        for raw in [
            "https://username:private-password@github.com/CyberdyneCorp/cybercode-cli.git",
            "ssh://git@github.com:22/CyberdyneCorp/cybercode-cli.git",
            "https://github.com:443/cyberdynecorp/CYBERCODE-CLI",
        ] {
            assert_eq!(Repository::parse(raw).unwrap(), expected);
        }
        assert!(
            !serde_json::to_string(&expected)
                .unwrap()
                .contains("private-password")
        );
    }
    #[test]
    fn forks_ports_and_other_hosts_remain_distinct() {
        let repo = Repository::parse("https://github.com/acme/project").unwrap();
        for raw in [
            "https://github.com/fork/project",
            "ssh://git@github.com:2222/acme/project.git",
            "https://git.example/acme/project",
        ] {
            assert_ne!(Repository::parse(raw).unwrap(), repo);
        }
        assert_ne!(
            Repository::parse("https://git.example/A/project").unwrap(),
            Repository::parse("https://git.example/a/project").unwrap()
        );
    }
    #[test]
    fn local_and_ambiguous_remotes_are_rejected() {
        for raw in [
            "../project",
            "/tmp/project",
            "C:\\repo",
            "C:/repo",
            "file:///tmp/project",
            "https://github.com/one",
            "https://github.com/a/b?token=secret",
            "https://github.com/a/b#fragment",
            "https://github.com/a%2Fb/project",
        ] {
            assert_eq!(Repository::parse(raw), Err(InvalidRepository), "{raw}");
        }
    }
}
