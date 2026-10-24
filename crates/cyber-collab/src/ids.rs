use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

#[derive(Debug, thiserror::Error)]
#[error("InvalidCollaborationIdError: invalid resource identifier")]
pub struct InvalidId;
macro_rules! id {
    ($name:ident,$prefix:literal) => {
        #[derive(
            Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
        )]
        #[serde(try_from = "String", into = "String")]
        #[schemars(with = "String")]
        pub struct $name(String);
        impl $name {
            pub fn new() -> Self {
                Self(format!("{}_{}", $prefix, ulid::Ulid::new()))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
        impl FromStr for $name {
            type Err = InvalidId;
            fn from_str(raw: &str) -> Result<Self, InvalidId> {
                let suffix = raw.strip_prefix(concat!($prefix, "_")).ok_or(InvalidId)?;
                let parsed = ulid::Ulid::from_string(suffix).map_err(|_| InvalidId)?;
                Ok(Self(format!("{}_{}", $prefix, parsed)))
            }
        }
        impl TryFrom<String> for $name {
            type Error = InvalidId;
            fn try_from(raw: String) -> Result<Self, InvalidId> {
                raw.parse()
            }
        }
        impl From<$name> for String {
            fn from(id: $name) -> Self {
                id.0
            }
        }
    };
}
id!(HubId, "chub");
id!(ProjectId, "cprj");
id!(MemberId, "cmem");
id!(RuntimeId, "crt");
id!(AgentId, "cagt");
id!(TaskId, "ctask");
id!(MessageId, "cmsg");
id!(InvitationId, "cinv");
id!(ApplicationId, "capp");
id!(ChallengeId, "cchl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resource_namespaces_cannot_be_substituted() {
        let project = ProjectId::new();
        assert!(project.as_str().parse::<MemberId>().is_err());
        assert_eq!(project.as_str().parse::<ProjectId>().unwrap(), project);
        assert!(serde_json::from_str::<ProjectId>("\"../project\"").is_err());
    }
}
