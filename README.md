# steam-free-games-bot

Rust Telegram bot that watches Steam for games that are normally paid but temporarily free, generates short Russian descriptions with DeepSeek, and publishes ready-to-post messages to Telegram.

## What the bot does

- Checks Steam on a schedule.
- Uses multiple best-effort Steam sources for promotions:
  - Steam Store `featuredcategories`
  - Steam Store Search free specials
  - optional SteamDB Free Promotions debug source
- Publishes only promotions that match all business rules:
  - regular/original price `> 0`
  - current/final price `== 0`
  - discount percent `== 100`
- Skips permanently free-to-play titles, demos, DLC, tools, soundtracks, software, and duplicate posts for the same active promotion.
- Tries to detect when the giveaway ends.
- Uses DeepSeek for short Russian text when posting real promotions.
- Publishes to:
  - the main channel from `TELEGRAM_MAIN_CHANNEL_ID`
  - all enabled private chats
  - all enabled groups

## Stack

- Rust stable
- tokio
- reqwest with `rustls`
- serde / serde_json
- teloxide
- sqlx + SQLite
- chrono / chrono-tz
- dotenvy
- tracing / tracing-subscriber
- thiserror

## User commands

### Private chat

- `/start` enables notifications for this private chat immediately.
- `/off` disables personal notifications.
- `/on` enables personal notifications again.

### Group or supergroup

- `/on` enables free-game posts in this group.
- `/off` disables free-game posts in this group.

The public Telegram command menu is intentionally simple and shows only:

- `/start`
- `/on`
- `/off`

## Admin commands

These commands are not shown in the public command menu. They can still be used manually by user IDs listed in `ADMIN_USER_IDS`.

- `/status`
- `/check_now`
- `/debug_steam_http`
- `/debug_store_search`
- `/debug_steamdb`
- `/debug_free_until <appid>`
- `/test_post`
- `/preview_app <appid>`

If a non-admin uses one of these commands, the bot replies:

`Эта команда доступна только администратору.`

## Create a Telegram bot with BotFather

