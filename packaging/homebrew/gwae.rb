# Homebrew formula for the tap hongnoul/homebrew-tap, so the user-facing
# command is `brew install hongnoul/tap/gwae`.
#
# This file is the source of truth; the release workflow's `bump-tap` job
# rewrites the version and SHA256s from the published .sha256 assets and
# pushes the result to the tap. Edit here, not in the tap.
class Gwae < Formula
  desc "niri's scrolling tiling for your CLI agents, in any terminal"
  homepage "https://github.com/hongnoul/gwae"
  version "1.1.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/hongnoul/gwae/releases/download/v1.1.0/gwae-aarch64-apple-darwin.tar.gz"
      sha256 "c81f64ed1a730e445d5bed70782c06865ede16661fb450b9faccc314778819e2"
    else
      url "https://github.com/hongnoul/gwae/releases/download/v1.1.0/gwae-x86_64-apple-darwin.tar.gz"
      sha256 "a60e09976068318acee90bf2cd888c6f8318aeca0042a580cce4a1fb4b505fae"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/hongnoul/gwae/releases/download/v1.1.0/gwae-aarch64-unknown-linux-musl.tar.gz"
      sha256 "ebc6603285f87bb2441a31407056eb184eb5cc1c82f4ae197ea6295777735184"
    else
      url "https://github.com/hongnoul/gwae/releases/download/v1.1.0/gwae-x86_64-unknown-linux-musl.tar.gz"
      sha256 "ef636c868450641b76064aed6da2f3b9f564407d5f3771c3712696846ca1a1d6"
    end
  end

  def install
    bin.install "gwae"
  end

  test do
    system "#{bin}/gwae", "doctor"
  end
end
