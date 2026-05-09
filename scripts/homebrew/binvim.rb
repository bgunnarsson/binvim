class Binvim < Formula
  desc "Vim-grammar TUI editor with batteries included"
  homepage "https://github.com/bgunnarsson/binvim"
  url "https://github.com/bgunnarsson/binvim/archive/refs/tags/v0.0.0.tar.gz"
  sha256 ""
  license :cannot_represent

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
    bin.install_symlink "binvim" => "bim"
  end

  test do
    assert_predicate bin/"binvim", :executable?
    assert_predicate bin/"bim", :exist?
  end
end
