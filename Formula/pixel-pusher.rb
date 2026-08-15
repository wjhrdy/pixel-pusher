class PixelPusher < Formula
  desc "Recover clean pixel grids and compact palettes from imperfect pixel art"
  homepage "https://github.com/wjhrdy/pixel-pusher"
  url "https://github.com/wjhrdy/pixel-pusher/archive/refs/tags/v0.0.7.tar.gz"
  sha256 "bfe138568924b13f1fef9a4d26842664232377545e6996825791bf21d1293661"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: ".")
  end

  test do
    assert_match "Recover a clean pixel grid", shell_output("#{bin}/pixel-pusher --help")
    assert_predicate bin/"pixel-pusher-gui", :executable?
  end
end
