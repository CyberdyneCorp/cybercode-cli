//! Throwaway signing material for the debug-only release trust hook (feature
//! `test-support`, tests only): a CA standing in for Fulcio, a log key standing in for
//! Rekor, and cosign-shaped bundles whose leaf certificate names a chosen identity and
//! issuer. `Trust` checks them with the relaxed hook policy (no inclusion proof, no SCT);
//! the chain, identity, issuer, signed entry timestamp and signature are all real.

use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use rcgen::{
    BasicConstraints, CertificateParams, CustomExtension, ExtendedKeyUsagePurpose, IsCa, Issuer,
    KeyPair, SanType,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use sigstore_verify::crypto::KeyPair as SigningKey;

/// Fulcio's OIDC issuer (v2) extension: a DER `UTF8String`.
const ISSUER_V2_OID: &[u64] = &[1, 3, 6, 1, 4, 1, 57264, 1, 8];

pub struct TestSigner {
    pub identity_prefix: String,
    pub issuer: String,
    ca: Issuer<'static, KeyPair>,
    ca_der: Vec<u8>,
    log_key: SigningKey,
    log_spki: Vec<u8>,
}

impl TestSigner {
    pub fn new(identity_prefix: &str, issuer: &str) -> Self {
        let ca_key = KeyPair::generate().expect("CA key");
        let mut params = CertificateParams::new(Vec::<String>::new()).expect("CA params");
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca_der = params.self_signed(&ca_key).expect("CA").der().to_vec();
        let log_key = SigningKey::generate_ecdsa_p256().expect("log key");
        let log_spki = log_key
            .public_key_der()
            .expect("log key SPKI")
            .as_bytes()
            .to_vec();
        Self {
            identity_prefix: identity_prefix.to_string(),
            issuer: issuer.to_string(),
            ca: Issuer::new(params, ca_key),
            ca_der,
            log_key,
            log_spki,
        }
    }

    /// The identity a release of `version` is signed with.
    pub fn identity(&self, version: &str) -> String {
        format!("{}v{version}", self.identity_prefix)
    }

    /// A bundle over `blob` signed as the release of `version`.
    pub fn sign_release(&self, version: &str, blob: &[u8]) -> Vec<u8> {
        self.sign(&self.identity(version), &self.issuer, blob)
    }

    /// A cosign v3 message-signature bundle over `blob`, with a hashedrekord 0.0.1 log
    /// entry whose signed entry timestamp the log key signs.
    pub fn sign(&self, identity: &str, issuer: &str, blob: &[u8]) -> Vec<u8> {
        let leaf_key = KeyPair::generate().expect("leaf key");
        let mut params = CertificateParams::new(Vec::<String>::new()).expect("leaf params");
        params.subject_alt_names = vec![SanType::URI(identity.try_into().expect("URI SAN"))];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::CodeSigning];
        params.custom_extensions = vec![CustomExtension::from_oid_content(
            ISSUER_V2_OID,
            der_utf8(issuer),
        )];
        let cert_der = params
            .signed_by(&leaf_key, &self.ca)
            .expect("leaf")
            .der()
            .to_vec();
        let signer = SigningKey::from_pkcs8_der(&leaf_key.serialize_der()).expect("leaf signer");
        let signature = signer.sign(blob).expect("signature").as_bytes().to_vec();
        let digest = Sha256::digest(blob);
        let body = self.rekord_body(&digest, &signature, &cert_der);
        let entry = self.log_entry(&body);
        let bundle = json!({
            "mediaType": "application/vnd.dev.sigstore.bundle.v0.3+json",
            "verificationMaterial": {
                "certificate": { "rawBytes": B64.encode(&cert_der) },
                "tlogEntries": [entry],
            },
            "messageSignature": {
                "messageDigest": { "algorithm": "SHA2_256", "digest": B64.encode(digest) },
                "signature": B64.encode(&signature),
            },
        });
        serde_json::to_vec_pretty(&bundle).expect("bundle")
    }

    fn rekord_body(&self, digest: &[u8], signature: &[u8], cert_der: &[u8]) -> Vec<u8> {
        let pem = format!(
            "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
            B64.encode(cert_der)
        );
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        let body = json!({
            "apiVersion": "0.0.1",
            "kind": "hashedrekord",
            "spec": {
                "data": { "hash": { "algorithm": "sha256", "value": hex } },
                "signature": {
                    "content": B64.encode(signature),
                    "publicKey": { "content": B64.encode(pem) },
                },
            },
        });
        serde_json::to_vec(&body).expect("rekord body")
    }

    fn log_entry(&self, body: &[u8]) -> serde_json::Value {
        let log_id = Sha256::digest(&self.log_spki);
        let integrated_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_secs();
        // The signed entry timestamp covers the canonical JSON of these four fields
        // (keys sorted, no whitespace).
        let payload = format!(
            r#"{{"body":"{}","integratedTime":{integrated_time},"logID":"{}","logIndex":1}}"#,
            B64.encode(body),
            log_id
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        let set = self.log_key.sign(payload.as_bytes()).expect("SET");
        json!({
            "logIndex": "1",
            "logId": { "keyId": B64.encode(log_id) },
            "kindVersion": { "kind": "hashedrekord", "version": "0.0.1" },
            "integratedTime": integrated_time.to_string(),
            "inclusionPromise": { "signedEntryTimestamp": B64.encode(set.as_bytes()) },
            "canonicalizedBody": B64.encode(body),
        })
    }

    /// The trusted root holding this CA and log key.
    pub fn trusted_root(&self) -> Vec<u8> {
        let start = "2020-01-01T00:00:00Z";
        let root = json!({
            "mediaType": "application/vnd.dev.sigstore.trustedroot+json;version=0.1",
            "tlogs": [{
                "baseUrl": "http://127.0.0.1/rekor",
                "hashAlgorithm": "SHA2_256",
                "publicKey": {
                    "rawBytes": B64.encode(&self.log_spki),
                    "keyDetails": "PKIX_ECDSA_P256_SHA_256",
                    "validFor": { "start": start },
                },
                "logId": { "keyId": B64.encode(Sha256::digest(&self.log_spki)) },
            }],
            "certificateAuthorities": [{
                "subject": { "organization": "cyber tests", "commonName": "test CA" },
                "uri": "http://127.0.0.1/fulcio",
                "certChain": { "certificates": [{ "rawBytes": B64.encode(&self.ca_der) }] },
                "validFor": { "start": start },
            }],
            "ctlogs": [],
            "timestampAuthorities": [],
        });
        serde_json::to_vec_pretty(&root).expect("trusted root")
    }

    /// Write `trusted_root.json` and `identity.json` for `CYBER_TEST_RELEASE_TRUST=<dir>`.
    pub fn write_trust_dir(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join("trusted_root.json"), self.trusted_root())?;
        let identity = json!({ "identity_prefix": self.identity_prefix, "issuer": self.issuer });
        std::fs::write(dir.join("identity.json"), identity.to_string())
    }
}

