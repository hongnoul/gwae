# Homebrew formula template (tap: gwae/homebrew-tap, wired at release time).
# Placeholder values; release.yml auto-bumps the URL and SHA256 on tag.
class Gwae < Formula
  desc "niri's scrolling tiling for your CLI agents, in any terminal"
  homepage "https://github.com/hongnoul/gwae"
  version "1.0.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/hongnoul/gwae/releases/download/v1.0.0/gwae-aarch64-apple-darwin.tar.gz"
      sha256 "5c24d94da2e27262e15b950987e6906200c8b04522ab07784607a3c0b1b2b49e"
    else
      url "https://github.com/hongnoul/gwae/releases/download/v1.0.0/gwae-x86_64-apple-darwin.tar.gz"
      sha256 "ad2c5dd7cc631355be34a34351ba86bad84f90682a01f291ce6a588a117c7faa"
    end
  end

  def install
    bin.install "gwae"
  end

  test do
    system "#{bin}/gwae", "doctor"
  end
end
