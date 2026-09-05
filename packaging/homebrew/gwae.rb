# Homebrew formula for the tap hongnoul/homebrew-tap, so the user-facing
# command is `brew install hongnoul/tap/gwae`.
#
# This file is the source of truth; the release workflow's `bump-tap` job
# rewrites the version and SHA256s from the published .sha256 assets and
# pushes the result to the tap. Edit here, not in the tap.
class Gwae < Formula
  desc "niri's scrolling tiling for your CLI agents, in any terminal"
  homepage "https://github.com/hongnoul/gwae"
  version "1.2.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/hongnoul/gwae/releases/download/v1.2.0/gwae-aarch64-apple-darwin.tar.gz"
      sha256 "d7cc8819d980226f28c79c3c2b2281544910394e2a88c5cb06e998c4159c4654"
    else
      url "https://github.com/hongnoul/gwae/releases/download/v1.2.0/gwae-x86_64-apple-darwin.tar.gz"
      sha256 "075b44088ecc926b8a1f0b032a68194be5859d565e38edc0dda34c11e042e873"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/hongnoul/gwae/releases/download/v1.2.0/gwae-aarch64-unknown-linux-musl.tar.gz"
      sha256 "615695d2ccd01e98d57bebe740e5f9c0a71a55d63a0320e00e2eb076438a3fa3"
    else
      url "https://github.com/hongnoul/gwae/releases/download/v1.2.0/gwae-x86_64-unknown-linux-musl.tar.gz"
      sha256 "9b3d95dfc62cdd77786366f8c033c135c2e7287858dd8b0f42e53766d02514c5"
    end
  end

  def install
    bin.install "gwae"
  end

  test do
    system "#{bin}/gwae", "doctor"
  end
end
