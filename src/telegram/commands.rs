use teloxide::types::BotCommand;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelegramCommand {
    Start,
    On,
    Off,
    Status,
    CheckNow,
    DebugSteamHttp,
    DebugStoreSearch,
    DebugSteamDb,
    TestPost,
    PreviewApp { appid: Option<String> },
}

impl TelegramCommand {
    pub fn public_menu_commands() -> Vec<BotCommand> {
        vec![
            BotCommand::new("start", "Включить личную рассылку в личном чате"),
            BotCommand::new("on", "Включить рассылку для этого чата"),
            BotCommand::new("off", "Выключить рассылку для этого чата"),
        ]
    }

    pub fn parse(text: &str, bot_username: &str) -> Option<Self> {
        let first_line = text.lines().next()?.trim();
        if !first_line.starts_with('/') {
            return None;
        }

        let mut parts = first_line.split_whitespace();
        let raw_command = parts.next()?.trim_start_matches('/');
        let args = parts.collect::<Vec<_>>().join(" ");

        let (command, mention) = match raw_command.split_once('@') {
            Some((command, mention)) => (command, Some(mention)),
            None => (raw_command, None),
        };

        if let Some(mention) = mention {
            if !bot_username.is_empty() && !mention.eq_ignore_ascii_case(bot_username) {
                return None;
            }
        }

        match command.to_ascii_lowercase().as_str() {
            "start" => Some(Self::Start),
            "on" => Some(Self::On),
            "off" => Some(Self::Off),
            "status" => Some(Self::Status),
            "check_now" => Some(Self::CheckNow),
            "debug_steam_http" => Some(Self::DebugSteamHttp),
            "debug_store_search" => Some(Self::DebugStoreSearch),
            "debug_steamdb" => Some(Self::DebugSteamDb),
            "test_post" => Some(Self::TestPost),
            "preview_app" => Some(Self::PreviewApp {
                appid: if args.is_empty() { None } else { Some(args) },
            }),
            _ => None,
        }
    }
}
