class PixelPusher < Formula
  desc "Recover clean pixel grids and compact palettes from imperfect pixel art"
  homepage "https://github.com/wjhrdy/pixel-pusher"
  url "https://github.com/wjhrdy/pixel-pusher/archive/refs/tags/v0.0.5.tar.gz"
  sha256 "87cf2be40636d47b58ce45cc20c13bb9f065d0767f4923d8fcf620cc33fde53c"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: ".")
  end

  test do
    assert_match "Recover a clean pixel grid", shell_output("#{bin}/pixel-pusher --help")
    assert_predicate bin/"pixel-pusher-gui", :executable?
  end
end
