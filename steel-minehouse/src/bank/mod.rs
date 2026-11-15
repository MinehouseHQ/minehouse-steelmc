//! The bank: accounts in АР on the site, ores in the game (the Fabric `MineHouse Bank`).
//!
//! One diamond ore is one АР. The money itself lives on the site (`POST /api/mc/bank/<action>`,
//! the contract is in minehouse-landing `bank-mod/README.md`); the server only moves ores in
//! and out of inventories and keeps a [`journal`] of what is half done.
//!
//! This is the base the ATM, the cards, the terminals and the spawn zones stand on: the calls
//! to the site, the journal, and what a player is given on join.

pub mod atm;
pub mod atm_block;
pub mod card;
pub mod gui;
pub mod journal;
pub mod ores;
mod settings;
pub mod terminal;
pub mod terminal_block;
pub mod terminals;
pub mod transfer;
pub mod zones;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, mem,
    sync::{Arc, OnceLock},
    time::Duration,
};

use serde_json::{Value, json};
use steel_core::{
    player::Player,
    server::{
        Server,
        jobs::{JobPoll, ServerJob, ServerJobContext},
    },
    world::block_guard::set_block_guard,
};
use steel_utils::locks::SyncMutex;
use tokio::runtime::Handle;
use uuid::Uuid;

pub use settings::BankConfig;

use crate::{config, moderation::format::render, site::SiteClient};
use journal::{DELIVERED, DEPOSIT, Entry, Journal, VENDING};
use terminals::Terminals;

/// How often half-done operations are tried again, in ticks.
const RETRY_TICKS: u64 = 20 * 30;
/// How often the labels of terminals are looked after, in ticks.
const LABEL_TICKS: u64 = 10;

/// What runs on the server tick once the site has answered.
type Done = Box<dyn FnOnce(&Arc<Server>) + Send>;

static BANK: OnceLock<Arc<Bank>> = OnceLock::new();

/// The bank of this server, once it is started.
#[must_use]
pub fn bank() -> Option<&'static Arc<Bank>> {
    BANK.get()
}

/// The running bank.
pub struct Bank {
    /// The settings.
    pub config: BankConfig,
    /// The half-done operations.
    pub journal: Journal,
    /// What each payment terminal is set to.
    pub terminals: Terminals,
    /// The spawn zones.
    pub zones: zones::Zones,
    /// None without a secret: the bank is offline.
    site: Option<SiteClient>,
    runtime: Handle,
    /// Answers of the site, waiting for the next tick.
    done: SyncMutex<Vec<Done>>,
    /// The numbers of the personal accounts of the players online.
    account_numbers: SyncMutex<BTreeMap<Uuid, String>>,
}

/// `{ok: false, error: "offline"}`: what a call answers when the site could not be asked.
#[must_use]
pub fn offline() -> Value {
    json!({"ok": false, "error": "offline", "offline": true})
}

/// Whether `answer` is [`offline`]: the site did not answer at all.
#[must_use]
pub fn is_offline(answer: &Value) -> bool {
    answer.get("offline").is_some()
}

/// Whether the site did what it was asked.
#[must_use]
pub fn ok(answer: &Value) -> bool {
    answer.get("ok").and_then(Value::as_bool) == Some(true)
}

/// The error code of a refusal.
#[must_use]
pub fn error(answer: &Value) -> &str {
    answer
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
}

/// «5 АР».
#[must_use]
pub fn ar(amount: i64) -> String {
    format!("{amount} АР")
}

/// Whether `account` of an answer is a personal one (not a shared one).
#[must_use]
pub fn personal(account: &Value) -> bool {
    account
        .get("personal")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

/// The body of a call about `player`.
#[must_use]
pub fn body(player: Uuid) -> Value {
    json!({"uuid": player.to_string()})
}

/// The undelivered withdrawals of a `pending` answer: the id and the ores of each.
fn pending_ops(answer: &Value) -> Vec<(String, i32)> {
    answer
        .get("ops")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|op| {
            let id = op.get("id")?.as_str()?;
            let amount = i32::try_from(op.get("amount")?.as_i64()?).ok()?;
            Some((id.to_owned(), amount))
        })
        .collect()
}

