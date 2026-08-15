class PixelPusher < Formula
  desc "Recover clean pixel grids and compact palettes from imperfect pixel art"
  homepage "https://github.com/wjhrdy/pixel-pusher"
  url "https://github.com/wjhrdy/pixel-pusher/archive/refs/tags/v0.0.6.tar.gz"
  sha256 "452fb293b8e94db9bc2039a0f161dd6b6368ad84e97247ace8015c4fe84c5754"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: ".")
  end

  test do
    assert_match "Recover a clean pixel grid", shell_output("#{bin}/pixel-pusher --help")
    assert_predicate bin/"pixel-pusher-gui", :executable?
  end
end
