class PixelPusher < Formula
  desc "Recover clean pixel grids and compact palettes from imperfect pixel art"
  homepage "https://github.com/wjhrdy/pixel-pusher"
  url "https://github.com/wjhrdy/pixel-pusher/archive/refs/tags/v0.0.1.tar.gz"
  sha256 "8b4363562c80e52b94860a4ab58ba9df94342cbb0c2e80465de706693af74ec9"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: ".")
  end

  test do
    assert_match "Recover a clean pixel grid", shell_output("#{bin}/pixel-pusher --help")
  end
end
