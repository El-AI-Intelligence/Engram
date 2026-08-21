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
      sha256 "67be8457125950efb347caa216a1bb59a96042f9ee2a30ec759848b860d593f4"
    else
      url "https://github.com/El-AI-Intelligence/engram/releases/download/v#{version}/engramd-darwin-x86_64.tar.gz"
      sha256 "9c466c76451e2a52cc58d837ee18ce40d18ac92a847097558d7cd572a5cd8fbd"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/El-AI-Intelligence/engram/releases/download/v#{version}/engramd-linux-arm64.tar.gz"
      sha256 "b73d32e5ecdb5fc919df366b14a60cb8c55baf41330d7f25c505f9cc8fd92f4f"
    else
      url "https://github.com/El-AI-Intelligence/engram/releases/download/v#{version}/engramd-linux-x86_64.tar.gz"
      sha256 "32db43c7e89f1534a996c9fa2bd241ff628bfb7b9241c50966b933b9f5974a25"
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
