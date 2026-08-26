# Homebrew formula template (tap: gwae/homebrew-tap, wired at release time).
# Version, URLs, and SHA256s are bumped by hand at release time and
# verified against the published .sha256 assets.
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

  def install
    bin.install "gwae"
  end

  test do
    system "#{bin}/gwae", "doctor"
  end
end