/// The line about what was sold while the owner was away, if anything was.
fn sales_line(answer: &Value) -> Option<String> {
    let count = answer.get("count")?.as_i64()?;
    let sum = answer.get("sum")?.as_i64()?;
    (count != 0).then(|| format!("&6Пока тебя не было: продано {count} раз, +{}", ar(sum)))
}

pub(crate) fn online(server: &Server, uuid: Uuid) -> Option<Arc<Player>> {
    server
        .get_players()
        .into_iter()
        .find(|player| player.gameprofile.id() == uuid)
}

impl Bank {
    /// Reads the settings and the journal and starts the bank. None when it is turned off.
    pub fn start(server: &Arc<Server>) -> Option<Arc<Self>> {
        let config = match config::load::<BankConfig>(&config::path("bank")) {
            Ok(config) => config,
            Err(error) => {
                log::error!("Bank is off: {error}");
                return None;
            }
        };
        if !config.enabled {
            log::info!("Bank is off (config/minehouse/bank.toml)");
            return None;
        }
        let config = config.resolved();
        let Ok(runtime) = Handle::try_current() else {
            log::error!("Bank is off: no async runtime");
            return None;
        };
        let site = if config.api_secret.is_empty() {
            log::warn!("Bank: no API secret, the bank is offline");
            None
        } else {
            SiteClient::new(
                &config.api_url,
                &config.api_secret,
                Duration::from_millis(config.http_timeout_ms),
                &config.server_id,
            )
            .inspect_err(|error| log::error!("Bank: no HTTP client, the bank is offline: {error}"))
            .ok()
        };
        let file = config::data_path("bank-journal.json");
        let journal = Journal::load(file.clone()).unwrap_or_else(|error| {
            // Better a bank that works than one that waits for a hand-fixed file: the broken
            // file is kept aside.
            log::error!(
                "Bank: could not read the journal {}: {error}",
                file.display()
            );
            let mut aside = file.clone().into_os_string();
            aside.push(".broken");
            if let Err(error) = fs::rename(&file, &aside) {
                log::error!("Bank: could not move the broken journal aside: {error}");
            }
            Journal::empty(file)
        });
        let bank = Arc::new(Self {
            config,
            journal,
            terminals: Terminals::load(config::data_path("bank-terminals.json")),
            zones: zones::Zones::new(),
            site,
            runtime,
            done: SyncMutex::new(Vec::new()),
            account_numbers: SyncMutex::new(BTreeMap::new()),
        });
        if BANK.set(Arc::clone(&bank)).is_err() {
            log::error!("Bank: already started");
            return None;
        }
        if !set_block_guard(&terminal::StockGuard) {
            log::error!("Bank: another block guard is installed, stock chests are not kept");
        }
        server.jobs.spawn(Tick {
            bank: Arc::clone(&bank),
            online: BTreeSet::new(),
        });
        bank.retry();
        log::info!(
            "Bank: {} as {:?}, {} terminals",
            bank.config.api_url,
            bank.config.server_id,
            bank.terminals.count()
        );
        Some(bank)
    }

    /// Whether the bank can reach the site at all.
    #[must_use]
    pub const fn configured(&self) -> bool {
        self.site.is_some()
    }

    /// Calls the site and runs `then` on the server tick; a failed call gives [`offline`].
    pub fn call(
        self: &Arc<Self>,
        action: &str,
        body: &Value,
        then: impl FnOnce(&Arc<Server>, Value) + Send + 'static,
    ) {
        let Some(site) = self.site.clone() else {
            self.finish(then, offline());
            return;
        };
        let bank = Arc::clone(self);
        let action = action.to_owned();
        let body = body.to_string();
        self.runtime.spawn(async move {
            let path = format!("/api/mc/bank/{action}");
            let answer = match site.post_json(&path, body).await {
                Ok(text) => serde_json::from_str::<Value>(&text)
                    .ok()
                    .filter(Value::is_object)
                    .unwrap_or_else(|| {
                        log::warn!("Bank: {action} failed: empty answer");
                        offline()
                    }),
                Err(error) => {
                    log::warn!("Bank: {action} failed: {error}");
                    offline()
                }
            };
            bank.finish(then, answer);
        });
    }

