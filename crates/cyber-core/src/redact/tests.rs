use super::*;

fn redact(text: &str) -> String {
    Redactor::builtin().redact(text).text.into_owned()
}

/// `tool-registry` → Tool output redaction: each provider format is replaced by its own
/// kind, and the text around it is kept.
#[test]
fn every_provider_format_is_redacted_with_its_kind() {
    let cases = [
        ("AKIAIOSFODNN7EXAMPL3", "aws_access_key"),
        ("ASIAABCDEFGHIJKLMNOP", "aws_access_key"),
        ("ghp_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789", "github_token"),
        ("gho_abcdefgh12345678", "github_token"),
        (
            "github_pat_11ABCDEFG0123456789_abcdefghijklmnop",
            "github_token",
        ),
        ("sk-proj-abcDEF123456ghiJKL789", "openai_key"),
        ("sk-abcdefghijklmnop1234", "openai_key"),
        ("sk-ant-api03-abcdefghijklmnopQRSTUV", "anthropic_key"),
        ("xoxb-1234567890-abcdefghijkl", "slack_token"),
        ("xoxp-1234567890-0987654321-abcdef", "slack_token"),
        ("sk_live_abcdefghijklmnop1234", "stripe_key"),
        ("rk_live_abcdefghijklmnop1234", "stripe_key"),
        ("AIzaSyA-abcdefghijklmnopqrstuvwxyz01234", "google_api_key"),
        ("glpat-abcdefghij0123456789", "gitlab_token"),
        ("npm_abcdefghijklmnopqrstuvwxyz0123456789", "npm_token"),
        ("cyp_pgr01HZX0ABCDEF_Qm9vbGVhbi1zZWNyZXQ", "peer_grant"),
        ("tid=0123456789abcdef;exp=4000000000;sku=x", "copilot_token"),
        (
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U",
            "jwt",
        ),
    ];
    for (secret, kind) in cases {
        let text = format!("before {secret} after");
        let redacted = redact(&text);
        assert_eq!(
            redacted,
            format!("before [REDACTED:{kind}] after"),
            "{secret}"
        );
    }
}

/// Output is often JSON: a token right after a `\n` escape is still found, and the
/// escape is kept.
#[test]
fn tokens_after_json_escapes_are_found() {
    let json = r#"{"log":"deploying\nghp_abcdefgh12345678\tAKIAIOSFODNN7EXAMPL3\nAuthorization: Bearer abc.DEF-123_456"}"#;
    assert_eq!(
        redact(json),
        r#"{"log":"deploying\n[REDACTED:github_token]\t[REDACTED:aws_access_key]\nAuthorization: Bearer [REDACTED:auth_header]"}"#
    );
    assert_eq!(
        redact(r"path\nsk_live_is_not_here"),
        r"path\nsk_live_is_not_here"
    );
}

#[test]
fn pem_private_keys_are_redacted_through_their_end_line() {
    let key = "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA0Z3VS5JJcds3xfn/ygWyF8PbnGy\nabcdefghijklmnop==\n-----END RSA PRIVATE KEY-----";
    let text = format!("key:\n{key}\nnext line");
    // The block keeps its line breaks, so line numbers do not move.
    assert_eq!(
        redact(&text),
        "key:\n[REDACTED:private_key]\n\n\n\nnext line"
    );
    for header in [
        "OPENSSH PRIVATE KEY",
        "PRIVATE KEY",
        "ENCRYPTED PRIVATE KEY",
        "PGP PRIVATE KEY BLOCK",
    ] {
        let key = format!("-----BEGIN {header}-----\nAAAA\n-----END {header}-----");
        assert_eq!(redact(&key), "[REDACTED:private_key]\n\n", "{header}");
    }
    // A key cut before its END line is redacted through its base64 body only.
    let cut = "-----BEGIN PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nMIIEowIBAAKCAQEA0Z3VS5JJ\nabcdEFGH==\nthen ordinary text here";
    assert_eq!(
        redact(cut),
        "[REDACTED:private_key]\n\n\n\nthen ordinary text here"
    );
    assert_eq!(
        redact("-----BEGIN PUBLIC KEY-----\nAAAA"),
        "-----BEGIN PUBLIC KEY-----\nAAAA"
    );
}

