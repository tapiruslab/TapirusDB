class Tapirus < Formula
  desc "Safe-Rust embedded multi-model AI database engine (SQLite simplicity + native vector/graph)"
  homepage "https://tapirusdb.com"
  url "https://github.com/tapiruslab/TapirusDB/archive/refs/tags/v0.1.2.tar.gz"
  version "0.1.2"
  license "BSL-1.1"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: ".")
  end

  test do
    # Verify version command output
    assert_match "TapirusDB v0.1.2", shell_output("#{bin}/tapirus --version")

    # Verify basic in-memory query execution
    test_sql = "CREATE TABLE brew_test (id INT PRIMARY KEY, name TEXT); INSERT INTO brew_test VALUES (1, 'Homebrew'); SELECT * FROM brew_test;"
    assert_match "Homebrew", pipe_output("#{bin}/tapirus :memory:", test_sql)
  end
end
