class R4dio < Formula
  desc "Terminal radio and audio player (internet radio, NTS metadata, song recognition)"
  homepage "https://github.com/ja-mf/r4dio"
  license "MIT"
  version "@@VERSION@@"

  depends_on "ffmpeg"
  depends_on "mpv"
  depends_on "yt-dlp"

  on_arm do
    url "https://github.com/ja-mf/r4dio/releases/download/v@@VERSION@@/r4dio-macos-arm64-cli.tar.gz"
    sha256 "@@SHA_CLI_ARM64@@"
  end

  on_intel do
    url "https://github.com/ja-mf/r4dio/releases/download/v@@VERSION@@/r4dio-macos-x86_64-cli.tar.gz"
    sha256 "@@SHA_CLI_X86_64@@"
  end

  def install
    # vibra lives next to the resolved r4dio binary in libexec so r4dio's
    # beside-the-exe discovery finds it (song recognition keeps working).
    libexec.install "r4dio", "vibra", "LICENSE"
    bin.install_symlink libexec/"r4dio"
  end

  def caveats
    <<~EOS
      r4dio is a TUI app — run it in a truecolor terminal
      (Ghostty, iTerm2, kitty, WezTerm). Terminal.app shows wrong colors.
      For the self-contained app that ships its own terminal:
        brew install --cask ja-mf/tap/r4dio
    EOS
  end

  test do
    assert_path_exists libexec/"r4dio"
    assert_path_exists libexec/"vibra"
  end
end