/// `NAME=value` and `"name": "value"` with a secret-looking name hide the value only.
#[test]
fn secret_assignments_hide_the_value_and_keep_the_name() {
    let cases = [
        (
            "DB_PASSWORD=Hunter2Hunter2",
            "DB_PASSWORD=[REDACTED:assignment]",
        ),
        (
            "export API_TOKEN=abc123def456",
            "export API_TOKEN=[REDACTED:assignment]",
        ),
        (
            r#""apiKey": "Zx81kq0Pmm2w""#,
            r#""apiKey": "[REDACTED:assignment]""#,
        ),
        (
            "client_secret: 'abcDEF123456'",
            "client_secret: '[REDACTED:assignment]'",
        ),
        (
            "aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            "aws_secret_access_key = [REDACTED:assignment]",
        ),
        (
            "--password=Sup3rSecret, next",
            "--password=[REDACTED:assignment], next",
        ),
        (
            "x?access_token=abc123def456&y=1",
            "x?access_token=[REDACTED:assignment]&y=1",
        ),
        (
            "PRIVATE_KEY = \"k3y-Material-99\"",
            "PRIVATE_KEY = \"[REDACTED:assignment]\"",
        ),
        (
            "//registry.npmjs.org/:_authToken=0a1b2c3d4e5f6a7b8c9d",
            "//registry.npmjs.org/:_authToken=[REDACTED:assignment]",
        ),
        (
            "Authorization: Bearer abc.DEF-123_456",
            "Authorization: Bearer [REDACTED:auth_header]",
        ),
        (
            "postgres://app:pa55word@db:5432/app",
            "postgres://app:[REDACTED:url_password]@db:5432/app",
        ),
        // Inside a quoted string or a JSON array, the closing quote ends the value.
        (
            r#"{"Env": ["API_KEY=abc123def456", "MODE=1"]}"#,
            r#"{"Env": ["API_KEY=[REDACTED:assignment]", "MODE=1"]}"#,
        ),
        (
            r#"{"log": "API_KEY=abc123def456\nnext"}"#,
            r#"{"log": "API_KEY=[REDACTED:assignment]\nnext"}"#,
        ),
    ];
    for (text, expected) in cases {
        assert_eq!(redact(text), expected, "{text}");
    }
}

/// Code, placeholders, short values, numbers and identifiers are not secrets.
#[test]
fn assignment_false_positives_are_kept() {
    for text in [
        "token = self.next_token()",
        "api_key = os.environ[\"API_KEY\"]",
        "fn lex(token: TokenKind) -> Token",
        "pub token: Option<String>,",
        "password: process.env.DB_PASSWORD,",
        "let kind = Token::Ident;\ntoken: TokenKind::Ident,",
        "max_tokens=100000000",
        "\"max_tokens\": 4096",
        "password: ********",
        "\"token\": \"string\"",
        "secret = \"SECRET_NAME_ENV\"",
        "\"x-api-key\": \"YOUR_API_KEY\"",
        "GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}",
        "api_key = \"<your key here>\"",
        "token=abc",
        "export GITHUB_TOKEN=$(gh auth token)",
        "tokenizer = AutoTokenizer.from_pretrained(name)",
        "\"js-tokens\": \"^4.0.0\",",
        "secret_key_base: true",
        "password = \"correct horse battery staple\"",
    ] {
        assert_eq!(redact(text), text, "{text}");
    }
}

