class Tapirus < Formula
  desc "Safe-Rust embedded quad-model AI database engine (SQL, Vectors, GraphRAG, Documents)"
  homepage "https://tapirusdb.com"
  url "https://github.com/tapiruslab/TapirusDB/archive/refs/tags/v1.0.0.tar.gz"
  sha256 "045aa1a96e69ead7eda40ad7f7662d4e5bd0d487b0988c6669387ed56f615537"
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
