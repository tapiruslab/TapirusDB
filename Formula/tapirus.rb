class Tapirus < Formula
  desc "Safe-Rust embedded quad-model AI database engine (SQL, Vectors, GraphRAG, Documents)"
  homepage "https://tapirusdb.com"
  url "https://github.com/tapiruslab/TapirusDB/archive/refs/tags/v1.0.0.tar.gz"
  sha256 "51ae1f9b86e3982ef923d1f46cbe5422aa172b9152ed91136bc2c9f96b14346a"
  version "1.0.0"
  license "BUSL-1.1"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: ".")
  end

  test do
    assert_match "tapirus", shell_output("#{bin}/tapirus --help")
  end
end