/// Hashes, IDs and encoded media are random but are not credentials.
#[test]
fn hashes_ids_and_media_are_never_redacted() {
    let sha = "commit 9fceb02d0ae598e95dc970b74767f19372d61af8\nAuthor: A <a@b.c>";
    let uuid = "id 550e8400-e29b-41d4-a716-446655440000 ok";
    let image = format!("data:image/png;base64,{}", "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==".repeat(20));
    let lock = concat!(
        "\"integrity\": \"sha512-Ivp/4zEyzDLHGYaGJzJv/ZTvyqzt7HZzzwR6yRfr0Mwx5/QJ1MzM7gNq8F6rWJm8x6Uq3W7y0z1A3B5C7D9E1F3==\",\n",
        "checksum = \"a3f2c5d8e9b1a0f4c6d7e8f9a0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9\"\n",
        "golang.org/x/net v0.17.0 h1:pVaXccu2ozPjCXewfr1S7xza/zcXTity9cCdXQYSjIM=\n",
        "\"resolved\": \"https://registry.npmjs.org/js-tokens/-/js-tokens-4.0.0.tgz\"\n",
    );
    for text in [sha, uuid, &image, lock] {
        let redacted = Redactor::builtin().redact(text);
        assert!(
            redacted.redactions.is_empty(),
            "{text} -> {}",
            redacted.text
        );
        assert!(matches!(redacted.text, Cow::Borrowed(_)));
    }
}

/// `tool-registry` → Tool output redaction, exact values: a configured or environment
/// secret is hidden wherever it appears, whatever its shape.
#[test]
fn exact_known_values_are_redacted_wherever_they_appear() {
    let env = [(
        "DEPLOY_TOKEN".to_string(),
        "plainwordsecretvalue".to_string(),
    )];
    let redactor = Redactor::new(&RedactSettings::default(), env_secrets(env));
    let redacted =
        redactor.redact("echo plainwordsecretvalue | cut -c1-4 ; x=plainwordsecretvalue");
    assert_eq!(
        redacted.text,
        "echo [REDACTED:known_secret] | cut -c1-4 ; x=[REDACTED:known_secret]"
    );
    assert_eq!(redacted.redactions.count, 2);
    assert_eq!(redacted.redactions.kinds, ["known_secret"]);
    // Shorter values are never matched.
    let short = Redactor::new(&RedactSettings::default(), ["short".to_string()]);
    assert_eq!(short.redact("a short word").text, "a short word");
    // The value is never printed by Debug.
    assert!(!format!("{redactor:?}").contains("plainwordsecretvalue"));
}

#[test]
fn custom_patterns_allow_entries_and_the_switch() {
    let settings = RedactSettings {
        enabled: true,
        patterns: vec!["corp_[a-z0-9]{12}".into()],
        allow: vec!["AKIAIOSFODNN7EXAMPLE".into(), "/sk-test-[a-z0-9]+/".into()],
    };
    let redactor = Redactor::new(&settings, []);
    let text = "corp_abcdef012345 AKIAIOSFODNN7EXAMPLE sk-test-abcdefgh123 AKIAIOSFODNN7EXAMPL3";
    assert_eq!(
        redactor.redact(text).text,
        "[REDACTED:custom] AKIAIOSFODNN7EXAMPLE sk-test-abcdefgh123 [REDACTED:aws_access_key]"
    );
    let off = Redactor::new(
        &RedactSettings {
            enabled: false,
            ..settings
        },
        [],
    );
    assert_eq!(off.redact(text).text, text);
}

/// Overlapping detections redact once, with the most specific kind; a second pass
/// finds nothing new, and existing markers are never re-matched.
#[test]
fn overlaps_take_the_specific_kind_and_redaction_is_idempotent() {
    let text = "GITHUB_TOKEN=ghp_abcdefgh12345678\nAuthorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N";
    let first = Redactor::builtin().redact(text);
    assert_eq!(
        first.text,
        "GITHUB_TOKEN=[REDACTED:github_token]\nAuthorization: Bearer [REDACTED:jwt]"
    );
    assert_eq!(first.redactions.count, 2);
    assert_eq!(first.redactions.kinds, ["github_token", "jwt"]);
    let second = Redactor::builtin().redact(&first.text);
    assert_eq!(second.text, first.text);
    assert!(second.redactions.is_empty());
    let custom = Redactor::new(
        &RedactSettings {
            patterns: vec!["REDACTED".into()],
            ..RedactSettings::default()
        },
        [],
    );
    assert_eq!(custom.redact(&first.text).text, first.text);
}

