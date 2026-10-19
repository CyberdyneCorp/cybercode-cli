//! The one-line pairing code `cyber peers invite` prints and `cyber peers add` reads
//! (`cross-session-messaging` → Peer pairing and grants):
//! `cyberpeer://<host>:<port>/?machine=<name>&fp=sha256:<hex>&t=cyp_…`, with `&tls=0` for
//! a plain listener on loopback (an SSH-forwarded port).

use std::fmt;

const SCHEME: &str = "cyberpeer://";

#[derive(Clone, PartialEq, Eq)]
pub struct PairingCode {
    /// `host:port` (an IPv6 host in brackets).
    pub authority: String,
    pub tls: bool,
    pub machine: String,
    pub fingerprint: String,
    pub token: String,
}

impl fmt::Debug for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairingCode")
            .field("authority", &self.authority)
            .field("tls", &self.tls)
            .field("machine", &self.machine)
            .field("fingerprint", &self.fingerprint)
            .field("token", &"<redacted>")
            .finish()
    }
}

impl fmt::Display for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{SCHEME}{}/?machine={}&fp={}&t={}",
            self.authority, self.machine, self.fingerprint, self.token
        )?;
        if !self.tls {
            f.write_str("&tls=0")?;
        }
        Ok(())
    }
}

impl PairingCode {
    /// The peer URL the code names: `https://<authority>`, or `http://` with `tls=0`.
    pub fn url(&self) -> String {
        let scheme = if self.tls { "https" } else { "http" };
        format!("{scheme}://{}", self.authority)
    }

    /// Read a code; errors never repeat the code, which holds a token.
    pub fn parse(text: &str) -> Result<Self, String> {
        let malformed = || {
            "not a pairing code: expected cyberpeer://<host>:<port>/?machine=…&fp=…&t=… as \
             printed by `cyber peers invite`"
                .to_string()
        };
        let rest = text.trim().strip_prefix(SCHEME).ok_or_else(malformed)?;
        let (authority, query) = rest.split_once("/?").ok_or_else(malformed)?;
        let valid_authority = !authority.is_empty()
            && !authority.contains(['/', '@', '?', '#'])
            && url::Url::parse(&format!("https://{authority}"))
                .is_ok_and(|url| url.port().is_some());
        if !valid_authority {
            return Err("the pairing code names no host and port".into());
        }
        let mut code = Self {
            authority: authority.to_string(),
            tls: true,
            machine: String::new(),
            fingerprint: String::new(),
            token: String::new(),
        };
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=').ok_or_else(malformed)?;
            match key {
                "machine" => code.machine = value.to_string(),
                "fp" => code.fingerprint = value.to_string(),
                "t" => code.token = value.to_string(),
                "tls" if value == "0" => code.tls = false,
                _ => return Err(format!("the pairing code has an unknown field `{key}`")),
            }
        }
        if code.machine.is_empty() || code.fingerprint.is_empty() || code.token.is_empty() {
            return Err("the pairing code lacks machine, fp or t; copy the whole line".into());
        }
        Ok(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FP: &str = "sha256:9f2c000000000000000000000000000000000000000000000000000000000001";

    fn code(authority: &str, tls: bool) -> PairingCode {
        PairingCode {
            authority: authority.into(),
            tls,
            machine: "desk".into(),
            fingerprint: FP.into(),
            token: "cyp_pgr01HZX_secret".into(),
        }
    }

    #[test]
    fn codes_round_trip_and_name_their_url() {
        for (authority, tls, url) in [
            ("desk.local:4748", true, "https://desk.local:4748"),
            ("127.0.0.1:14748", false, "http://127.0.0.1:14748"),
            ("[fd00::5]:4748", true, "https://[fd00::5]:4748"),
        ] {
            let code = code(authority, tls);
            let text = code.to_string();
            assert!(text.starts_with("cyberpeer://"), "{text}");
            assert_eq!(PairingCode::parse(&format!(" {text}\n")).unwrap(), code);
            assert_eq!(code.url(), url);
        }
        assert_eq!(
            code("desk.local:4748", true).to_string(),
            format!("cyberpeer://desk.local:4748/?machine=desk&fp={FP}&t=cyp_pgr01HZX_secret")
        );
        assert!(!format!("{:?}", code("a:1", true)).contains("secret"));
    }

    #[test]
    fn malformed_codes_are_refused_without_echoing_them() {
        for text in [
            "https://desk:4748/?machine=desk&fp=x&t=cyp_secret",
            "cyberpeer://desk/?machine=desk&fp=x&t=cyp_secret",
            "cyberpeer://user@desk:1/?machine=desk&fp=x&t=cyp_secret",
            "cyberpeer://desk:4748/?machine=desk&fp=x",
            "cyberpeer://desk:4748/?machine=desk&fp=x&t=cyp_secret&extra=1",
            "cyberpeer://desk:4748",
        ] {
            let error = PairingCode::parse(text).unwrap_err();
            assert!(!error.contains("cyp_secret"), "{error}");
        }
    }
}
