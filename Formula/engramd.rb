# Engram Memory Vault — Homebrew Formula
#
# Install:
#   brew tap pixelphantomai/axiom-os
#   brew install engramd
#
# Or directly:
#   brew install pixelphantomai/axiom-os/engramd

class Engramd < Formula
  desc "Engram Memory Vault — Your AI deserves a memory"
  homepage "https://github.com/El-AI-Intelligence/engram"
  license "MIT"

  # Set VERSION before running: export VERSION=0.1.0 && brew install ./Formula/engramd.rb
  # SHA256 from release workflow: sha256sum engramd-*.tar.gz
  # Update on each release — the release CI generates these in the build step.

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/El-AI-Intelligence/engram/releases/download/v#{version}/engramd-darwin-arm64.tar.gz"
      sha256 "PLACEHOLDER_UPDATE_ON_RELEASE"
    else
      url "https://github.com/El-AI-Intelligence/engram/releases/download/v#{version}/engramd-darwin-x86_64.tar.gz"
      sha256 "PLACEHOLDER_UPDATE_ON_RELEASE"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/El-AI-Intelligence/engram/releases/download/v#{version}/engramd-linux-arm64.tar.gz"
      sha256 "PLACEHOLDER_UPDATE_ON_RELEASE"
    else
      url "https://github.com/El-AI-Intelligence/engram/releases/download/v#{version}/engramd-linux-x86_64.tar.gz"
      sha256 "PLACEHOLDER_UPDATE_ON_RELEASE"
    end
  end

  def install
    bin.install "engram"
    bin.install "engramd"
  end

  test do
    system "#{bin}/engram", "--version"
  end
end