#[test]
fn introduced_markers_are_found_and_existing_ones_kept() {
    assert_eq!(
        introduced_marker("", "a [REDACTED:assignment] b"),
        Some("[REDACTED:assignment]".into())
    );
    let doc = "markers look like [REDACTED:github_token]";
    assert_eq!(introduced_marker(doc, &format!("{doc}\nmore")), None);
    assert_eq!(
        introduced_marker(doc, &format!("{doc}\n[REDACTED:github_token]")),
        Some("[REDACTED:github_token]".into())
    );
    assert_eq!(
        introduced_marker("x", "[REDACTED] and [REDACTED:Bad Kind]"),
        None
    );
    assert_eq!(markers("[REDACTED:a] [REDACTED:b_2]").count(), 2);
}

/// The legacy callers keep their own marker and their permissive assignment filter.
#[test]
fn legacy_redaction_keeps_the_callers_marker() {
    assert_eq!(
        redact_credentials_with("token=abc and sk-abcdefghijkl", "***"),
        "token=*** and ***"
    );
    assert_eq!(redact_credentials_with("sk-abcdefghijkl", "$0"), "$0");
    assert!(contains_credential("password: hunter2"));
    assert!(contains_credential("description: 'OPENAI_API_KEY=sk-...'"));
    assert!(!contains_credential("plain words only"));
}

#[test]
fn added_markers_count_only_new_ones() {
    let original = "doc [REDACTED:jwt]\nAPI_KEY=abc123def456";
    let redacted = Redactor::builtin().redact(original).text.into_owned();
    let added = added_markers(original, &redacted);
    assert_eq!(
        (added.count, added.kinds),
        (1, vec!["assignment".to_string()])
    );
}

#[test]
fn redactions_merge_counts_and_kinds() {
    let mut total = Redactions::default();
    let found = Redactor::builtin().redact("ghp_abcdefgh12345678 sk-abcdefghijkl");
    total.merge(&found.redactions);
    total.merge(
        &Redactor::builtin()
            .redact("glpat-abcdefghij0123456789")
            .redactions,
    );
    assert_eq!(total.count, 3);
    assert_eq!(total.kinds, ["github_token", "gitlab_token", "openai_key"]);
}

/// Redacting a 1 MiB result stays well under 10 ms on a dev machine (the regex crates
/// are optimized in dev builds); the bound here leaves room for a loaded CI runner.
#[test]
fn one_mebibyte_is_redacted_quickly() {
    // Log lines with IDs and hashes, and JSON dense with `token` keys.
    let line = concat!(
        "2026-10-30T12:00:00Z INFO request id=550e8400-e29b-41d4-a716-446655440000 path=/api/v1/items status=200 sha=9fceb02d0ae598e95dc970b74767f19372d61af8\n",
        "{\"usage\": {\"input_tokens\": 1234, \"output_tokens\": 56}, \"token_type\": \"bearer\", \"secret_ref\": \"vault.kv.app\"}\n",
    );
    let mut text = line.repeat((1 << 20) / line.len());
    text.push_str("DB_PASSWORD=Hunter2Hunter2\nghp_abcdefgh12345678\n");
    let known = (0..50).map(|i| format!("known-secret-value-{i:04}"));
    let redactor = Redactor::new(&RedactSettings::default(), known);
    let best = (0..5)
        .map(|_| {
            let started = std::time::Instant::now();
            let redacted = redactor.redact(&text);
            assert_eq!(redacted.redactions.count, 2);
            started.elapsed()
        })
        .min()
        .unwrap();
    let limit = if cfg!(debug_assertions) { 50 } else { 10 };
    assert!(best.as_millis() < limit, "{best:?}");
}
