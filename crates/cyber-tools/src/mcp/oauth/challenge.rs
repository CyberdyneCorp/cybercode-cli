//! RFC 7235 / RFC 6750 `WWW-Authenticate` parsing for the `Bearer` challenge a remote MCP
//! server sends with 401/403. Only the parameters OAuth discovery and step-up use are kept,
//! and `Debug` shows which are present, never their values.
use reqwest::header::{HeaderMap, WWW_AUTHENTICATE};

/// Longer headers are ignored rather than parsed.
const HEADER_LIMIT: usize = 16 * 1024;

/// The `Bearer` challenge parameters MCP OAuth uses.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct AuthChallenge {
    /// RFC 9728 protected-resource metadata URL.
    pub resource_metadata: Option<String>,
    /// Space-separated scopes the server asks for (RFC 6750 `scope`).
    pub scope: Option<String>,
    /// RFC 6750 error code such as `invalid_token` or `insufficient_scope`.
    pub error: Option<String>,
}

impl std::fmt::Debug for AuthChallenge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthChallenge")
            .field("resource_metadata", &self.resource_metadata.is_some())
            .field("scope", &self.scope.is_some())
            .field("error", &self.error.is_some())
            .finish()
    }
}

impl AuthChallenge {
    /// The first `Bearer` challenge across every `WWW-Authenticate` header.
    pub fn from_headers(headers: &HeaderMap) -> Option<Self> {
        headers
            .get_all(WWW_AUTHENTICATE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .find_map(Self::parse)
    }

    /// The first `Bearer` challenge in one header value; `None` when there is none or the
    /// header is malformed (an unterminated quoted string) or oversized.
    pub fn parse(header: &str) -> Option<Self> {
        if header.len() > HEADER_LIMIT {
            return None;
        }
        let mut scanner = Scanner {
            rest: header.as_bytes(),
        };
        let mut bearer: Option<Self> = None;
        while let Some(item) = scanner.next_item()? {
            match item {
                Item::Scheme(_) if bearer.is_some() => break,
                Item::Scheme(scheme) if scheme.eq_ignore_ascii_case("bearer") => {
                    bearer = Some(Self::default());
                }
                Item::Param(name, value) => {
                    if let Some(challenge) = bearer.as_mut() {
                        challenge.set(&name, value);
                    }
                }
                Item::Scheme(_) | Item::Token68 => {}
            }
        }
        bearer
    }

    /// The first occurrence of each parameter wins; values with control characters are
    /// dropped.
    fn set(&mut self, name: &str, value: String) {
        if value.chars().any(char::is_control) {
            return;
        }
        let slot = match name.to_ascii_lowercase().as_str() {
            "resource_metadata" => &mut self.resource_metadata,
            "scope" => &mut self.scope,
            "error" => &mut self.error,
            _ => return,
        };
        slot.get_or_insert(value);
    }
}

enum Item {
    Scheme(String),
    Param(String, String),
    Token68,
}

struct Scanner<'a> {
    rest: &'a [u8],
}

fn tchar(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~/".contains(&byte)
}

