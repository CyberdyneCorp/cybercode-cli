//! Pure background-update decisions (`installation-upgrade` → Background update policy,
//! Minimum supported version): whether the server checks at all, how big a release is,
//! and whether it is announced or installed silently. The server's task does the IO.

use cyber_core::config::AutoUpdate;
use cyber_core::env::EnvSource;
use cyber_core::version::Channel;
use serde::Serialize;

use crate::method::InstallMethod;

/// Everything that can turn the background check off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckInputs {
    /// `autoupdate` from the user and project layers.
    pub autoupdate: AutoUpdate,
    /// `policy.autoupdate` from organization policy (managed configuration); it wins.
    pub managed: Option<AutoUpdate>,
    pub channel: Channel,
    /// `CYBER_DISABLE_AUTOUPDATE`.
    pub disabled: bool,
    /// `CYBER_OFFLINE`.
    pub offline: bool,
    /// `CI` is true.
    pub ci: bool,
}

impl CheckInputs {
    /// The inputs with the three environment switches read from `env`.
    pub fn new(
        autoupdate: AutoUpdate,
        managed: Option<AutoUpdate>,
        channel: Channel,
        env: &dyn EnvSource,
    ) -> Self {
        Self {
            autoupdate,
            managed,
            channel,
            disabled: env.flag("CYBER_DISABLE_AUTOUPDATE"),
            offline: env.flag("CYBER_OFFLINE"),
            ci: env.flag("CI"),
        }
    }

    /// The setting in effect: organization policy over user configuration.
    pub fn effective(&self) -> AutoUpdate {
        self.managed.unwrap_or(self.autoupdate)
    }
}

/// Whether the background check runs at all. A `dev` build never checks.
pub fn should_check(inputs: &CheckInputs) -> bool {
    inputs.effective() != AutoUpdate::Off
        && !inputs.disabled
        && !inputs.offline
        && !inputs.ci
        && inputs.channel != Channel::Dev
}

/// How far a newer release is from the running one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Bump {
    Major,
    Minor,
    Patch,
}

impl Bump {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Major => "major",
            Self::Minor => "minor",
            Self::Patch => "patch",
        }
    }
}

/// The bump from `current` to `latest`, or `None` when `latest` is not newer or either
/// is not semver. A pre-release difference within one version counts as a patch.
pub fn bump(current: &str, latest: &str) -> Option<Bump> {
    let current = semver::Version::parse(current).ok()?;
    let latest = semver::Version::parse(latest).ok()?;
    if latest <= current {
        return None;
    }
    Some(if latest.major != current.major {
        Bump::Major
    } else if latest.minor != current.minor {
        Bump::Minor
    } else {
        Bump::Patch
    })
}

/// What one check does with its answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// The running version is the latest.
    Current,
    /// Publish `installation.update_available.1`.
    Notify(Bump),
    /// Upgrade without asking, then publish `installation.updated.1`; a failure falls
    /// back to [`Decision::Notify`].
    Silent,
}

/// Only a patch bump with `autoupdate = true` on a method cyber upgrades unattended
/// (`script`, `brew`, `npm`) is installed silently; everything else is announced.
pub fn decide(autoupdate: AutoUpdate, method: InstallMethod, bump: Option<Bump>) -> Decision {
    let Some(bump) = bump else {
        return Decision::Current;
    };
    let unattended = matches!(
        method,
        InstallMethod::Script | InstallMethod::Brew | InstallMethod::Npm
    );
    if bump == Bump::Patch && autoupdate == AutoUpdate::On && unattended {
        Decision::Silent
    } else {
        Decision::Notify(bump)
    }
}

