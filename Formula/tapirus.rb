class Tapirus < Formula
  desc "Embedded database with SQL, vector search and local AI memory APIs"
  homepage "https://tapirusdb.com"
  url "https://github.com/tapiruslab/TapirusDB/archive/refs/tags/v1.0.2.tar.gz"
  sha256 "dbd9da35eb0acd5be79d601e2940fffb09dfffb45756e0f2a417b87dd2f133e1"
  version "1.0.2"
  license "BUSL-1.1"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: ".")
  end

  test do
    assert_match "tapirus", shell_output("#{bin}/tapirus --help")
  end
end
