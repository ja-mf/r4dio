cask "r4dio" do
  version "@@VERSION@@"

  on_arm do
    sha256 "@@SHA_APP_ARM64@@"
    url "https://github.com/ja-mf/r4dio/releases/download/v#{version}/r4dio-macos-arm64-app.zip"
  end

  on_intel do
    sha256 "@@SHA_APP_X86_64@@"
    url "https://github.com/ja-mf/r4dio/releases/download/v#{version}/r4dio-macos-x86_64-app.zip"
  end

  name "r4dio"
  desc "Terminal radio player as a self-contained app (ships its own Ghostty terminal)"
  homepage "https://github.com/ja-mf/r4dio"

  app "r4dio.app"

  postflight do
    system_command "/usr/bin/xattr",
                   args: ["-dr", "com.apple.quarantine", "#{appdir}/r4dio.app"]
  end

  zap trash: "~/.config/radio"
end
