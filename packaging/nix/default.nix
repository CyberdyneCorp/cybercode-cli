# Nix expression for `cyber-code`. Rendered for each release by
# scripts/release_assets.py; submitting it to nixpkgs is a follow-up step.
{ lib, stdenvNoCC, fetchurl }:

let
  sources = {
    x86_64-linux = {
      url = "{{ url.x86_64-unknown-linux-musl }}";
      sha256 = "{{ sha256.x86_64-unknown-linux-musl }}";
    };
    aarch64-linux = {
      url = "{{ url.aarch64-unknown-linux-musl }}";
      sha256 = "{{ sha256.aarch64-unknown-linux-musl }}";
    };
    x86_64-darwin = {
      url = "{{ url.x86_64-apple-darwin }}";
      sha256 = "{{ sha256.x86_64-apple-darwin }}";
    };
    aarch64-darwin = {
      url = "{{ url.aarch64-apple-darwin }}";
      sha256 = "{{ sha256.aarch64-apple-darwin }}";
    };
  };
  system = stdenvNoCC.hostPlatform.system;
in
stdenvNoCC.mkDerivation {
  pname = "cyber-code";
  version = "{{ version }}";
  src = fetchurl (sources.${system} or (throw "cyber-code: unsupported system ${system}"));
  sourceRoot = ".";
  installPhase = ''
    runHook preInstall
    install -Dm755 cyber $out/bin/cyber
    install -Dm755 cyber-sandbox-exec $out/bin/cyber-sandbox-exec
    runHook postInstall
  '';
  meta = {
    description = "Local-first coding agent";
    homepage = "https://github.com/CyberdyneCorp/cybercode-cli";
    license = lib.licenses.asl20;
    mainProgram = "cyber";
    platforms = builtins.attrNames sources;
  };
}