1. Open Telegram and find [@BotFather](https://t.me/BotFather).
2. Run `/newbot`.
3. Choose a name and username for the bot.
4. Copy the token into `TELEGRAM_BOT_TOKEN`.

## Add the bot to a channel

1. Create or open your Telegram channel.
2. Add the bot as an administrator.
3. Allow it to post messages and photos.
4. Set `TELEGRAM_MAIN_CHANNEL_ID` to:
   - a numeric chat ID such as `-1001234567890`, or
   - a public channel username such as `@your_channel`

The configured main channel is always included as a publish target when `TELEGRAM_MAIN_CHANNEL_ID` is set.

## Add the bot to a group

1. Add the bot to the group.
2. Make sure the bot can receive commands.
3. Run `/on` in the group.
4. Run `/off` later if you want to stop posts in that group.

## Configuration

Copy the example file:

```bash
cp .env.example .env
```

Fill in the values you need:

| Variable | Description |
| --- | --- |
| `TELEGRAM_BOT_TOKEN` | Bot token from BotFather |
| `TELEGRAM_MAIN_CHANNEL_ID` | Main channel username or chat ID |
| `ADMIN_USER_IDS` | Comma-separated Telegram user IDs allowed to run admin commands |
| `DEEPSEEK_API_KEY` | DeepSeek API key |
| `DEEPSEEK_MODEL` | DeepSeek model name, default `deepseek-v4-flash` |
| `STEAM_COUNTRY` | Steam country code, default `DE` |
| `STEAM_LANGUAGE` | Steam language, default `russian` |
| `ENABLE_STEAM_STORE_SEARCH_SOURCE` | Enable or disable the Steam Store Search source |
| `STEAM_STORE_SEARCH_COUNT` | Number of search rows requested from Steam Store Search |
| `STEAM_STORE_SEARCH_URL` | Steam Store Search endpoint URL |
| `ENABLE_STORE_PAGE_FREE_UNTIL_LOOKUP` | Best-effort lookup of promotion end dates from individual Steam app pages |
| `STORE_PAGE_LOOKUP_DELAY_MS` | Delay between Steam Store app page free-until lookups |
| `ENABLE_STEAMDB_SOURCE` | Enable or disable the optional SteamDB source, default `false` |
| `STEAMDB_FREE_PROMOTIONS_URL` | SteamDB Free Promotions page URL |
| `STEAMDB_USER_AGENT` | User-Agent used for SteamDB requests |
| `STEAMDB_TIMEOUT_SECONDS` | HTTP timeout for SteamDB page fetch |
| `CHECK_INTERVAL_MINUTES` | Scheduler interval in minutes |
| `RUN_STARTUP_CHECK` | If `true`, run a Steam check immediately on startup |
| `DATABASE_URL` | SQLite URL, default `sqlite://data/bot.sqlite` |
| `RUST_LOG` | Log level, for example `info` |

`.env` is intentionally not committed.

## Local run

Build:

```bash
cargo build
```

Run:

```bash
cargo run
```

Release build:

```bash
cargo build --release
```

On startup the bot will:

1. Load configuration from `.env`.
2. Create the `data/` directory if needed.
3. Open SQLite and run migrations.
4. Upsert the main channel from `TELEGRAM_MAIN_CHANNEL_ID` into `chats`.
5. Start the background scheduler.
6. Start the manual Telegram `get_updates` polling loop.

## Production behavior

- The bot keeps Telegram polling alive even if a Steam check fails.
- Steam checks are guarded so one failed promotion does not stop the whole run.
- If Steam Store Search or SteamDB fails, the bot logs it and continues with the other enabled sources.
- DeepSeek failures fall back to Steam short descriptions instead of crashing the check.
- If Telegram photo sending fails, the bot falls back to a text message.

## Scheduler and publishing

- `RUN_STARTUP_CHECK=false` by default to keep startup safe and predictable.
- Scheduled checks and `/check_now` share the same publishing logic.
- Publish targets are resolved centrally as:
  - all chats with `enabled = true`
  - plus `TELEGRAM_MAIN_CHANNEL_ID` if configured
- Duplicate posts are prevented per chat and per active price event.
- Promotions from different sources are deduplicated through the existing `price_events` / `published_posts` pipeline.

## Sources

### Steam Store

- The bot uses Steam `featuredcategories` as source #1.
- Candidate price data is prefiltered before calling `appdetails`.

### Steam Store Search

- The bot uses Steam Store Search free specials as source #2.
- It parses `results_html` from the official Steam Store search endpoint.
- Only rows that already look like paid games with a 100% discount and final price `0` are sent to `appdetails`.
- This source helps catch some free-to-keep promotions that are missing from `featuredcategories`.
- When enabled, the bot also does a best-effort lookup on the individual Steam app page to extract the promotion end date from the visible discount countdown or embedded promo text.

### SteamDB Free Promotions

- The bot can optionally use [SteamDB Free Promotions](https://steamdb.info/upcoming/free/) as an extra best-effort source.
- Only `Free to Keep` entries are accepted.
- `Play For Free` entries are skipped.
- SteamDB entries still go through Steam `appdetails` validation before publishing.
- If Steam price data is missing but SteamDB marks the promotion as `Free to Keep`, the bot may still publish it when `appdetails` confirms the app is a real game and not free-to-play.
- SteamDB is disabled by default because some VPS environments receive `403 Forbidden`.

## Steam detection rules

An app is publishable only when all of these are true:

- app type is `game` when type is available
- `is_free_to_play != true`
- `price_overview.initial > 0`
- `price_overview.final == 0`
- `price_overview.discount_percent == 100`

The bot skips:

- permanently free-to-play games
- demos
- DLC
- tools
- soundtracks
- software
- missing price data

## DeepSeek integration

Real posts use:

`POST https://api.deepseek.com/chat/completions`

The model is asked to return strict JSON:

```json
{
  "short_description": "...",
  "why_play": "...",
  "tags_line": "..."
}
```

DeepSeek is used only for real Steam posts, not for `/test_post` and not for `/preview_app`.

## Telegram post style

The bot sends Telegram HTML with the larger visual layout and keeps the Steam preview card enabled:

```text
🎮 <b>{game_name}</b>

💸 <s>{regular_price}</s> → <b>0 €</b>
⏳ Бесплатно до: <b>{free_until}</b>

🧠 <b>Коротко:</b>
{short_description}

✨ <b>Почему может понравиться:</b>
{why_play}

🏷 {tags_line}

🔗 <a href="{steam_url}">Забрать в Steam</a>
```

## Database

SQLite tables are created by [migrations/0001_init.sql](./migrations/0001_init.sql):

- `chats`
- `games`
- `price_events`
- `published_posts`
- `ai_descriptions`

## systemd deployment

1. Build a release binary:

```bash
cargo build --release
```

2. Copy the project to your server, for example `/opt/steam-free-games-bot`.
3. Put your real `.env` file there.
4. Adjust paths in [systemd/steam-free-games-bot.service.example](./systemd/steam-free-games-bot.service.example).
5. Install the unit:

```bash
sudo cp systemd/steam-free-games-bot.service.example /etc/systemd/system/steam-free-games-bot.service
```

6. Reload and start:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now steam-free-games-bot
```

7. Follow logs:

```bash
sudo journalctl -u steam-free-games-bot -f
```

## Useful manual tests

- Private `/start`
- Private `/off`
- Private `/on`
- Group `/on`
- Group `/off`
- Admin `/test_post`
- Admin `/check_now`
- Admin `/debug_steam_http`
- Admin `/debug_store_search`
- Admin `/debug_steamdb`
- Admin `/debug_free_until 489630`
- Admin `/preview_app 570`

## Known limitations

- Steam may not always provide the promotion end date, so the bot will honestly say when Steam did not provide it.
- Steam Store endpoints are inconsistent, so candidate discovery and free-until detection are best-effort.
- SteamDB can return `403 Forbidden`, a browser challenge page, or changed HTML, so SteamDB parsing is strictly best-effort and optional.
- The project uses long polling and SQLite, which is appropriate for a single-instance MVP but not for horizontal scaling.
