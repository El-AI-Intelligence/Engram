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

  # Set VERSION before running: export VERSION=0.1.0 && brew install ./Formula/engramd.rb
  # SHA256 from release workflow: sha256sum engramd-*.tar.gz
  # Update on each release — the release CI generates these in the build step.

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/El-AI-Intelligence/engram/releases/download/v#{version}/engramd-darwin-arm64.tar.gz"
      sha256 "e3e47831eb5877942e60e39c537693fe9e4c53afcdfd51fcdc571d5877c52173"
    else
      url "https://github.com/El-AI-Intelligence/engram/releases/download/v#{version}/engramd-darwin-x86_64.tar.gz"
      sha256 "069a10ee2ef989a058c440d72db2f72271c8f3c56b3d76d16de5b1b53e841b14"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/El-AI-Intelligence/engram/releases/download/v#{version}/engramd-linux-arm64.tar.gz"
      sha256 "d16287a0efe3b91ae3c70cde39b16fee3b430856f7c224220dcc81db162f0652"
    else
      url "https://github.com/El-AI-Intelligence/engram/releases/download/v#{version}/engramd-linux-x86_64.tar.gz"
      sha256 "5db0d5b105a6dbb0d60f13d30060054ae4ba758fde8327769597cad9bb117253"
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
