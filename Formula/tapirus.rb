class Tapirus < Formula
  desc "Safe-Rust embedded quad-model AI database engine (SQL, Vectors, GraphRAG, Documents)"
  homepage "https://tapirusdb.com"
  url "https://github.com/tapiruslab/TapirusDB/archive/refs/tags/v1.0.1.tar.gz"
  sha256 "90f2930bf449ae1ee9165d1795f0dd0fab5237d20a44da30b871e16d06e50c89"
  version "1.0.1"
  license "BUSL-1.1"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: ".")
  end

  test do
    assert_match "tapirus", shell_output("#{bin}/tapirus --help")
  end
end
