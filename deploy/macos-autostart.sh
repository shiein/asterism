#!/bin/sh
set -eu
LABEL=dev.asterism.desktop
EXE="${1:-$(pwd)/target/release/asterism-desktop}"
DIR="$HOME/Library/LaunchAgents"
mkdir -p "$DIR"
cat > "$DIR/$LABEL.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key><array><string>$EXE</string></array>
  <key>RunAtLoad</key><true/>
</dict></plist>
EOF
launchctl load "$DIR/$LABEL.plist" 2>/dev/null || true
echo "wrote $DIR/$LABEL.plist"
