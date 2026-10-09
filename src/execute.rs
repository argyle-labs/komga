//! Admin check for this plugin's mutating tools.
//!
//! Mutating tools set `execute_gated = false` and own their `execute` flag, so
//! the dry run can report the exact drift. Opting out of the central gate also
//! opts out of the role check orca runs inside it, so [`require_admin`]
//! replaces it, on the dry run too.

use std::collections::BTreeSet;

use plugin_toolkit::contract::CallerIdentity;
use plugin_toolkit::prelude::*;

pub fn require_admin(tool: &str, ctx: &ToolCtx) -> Result<()> {
    check_admin(tool, ctx.caller().as_ref())
}

/// Split the operator's confirmed ids against the current plan:
/// `(act_on, dropped)`. Confirmed ids no longer in the plan are reported,
/// never acted on.
pub fn intersect(confirmed: &[String], current: &[String]) -> (Vec<String>, Vec<String>) {
    let now: BTreeSet<&str> = current.iter().map(String::as_str).collect();
    let mut seen = BTreeSet::new();
    let mut act = Vec::new();
    let mut dropped = Vec::new();
    for item in confirmed {
        if !seen.insert(item.as_str()) {
            continue;
        }
        if now.contains(item.as_str()) {
            act.push(item.clone());
        } else {
            dropped.push(item.clone());
        }
    }
    (act, dropped)
}

fn check_admin(subject: &str, caller: Option<&CallerIdentity>) -> Result<()> {
    match caller {
        Some(c) if c.role == "admin" => Ok(()),
        Some(c) => bail!(
            "{subject} requires role 'admin'; caller '{}' has '{}'",
            c.username,
            c.role
        ),
        None => bail!(
            "{subject} refused: the call carries no caller identity, so admin cannot be verified"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller(role: &str) -> CallerIdentity {
        CallerIdentity {
            user_id: "u".into(),
            username: "op".into(),
            role: role.into(),
            can_mutate: true,
        }
    }

    #[test]
    fn needs_an_admin_caller() {
        assert!(check_admin("t", Some(&caller("admin"))).is_ok());
        let err = check_admin("t", Some(&caller("user"))).unwrap_err();
        assert!(err.to_string().contains("requires role 'admin'"), "{err}");
        let err = check_admin("t", None).unwrap_err();
        assert!(err.to_string().contains("no caller identity"), "{err}");
    }

    #[test]
    fn intersect_acts_only_on_confirmed_ids_still_planned() {
        let confirmed = vec!["a".to_string(), "b".to_string(), "a".to_string()];
        let current = vec!["b".to_string(), "c".to_string()];
        let (act, dropped) = intersect(&confirmed, &current);
        assert_eq!(act, vec!["b"]);
        assert_eq!(dropped, vec!["a"]);
    }
}