    fn finish(&self, then: impl FnOnce(&Arc<Server>, Value) + Send + 'static, answer: Value) {
        self.later(move |server| then(server, answer));
    }

    /// Runs `then` on the next server tick: for what a window can't do while it handles a
    /// click, like opening another window.
    pub fn later(&self, then: impl FnOnce(&Arc<Server>) + Send + 'static) {
        self.done.lock().push(Box::new(then));
    }

    /// Remembers the number of the personal account of a player online.
    pub(crate) fn remember_account(&self, player: Uuid, number: &str) {
        self.account_numbers
            .lock()
            .insert(player, number.to_owned());
    }

    /// The number of the personal account of a player online, once the site has told it.
    #[must_use]
    pub fn account_number(&self, player: Uuid) -> Option<String> {
        self.account_numbers.lock().get(&player).cloned()
    }

    /// Asks for the summary of an account: `account_id`, else the account of `card_id`, else
    /// the personal one. The answer also lists all accounts the player may use (`accounts`).
    pub fn account(
        self: &Arc<Self>,
        player: Uuid,
        account_id: Option<&str>,
        card_id: Option<&str>,
        then: impl FnOnce(&Arc<Server>, Value) + Send + 'static,
    ) {
        let mut body = body(player);
        if let Some(id) = account_id {
            body["account_id"] = json!(id);
        }
        if let Some(id) = card_id {
            body["card_id"] = json!(id);
        }
        let bank = Arc::clone(self);
        self.call("account", &body, move |server, answer| {
            if ok(&answer)
                && let Some(account) = answer.get("account").filter(|account| account.is_object())
                && personal(account)
                && let Some(number) = account.get("number").and_then(Value::as_str)
            {
                bank.remember_account(player, number);
            }
            then(server, answer);
        });
    }

    /// Marks the owner's sales as seen, so the next join summary doesn't repeat ones shown live.
    pub fn sales_seen(self: &Arc<Self>, player: Uuid) {
        let mut body = body(player);
        body["seen"] = json!(true);
        self.call("sales", &body, |_, _| {});
    }

    /// Remembers that ores for `op` were handed out and tells the site; retried until it hears.
    pub fn delivered(self: &Arc<Self>, op: &str, player: Uuid, amount: i32) {
        let entry = Entry {
            op: Some(op.to_owned()),
            amount,
            ..Entry::new(DELIVERED, player)
        };
        self.journal.add(entry.clone());
        self.send_delivered(entry);
    }

    fn send_delivered(self: &Arc<Self>, entry: Entry) {
        let bank = Arc::clone(self);
        let body = json!({"op_id": entry.op});
        self.call("delivered", &body, move |_, answer| {
            // The site has the op marked delivered (or never had it): forget it only on a real
            // answer.
            if !is_offline(&answer) {
                bank.journal.remove(&entry);
            }
        });
    }

    /// Every 30 s and on start: finishes operations a crash or a site outage left half-done.
    fn retry(self: &Arc<Self>) {
        if !self.configured() {
            return;
        }
        for entry in self.journal.all() {
            if entry.kind == DELIVERED {
                self.send_delivered(entry);
            } else if entry.kind == DEPOSIT {
                atm::retry_deposit(self, entry);
            } else if entry.kind == VENDING {
                terminal::retry_vending(self, entry);
            }
        }
    }

    fn on_join(self: &Arc<Self>, player: &Player) {
        let uuid = player.gameprofile.id();
        // Goods and ores the bank still owes this player.
        for items in self.journal.take_owed(uuid) {
            for item in items {
                ores::give_stack(player, item);
            }
            player.send_message(&render(
                "&aБанк: выданы вещи, которые не дошли до тебя в прошлый раз.",
                &[],
            ));
        }
        self.account(uuid, None, None, |_, _| {});
        // Withdrawals paid for but not handed out (a crash right after the payment).
        let bank = Arc::clone(self);
        self.call("pending", &body(uuid), move |server, answer| {
            if !ok(&answer) {
                return;
            }
            let Some(player) = online(server, uuid) else {
                return;
            };
            for (id, amount) in pending_ops(&answer) {
                if bank.journal.has_op(DELIVERED, &id) {
                    continue;
                }
                ores::give(&player, amount);
                bank.delivered(&id, uuid, amount);
                player.send_message(&render(
                    &format!(
                        "&aБанк: выдано {} по прошлому снятию.",
                        ar(i64::from(amount))
                    ),
                    &[],
                ));
            }
        });
        if let Some(running) = self::bank() {
            zones::gui::remind(running, uuid);
        }
        self.call("sales", &body(uuid), move |server, answer| {
            if !ok(&answer) {
                return;
            }
            if let Some(line) = sales_line(&answer)
                && let Some(player) = online(server, uuid)
            {
                player.send_message(&render(&line, &[]));
            }
        });
    }

