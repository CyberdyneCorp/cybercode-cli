# Homebrew formula for `brew install cyber-code/tap/cyber`. Rendered for each release by
# scripts/release_assets.py; publishing it to the tap is a follow-up step.
class Cyber < Formula
  desc "Local-first coding agent"
  homepage "https://github.com/CyberdyneCorp/cybercode-cli"
  version "{{ version }}"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "{{ url.aarch64-apple-darwin }}"
      sha256 "{{ sha256.aarch64-apple-darwin }}"
    end
    on_intel do
      url "{{ url.x86_64-apple-darwin }}"
      sha256 "{{ sha256.x86_64-apple-darwin }}"
    end
  end

  on_linux do
    on_arm do
      url "{{ url.aarch64-unknown-linux-gnu }}"
      sha256 "{{ sha256.aarch64-unknown-linux-gnu }}"
    end
    on_intel do
      url "{{ url.x86_64-unknown-linux-gnu }}"
      sha256 "{{ sha256.x86_64-unknown-linux-gnu }}"
    end
  end

  def install
    bin.install "cyber", "cyber-sandbox-exec"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/cyber --version")
  end
end
