class PixelPusher < Formula
  desc "Recover clean pixel grids and compact palettes from imperfect pixel art"
  homepage "https://github.com/wjhrdy/pixel-pusher"
  url "https://github.com/wjhrdy/pixel-pusher/archive/refs/tags/v0.0.2.tar.gz"
  sha256 "3b7660fef906930be83b5ad2690a58ce72210eea194fcb1c3031a19fc8a6abc9"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: ".")
  end

  test do
    assert_match "Recover a clean pixel grid", shell_output("#{bin}/pixel-pusher --help")
  end
end
