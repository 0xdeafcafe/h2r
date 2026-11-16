//! `h2 live account code|recovery|recover` and `h2 live invite
//! create|list|revoke` (docs/live/live-security.md §1 Recovery,
//! docs/live/live-admin.md §5).
//!
//! - `account code` prints the current recovery code of a local account,
//!   for typing on a new device.
//! - `account recovery [--reset]` prints the authenticator key and its
//!   `otpauth://` URI (sets recovery up on an older account, or with
//!   `--reset` replaces the secret).
//! - `account recover <gamertag> [--code C]` makes a key here and adds it
//!   to the account with a current code, then keeps it in accounts.json.
//! - `invite create|list|revoke` as the operator (`--data`) or, with
//!   `--as`, as an admin account.

use std::io::{BufRead, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use live_proto::recovery::{RecoverBody, RecoverRequest, RecoveryInfo};
use live_proto::sign::{EventBody, KeyPair, issue_token, random_id};
use live_proto::{CreatedAccount, Gamertag, lxm};
use serde_json::{Value, json};

use crate::live_accounts::{self, LocalAccount};
use crate::live_cmd::{Args, Client, accounts_path, call, now, operator, pick_account, status_of};
use crate::live_query::path_part;

/// The recovery block printed after `account create` and `recovery`.
pub(crate) fn print_recovery(info: &RecoveryInfo) {
    let grouped: Vec<String> = info
        .secret
        .as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect();
    println!("recovery: on a new device, enter the gamertag and a current code");
    println!(
        "  code now:          {} (h2 live account code shows it again)",
        info.code
    );
    println!("  authenticator key: {}", grouped.join(" "));
    println!("  authenticator URI: {}", info.otpauth);
    println!("  add the key to an authenticator app so you can recover with no device left");
}

fn author_of(client: &Client, a: &LocalAccount) -> String {
    a.id.clone()
        .unwrap_or_else(|| live_accounts::author_id(a.xuid, &client.discovery.id))
}

/// The account's recovery info; `reset` makes a new secret first.
pub(crate) fn recovery_info(
    client: &Client,
    account: &LocalAccount,
    reset: bool,
) -> Result<RecoveryInfo> {
    let key = account.key_pair()?;
    let author = author_of(client, account);
    let path = client.url(&format!("/accounts/by-xuid/{}/recovery", account.xuid));
    let (method, req) = if reset {
        (lxm::RECOVERY_WRITE, client.agent.post(&path))
    } else {
        (lxm::RECOVERY_READ, client.agent.get(&path))
    };
    let token = issue_token(&key, &author, &client.discovery.id, method, now());
    let req = client.authorized(req, token)?;
    let text = call(req, reset.then_some(&b""[..]))?;
    Ok(serde_json::from_str(&text)?)
}

/// Adds a new key here to `gamertag`'s account with `code` and stores it.
pub(crate) fn recover(
    client: &Client,
    accounts: &Path,
    gamertag: &Gamertag,
    code: &str,
) -> Result<LocalAccount> {
    let key = KeyPair::generate();
    let body = RecoverBody {
        request: RecoverRequest::sign(&key, gamertag.as_str(), &client.discovery.id, now()),
        code: code.trim().to_string(),
    };
    let req = client
        .agent
        .post(&client.url(&format!(
            "/accounts/{}/recover",
            path_part(gamertag.as_str())
        )))
        .set("content-type", "application/json");
    let created: CreatedAccount = match call(req, Some(serde_json::to_string(&body)?.as_bytes())) {
        Ok(text) => serde_json::from_str(&text)?,
        Err(e) => {
            return Err(match status_of(&e) {
                Some(403) => e.context("the code is wrong or the account has no recovery"),
                Some(429) => e.context("too many wrong codes: wait, then try again"),
                Some(404) => e.context(format!("{gamertag} is not an account on {}", client.base)),
                _ => e,
            });
        }
    };
    let account = LocalAccount::new(
        created.xuid,
        created.gamertag.as_str(),
        &client.base,
        &client.discovery,
        &key,
    );
    let mut all = live_accounts::load(accounts)?;
    live_accounts::upsert(&mut all, account.clone());
    live_accounts::save(accounts, &all)?;
    Ok(account)
}

fn prompt(what: &str) -> Result<String> {
    print!("{what}: ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

pub(crate) fn account_code(mut args: Args) -> Result<()> {
    let server = args.take("--server")?;
    let as_tag = args.take("--as")?;
    args.positional()?;
    let client = Client::connect(server)?;
    let account = pick_account(&client, &accounts_path()?, as_tag.as_deref())?;
    let info = recovery_info(&client, &account, false).map_err(not_set_up)?;
    let left = (info.expires_at - now()).max(0);
    println!("{}  ({} for {left} s more)", info.code, account.gamertag);
    Ok(())
}

pub(crate) fn account_recovery(mut args: Args) -> Result<()> {
    let server = args.take("--server")?;
    let as_tag = args.take("--as")?;
    let reset = args.flag("--reset");
    args.positional()?;
    let client = Client::connect(server)?;
    let account = pick_account(&client, &accounts_path()?, as_tag.as_deref())?;
    let info = match recovery_info(&client, &account, reset) {
        Err(e) if !reset && status_of(&e) == Some(404) => {
            println!("{} has no recovery yet: setting it up", account.gamertag);
            recovery_info(&client, &account, true)?
        }
        r => r?,
    };
    if reset {
        println!(
            "new recovery secret for {}; the old one no longer works",
            account.gamertag
        );
    }
    print_recovery(&info);
    Ok(())
}

fn not_set_up(e: anyhow::Error) -> anyhow::Error {
    if status_of(&e) == Some(404) {
        e.context("this account has no recovery yet: run `h2 live account recovery`")
    } else {
        e
    }
}

pub(crate) fn account_recover(mut args: Args) -> Result<()> {
    let server = args.take("--server")?;
    let code = args.take("--code")?;
    let [gamertag] = args.positional()?.try_into().map_err(|_| {
        anyhow::anyhow!("account recover takes one gamertag (quote it if it has spaces)")
    })?;
    let tag = Gamertag::new(&gamertag)
        .with_context(|| format!("{gamertag:?} is not a valid gamertag"))?;
    let client = Client::connect(server)?;
    let code = match code {
        Some(c) => c,
        None => prompt(&format!(
            "current recovery code for {tag} (a signed-in device or your authenticator shows it)"
        ))?,
    };
    let path = accounts_path()?;
    let account = recover(&client, &path, &tag, &code)?;
    println!(
        "recovered {} ({}): a new key for this machine is in {}",
        account.gamertag,
        account.xuid,
        path.display()
    );
    Ok(())
}

/// Who signs invite requests: the operator (`--data`) or an admin account
/// (`--as`).
pub(crate) enum Signer {
    Operator(Box<live_server::ServerIdentity>),
    Admin(Box<LocalAccount>),
}

impl Signer {
    fn from_args(args: &mut Args, client: &Client) -> Result<Self> {
        let data = args.take("--data")?;
        let as_tag = args.take("--as")?;
        match (data, as_tag) {
            (Some(_), Some(_)) => {
                bail!("--data (the operator) or --as (an admin account), not both")
            }
            (None, Some(tag)) => Ok(Signer::Admin(Box::new(pick_account(
                client,
                &accounts_path()?,
                Some(&tag),
            )?))),
            (data, None) => Ok(Signer::Operator(Box::new(operator(data, client)?))),
        }
    }

    fn token(&self, client: &Client, method: &str) -> Result<String> {
        Ok(match self {
            Signer::Operator(me) => issue_token(&me.key, &me.author(), &me.id, method, now()),
            Signer::Admin(a) => issue_token(
                &a.key_pair()?,
                &author_of(client, a),
                &client.discovery.id,
                method,
                now(),
            ),
        })
    }

    /// A signed event on the invites stream (admin accounts only).
    fn event(&self, client: &Client, kind: &str, data: Value) -> Result<String> {
        let Signer::Admin(a) = self else {
            bail!("only an admin account signs invite events");
        };
        let key = a.key_pair()?;
        let author = author_of(client, a);
        Ok(EventBody::at_unix(&key, &author, "invites", kind, data, now()).sign(&key))
    }
}

/// `POST /v1/invites`: the reply `{invite, jti, exp, custom}`.
pub(crate) fn invite_create_with(
    client: &Client,
    signer: &Signer,
    days: i64,
    code: Option<&str>,
) -> Result<Value> {
    let token = signer.token(client, lxm::INVITES_WRITE)?;
    let req = client.agent.post(&client.url("/invites"));
    let text = match signer {
        Signer::Operator(_) => {
            let mut req = req.query("days", &days.to_string());
            if let Some(c) = code {
                req = req.query("code", c);
            }
            call(client.authorized(req, token)?, Some(b""))?
        }
        Signer::Admin(_) => {
            let exp = now() + days.clamp(1, 30) * 86_400;
            let event = signer.event(
                client,
                "InviteCreated",
                json!({"jti": random_id(), "exp": exp}),
            )?;
            let body = match code {
                Some(c) => json!({"event": event, "code": c}).to_string(),
                None => event,
            };
            call(client.authorized(req, token)?, Some(body.as_bytes()))?
        }
    };
    Ok(serde_json::from_str(&text)?)
}

pub(crate) fn invite_create(mut args: Args) -> Result<()> {
    let days: i64 = args
        .take("--days")?
        .map_or(Ok(7), |d| d.parse())
        .context("--days must be a whole number")?;
    let code = args.take("--code")?;
    let server = args.take("--server")?;
    let client = Client::connect(server)?;
    let signer = Signer::from_args(&mut args, &client)?;
    args.positional()?;
    let reply = invite_create_with(&client, &signer, days, code.as_deref())?;
    let invite = reply["invite"]
        .as_str()
        .context("the server's reply has no invite")?;
    println!("{invite}");
    Ok(())
}

pub(crate) fn invite_list(mut args: Args) -> Result<()> {
    let server = args.take("--server")?;
    let client = Client::connect(server)?;
    let signer = Signer::from_args(&mut args, &client)?;
    args.positional()?;
    let token = signer.token(&client, live_server::invites::LXM_ADMIN_READ)?;
    let req = client.authorized(client.agent.get(&client.url("/invites")), token)?;
    let v: Value = serde_json::from_str(&call(req, None)?)?;
    let items = v["items"].as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        println!("no invites");
    }
    for i in items {
        let code = match (i["invite"].as_str(), i["custom"].as_bool()) {
            (Some(c), _) => c.to_string(),
            (None, Some(true)) => "(custom code)".into(),
            _ => "-".into(),
        };
        println!(
            "{:<8} {:<30} {} by {}{}",
            i["state"].as_str().unwrap_or("?"),
            code,
            i["jti"].as_str().unwrap_or("?"),
            i["by_gamertag"]
                .as_str()
                .or(i["by"].as_str())
                .unwrap_or("?"),
            i["used_by_gamertag"]
                .as_str()
                .map(|t| format!(", used by {t}"))
                .unwrap_or_default(),
        );
    }
    Ok(())
}

pub(crate) fn invite_revoke(mut args: Args) -> Result<()> {
    let server = args.take("--server")?;
    let client = Client::connect(server)?;
    let signer = Signer::from_args(&mut args, &client)?;
    let [jti] = args.positional()?.try_into().map_err(|_| {
        anyhow::anyhow!("invite revoke takes the invite's jti (h2 live invite list)")
    })?;
    if matches!(signer, Signer::Operator(_)) {
        bail!("revoking is signed by an admin account: pass --as <gamertag>");
    }
    let token = signer.token(&client, lxm::INVITES_WRITE)?;
    let event = signer.event(&client, "InviteRevoked", json!({"jti": jti}))?;
    let req = client.authorized(
        client
            .agent
            .post(&client.url(&format!("/invites/{}/revoke", path_part(&jti)))),
        token,
    )?;
    call(req, Some(event.as_bytes()))?;
    println!("revoked {jti}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use live_proto::recovery::TotpSecret;
    use std::fs;

    fn server(dir: &Path) -> live_server::Embedded {
        live_server::start_embedded(
            live_server::MEMORY_DB,
            "127.0.0.1:0".parse().unwrap(),
            &dir.join("server"),
            Default::default(),
        )
        .unwrap()
    }

    #[test]
    fn create_show_code_and_recover_on_another_machine() {
        let dir = std::env::temp_dir().join(format!("h2-live-recovery-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let server = server(&dir);
        let client = Client::at(&server.url).unwrap();
        let (created, key) = client.create_account("Keeper", None).unwrap();
        let info = created
            .recovery
            .clone()
            .expect("a new account has recovery");
        let here = LocalAccount::new(created.xuid, "Keeper", &server.url, &client.discovery, &key);

        // A signed-in machine reads the same secret.
        let read = recovery_info(&client, &here, false).unwrap();
        assert_eq!(read.secret, info.secret);

        // Another machine: a code from the secret, a new key there.
        let elsewhere = dir.join("elsewhere.json");
        let code = TotpSecret::from_base32(&info.secret)
            .unwrap()
            .code_at(now());
        let tag = Gamertag::new("keeper").unwrap();
        let got = recover(&client, &elsewhere, &tag, &format!("{code:06}")).unwrap();
        assert_eq!((got.xuid, got.gamertag.as_str()), (created.xuid, "Keeper"));
        let stored = live_accounts::load(&elsewhere).unwrap();
        assert_eq!(stored.len(), 1);
        assert_ne!(stored[0].key_pair().unwrap().kid(), key.kid());
        // The recovered key works: it reads the recovery info too.
        assert_eq!(
            recovery_info(&client, &stored[0], false).unwrap().secret,
            info.secret
        );

        // Reset: a new secret.
        let fresh = recovery_info(&client, &stored[0], true).unwrap();
        assert_ne!(fresh.secret, info.secret);

        // A wrong code is a clear error and stores nothing.
        let other = dir.join("other.json");
        let wrong = if code == 0 { "111111" } else { "000000" };
        let err = recover(&client, &other, &tag, wrong).unwrap_err();
        assert_eq!(status_of(&err), Some(403));
        assert!(!other.exists());
        drop(server);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn operator_invites_generated_and_custom() {
        let dir = std::env::temp_dir().join(format!("h2-live-invites-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let server = server(&dir);
        let client = Client::at(&server.url).unwrap();
        let op = Signer::Operator(Box::new(
            operator(Some(dir.join("server").display().to_string()), &client).unwrap(),
        ));
        let made = invite_create_with(&client, &op, 7, None).unwrap();
        let code = made["invite"].as_str().unwrap();
        assert_eq!((code.len(), made["custom"].as_bool()), (29, Some(false)));
        let made = invite_create_with(&client, &op, 3, Some("Friday Night")).unwrap();
        assert_eq!(
            (made["invite"].as_str(), made["custom"].as_bool()),
            (Some("Friday Night"), Some(true))
        );
        // Only an admin account signs a revoke.
        assert!(op.event(&client, "InviteRevoked", json!({})).is_err());
        drop(server);
        fs::remove_dir_all(&dir).unwrap();
    }
}
