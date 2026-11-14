//! `config/minehouse/auth.toml`, the settings of the Fabric `minehouse-auth.json`.

use serde::{Deserialize, Serialize};

use crate::config::env_value;

/// Login check settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// Turns the check off entirely: everyone the proxy forwards gets in.
    pub enabled: bool,
    /// Site with `/api/mc/*`. Env `MINEHOUSE_API_URL` overrides it.
    pub api_url: String,
    /// `MC_API_SECRET` of the site. Env `MINEHOUSE_API_SECRET` overrides it. Without a secret
    /// only players from the local cache get in.
    pub api_secret: String,
    /// Which server this is for the site: `main` (production) lets in the main whitelist,
    /// any other id has its own. Env `MINEHOUSE_AUTH_SERVER_ID`, then `MINEHOUSE_SERVER_ID`.
    pub server_id: String,
    /// How often to ask the site about a code that waits for confirmation.
    pub poll_interval_ms: u64,
    /// Timeout of one HTTP call.
    pub http_timeout_ms: u64,
    /// How long a locally remembered nickname + IP pair lets the player in while the site is
    /// down. `0` turns the cache off.
    pub offline_trust_days: u64,
    /// Shown in the dialog: where to enter the code.
    pub site_name: String,
    /// Shown when the player is not in the Discord server.
    pub discord_invite: String,
    /// Texts shown to players.
    pub messages: Messages,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            api_url: "https://minehouse.pro".to_owned(),
            api_secret: String::new(),
            server_id: "main".to_owned(),
            poll_interval_ms: 2000,
            http_timeout_ms: 5000,
            offline_trust_days: 7,
            site_name: "minehouse.pro".to_owned(),
            discord_invite: "discord.gg/gCYGxDzaCb".to_owned(),
            messages: Messages::default(),
        }
    }
}

impl AuthConfig {
    /// Applies environment overrides and clamps values to sane ranges.
    #[must_use]
    pub fn resolved(mut self) -> Self {
        if let Some(url) = env_value("MINEHOUSE_API_URL") {
            self.api_url = url;
        }
        if let Some(secret) = env_value("MINEHOUSE_API_SECRET") {
            self.api_secret = secret;
        }
        if let Some(id) =
            env_value("MINEHOUSE_AUTH_SERVER_ID").or_else(|| env_value("MINEHOUSE_SERVER_ID"))
        {
            self.server_id = id;
        }
        if self.server_id.trim().is_empty() {
            "main".clone_into(&mut self.server_id);
        }
        self.poll_interval_ms = self.poll_interval_ms.max(500);
        self.http_timeout_ms = self.http_timeout_ms.max(1000);
        self
    }
}

/// Texts of the login dialog and of the disconnect screens. `{site}`, `{invite}`, `{code}`
/// and `{nickname}` are filled in where the Fabric mod fills them.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
#[expect(missing_docs, reason = "each field is the text its name says")]
pub struct Messages {
    pub dialog_title: String,
    pub code_label: String,
    pub instructions: String,
    pub warning: String,
    pub copy_button: String,
    pub copy_tooltip: String,
    pub cancel_button: String,
    pub waiting: String,

    pub not_in_guild: String,
    pub banned: String,
    pub denied: String,
    pub expired: String,
    pub cancelled: String,
    pub replaced: String,
    pub api_unavailable: String,
    pub already_online: String,
    pub already_authenticating: String,
}

impl Default for Messages {
    fn default() -> Self {
        Self {
            dialog_title: "Вход на MineHouse".to_owned(),
            code_label: "Твой код:".to_owned(),
            instructions: "Введи его на {site} или отправь боту в Discord: /вход {code}".to_owned(),
            warning: "Подтверждай, только если это ты заходишь в игру прямо сейчас.".to_owned(),
            copy_button: "Скопировать код".to_owned(),
            copy_tooltip: "Код попадёт в буфер обмена".to_owned(),
            cancel_button: "Отмена".to_owned(),
            waiting: "Окно закроется само, когда ты подтвердишь вход.".to_owned(),

            not_in_guild: "Ты не в нашем Discord. Зайди: {invite} и попробуй снова.".to_owned(),
            banned: "Ты заблокирован на MineHouse.".to_owned(),
            denied: "Вход отклонён.".to_owned(),
            expired: "Код истёк, зайди снова.".to_owned(),
            cancelled: "Вход отменён.".to_owned(),
            replaced: "Запрос на вход заменён новым. Зайди снова.".to_owned(),
            api_unavailable: "Сайт авторизации недоступен, а это устройство ещё не подтверждено. Попробуй позже.".to_owned(),
            already_online: "Игрок с ником {nickname} уже на сервере. Если это ты, подожди минуту и зайди снова.".to_owned(),
            already_authenticating: "Под ником {nickname} уже идёт вход. Заверши его или подожди пару минут.".to_owned(),
        }
    }
}
