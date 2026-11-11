//! The keys a start of an agent is given: those the credential store keeps
//! for a method its row still offers as the start is made.

use std::path::Path;

use crucible_external_harnesses::Request;
use crucible_external_harnesses::catalog::Method;
use crucible_types::AgentKey;

use crate::backends::{AgentRow, Started};
use crate::providers;

/// `request` with each key the credential store in `home` keeps for `row`'s
/// agent set in the variable of the method it was typed for, where the agent
/// still signs in by that method as it is `started`, and that variable taken
/// off the ones whose value may be shown, so its value is withheld from what
/// the agent says.
pub(crate) fn keyed(
    mut request: Request,
    row: &AgentRow,
    home: &Path,
    started: Started,
) -> Request {
    let methods = row.methods(started);
    let variables: Vec<(&str, &str)> = methods
        .iter()
        .filter_map(|method| match &method.method {
            Method::EnvVar { var } => Some((&*method.id, &**var)),
            Method::Agent | Method::Terminal { .. } => None,
        })
        .collect();
    if variables.is_empty() {
        return request;
    }
    let Ok(agent) = AgentKey::new(row.named()) else {
        return request;
    };
    let rows = providers::Rows::production();
    let stored = crucible_auth::Store::in_home(home)
        .naming(rows.names())
        .read();
    for (id, var) in variables {
        let Ok(name) = rows.signing_in(&agent, id) else {
            continue;
        };
        if let Some(key) = stored.get(&name) {
            request.shown.retain(|shown| {
                if cfg!(windows) {
                    !shown.eq_ignore_ascii_case(var)
                } else {
                    *shown != var
                }
            });
            key.set_in(&mut request.environment, var);
        }
    }
    request
}
