# Engram by El AI Intelligence — Homebrew Formula
#
# Install:
#   brew tap El-AI-Intelligence/engram
#   brew install engramd
#
# Or directly:
#   brew install El-AI-Intelligence/engram/engramd

class Engramd < Formula
  desc "Engram by El AI Intelligence — Your AI deserves a memory"
  homepage "https://github.com/El-AI-Intelligence/engram"
  license "MIT"
  version "0.1.2"

  # SHA256 from release workflow: sha256sum engramd-*.tar.gz
  # Update on each release with: VERSION=vX.Y.Z ./scripts/update-formula.sh

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/El-AI-Intelligence/engram/releases/download/v#{version}/engramd-darwin-arm64.tar.gz"
      sha256 "39aca5821d9cc1839acba0b220d98690f0628b502e10bb1ea5272218f7a20ae8"
    else
      url "https://github.com/El-AI-Intelligence/engram/releases/download/v#{version}/engramd-darwin-x86_64.tar.gz"
      sha256 "e76db79bcd493a24f381fd18dbfd34cf5fc1dcf574b2916a5f42233e8091e155"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/El-AI-Intelligence/engram/releases/download/v#{version}/engramd-linux-arm64.tar.gz"
      sha256 "1606dc9336d53eda356c451eb163819fcd5356ab4dd0841d1af6fbc32c13d7e3"
    else
      url "https://github.com/El-AI-Intelligence/engram/releases/download/v#{version}/engramd-linux-x86_64.tar.gz"
      sha256 "5921ced29e7ba5a6f987ab455880706cf1812c9a0cc0740cfc3c5261b991f612"
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
