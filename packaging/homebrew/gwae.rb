# Homebrew formula for the tap hongnoul/homebrew-tap, so the user-facing
# command is `brew install hongnoul/tap/gwae`.
#
# This file is the source of truth; the release workflow's `bump-tap` job
# rewrites the version and SHA256s from the published .sha256 assets and
# pushes the result to the tap. Edit here, not in the tap.
#
# Homebrew is the absolute source of truth for gwae deployments. There are
# no other packaged channels: no Linux bottles, no Windows zip, no scoop,
# no AUR, no nix flake, no install script.
class Gwae < Formula
  desc "A scrolling terminal multiplexer for macOS. Panes never shrink."
  homepage "https://github.com/hongnoul/gwae"
  version "1.3.0"
  license "MIT"

  depends_on :macos

  if Hardware::CPU.arm?
    url "https://github.com/hongnoul/gwae/releases/download/v1.3.0/gwae-aarch64-apple-darwin.tar.gz"
    sha256 "71ddf0cfc404bed12eb099a6b189309f59a21f4629c745600a988fc3d47cfd95"
  else
    url "https://github.com/hongnoul/gwae/releases/download/v1.3.0/gwae-x86_64-apple-darwin.tar.gz"
    sha256 "707a7eda77b3db28c784ee5faad1f8fdfc4c0c02274a9111e4f940c2a1be24aa"
  end

  def install
    bin.install "gwae"
  end

  test do
    system "#{bin}/gwae", "doctor"
  end
end
