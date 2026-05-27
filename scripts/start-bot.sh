#!/usr/bin/env bash
set -euo pipefail

cd /opt/steam-monitoring/steam-monitoring

# Clear stale Telegram updates after downtime so the bot does not replay old
# heavy commands like /check_now on startup. This is safe here because the bot
# handles informational commands rather than critical transactions.
BOT_TOKEN=$(grep '^TELEGRAM_BOT_TOKEN=' .env | cut -d '=' -f2- | tr -d '"' | tr -d "'")

if [ -n "$BOT_TOKEN" ]; then
  curl -s "https://api.telegram.org/bot${BOT_TOKEN}/deleteWebhook?drop_pending_updates=true" >/dev/null || true
fi

exec /opt/steam-monitoring/steam-monitoring/target/release/steam-free-games-bot
