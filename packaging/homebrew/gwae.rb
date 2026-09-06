# Homebrew formula for the tap hongnoul/homebrew-tap, so the user-facing
# command is `brew install hongnoul/tap/gwae`.
#
# This file is the source of truth; the release workflow's `bump-tap` job
# rewrites the version and SHA256s from the published .sha256 assets and
# pushes the result to the tap. Edit here, not in the tap.
class Gwae < Formula
  desc "niri's scrolling tiling for your CLI agents, in any terminal"
  homepage "https://github.com/hongnoul/gwae"
  version "1.3.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/hongnoul/gwae/releases/download/v1.3.0/gwae-aarch64-apple-darwin.tar.gz"
      sha256 "71ddf0cfc404bed12eb099a6b189309f59a21f4629c745600a988fc3d47cfd95"
    else
      url "https://github.com/hongnoul/gwae/releases/download/v1.3.0/gwae-x86_64-apple-darwin.tar.gz"
      sha256 "707a7eda77b3db28c784ee5faad1f8fdfc4c0c02274a9111e4f940c2a1be24aa"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/hongnoul/gwae/releases/download/v1.3.0/gwae-aarch64-unknown-linux-musl.tar.gz"
      sha256 "a693a5b07645817c2cb9297b5197cd4d83d2e8af1bcb70c1fb1d2c857a35a164"
    else
      url "https://github.com/hongnoul/gwae/releases/download/v1.3.0/gwae-x86_64-unknown-linux-musl.tar.gz"
      sha256 "025644b0022df532db684b69eb181ce0dfa96a817d39c94662d56aebadaf817b"
    end
  end

  def install
    bin.install "gwae"
  end

  test do
    system "#{bin}/gwae", "doctor"
  end
end
