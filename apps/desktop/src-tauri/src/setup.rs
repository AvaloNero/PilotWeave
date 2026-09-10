//! Setup progress is derived from current observations, never a saved success Boolean.
use crate::{
    account::{
        AccountObservationState, AccountStatusSnapshot, GithubIdentity, SurfaceAccountObservation,
    },
    commands::{self, ManagedState},
    deployment,
    domain::{ClientTarget, Connection, DeploymentRecord, DeploymentStatus},
    error::AppResult,
    fingerprint,
    usage::{
        commands::{error, with_db},
        db_error, invalid, store,
    },
    usage_db::UsageDb,
};
use chrono::{DateTime, Utc};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use tauri::State;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    pub connection_id: Option<String>,
    pub account_confirmation: Option<AccountConfirmation>,
    pub manual_connection_revision: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountConfirmation {
    pub target: GithubIdentity,
    pub confirmed_at: DateTime<Utc>,
    pub evidence_fingerprint: String,
    pub surfaces: Vec<SurfaceAccountObservation>,
}
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Alignment {
    VerifiedSameAccount,
    UserConfirmedSameAccount,
    PartiallyVerified,
    Conflict,
    NotSignedIn,
}
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Drift {
    InSync,
    NotDeployed,
    OutOfDate,
    Manual,
    Conflict,
    Unknown,
    NotInstalled,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetState {
    pub target_id: String,
    pub state: Drift,
    pub detail: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupStatus {
    pub preferences: Preferences,
    pub target: Option<GithubIdentity>,
    pub alignment: Alignment,
    pub evidence_fingerprint: String,
    pub accounts: AccountStatusSnapshot,
    pub targets: Vec<TargetState>,
    pub manual_confirmed: bool,
    pub scanned_at: DateTime<Utc>,
    pub preferences_error: Option<String>,
}

fn load(db: &UsageDb) -> AppResult<Preferences> {
    db.conn
        .query_row(
            "SELECT payload FROM setup_preferences WHERE id=1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error)?
        .map(store::decode)
        .transpose()
        .map(|p| p.unwrap_or_default())
}
fn save(db: &UsageDb, p: &Preferences) -> AppResult<()> {
    db.conn.execute("INSERT INTO setup_preferences VALUES (1,?1) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload",[store::encode(p)?]).map_err(db_error)?;
    Ok(())
}

pub fn evidence(accounts: &AccountStatusSnapshot) -> AppResult<String> {
    let surfaces: Vec<_> = accounts
        .surfaces
        .iter()
        .map(|s| (&s.surface, &s.state, &s.identity, &s.evidence, &s.detail))
        .collect();
    fingerprint::json(
        "account-confirmation-v1",
        &(
            surfaces,
            &accounts.anchor.state,
            &accounts.anchor.identity,
            accounts.login_runs.first().map(|r| &r.id),
        ),
    )
}
fn same(a: &GithubIdentity, b: &GithubIdentity) -> bool {
    a.host.eq_ignore_ascii_case(&b.host)
        && a.login.eq_ignore_ascii_case(&b.login)
        && a.user_id.zip(b.user_id).is_none_or(|(a, b)| a == b)
}
pub fn alignment(
    accounts: &AccountStatusSnapshot,
    target: Option<&GithubIdentity>,
    saved: Option<&AccountConfirmation>,
    digest: &str,
) -> Alignment {
    let Some(target) = target else {
        return Alignment::NotSignedIn;
    };
    if accounts.surfaces.iter().any(|s| {
        s.state == AccountObservationState::Conflict
            || s.identity
                .as_ref()
                .is_some_and(|id| s.state == AccountObservationState::Verified && !same(id, target))
    }) {
        return Alignment::Conflict;
    }
    if accounts.surfaces.len() == 3
        && accounts.surfaces.iter().all(|s| {
            s.state == AccountObservationState::Verified
                && s.identity.as_ref().is_some_and(|id| same(id, target))
        })
    {
        return Alignment::VerifiedSameAccount;
    }
    if saved.is_some_and(|s| {
        s.evidence_fingerprint == digest
            && same(&s.target, target)
            && s.confirmed_at <= Utc::now()
            && Utc::now() - s.confirmed_at < chrono::Duration::days(30)
    }) && accounts.surfaces.len() == 3
        && accounts
            .surfaces
            .iter()
            .all(|s| s.state != AccountObservationState::NotInstalled)
    {
        return Alignment::UserConfirmedSameAccount;
    }
    Alignment::PartiallyVerified
}

pub fn drift(
    target: &ClientTarget,
    connection: Option<&Connection>,
    records: &[DeploymentRecord],
    live: AppResult<String>,
) -> TargetState {
    let (state, detail) = if !target.detected {
        (Drift::NotInstalled, "Client is not installed")
    } else if !target.supports_write {
        (Drift::Manual, "Complete the provider setup in the app")
    } else if let Some(connection) = connection {
        match records
            .iter()
            .filter(|r| r.target_id == target.id)
            .max_by_key(|r| r.created_at)
        {
            None => (Drift::NotDeployed, "No deployment has been recorded"),
            Some(r) if r.status != DeploymentStatus::Applied => {
                (Drift::Conflict, "The latest deployment needs review")
            }
            Some(r)
                if r.connection_id != connection.id
                    || r.connection_revision.as_ref()
                        != fingerprint::json("setup-connection-v1", connection)
                            .ok()
                            .as_ref() =>
            {
                (
                    Drift::OutOfDate,
                    "Selected Connection differs from the last verified deployment",
                )
            }
            Some(r) => match live {
                Ok(current) if r.target_fingerprint.as_ref() == Some(&current) => (
                    Drift::InSync,
                    "Live configuration matches the recorded deployment",
                ),
                Ok(_) => (
                    Drift::Conflict,
                    "Configuration changed externally; review before deployment",
                ),
                Err(_) => (
                    Drift::Unknown,
                    "Live target fingerprint could not be verified",
                ),
            },
        }
    } else {
        (
            Drift::NotDeployed,
            "Select a Connection to configure this client",
        )
    };
    TargetState {
        target_id: target.id.clone(),
        state,
        detail: detail.into(),
    }
}

#[tauri::command]
pub async fn get_setup_status(state: State<'_, ManagedState>) -> Result<SetupStatus, String> {
    let accounts = commands::get_account_status(state.clone()).await?;
    build(&state, accounts)
}
fn build(state: &ManagedState, accounts: AccountStatusSnapshot) -> Result<SetupStatus, String> {
    let (mut preferences, preferences_error) = match with_db(state, load) {
        Ok(p) => (p, None),
        Err(error) => (Preferences::default(), Some(error)),
    };
    let (connections, records) = {
        let s = state.store().map_err(error)?;
        (s.connections().to_vec(), s.deployments().to_vec())
    };
    if !connections
        .iter()
        .any(|c| Some(&c.id) == preferences.connection_id.as_ref())
    {
        preferences.connection_id = connections.first().map(|c| c.id.clone());
    }
    let selected = connections
        .iter()
        .find(|c| Some(&c.id) == preferences.connection_id.as_ref());
    let digest = evidence(&accounts).map_err(error)?;
    let authorized = state.github_authorization().map_err(error)?.status();
    let target = accounts
        .anchor
        .identity
        .clone()
        .filter(|_| accounts.anchor.state == AccountObservationState::Verified)
        .or_else(|| {
            authorized
                .identity
                .filter(|_| {
                    authorized.state == crate::github_auth::GithubAuthorizationState::Verified
                })
                .map(|i| GithubIdentity {
                    host: i.host,
                    login: i.login,
                    user_id: Some(i.user_id),
                    avatar_url: i.avatar_url,
                })
        })
        .or_else(|| {
            preferences
                .account_confirmation
                .as_ref()
                .map(|c| c.target.clone())
        });
    let alignment = alignment(
        &accounts,
        target.as_ref(),
        preferences.account_confirmation.as_ref(),
        &digest,
    );
    let targets = crate::adapters::discover_all()
        .iter()
        .map(|t| drift(t, selected, &records, deployment::fingerprint_target(t)))
        .collect();
    let manual_confirmed = selected
        .and_then(|c| fingerprint::json("setup-connection-v1", c).ok())
        .is_some_and(|r| preferences.manual_connection_revision.as_ref() == Some(&r));
    Ok(SetupStatus {
        preferences,
        target,
        alignment,
        evidence_fingerprint: digest,
        accounts,
        targets,
        manual_confirmed,
        scanned_at: Utc::now(),
        preferences_error,
    })
}

#[tauri::command(rename_all = "camelCase")]
pub fn select_setup_connection(
    state: State<'_, ManagedState>,
    connection_id: String,
) -> Result<(), String> {
    state
        .store()
        .and_then(|s| s.connection(&connection_id))
        .map_err(error)?;
    with_db(&state, |db| {
        let mut p = load(db)?;
        p.connection_id = Some(connection_id);
        save(db, &p)
    })
}
#[tauri::command(rename_all = "camelCase")]
pub fn confirm_manual_provider(
    state: State<'_, ManagedState>,
    connection_id: String,
    confirmed: bool,
) -> Result<(), String> {
    if !confirmed {
        return Err("Confirm completing the app's manual provider setup".into());
    }
    let revision = {
        let s = state.store().map_err(error)?;
        fingerprint::json(
            "setup-connection-v1",
            &s.connection(&connection_id).map_err(error)?,
        )
        .map_err(error)?
    };
    with_db(&state, |db| {
        let mut p = load(db)?;
        p.manual_connection_revision = Some(revision);
        save(db, &p)
    })
}
#[tauri::command(rename_all = "camelCase")]
pub async fn confirm_account_alignment(
    state: State<'_, ManagedState>,
    login: String,
    evidence_fingerprint: String,
    confirmed: bool,
) -> Result<(), String> {
    if !confirmed
        || login.is_empty()
        || login.len() > 39
        || !login
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err("Confirm a valid github.com account login".into());
    }
    // The public user endpoint establishes a stable account ID. The claim that
    // clients display it remains explicitly UserConfirmed, never Verified.
    let identity = tauri::async_runtime::spawn_blocking(move || resolve_public_identity(&login))
        .await
        .map_err(|_| "Account lookup failed".to_string())?
        .map_err(error)?;
    let accounts = commands::get_account_status(state.clone()).await?;
    let digest = evidence(&accounts).map_err(error)?;
    if digest != evidence_fingerprint {
        return Err(
            "Account observations changed. Rescan and confirm the current evidence.".into(),
        );
    }
    if accounts.surfaces.len() != 3
        || accounts
            .surfaces
            .iter()
            .any(|s| s.state == AccountObservationState::NotInstalled)
        || accounts.anchor.identity.as_ref().is_some_and(|a| {
            accounts.anchor.state == AccountObservationState::Verified && !same(a, &identity)
        })
        || alignment(&accounts, Some(&identity), None, &digest) == Alignment::Conflict
    {
        return Err(
            "Resolve missing clients or conflicting verified accounts before confirmation".into(),
        );
    }
    with_db(&state, |db| {
        let mut p = load(db)?;
        p.account_confirmation = Some(AccountConfirmation {
            target: identity,
            confirmed_at: Utc::now(),
            evidence_fingerprint: digest,
            surfaces: accounts.surfaces,
        });
        save(db, &p)
    })
}
fn resolve_public_identity(login: &str) -> AppResult<GithubIdentity> {
    let agent: ureq::Agent = crate::platform::http_config()
        .timeout_global(Some(std::time::Duration::from_secs(15)))
        .max_redirects(0)
        .user_agent("PilotWeave/0.1")
        .build()
        .into();
    let mut response = agent
        .get(crate::platform::endpoint(&format!(
            "https://api.github.com/users/{login}"
        )))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2026-03-10")
        .call()
        .map_err(|_| invalid("Public GitHub account lookup is unavailable"))?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(64 * 1024)
        .read_to_vec()
        .map_err(|_| invalid("Public account response is oversized"))?;
    #[derive(Deserialize)]
    struct PublicUser {
        login: String,
        id: u64,
    }
    let user: PublicUser =
        serde_json::from_slice(&bytes).map_err(|_| invalid("Unsupported public account schema"))?;
    if !user.login.eq_ignore_ascii_case(login) || user.id == 0 {
        return Err(invalid(
            "Public account identity differs from the selected login",
        ));
    }
    Ok(GithubIdentity {
        host: "github.com".into(),
        login: user.login,
        user_id: Some(user.id),
        avatar_url: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn accounts() -> AccountStatusSnapshot {
        let surfaces=["vsCodeCopilot","copilotCli","githubCopilotApp"].map(|surface|json!({"surface":surface,"state":"actionRequired","identity":null,"evidence":"Official UI confirmation needed","detail":"No safe identity probe","observedAt":Utc::now()}));
        serde_json::from_value(json!({"anchor":{"state":"verified","identity":{"host":"github.com","login":"fixture","userId":1},"evidence":"API identity","detail":"Verified anchor only","observedAt":Utc::now()},
            "surfaces":surfaces,"observedAt":Utc::now(),"loginRuns":[],"historyRecovery":null})).unwrap()
    }
    #[test]
    fn manual_confirmation_is_distinct_expires_and_is_invalidated_by_evidence() {
        let mut a = accounts();
        let target = a.anchor.identity.clone().unwrap();
        let digest = evidence(&a).unwrap();
        let mut saved = AccountConfirmation {
            target: target.clone(),
            confirmed_at: Utc::now(),
            evidence_fingerprint: digest.clone(),
            surfaces: a.surfaces.clone(),
        };
        assert_eq!(
            alignment(&a, Some(&target), None, &digest),
            Alignment::PartiallyVerified
        );
        assert_eq!(
            alignment(&a, Some(&target), Some(&saved), &digest),
            Alignment::UserConfirmedSameAccount
        );
        a.observed_at = Utc::now();
        a.surfaces[0].observed_at = Utc::now();
        assert_eq!(evidence(&a).unwrap(), digest);
        saved.confirmed_at = Utc::now() - chrono::Duration::days(31);
        assert_eq!(
            alignment(&a, Some(&target), Some(&saved), &digest),
            Alignment::PartiallyVerified
        );
        saved.confirmed_at = Utc::now();
        a.surfaces[0].state = AccountObservationState::NotInstalled;
        assert_eq!(
            alignment(&a, Some(&target), Some(&saved), &evidence(&a).unwrap()),
            Alignment::PartiallyVerified
        );
        for s in &mut a.surfaces {
            s.state = AccountObservationState::Verified;
            s.identity = Some(target.clone());
        }
        assert_eq!(
            alignment(&a, Some(&target), None, &digest),
            Alignment::VerifiedSameAccount
        );
        a.surfaces[0].identity.as_mut().unwrap().user_id = Some(2);
        assert_eq!(
            alignment(&a, Some(&target), Some(&saved), &digest),
            Alignment::Conflict
        );
    }
    #[test]
    fn setup_preferences_reopen_without_turning_confirmation_into_automatic_verification() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite3");
        let db = UsageDb::open_at(&path).unwrap();
        let a = accounts();
        let mut p = Preferences {
            connection_id: Some("connection".into()),
            ..Preferences::default()
        };
        p.account_confirmation = Some(AccountConfirmation {
            target: a.anchor.identity.clone().unwrap(),
            confirmed_at: Utc::now(),
            evidence_fingerprint: evidence(&a).unwrap(),
            surfaces: a.surfaces.clone(),
        });
        save(&db, &p).unwrap();
        drop(db);
        let loaded = load(&UsageDb::open_at(&path).unwrap()).unwrap();
        assert_eq!(loaded.connection_id, p.connection_id);
        assert_eq!(
            loaded.account_confirmation.unwrap().surfaces[0].state,
            AccountObservationState::ActionRequired
        );
    }
    #[test]
    fn live_fingerprint_and_connection_revision_are_both_required_for_in_sync() {
        let target:ClientTarget=serde_json::from_value(json!({"id":"target","kind":"vs-code-copilot","name":"Fixture target","detail":"Temp fixture","path":null,"detected":true,"supportsWrite":true,"status":"available","diagnostic":null})).unwrap();
        let c:Connection=serde_json::from_value(json!({"id":"connection","name":"Fixture","baseUrl":"https://example.invalid","secretRef":"fixture","createdAt":Utc::now(),"updatedAt":Utc::now()})).unwrap();
        let mut r:DeploymentRecord=serde_json::from_value(json!({"id":"record","planId":"plan","connectionId":c.id,"targetId":target.id,"targetKind":"vs-code-copilot","status":"applied","detail":"applied","createdAt":Utc::now()})).unwrap();
        assert_eq!(
            drift(&target, Some(&c), &[r.clone()], Ok("hash".into())).state,
            Drift::OutOfDate
        );
        r.target_fingerprint = Some("hash".into());
        r.connection_revision = Some(fingerprint::json("setup-connection-v1", &c).unwrap());
        assert_eq!(
            drift(&target, Some(&c), &[r.clone()], Ok("hash".into())).state,
            Drift::InSync
        );
        assert_eq!(
            drift(&target, Some(&c), &[r.clone()], Ok("changed".into())).state,
            Drift::Conflict
        );
        let mut changed = c.clone();
        changed.name = "Edited".into();
        assert_eq!(
            drift(&target, Some(&changed), &[r], Ok("hash".into())).state,
            Drift::OutOfDate
        );
    }
}
