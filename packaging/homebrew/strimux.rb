# Homebrew formula template (tap: strimux/homebrew-tap, wired at release time).
# Placeholder values; release.yml auto-bumps the URL and SHA256 on tag.
class Strimux < Formula
  desc "niri's scrolling tiling for your CLI agents, in any terminal"
  homepage "https://github.com/hongnoul/gwae"
  version "1.0.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/hongnoul/gwae/releases/download/v1.0.0/strimux-aarch64-apple-darwin.tar.gz"
      sha256 "PENDING"
    else
      url "https://github.com/hongnoul/gwae/releases/download/v1.0.0/strimux-x86_64-apple-darwin.tar.gz"
      sha256 "PENDING"
    end
  end

  def install
    bin.install "strimux"
  end

  test do
    system "#{bin}/strimux", "doctor"
  end
end