impl Scanner<'_> {
    fn peek(&self) -> Option<u8> {
        self.rest.first().copied()
    }

    fn bump(&mut self) {
        self.rest = &self.rest[1..];
    }

    fn skip(&mut self, separators: &[u8]) {
        while self.peek().is_some_and(|byte| separators.contains(&byte)) {
            self.bump();
        }
    }

    fn token(&mut self) -> String {
        let length = self.rest.iter().take_while(|byte| tchar(**byte)).count();
        let (token, rest) = self.rest.split_at(length);
        self.rest = rest;
        String::from_utf8_lossy(token).into_owned()
    }

    /// `Ok(None)` at the end; `None` (outer) for a malformed header.
    fn next_item(&mut self) -> Option<Option<Item>> {
        loop {
            self.skip(b" \t,");
            if self.peek().is_none() {
                return Some(None);
            }
            let token = self.token();
            if token.is_empty() {
                // Skip a byte no grammar production accepts here.
                self.bump();
                continue;
            }
            self.skip(b" \t");
            if self.peek() != Some(b'=') {
                return Some(Some(Item::Scheme(token)));
            }
            self.bump();
            self.skip(b" \t");
            return self.value().map(|value| {
                Some(match value {
                    Some(value) => Item::Param(token, value),
                    None => Item::Token68,
                })
            });
        }
    }

    /// A parameter value; `Some(None)` for token68 padding (`abc==`), `None` when a quoted
    /// string is unterminated.
    fn value(&mut self) -> Option<Option<String>> {
        match self.peek() {
            Some(b'"') => self.quoted().map(Some),
            Some(b'=' | b',') | None => {
                self.skip(b"=");
                Some(None)
            }
            Some(_) => Some(Some(self.token())),
        }
    }

    fn quoted(&mut self) -> Option<String> {
        self.bump();
        let mut value = Vec::new();
        loop {
            let byte = self.peek()?;
            self.bump();
            match byte {
                b'"' => return Some(String::from_utf8_lossy(&value).into_owned()),
                b'\\' => {
                    value.push(self.peek()?);
                    self.bump();
                }
                _ => value.push(byte),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quoted_escaped_and_token_parameters() {
        let challenge = AuthChallenge::parse(
            r#"Bearer realm="a \"quoted\" realm", resource_metadata="https://mcp.example.test/.well-known/oauth-protected-resource/mcp", scope="files:read files:write", error=invalid_token"#,
        )
        .unwrap();
        assert_eq!(
            challenge.resource_metadata.as_deref(),
            Some("https://mcp.example.test/.well-known/oauth-protected-resource/mcp")
        );
        assert_eq!(challenge.scope.as_deref(), Some("files:read files:write"));
        assert_eq!(challenge.error.as_deref(), Some("invalid_token"));
        let escaped = AuthChallenge::parse(r#"Bearer scope="a\\b \"c\"""#).unwrap();
        assert_eq!(escaped.scope.as_deref(), Some(r#"a\b "c""#));
    }

    #[test]
    fn selects_the_bearer_challenge_among_several() {
        let header = r#"Basic realm="basic", Bearer error="insufficient_scope", scope="admin", Negotiate abc=="#;
        let challenge = AuthChallenge::parse(header).unwrap();
        assert_eq!(challenge.error.as_deref(), Some("insufficient_scope"));
        assert_eq!(challenge.scope.as_deref(), Some("admin"));
        assert_eq!(challenge.resource_metadata, None);
        // Parameters of a later challenge never leak into the Bearer one.
        let challenge =
            AuthChallenge::parse(r#"Bearer realm="x", DPoP error="use_dpop_nonce""#).unwrap();
        assert_eq!(challenge.error, None);
        // Case-insensitive scheme and names; token68 padding is skipped.
        let challenge = AuthChallenge::parse("Basic dXNlcjpw/YXNz==, bearer SCOPE=read").unwrap();
        assert_eq!(challenge.scope.as_deref(), Some("read"));
        let mut headers = HeaderMap::new();
        headers.append(WWW_AUTHENTICATE, "Basic realm=x".parse().unwrap());
        headers.append(
            WWW_AUTHENTICATE,
            "Bearer error=\"invalid_token\"".parse().unwrap(),
        );
        assert_eq!(
            AuthChallenge::from_headers(&headers)
                .unwrap()
                .error
                .as_deref(),
            Some("invalid_token")
        );
    }

    #[test]
    fn refuses_missing_malformed_and_oversized_challenges() {
        assert_eq!(AuthChallenge::parse("Basic realm=\"x\""), None);
        assert_eq!(AuthChallenge::parse(""), None);
        assert_eq!(AuthChallenge::parse("Bearer scope=\"unterminated"), None);
        let long = format!("Bearer scope=\"{}\"", "a".repeat(HEADER_LIMIT));
        assert_eq!(AuthChallenge::parse(&long), None);
        // A bare Bearer challenge carries no parameters.
        assert_eq!(
            AuthChallenge::parse("Bearer"),
            Some(AuthChallenge::default())
        );
        // First occurrence wins; control characters are dropped.
        let challenge =
            AuthChallenge::parse("Bearer scope=first, scope=second, error=\"a\u{7}b\"").unwrap();
        assert_eq!(challenge.scope.as_deref(), Some("first"));
        assert_eq!(challenge.error, None);
        let shown = format!("{challenge:?}");
        assert!(!shown.contains("first"), "{shown}");
    }
}
