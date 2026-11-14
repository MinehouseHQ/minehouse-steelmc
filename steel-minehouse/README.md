# steel-minehouse

MineHouse features for our Steel fork: the survival mods from the Fabric server
(`P5ina/minehouse-landing`, `P5ina/minehouse-mods`), ported one by one. Everything
MineHouse-specific lives here; `steel-core` only gets small hooks.

Configs are in `config/minehouse/<module>.toml`, created with defaults on first start.

## Console

Port of `console-mod` (remote console without open ports). Polls
`POST /api/mc/console/poll` on the site, runs the queued commands as RCON and reports their
output, new log lines and the server state.

`config/minehouse/console.toml` has the fields of the Fabric `minehouse-console.json` in
snake_case. Environment overrides: `MINEHOUSE_CONSOLE_SECRET` (the site's
`MC_CONSOLE_SECRET_<ID>`), `MINEHOUSE_SERVER_ID`, `MINEHOUSE_SERVER_NAME`, `MINEHOUSE_API_URL`.
Without a secret the console stays off.

## Auth

Port of the login check of `mod/` (MineHouse Auth). A verified login waits in the
configuration phase while the site answers `POST /api/mc/check` for its nickname and IP:

- a trusted device, or a licensed login Gate verified with Mojang, goes straight in;
- a banned account, one outside Discord or one missing from this server's whitelist is
  disconnected with the site's text;
- anyone else sees the «Вход на MineHouse» dialog with a code and gets in once the site says
  the code was confirmed (`GET /api/mc/request/{id}`); «Отмена» or leaving cancels the request.

While the site is down, nickname + IP pairs confirmed in the last `offline_trust_days` still
get in (`auth-trusted.json` in `MINEHOUSE_DATA_DIR`, default `config/minehouse/data`). A
login is refused while the nickname is already playing, so nobody can kick a player by
joining under their name.

Steel has to sit behind Gate: Gate checks licensed nicknames and hands out the stable UUID
from the site, this module only reads the result from the forwarded profile.

`config/minehouse/auth.toml` has the fields of the Fabric `minehouse-auth.json` in
snake_case. Environment overrides: `MINEHOUSE_API_SECRET` (the site's `MC_API_SECRET`),
`MINEHOUSE_API_URL`, `MINEHOUSE_AUTH_SERVER_ID` (falls back to `MINEHOUSE_SERVER_ID`).
Without a secret only players from the local cache get in. `enabled = false` turns the check
off.

In `steel-core` this is the `LoginGate` hook (`server/login_gate.rs`); `steel-login` holds the
client, keeps it alive and refuses a client that finishes the configuration on its own.

Not ported yet: ban, warn and whitelist commands, kicking players banned while online, Discord
roles, feature flags, Twitch.
