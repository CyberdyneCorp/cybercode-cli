use serde::{Deserialize, Serialize};
use std::fmt;
use zeroize::Zeroizing;

/// A credential. Debug and Display never reveal it; only protocol/storage adapters
/// should call `expose`. Serialization is exclusively for protected storage.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(#[serde(with = "secret_string")] Zeroizing<String>);

impl Secret {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

mod secret_string {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        s: &Zeroizing<String>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        s.as_str().serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> Result<Zeroizing<String>, D::Error> {
        String::deserialize(d).map(Zeroizing::new)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Organization {
    pub id: String,
    #[serde(default)]
    pub short_name: Option<String>,
    #[serde(default)]
    pub github_login: Option<String>,
}

/// Display claims from verified tokens. Display/selection alone grants no hub rights:
/// the hub must obtain a current `IdentityLease` from live introspection.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    pub issuer: String,
    pub sub: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub is_admin: bool,
    pub org: Option<Organization>,
    pub orgs: Vec<Organization>,
    pub entitlements: Vec<String>,
    pub roles: Vec<String>,
    pub scopes: Vec<String>,
    pub expires_at: u64,
    pub auth_time: Option<u64>,
    pub amr: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TokenSet {
    pub(crate) access_token: Secret,
    pub(crate) refresh_token: Option<Secret>,
    pub(crate) id_token: Secret,
    pub(crate) client_id: String,
    pub(crate) nonce: Secret,
    pub(crate) account: Account,
}

impl TokenSet {
    pub fn account(&self) -> &Account {
        &self.account
    }
    pub fn client_id(&self) -> &str {
        &self.client_id
    }
    /// Only trusted network adapters may use this; never include it in API results.
    pub fn access_token(&self) -> &Secret {
        &self.access_token
    }
}

#[derive(Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum Audience {
    One(String),
    Many(Vec<String>),
}
impl Audience {
    pub fn contains(&self, expected: &str) -> bool {
        match self {
            Self::One(s) => s == expected,
            Self::Many(v) => v.iter().any(|s| s == expected),
        }
    }
    pub fn multiple(&self) -> bool {
        matches!(self, Self::Many(v) if v.len() > 1)
    }
}

#[derive(Clone, Deserialize)]
pub(crate) struct Claims {
    pub iss: String,
    pub sub: String,
    pub aud: Audience,
    pub exp: u64,
    pub iat: u64,
    pub nonce: Option<String>,
    pub azp: Option<String>,
    pub email: Option<String>,
    pub name: Option<String>,
    #[serde(default)]
    pub is_admin: bool,
    pub org: Option<Organization>,
    #[serde(default)]
    pub orgs: Vec<Organization>,
    #[serde(default)]
    pub entitlements: Vec<String>,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub scope: String,
    pub auth_time: Option<u64>,
    #[serde(default)]
    pub amr: Vec<String>,
}

impl Claims {
    pub fn account(&self, client: &str) -> Account {
        Account {
            issuer: self.iss.clone(),
            sub: self.sub.clone(),
            email: self.email.clone(),
            name: self.name.clone(),
            is_admin: self.is_admin,
            org: self.org.clone(),
            orgs: self.orgs.clone(),
            entitlements: self.entitlements.clone(),
            roles: self
                .roles
                .iter()
                .filter(|s| s.starts_with(&format!("{client}:")))
                .cloned()
                .collect(),
            scopes: self.scope.split_whitespace().map(str::to_owned).collect(),
            expires_at: self.exp,
            auth_time: self.auth_time,
            amr: self.amr.clone(),
        }
    }
}

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
