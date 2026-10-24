# Vendored Sigstore fixtures

`cosign-v3-blob.txt` and `cosign-v3-blob.sigstore.json` are copied unchanged from
`sigstore-verify` 0.14.0 (`test_data/bundles/`, https://github.com/sigstore/sigstore-rust),
licensed under the Apache License 2.0. They are a real cosign v3 keyless signature logged
in the public-good Rekor instance, so `verify` tests run the production policy offline.