/// Whether `current` is older than the manifest's `min_supported`.
pub fn unsupported(current: &str, min_supported: Option<&str>) -> bool {
    let parse = |v: &str| semver::Version::parse(v).ok();
    match (parse(current), min_supported.and_then(parse)) {
        (Some(current), Some(minimum)) => current < minimum,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn inputs() -> CheckInputs {
        CheckInputs {
            autoupdate: AutoUpdate::Notify,
            managed: None,
            channel: Channel::Stable,
            disabled: false,
            offline: false,
            ci: false,
        }
    }

    #[test]
    fn should_check_table() {
        let base = inputs();
        let cases = [
            ("notify by default", base, true),
            (
                "autoupdate true",
                CheckInputs {
                    autoupdate: AutoUpdate::On,
                    ..base
                },
                true,
            ),
            (
                "autoupdate false",
                CheckInputs {
                    autoupdate: AutoUpdate::Off,
                    ..base
                },
                false,
            ),
            (
                "CYBER_DISABLE_AUTOUPDATE",
                CheckInputs {
                    disabled: true,
                    ..base
                },
                false,
            ),
            (
                "CYBER_OFFLINE",
                CheckInputs {
                    offline: true,
                    ..base
                },
                false,
            ),
            ("CI", CheckInputs { ci: true, ..base }, false),
            (
                "dev channel",
                CheckInputs {
                    channel: Channel::Dev,
                    ..base
                },
                false,
            ),
            (
                "beta channel",
                CheckInputs {
                    channel: Channel::Beta,
                    ..base
                },
                true,
            ),
            (
                "managed false over user true",
                CheckInputs {
                    autoupdate: AutoUpdate::On,
                    managed: Some(AutoUpdate::Off),
                    ..base
                },
                false,
            ),
            (
                "managed notify over user false",
                CheckInputs {
                    autoupdate: AutoUpdate::Off,
                    managed: Some(AutoUpdate::Notify),
                    ..base
                },
                true,
            ),
        ];
        for (name, inputs, expected) in cases {
            assert_eq!(should_check(&inputs), expected, "{name}");
        }
    }

    #[test]
    fn environment_switches_are_flags() {
        let env: HashMap<String, String> = [
            ("CI".to_string(), "true".to_string()),
            ("CYBER_OFFLINE".to_string(), "1".to_string()),
            ("CYBER_DISABLE_AUTOUPDATE".to_string(), "no".to_string()),
        ]
        .into();
        let read = CheckInputs::new(AutoUpdate::Notify, None, Channel::Stable, &env);
        assert!(read.ci && read.offline && !read.disabled);
        let none = CheckInputs::new(AutoUpdate::Notify, None, Channel::Stable, &HashMap::new());
        assert_eq!(none, inputs());
    }

    #[test]
    fn bump_table() {
        let cases = [
            ("1.4.0", "2.0.0", Some(Bump::Major)),
            ("1.4.0", "1.5.0", Some(Bump::Minor)),
            ("1.4.0", "1.4.1", Some(Bump::Patch)),
            ("1.4.1-beta.1", "1.4.1", Some(Bump::Patch)),
            ("1.4.0", "1.4.0", None),
            ("1.5.0", "1.4.9", None),
            ("1.4.0", "latest", None),
            ("dev", "1.4.0", None),
        ];
        for (current, latest, expected) in cases {
            assert_eq!(bump(current, latest), expected, "{current} → {latest}");
        }
    }

    #[test]
    fn decide_table() {
        use AutoUpdate::{Notify as N, On};
        use InstallMethod::{Brew, Cargo, Nix, Npm, Pnpm, Script, Unknown};
        let cases = [
            (N, Script, None, Decision::Current),
            (On, Script, None, Decision::Current),
            (N, Script, Some(Bump::Patch), Decision::Notify(Bump::Patch)),
            (On, Script, Some(Bump::Patch), Decision::Silent),
            (On, Brew, Some(Bump::Patch), Decision::Silent),
            (On, Npm, Some(Bump::Patch), Decision::Silent),
            (On, Script, Some(Bump::Minor), Decision::Notify(Bump::Minor)),
            (On, Script, Some(Bump::Major), Decision::Notify(Bump::Major)),
            (On, Pnpm, Some(Bump::Patch), Decision::Notify(Bump::Patch)),
            (On, Cargo, Some(Bump::Patch), Decision::Notify(Bump::Patch)),
            (On, Nix, Some(Bump::Patch), Decision::Notify(Bump::Patch)),
            (
                On,
                Unknown,
                Some(Bump::Patch),
                Decision::Notify(Bump::Patch),
            ),
        ];
        for (autoupdate, method, bump, expected) in cases {
            assert_eq!(
                decide(autoupdate, method, bump),
                expected,
                "{autoupdate:?} {method:?} {bump:?}"
            );
        }
    }

    #[test]
    fn unsupported_compares_with_the_minimum() {
        assert!(unsupported("1.0.0", Some("1.2.0")));
        assert!(!unsupported("1.2.0", Some("1.2.0")));
        assert!(!unsupported("1.3.0", Some("1.2.0")));
        assert!(!unsupported("1.0.0", None));
        assert!(!unsupported("1.0.0", Some("soon")));
        assert_eq!(
            cyber_core::version::unsupported_warning("1.0.0", "1.2.0"),
            "warning: cyber 1.0.0 is no longer supported (minimum 1.2.0); run \"cyber upgrade\""
        );
    }
}
