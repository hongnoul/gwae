# Homebrew formula for the tap hongnoul/homebrew-tap, so the user-facing
# command is `brew install hongnoul/tap/gwae`.
#
# This file is the source of truth; the release workflow's `bump-tap` job
# rewrites the version and SHA256s from the published .sha256 assets and
# pushes the result to the tap. Edit here, not in the tap.
class Gwae < Formula
  desc "niri's scrolling tiling for your CLI agents, in any terminal"
  homepage "https://github.com/hongnoul/gwae"
  version "1.0.1"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/hongnoul/gwae/releases/download/v1.0.1/gwae-aarch64-apple-darwin.tar.gz"
      sha256 "7a302cf9faadd270d65eba0e5a7a77241ce199fac95540b63e65bc52f82d5e33"
    else
      url "https://github.com/hongnoul/gwae/releases/download/v1.0.1/gwae-x86_64-apple-darwin.tar.gz"
      sha256 "2b9f443bd0ef12ec1b09525e4fe4e66fd9536d3968bc1eb93cfac9bb26fda0e1"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/hongnoul/gwae/releases/download/v1.0.1/gwae-aarch64-unknown-linux-musl.tar.gz"
      sha256 "8b5a6d201c880ed76afddd6b92ed74e9a41cca315ff87d82b05d93d1f740b727"
    else
      url "https://github.com/hongnoul/gwae/releases/download/v1.0.1/gwae-x86_64-unknown-linux-musl.tar.gz"
      sha256 "c6fdbf4b7d1b8839043b52341fb2369aed228c8a15bed3dfd28e46dc9ce03fd5"
    end
  end

  def install
    bin.install "gwae"
  end

  test do
    system "#{bin}/gwae", "doctor"
  end
end