    fn on_leave(self: &Arc<Self>, player: Uuid) {
        self.leave_zones(player);
        // Sales while online were announced live; the join summary counts only the time away.
        if self.account_numbers.lock().remove(&player).is_some() {
            self.sales_seen(player);
        }
    }
}

/// The bank's part of the server tick: answers of the site, joins and leaves, retries.
struct Tick {
    bank: Arc<Bank>,
    online: BTreeSet<Uuid>,
}

impl ServerJob for Tick {
    fn poll(&mut self, context: &mut ServerJobContext) -> JobPoll {
        let Some(server) = context.server() else {
            return JobPoll::Finished;
        };
        let done = mem::take(&mut *self.bank.done.lock());
        for then in done {
            then(&server);
        }

        let players = server.get_players();
        let now: BTreeSet<Uuid> = players
            .iter()
            .map(|player| player.gameprofile.id())
            .collect();
        for player in &players {
            if !self.online.contains(&player.gameprofile.id()) {
                self.bank.on_join(player);
            }
        }
        for player in self.online.difference(&now) {
            self.bank.on_leave(*player);
        }
        self.online = now;

        if context.tick_count.is_multiple_of(LABEL_TICKS) {
            terminal::tick_labels(&self.bank, &server);
        }
        self.bank.tick_zones(&server, context.tick_count);
        if context.tick_count.is_multiple_of(RETRY_TICKS) {
            self.bank.retry();
        }
        JobPoll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_read_like_the_fabric_mod_reads_them() {
        assert!(ok(&json!({"ok": true})));
        assert!(!ok(&json!({"ok": false, "error": "frozen"})));
        assert!(!ok(&json!({})));
        assert_eq!(error(&json!({"ok": false, "error": "frozen"})), "frozen");
        assert_eq!(error(&json!({"ok": false, "error": null})), "unknown");
        assert!(is_offline(&offline()));
        assert!(!ok(&offline()));
        assert_eq!(error(&offline()), "offline");
        assert!(!is_offline(&json!({"ok": false, "error": "frozen"})));
    }

    #[test]
    fn an_account_is_personal_unless_the_site_says_otherwise() {
        assert!(personal(&json!({"number": "MH-1"})));
        assert!(personal(&json!({"personal": null})));
        assert!(personal(&json!({"personal": true})));
        assert!(!personal(&json!({"personal": false})));
    }

    #[test]
    fn undelivered_withdrawals_are_read_from_the_answer() {
        let answer = json!({"ok": true, "ops": [
            {"id": "a", "amount": 5},
            {"id": "b"},
            {"id": "c", "amount": 130},
        ]});
        assert_eq!(
            pending_ops(&answer),
            vec![("a".to_owned(), 5), ("c".to_owned(), 130)]
        );
        assert_eq!(pending_ops(&json!({"ok": true})), Vec::new());
    }

    #[test]
    fn sales_are_told_only_when_there_were_some() {
        assert_eq!(sales_line(&json!({"ok": true, "count": 0, "sum": 0})), None);
        assert_eq!(
            sales_line(&json!({"ok": true, "count": 3, "sum": 12})).as_deref(),
            Some("&6Пока тебя не было: продано 3 раз, +12 АР")
        );
    }

    #[test]
    fn the_body_of_a_call_names_the_player() {
        assert_eq!(
            body(Uuid::from_u128(1)).to_string(),
            r#"{"uuid":"00000000-0000-0000-0000-000000000001"}"#
        );
    }
}
