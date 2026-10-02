//! Login follow-up is an observation, never a credential-copy or a client kill.
use super::*;
use crate::usage::runtime::RpcResult;

fn terminal(status: LoginRunStatus) -> bool {
    matches!(
        status,
        LoginRunStatus::Cancelled
            | LoginRunStatus::Completed
            | LoginRunStatus::Failed
            | LoginRunStatus::TimedOut
    )
}

pub(crate) fn needs_cli_probe(run: &LoginRunRecord) -> bool {
    !terminal(run.status)
        && Utc::now() - run.started_at < ChronoDuration::minutes(15)
        && run.steps.iter().any(|s| {
            s.surface == LoginSurface::CopilotCli
                && matches!(
                    s.status,
                    LoginStepStatus::ActionRequired
                        | LoginStepStatus::Launched
                        | LoginStepStatus::Conflict
                        | LoginStepStatus::Verified
                )
        })
        && present_auth_environment().is_empty()
}

pub(crate) fn reconcile(
    mut run: LoginRunRecord,
    observation: Option<RpcResult<String>>,
    now: DateTime<Utc>,
) -> LoginRunRecord {
    if terminal(run.status) {
        return run;
    }
    if now - run.started_at >= ChronoDuration::minutes(15) {
        for step in &mut run.steps {
            if matches!(
                step.status,
                LoginStepStatus::ActionRequired
                    | LoginStepStatus::Launched
                    | LoginStepStatus::Conflict
                    | LoginStepStatus::Pending
            ) {
                step.status = LoginStepStatus::TimedOut;
                step.detail = "The verification window expired; check the official client and start a fresh reviewed flow".into();
            }
        }
        run.status = LoginRunStatus::TimedOut;
        run.summary = "Verification timed out. No client was signed out or terminated".into();
        run.finished_at = Some(now);
        return run;
    }
    for step in &mut run.steps {
        if step.surface != LoginSurface::CopilotCli
            || !matches!(
                step.status,
                LoginStepStatus::ActionRequired
                    | LoginStepStatus::Launched
                    | LoginStepStatus::Conflict
                    | LoginStepStatus::Verified
            )
        {
            continue;
        }
        match observation.as_ref() {
            Some(Ok(login)) => {
                run.observed_cli_login = Some(login.clone());
                if run.target_identity.as_ref().is_some_and(|target| {
                    target.host != "github.com" || !target.login.eq_ignore_ascii_case(login)
                }) {
                    step.status = LoginStepStatus::Conflict;
                    step.detail = "The official CLI runtime reports a different GitHub account. Resolve the account selection in the official client and verify again".into();
                } else {
                    step.status = LoginStepStatus::Verified;
                    step.detail = "Official protocol-3 auth.getStatus verified the current github.com CLI/runtime identity. This does not verify VS Code or the app".into();
                }
            }
            Some(Err(_)) => {
                run.observed_cli_login = None;
                step.status = LoginStepStatus::ActionRequired;
                step.detail = "Official CLI identity is unavailable or unsupported. Finish sign-in in the client and retry verification; no inferred identity was promoted".into();
            }
            None => {
                run.observed_cli_login = None;
                step.status = LoginStepStatus::ActionRequired;
                step.detail = "CLI account was not probed. Authentication overrides or an unavailable public interface require checking /user in the official client".into();
            }
        }
    }
    let verified = run
        .steps
        .iter()
        .filter(|s| s.status == LoginStepStatus::Verified)
        .count();
    run.status = if run
        .steps
        .iter()
        .any(|s| s.status == LoginStepStatus::Conflict)
    {
        LoginRunStatus::Conflict
    } else if verified > 0 && verified == run.requested_surfaces.len() {
        LoginRunStatus::Completed
    } else if verified > 0 {
        LoginRunStatus::Partial
    } else {
        LoginRunStatus::ActionRequired
    };
    run.summary = match run.status {
        LoginRunStatus::Completed => "The selected CLI/runtime account was verified through its official read-only interface",
        LoginRunStatus::Partial => "CLI identity was verified; other clients still require their own official account check",
        LoginRunStatus::Conflict => "A different CLI account was observed; no identity or credentials were synchronized",
        _ => "Client-specific account checks still require action; opening a flow is not sign-in success",
    }.into();
    run.finished_at = Some(now);
    run
}

pub(crate) fn cancel(mut run: LoginRunRecord, now: DateTime<Utc>) -> LoginRunRecord {
    if terminal(run.status) {
        return run;
    }
    for step in &mut run.steps {
        if !matches!(
            step.status,
            LoginStepStatus::Verified
                | LoginStepStatus::SkippedNotInstalled
                | LoginStepStatus::Unsupported
                | LoginStepStatus::Failed
        ) {
            step.status = LoginStepStatus::Cancelled;
            step.detail = "PilotWeave stopped following this run. Any already-open official sign-in window remains under your control".into();
        }
    }
    run.status = LoginRunStatus::Cancelled;
    run.summary =
        "Sign-in follow-up cancelled. No client was closed, signed out, or had credentials removed"
            .into();
    run.finished_at = Some(now);
    run
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run() -> LoginRunRecord {
        LoginRunRecord {
            id: Uuid::new_v4().to_string(),
            plan_id: Uuid::new_v4().to_string(),
            target_identity: Some(GithubIdentity {
                host: "github.com".into(),
                login: "target-user".into(),
                user_id: Some(42),
                avatar_url: None,
            }),
            observed_cli_login: None,
            requested_surfaces: vec![LoginSurface::CopilotCli],
            status: LoginRunStatus::ActionRequired,
            steps: vec![LoginStepResult {
                surface: LoginSurface::CopilotCli,
                status: LoginStepStatus::ActionRequired,
                detail: "Finish official flow".into(),
            }],
            summary: "Waiting".into(),
            started_at: Utc::now(),
            finished_at: None,
        }
    }
    #[test]
    fn verified_conflict_and_manual_surfaces_stay_distinct() {
        let now = Utc::now();
        assert_eq!(
            reconcile(run(), Some(Ok("TARGET-user".into())), now).status,
            LoginRunStatus::Completed
        );
        assert_eq!(
            reconcile(run(), Some(Ok("other-user".into())), now).status,
            LoginRunStatus::Conflict
        );
        assert_eq!(
            reconcile(run(), None, now).status,
            LoginRunStatus::ActionRequired
        );
        let mut mixed = run();
        mixed.requested_surfaces.push(LoginSurface::VsCodeCopilot);
        mixed.steps.push(LoginStepResult {
            surface: LoginSurface::VsCodeCopilot,
            status: LoginStepStatus::ActionRequired,
            detail: "Manual".into(),
        });
        let mixed = reconcile(mixed, Some(Ok("target-user".into())), now);
        assert_eq!(mixed.status, LoginRunStatus::Partial);
        assert_eq!(mixed.steps[1].status, LoginStepStatus::ActionRequired);
    }
    #[test]
    fn cancellation_cannot_be_reversed_by_a_late_probe_and_timeout_is_not_success() {
        let now = Utc::now();
        let cancelled = cancel(run(), now);
        assert_eq!(
            reconcile(cancelled, Some(Ok("target-user".into())), now).status,
            LoginRunStatus::Cancelled
        );
        let mut expired = run();
        expired.started_at = now - ChronoDuration::minutes(16);
        assert_eq!(
            reconcile(expired, Some(Ok("target-user".into())), now).status,
            LoginRunStatus::TimedOut
        );
    }
}
