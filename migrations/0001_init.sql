CREATE TABLE IF NOT EXISTS chats (
    chat_id TEXT PRIMARY KEY,
    chat_type TEXT NOT NULL,
    title TEXT,
    enabled BOOLEAN NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS games (
    appid INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    steam_url TEXT NOT NULL,
    type TEXT,
    is_free_to_play BOOLEAN DEFAULT 0,
    header_image TEXT,
    capsule_image TEXT,
    short_description TEXT,
    genres_json TEXT,
    categories_json TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS price_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    appid INTEGER NOT NULL,
    currency TEXT,
    regular_price_cents INTEGER,
    final_price_cents INTEGER,
    discount_percent INTEGER,
    free_until TEXT,
    source TEXT,
    detected_at TEXT NOT NULL,
    ended_at TEXT
);

CREATE TABLE IF NOT EXISTS published_posts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    appid INTEGER NOT NULL,
    chat_id TEXT NOT NULL,
    message_id INTEGER,
    price_event_id INTEGER,
    published_at TEXT NOT NULL,
    UNIQUE(appid, chat_id, price_event_id)
);

CREATE TABLE IF NOT EXISTS ai_descriptions (
    appid INTEGER PRIMARY KEY,
    language TEXT NOT NULL,
    short_description TEXT NOT NULL,
    why_play TEXT NOT NULL,
    tags_line TEXT,
    model TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_price_events_appid_active
    ON price_events(appid, ended_at);

CREATE INDEX IF NOT EXISTS idx_published_posts_lookup
    ON published_posts(appid, chat_id, price_event_id);

