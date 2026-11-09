#!/bin/zsh
# Copy the Mac's NEXUS data and keys to the phone app, over USB.
#
# Until the phone syncs with the Mac (Phase 2 in docs/MOBILE_APP.md), this is
# how the phone gets the Mac's data: a consistent snapshot of the whole
# database, which the phone puts in place of its own on its next start
# (src-tauri/src/db/from_mac.rs). The Mac is the master copy: anything
# entered only on the phone is replaced. Run it again after changes on the
# Mac to bring the phone up to date.
#
# Keys and tokens are sealed for the phone: each is read from the Keychain
# (macOS asks to allow it) and piped straight into encryption with the
# public key the phone's Android Keystore published, whose private half
# never leaves the phone (MacTransfer.kt). No secret is written to disk in
# the clear or passed as an argument, here or on the phone.
#
# Usage: scripts/mac-to-phone.sh [adb serial]
set -euo pipefail

DB="$HOME/Library/Application Support/com.nexus.desktop/nexus.db"
APP=com.nexus.mobile
NAME=nexus-db-from-mac.sqlite
SEALED=nexus-secrets-from-mac.json
PHONE_DIR="/sdcard/Android/data/$APP/files"
OPENSSL=/usr/bin/openssl
ADB="${ANDROID_HOME:-$HOME/Library/Android/sdk}/platform-tools/adb"
[[ -n "${1:-}" ]] && export ANDROID_SERIAL="$1"

[[ -f "$DB" ]] || { echo "No NEXUS database on this Mac: $DB" >&2; exit 1; }

out=$(mktemp -t nexus-db)
key=$(mktemp -t nexus-phone-key)
sealed=$(mktemp -t nexus-sealed)
trap 'rm -f "$out" "$key" "$sealed"' EXIT

# SQLite's own backup, so the copy is consistent even while NEXUS on the Mac
# is writing.
python3 -I - "$DB" "$out" <<'PY'
import sqlite3, sys
source = sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True)
target = sqlite3.connect(sys.argv[2])
source.backup(target)
target.close()
PY

# The app makes its external folder and its transfer key on start, so it has
# to have run once.
"$ADB" shell am start -W -n "$APP/.MainActivity" >/dev/null
for _ in {1..20}; do
  "$ADB" pull "$PHONE_DIR/nexus-transfer-key.pem" "$key" >/dev/null 2>&1 && break
  sleep 1
done
[[ -s "$key" ]] || { echo "The phone has not published its transfer key. Open NEXUS on it and try again." >&2; exit 1; }

# Which secrets: service and account, as the Keychain files them.
jira_email=$(python3 -I -c 'import json,sqlite3,sys; c=sqlite3.connect(f"file:{sys.argv[1]}?mode=ro",uri=True).execute("SELECT config_json FROM connectors WHERE connector_id=?",("jira",)).fetchone(); print(json.loads(c[0]).get("email","") if c and c[0] else "")' "$DB")
github_login=$(python3 -I -c 'import json,sqlite3,sys; c=sqlite3.connect(f"file:{sys.argv[1]}?mode=ro",uri=True).execute("SELECT config_json FROM connectors WHERE connector_id=?",("github",)).fetchone(); a=json.loads(c[0]).get("accounts",{}) if c and c[0] else {}; print(next(iter(a.values()),""))' "$DB")
wanted=(
  "nexus-jira" "$jira_email"
  "nexus-github" "$github_login"
  "nexus-reasoning" "anthropic"
  "nexus-reasoning" "openai"
  "nexus-reasoning" "groq"
)

print -n "[" > "$sealed"
sep=""
for service account in "${wanted[@]}"; do
  [[ -n "$account" ]] || continue
  security find-generic-password -s "$service" -a "$account" >/dev/null 2>&1 || continue
  # The secret only ever exists in this pipe.
  blob=$(security find-generic-password -s "$service" -a "$account" -w \
    | tr -d '\n' \
    | "$OPENSSL" pkeyutl -encrypt -pubin -inkey "$key" -pkeyopt rsa_padding_mode:oaep \
    | base64 | tr -d '\n') || blob=""
  if [[ -z "$blob" ]]; then
    echo "Skipped $service ($account): not allowed, or too long to seal." >&2
    continue
  fi
  print -n "$sep{\"service\":\"$service\",\"account\":\"$account\",\"sealed\":\"$blob\"}" >> "$sealed"
  sep=","
  echo "Sealed $service ($account)"
done
print -n "]" >> "$sealed"

"$ADB" push "$out" "$PHONE_DIR/$NAME" >/dev/null
"$ADB" push "$sealed" "$PHONE_DIR/$SEALED" >/dev/null
"$ADB" shell am force-stop "$APP"
"$ADB" shell am start -n "$APP/.MainActivity" >/dev/null
echo "Data and keys sent. NEXUS on the phone has restarted with them."
