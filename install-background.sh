#!/bin/zsh
# Install the local bundle and an event-driven per-user USB/login observer.
set -eu
repo="$(cd "$(dirname "$0")" && pwd)"
source_app="${1:-$repo/../Pixy Control.app}"
app="$HOME/Applications/Pixy Control.app"
agent="$HOME/Library/LaunchAgents/dev.ndelanhese.pixy-control.device.plist"
label="gui/$(id -u)/dev.ndelanhese.pixy-control.device"
if [[ "${1:-}" == "--uninstall" ]]; then
    launchctl bootout "$label" 2>/dev/null || true
    rm -f "$agent"
    echo "Inicialização automática desativada."
    exit 0
fi
test -x "$source_app/Contents/MacOS/Pixy Device Launcher"
launchctl bootout "$label" 2>/dev/null || true
mkdir -p "$HOME/Applications" "$HOME/Library/LaunchAgents"
if [[ "$source_app" != "$app" ]]; then ditto "$source_app" "$app"; fi
python3 - "$agent" "$app" <<'PY'
import plistlib, sys
from pathlib import Path
agent, app = sys.argv[1:]
data = {
    'Label': 'dev.ndelanhese.pixy-control.device',
    'ProgramArguments': [app + '/Contents/MacOS/Pixy Device Launcher', app],
    'RunAtLoad': True,
    'KeepAlive': True,
    'ThrottleInterval': 10,
    'ProcessType': 'Background',
}
Path(agent).write_bytes(plistlib.dumps(data))
Path(agent).chmod(0o644)
PY
launchctl bootstrap "gui/$(id -u)" "$agent"
echo "Pixy Control instalado; inicia em segundo plano ao detectar a Pixy."
