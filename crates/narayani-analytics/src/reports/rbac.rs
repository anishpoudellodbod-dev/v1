use narayani_auth::{is_super_admin_role, Claims};
use narayani_core::{normalize_key, DatabaseResource, Roles};
use narayani_storage::{ProjectedReceipt, ReportIndex};
use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct CallerBranchContext {
    pub is_admin: bool,
    pub assigned_branch_id: Option<String>,
    pub assigned_branch_name: Option<String>,
    pub assigned_branch_lower: Option<String>,
}

pub fn resolve_caller_branch_context(
    claims: Option<&Claims>,
    users: &[DatabaseResource],
    index: &ReportIndex,
) -> CallerBranchContext {
    let Some(claims) = claims else {
        return CallerBranchContext::default();
    };
    let is_admin =
        claims.role == Roles::Admin || is_super_admin_role(&claims.effective_role_name());
    if is_admin {
        return CallerBranchContext {
            is_admin: true,
            ..Default::default()
        };
    }

    let matched_user = users.iter().find(|u| {
        if let Some(uid) = claims.user_id {
            if u.id == Some(uid) {
                return true;
            }
        }
        u.inner_content.as_ref().is_some_and(|inner| {
            inner
                .get("email")
                .or_else(|| inner.get("username"))
                .and_then(Value::as_str)
                .is_some_and(|e| e.trim().eq_ignore_ascii_case(claims.sub.trim()))
        })
    });

    let mut branch_id: Option<String> = None;
    let mut branch_name: Option<String> = None;

    if let Some(user_res) = matched_user {
        if let Some(inner) = user_res.inner_content.as_ref() {
            branch_id = narayani_core::extract_string_or_int(inner.get("branchId")).or_else(|| {
                inner
                    .get("branch")
                    .and_then(|b| narayani_core::extract_string_or_int(b.get("id")))
            });
            branch_name = inner
                .get("branchName")
                .or_else(|| inner.get("branch").and_then(|b| b.get("name")))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from);

            if branch_name.is_none() {
                if let Some(ref bid) = branch_id {
                    if let Some(bdef) = index.branches_by_id.get(bid.as_str()) {
                        branch_name = Some(bdef.name.clone());
                    }
                }
            }
        }
    }

    let branch_lower = branch_name.as_deref().map(normalize_key).filter(|s| !s.is_empty());

    CallerBranchContext {
        is_admin: false,
        assigned_branch_id: branch_id,
        assigned_branch_name: branch_name,
        assigned_branch_lower: branch_lower,
    }
}

impl CallerBranchContext {
    pub fn effective_branch_lower(&self, requested_branch: Option<&str>) -> Option<String> {
        if !self.is_admin {
            return Some(
                self.assigned_branch_lower
                    .clone()
                    .unwrap_or_else(|| "__unassigned_branch__".to_string()),
            );
        }
        let req = requested_branch.map(str::trim).unwrap_or("");
        if req.is_empty() || req.eq_ignore_ascii_case("all") {
            None
        } else {
            Some(normalize_key(req))
        }
    }

    pub fn effective_branch_display(&self, requested_branch: Option<&str>) -> Option<String> {
        if !self.is_admin {
            return self.assigned_branch_name.clone();
        }
        let req = requested_branch.map(str::trim).unwrap_or("");
        if req.is_empty() || req.eq_ignore_ascii_case("all") {
            None
        } else {
            Some(req.to_string())
        }
    }

    pub fn matches_receipt(&self, r: &ProjectedReceipt, effective_lower: Option<&str>) -> bool {
        if !self.is_admin {
            let name_match = self
                .assigned_branch_lower
                .as_deref()
                .is_some_and(|bl| !bl.is_empty() && r.branch_name_lower == bl);
            let id_match = self
                .assigned_branch_id
                .as_deref()
                .is_some_and(|bid| !bid.is_empty() && r.branch_id == bid);
            return name_match || id_match;
        }
        let Some(target) = effective_lower else {
            return true;
        };
        if r.branch_name_lower == target {
            return true;
        }
        false
    }
}
