//! Operator commands: grant-comp, set-entitlement, revoke-sessions, migrate.

use std::sync::Arc;

use crate::accounts;
use crate::{db, AppState, BoxError, Config};

pub async fn run(args: &[String], cfg: &Config) -> Result<(), BoxError> {
    let cmd = args.get(1).map(String::as_str).unwrap_or("");
    if !matches!(cmd, "grant-comp" | "set-entitlement" | "revoke-sessions" | "migrate") {
        return Err(format!("unknown command {cmd}").into());
    }
    if let Some(parent) = cfg.db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let state = Arc::new(AppState::with_url(
        &cfg.db_path,
        cfg.database_url.as_deref(),
        cfg.master_key.clone(),
    ));
    db::ensure_schema(&state.any).await?;

    match cmd {
        "grant-comp" => {
            let email = args.get(2).ok_or("usage: agent-platformd grant-comp <email> [--reason R] [--expires T]")?;
            let reason = flag(args, "--reason").unwrap_or_else(|| "cli".into());
            let expires = flag(args, "--expires");
            let email = accounts::normalize_email(email).map_err(|e| e.message)?;
            let row = accounts::grant_comp(&state, &email, &reason, expires.as_deref())
                .await
                .map_err(|e| e.message)?;
            logd!("granted comp to {} ({})", row.email, row.entitlement);
        }
        "set-entitlement" => {
            let email = args.get(2).ok_or("usage: agent-platformd set-entitlement <email> <trial|paid|comp|blocked> [--card US] [--billing BD]")?;
            let ent = args.get(3).ok_or("missing entitlement")?;
            let card = flag(args, "--card");
            let billing = flag(args, "--billing");
            let email = accounts::normalize_email(email).map_err(|e| e.message)?;
            let row = accounts::set_entitlement(&state, &email, ent, card.as_deref(), billing.as_deref())
                .await
                .map_err(|e| e.message)?;
            logd!(
                "{} is {} region={}",
                row.email,
                row.entitlement,
                row.billing_region.as_deref().unwrap_or("-")
            );
        }
        "revoke-sessions" => {
            let email = args.get(2).ok_or("usage: agent-platformd revoke-sessions <email>")?;
            let email = accounts::normalize_email(email).map_err(|e| e.message)?;
            let n = accounts::revoke_sessions(&state, &email).await.map_err(|e| e.message)?;
            logd!("revoked {n} session(s) for {email}");
        }
        "migrate" => {
            logd!(
                "schema is at head ({})",
                if state.backend == db::Backend::Postgres {
                    "postgres"
                } else {
                    "sqlite"
                }
            );
        }
        _ => unreachable!(),
    }
    Ok(())
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
}