/// DER `UTF8String` (tag 12) with a short or two-byte long-form length.
fn der_utf8(value: &str) -> Vec<u8> {
    let bytes = value.as_bytes();
    let mut out = vec![12];
    match u8::try_from(bytes.len()) {
        Ok(len) if len < 0x80 => out.push(len),
        Ok(len) => out.extend([0x81, len]),
        Err(_) => {
            let len = u16::try_from(bytes.len()).expect("issuer too long");
            out.push(0x82);
            out.extend(len.to_be_bytes());
        }
    }
    out.extend_from_slice(bytes);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verify::{Trust, VerifyError};
    use cyber_core::version::ReleaseSource;

    const PREFIX: &str = "https://example.test/release.yml@refs/tags/";
    const ISSUER: &str = "https://issuer.example.test";

    fn trust(signer: &TestSigner) -> (tempfile::TempDir, Trust) {
        let dir = tempfile::tempdir().unwrap();
        signer.write_trust_dir(dir.path()).unwrap();
        let mut source = ReleaseSource::built_in();
        source.test_trusted_root = Some(dir.path().join("trusted_root.json"));
        (dir, Trust::for_source(&source).unwrap())
    }

    #[test]
    fn a_test_signature_verifies_under_the_test_root_only() {
        let signer = TestSigner::new(PREFIX, ISSUER);
        let (_dir, trust) = trust(&signer);
        let bundle = signer.sign_release("1.5.0", b"sums");
        let identity = format!("{PREFIX}v1.5.0");
        trust.verify(b"sums", &bundle, &identity, ISSUER).unwrap();

        let wrong_version = trust.verify(b"sums", &bundle, &format!("{PREFIX}v1.4.0"), ISSUER);
        assert!(matches!(wrong_version, Err(VerifyError::Signature(_))));
        let wrong_issuer = trust.verify(b"sums", &bundle, &identity, "https://other.test");
        assert!(matches!(wrong_issuer, Err(VerifyError::Signature(_))));
        let forged = trust.verify(b"sums!", &bundle, &identity, ISSUER);
        assert!(matches!(forged, Err(VerifyError::Signature(_))));

        // Another throwaway CA is not trusted, nor is any of them in production.
        let other = TestSigner::new(PREFIX, ISSUER).sign_release("1.5.0", b"sums");
        assert!(trust.verify(b"sums", &other, &identity, ISSUER).is_err());
        let production = Trust::production().unwrap();
        assert!(
            production
                .verify(b"sums", &bundle, &identity, ISSUER)
                .is_err()
        );
    }

    #[test]
    fn der_utf8_encodes_short_and_long_lengths() {
        assert_eq!(der_utf8("ab"), vec![12, 2, b'a', b'b']);
        assert_eq!(&der_utf8(&"x".repeat(200))[..3], &[12, 0x81, 200]);
        assert_eq!(&der_utf8(&"x".repeat(300))[..4], &[12, 0x82, 1, 44]);
    }
}
