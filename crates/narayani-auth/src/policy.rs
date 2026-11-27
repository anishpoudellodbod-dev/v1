use narayani_core::{DatabaseResource, ResourceName};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

pub const PERMISSION_SCHEMA_VERSION: &str = "1.0";
pub const PLATFORM_SUPER_ADMIN_ROLE: &str = "SuperAdmin";

pub mod action {
    pub const WILDCARD: &str = "*";

    // Basic actions mapped directly from generic CRUD/search/stream routes
    pub const CREATE: &str = "CREATE";
    pub const READ: &str = "READ";
    pub const UPDATE: &str = "UPDATE";
    pub const DELETE: &str = "DELETE";

    // BillResource domain actions
    pub const CANCEL: &str = "CANCEL";
    pub const CORRECT: &str = "CORRECT";
    pub const PRINT: &str = "PRINT";
    pub const REPRINT: &str = "REPRINT";
    pub const REPAIR_INVOICE: &str = "REPAIR_INVOICE";
    pub const VOID: &str = "VOID";

    // MemberResource domain actions
    pub const CHANGE_OWNER: &str = "CHANGE_OWNER";
    pub const RECONCILE: &str = "RECONCILE";
    pub const BLACKLIST: &str = "BLACKLIST";
    pub const UNBLACKLIST: &str = "UNBLACKLIST";

    // VehicleResource domain actions
    pub const CONFIRM_NUMBER_CHANGE: &str = "CONFIRM_NUMBER_CHANGE";
    pub const DISPLACE: &str = "DISPLACE";

    // BillingRuleResource domain actions
    pub const ACTIVATE: &str = "ACTIVATE";
    pub const DEACTIVATE: &str = "DEACTIVATE";

    // ReportResource / logs / audit domain actions
    pub const EXPORT: &str = "EXPORT";

    // SettingResource domain actions
    pub const MANAGE_FISCAL_YEAR: &str = "MANAGE_FISCAL_YEAR";
    pub const MANAGE_SMS: &str = "MANAGE_SMS";

    // UserResource domain actions
    pub const RESET_PASSWORD: &str = "RESET_PASSWORD";

    // PermissionResource domain actions
    pub const ASSIGN: &str = "ASSIGN";
    pub const REPAIR_ACCESS_CONTROL: &str = "REPAIR_ACCESS_CONTROL";

    // CacheResource domain actions
    pub const CLEAR: &str = "CLEAR";
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceActionCatalogEntry {
    pub resource: &'static str,
    pub basic_actions: &'static [&'static str],
    pub domain_actions: &'static [&'static str],
    pub internal_only: bool,
    pub immutable_write_blocked: bool,
}

const CRUD_BASIC: &[&str] = &[
    action::CREATE,
    action::READ,
    action::UPDATE,
    action::DELETE,
];
const READ_ONLY_BASIC: &[&str] = &[action::READ];
const SETTINGS_BASIC: &[&str] = &[action::READ, action::UPDATE];
const NO_DOMAIN: &[&str] = &[];

/// Authoritative in-code action catalog shared across all tenants (never stored per-tenant in DB).
pub const ACTION_CATALOG: &[ResourceActionCatalogEntry] = &[
    ResourceActionCatalogEntry {
        resource: "BillResource",
        basic_actions: CRUD_BASIC,
        domain_actions: &[
            action::CANCEL,
            action::CORRECT,
            action::PRINT,
            action::REPRINT,
            action::REPAIR_INVOICE,
            action::VOID,
        ],
        internal_only: false,
        immutable_write_blocked: false,
    },
    ResourceActionCatalogEntry {
        resource: "MemberResource",
        basic_actions: CRUD_BASIC,
        domain_actions: &[
            action::CHANGE_OWNER,
            action::RECONCILE,
            action::BLACKLIST,
            action::UNBLACKLIST,
        ],
        internal_only: false,
        immutable_write_blocked: false,
    },
    ResourceActionCatalogEntry {
        resource: "VehicleResource",
        basic_actions: CRUD_BASIC,
        domain_actions: &[
            action::CONFIRM_NUMBER_CHANGE,
            action::DISPLACE,
            action::BLACKLIST,
            action::UNBLACKLIST,
        ],
        internal_only: false,
        immutable_write_blocked: false,
    },
    ResourceActionCatalogEntry {
        resource: "VehicleCategoryResource",
        basic_actions: CRUD_BASIC,
        domain_actions: NO_DOMAIN,
        internal_only: false,
        immutable_write_blocked: false,
    },
    ResourceActionCatalogEntry {
        resource: "BillingRuleResource",
        basic_actions: CRUD_BASIC,
        domain_actions: &[action::ACTIVATE, action::DEACTIVATE],
        internal_only: false,
        immutable_write_blocked: false,
    },
    ResourceActionCatalogEntry {
        resource: "BillingEntityResource",
        basic_actions: CRUD_BASIC,
        domain_actions: NO_DOMAIN,
        internal_only: false,
        immutable_write_blocked: false,
    },
    ResourceActionCatalogEntry {
        resource: "BranchResource",
        basic_actions: CRUD_BASIC,
        domain_actions: NO_DOMAIN,
        internal_only: false,
        immutable_write_blocked: false,
    },
    ResourceActionCatalogEntry {
        resource: "ReportResource",
        basic_actions: READ_ONLY_BASIC,
        domain_actions: &[action::EXPORT],
        internal_only: false,
        immutable_write_blocked: false,
    },
    ResourceActionCatalogEntry {
        resource: "SettingResource",
        basic_actions: SETTINGS_BASIC,
        domain_actions: &[action::MANAGE_FISCAL_YEAR, action::MANAGE_SMS],
        internal_only: false,
        immutable_write_blocked: false,
    },
    ResourceActionCatalogEntry {
        resource: "UserResource",
        basic_actions: CRUD_BASIC,
        domain_actions: &[action::RESET_PASSWORD, action::ACTIVATE, action::DEACTIVATE],
        internal_only: false,
        immutable_write_blocked: false,
    },
    ResourceActionCatalogEntry {
        resource: "PermissionResource",
        basic_actions: CRUD_BASIC,
        domain_actions: &[action::ASSIGN, action::REPAIR_ACCESS_CONTROL],
        internal_only: false,
        immutable_write_blocked: false,
    },
    ResourceActionCatalogEntry {
        resource: "CacheResource",
        basic_actions: READ_ONLY_BASIC,
        domain_actions: &[action::CLEAR],
        internal_only: true,
        immutable_write_blocked: false,
    },
    ResourceActionCatalogEntry {
        resource: "ActionLogsResource",
        basic_actions: READ_ONLY_BASIC,
        domain_actions: &[action::EXPORT],
        internal_only: false,
        immutable_write_blocked: true,
    },
    ResourceActionCatalogEntry {
        resource: "AuditTrailResource",
        basic_actions: READ_ONLY_BASIC,
        domain_actions: &[action::EXPORT],
        internal_only: false,
        immutable_write_blocked: true,
    },
    ResourceActionCatalogEntry {
        resource: "VehicleHistoryResource",
        basic_actions: CRUD_BASIC,
        domain_actions: NO_DOMAIN,
        internal_only: false,
        immutable_write_blocked: false,
    },
    ResourceActionCatalogEntry {
        resource: "FileResource",
        basic_actions: CRUD_BASIC,
        domain_actions: NO_DOMAIN,
        internal_only: false,
        immutable_write_blocked: false,
    },
];

/// Returns PascalCase canonical resource name (fixes `VehicleResource::to_string()` returning `"vehicleresource"`).
pub fn canonical_resource_name(resource_name: ResourceName) -> &'static str {
    match resource_name {
        ResourceName::VehicleResource => "VehicleResource",
        ResourceName::UserResource => "UserResource",
        ResourceName::BillResource => "BillResource",
        ResourceName::MemberResource => "MemberResource",
        ResourceName::ReportResource => "ReportResource",
        ResourceName::SettingResource => "SettingResource",
        ResourceName::PermissionResource => "PermissionResource",
        ResourceName::CacheResource => "CacheResource",
        ResourceName::VehicleCategoryResource => "VehicleCategoryResource",
        ResourceName::BillingRuleResource => "BillingRuleResource",
        ResourceName::BillingEntityResource => "BillingEntityResource",
        ResourceName::BranchResource => "BranchResource",
        ResourceName::ActionLogsResource => "ActionLogsResource",
        ResourceName::AuditTrailResource => "AuditTrailResource",
        ResourceName::VehicleHistoryResource => "VehicleHistoryResource",
        ResourceName::FileResource => "FileResource",
    }
}

pub fn action_catalog() -> &'static [ResourceActionCatalogEntry] {
    ACTION_CATALOG
}

pub fn resolve_catalog_resource_name(raw: &str) -> Option<&'static str> {
    let trimmed = raw.trim();
    if trimmed == action::WILDCARD {
        return Some(action::WILDCARD);
    }
    ResourceName::from_str_case_insensitive(trimmed).map(canonical_resource_name)
}

pub fn catalog_entry_for(resource: &str) -> Option<&'static ResourceActionCatalogEntry> {
    let trimmed = resource.trim();
    ACTION_CATALOG
        .iter()
        .find(|e| e.resource.eq_ignore_ascii_case(trimmed))
}

pub fn normalize_action_token(raw: &str) -> String {
    raw.trim().to_ascii_uppercase()
}

pub fn is_known_action_token(action_name: &str) -> bool {
    let norm = normalize_action_token(action_name);
    if norm == action::WILDCARD {
        return true;
    }
    ACTION_CATALOG.iter().any(|entry| {
        entry.basic_actions.contains(&norm.as_str())
            || entry.domain_actions.contains(&norm.as_str())
    })
}

pub fn is_valid_action_for_resource(resource: &str, action_name: &str) -> bool {
    let norm_act = normalize_action_token(action_name);
    if resource.trim() == action::WILDCARD {
        return is_known_action_token(&norm_act);
    }
    if norm_act == action::WILDCARD {
        return catalog_entry_for(resource).is_some();
    }
    catalog_entry_for(resource).is_some_and(|entry| {
        entry.basic_actions.contains(&norm_act.as_str())
            || entry.domain_actions.contains(&norm_act.as_str())
    })
}

/// System-generated audit/log tables can never be created, updated, or deleted via role permissions.
pub fn is_system_immutable_blocked(resource: &str, action_name: &str) -> bool {
    let norm_act = normalize_action_token(action_name);
    catalog_entry_for(resource).is_some_and(|entry| {
        entry.immutable_write_blocked
            && matches!(
                norm_act.as_str(),
                action::CREATE | action::UPDATE | action::DELETE
            )
    })
}

/// Normalizes tenant/branch strings into a canonical lowercase slug (`"Loadboard sthaniya"` -> `"loadboard-sthaniya"`).
pub fn normalize_tenant_id(raw: &str) -> String {
    let mut slug = String::with_capacity(raw.len());
    let mut prev_hyphen = true;
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_hyphen = false;
        } else if !prev_hyphen {
            slug.push('-');
            prev_hyphen = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

pub fn is_super_admin_role(role: &str) -> bool {
    let norm: String = role
        .trim()
        .chars()
        .filter(|c| !matches!(c, '_' | '-' | ' '))
        .flat_map(|c| c.to_lowercase())
        .collect();
    norm == "superadmin"
}

fn default_schema_version() -> String {
    PERMISSION_SCHEMA_VERSION.to_string()
}

fn default_status_active() -> String {
    "ACTIVE".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RolePermissionPolicy {
    #[serde(default = "default_schema_version", alias = "schema_version")]
    pub schema_version: String,
    #[serde(alias = "tenant_id")]
    pub tenant_id: String,
    pub role: String,
    #[serde(default = "default_status_active")]
    pub status: String,
    #[serde(default, alias = "branch_ids")]
    pub branch_ids: Vec<i64>,
    #[serde(default)]
    pub allow: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub deny: BTreeMap<String, Vec<String>>,
    #[serde(
        default,
        alias = "require_audit",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub require_audit: Vec<String>,
    #[serde(default, alias = "created_by", skip_serializing_if = "Option::is_none")]
    pub created_by: Option<i32>,
    #[serde(default, alias = "updated_by", skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RolePermissionResource {
    pub id: Option<i32>,
    pub resource_name: ResourceName,
    pub inner_content: RolePermissionPolicy,
    pub external_content: Option<Value>,
}

impl RolePermissionResource {
    pub fn to_database_resource(&self) -> DatabaseResource {
        DatabaseResource {
            id: self.id,
            resource_name: Some(ResourceName::PermissionResource),
            inner_content: serde_json::to_value(&self.inner_content).ok(),
            external_content: self.external_content.clone(),
        }
    }

    pub fn from_database_resource(res: &DatabaseResource) -> Result<Self, String> {
        let inner = res
            .inner_content
            .as_ref()
            .ok_or_else(|| "PermissionResource missing inner_content".to_string())?;
        let mut policy: RolePermissionPolicy = serde_json::from_value(inner.clone())
            .map_err(|e| format!("Invalid v1.0 RolePermissionPolicy inner_content: {e}"))?;
        policy.tenant_id = normalize_tenant_id(&policy.tenant_id);
        Ok(Self {
            id: res.id,
            resource_name: ResourceName::PermissionResource,
            inner_content: policy,
            external_content: res.external_content.clone(),
        })
    }
}

/// Identifies whether a `PermissionResource` JSON payload uses the v1.0 role-per-tenant schema
/// (`tenantId` + no `userId`) vs the legacy per-user schema (`userId`).
pub fn is_role_permission_payload(inner: &Value) -> bool {
    let Some(obj) = inner.as_object() else {
        return false;
    };
    let has_user_id = obj.contains_key("userId") || obj.contains_key("user_id");
    if has_user_id {
        return false;
    }
    obj.contains_key("tenantId") || obj.contains_key("tenant_id")
}

fn normalize_action_map(
    map: &BTreeMap<String, Vec<String>>,
    field_name: &str,
    reject_immutable_grants: bool,
    errors: &mut Vec<String>,
) -> BTreeMap<String, Vec<String>> {
    let mut normalized = BTreeMap::new();
    for (raw_res, raw_actions) in map {
        let Some(canon_res) = resolve_catalog_resource_name(raw_res) else {
            errors.push(format!("Unknown resource '{}' in {}", raw_res, field_name));
            continue;
        };
        let mut seen = HashSet::new();
        let mut clean_actions = Vec::new();
        for raw_act in raw_actions {
            let act = normalize_action_token(raw_act);
            if act.is_empty() {
                continue;
            }
            let allowed_in_map = is_valid_action_for_resource(canon_res, &act)
                || (!reject_immutable_grants && CRUD_BASIC.contains(&act.as_str()));
            if !allowed_in_map {
                errors.push(format!(
                    "Unknown action '{}' for resource '{}' in {}",
                    raw_act, canon_res, field_name
                ));
                continue;
            }
            if reject_immutable_grants
                && is_system_immutable_blocked(canon_res, &act)
            {
                errors.push(format!(
                    "System resource '{}' cannot be granted '{}' in allow",
                    canon_res, act
                ));
                continue;
            }
            if reject_immutable_grants
                && act == action::WILDCARD
                && catalog_entry_for(canon_res).is_some_and(|e| e.immutable_write_blocked)
            {
                errors.push(format!(
                    "System resource '{}' cannot use '*' wildcard in allow",
                    canon_res
                ));
                continue;
            }
            if seen.insert(act.clone()) {
                clean_actions.push(act);
            }
        }
        normalized.insert(canon_res.to_string(), clean_actions);
    }
    normalized
}

/// Validates and canonicalizes a `RolePermissionPolicy` against the in-code action catalog and
/// security invariants (e.g. `SuperAdmin` exclusion and `ActionLogsResource`/`AuditTrailResource` immutability).
pub fn normalize_and_validate_policy(policy: &mut RolePermissionPolicy) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();

    if policy.schema_version.trim().is_empty() {
        policy.schema_version = PERMISSION_SCHEMA_VERSION.to_string();
    }

    let clean_tenant = normalize_tenant_id(&policy.tenant_id);
    if clean_tenant.is_empty() {
        errors.push("tenantId is required".to_string());
    } else {
        policy.tenant_id = clean_tenant;
    }

    let clean_role = policy.role.trim().to_string();
    if clean_role.is_empty() {
        errors.push("role is required".to_string());
    } else if is_super_admin_role(&clean_role) {
        errors.push(
            "Platform SuperAdmin is checked in code and must not have a PermissionResource"
                .to_string(),
        );
    } else {
        policy.role = clean_role;
    }

    let clean_status = policy.status.trim().to_ascii_uppercase();
    if clean_status == "ACTIVE" || clean_status == "INACTIVE" {
        policy.status = clean_status;
    } else {
        errors.push(format!(
            "Invalid status '{}': expected ACTIVE or INACTIVE",
            policy.status
        ));
    }

    let mut seen_branches = HashSet::new();
    policy
        .branch_ids
        .retain(|b| seen_branches.insert(*b));

    policy.allow = normalize_action_map(&policy.allow, "allow", true, &mut errors);
    policy.deny = normalize_action_map(&policy.deny, "deny", false, &mut errors);

    let mut seen_audit = HashSet::new();
    let mut normalized_audit = Vec::new();
    for pair in &policy.require_audit {
        let Some((raw_res, raw_act)) = pair.split_once(':') else {
            errors.push(format!(
                "Invalid requireAudit entry '{}': expected 'Resource:ACTION'",
                pair
            ));
            continue;
        };
        let Some(canon_res) = resolve_catalog_resource_name(raw_res) else {
            errors.push(format!(
                "Unknown resource '{}' in requireAudit '{}'",
                raw_res, pair
            ));
            continue;
        };
        let act = normalize_action_token(raw_act);
        if act == action::WILDCARD || !is_valid_action_for_resource(canon_res, &act) {
            errors.push(format!(
                "Invalid action '{}' for resource '{}' in requireAudit",
                raw_act, canon_res
            ));
            continue;
        }
        let canon_pair = format!("{}:{}", canon_res, act);
        if seen_audit.insert(canon_pair.clone()) {
            normalized_audit.push(canon_pair);
        }
    }
    policy.require_audit = normalized_audit;

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Core permission evaluator: deny-by-default, deny-always-wins, wildcard-aware, branch-scoped.
pub fn is_allowed(
    p: &RolePermissionPolicy,
    tenant: &str,
    resource: &str,
    action_name: &str,
    branch: Option<i64>,
) -> bool {
    if !p.status.eq_ignore_ascii_case("ACTIVE")
        || normalize_tenant_id(&p.tenant_id) != normalize_tenant_id(tenant)
    {
        return false;
    }
    if let Some(b) = branch {
        if !p.branch_ids.is_empty() && !p.branch_ids.contains(&b) {
            return false;
        }
    }

    let canon_res = resolve_catalog_resource_name(resource).unwrap_or(resource.trim());
    let norm_act = normalize_action_token(action_name);

    // System audit/log resources are strictly append-only in code even if legacy DB docs differ.
    if is_system_immutable_blocked(canon_res, &norm_act) {
        return false;
    }

    let hit = |m: &BTreeMap<String, Vec<String>>| {
        m.iter()
            .filter(|(k, _)| {
                k.eq_ignore_ascii_case(canon_res) || k.as_str() == action::WILDCARD
            })
            .any(|(_, acts)| {
                acts.iter().any(|x| {
                    let up = normalize_action_token(x);
                    up == norm_act || up == action::WILDCARD
                })
            })
    };

    if hit(&p.deny) {
        return false;
    }
    hit(&p.allow)
}

/// Returns true if `(resource, action)` is configured in `p.require_audit`.
pub fn requires_audit(p: &RolePermissionPolicy, resource: &str, action_name: &str) -> bool {
    let canon_res = resolve_catalog_resource_name(resource).unwrap_or(resource.trim());
    let norm_act = normalize_action_token(action_name);
    p.require_audit.iter().any(|pair| {
        if let Some((r, a)) = pair.split_once(':') {
            (r.eq_ignore_ascii_case(canon_res) || r.trim() == action::WILDCARD)
                && a.trim().eq_ignore_ascii_case(&norm_act)
        } else {
            false
        }
    })
}

pub fn svec(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

/// Full explicit Admin policy listing every catalog resource/action explicitly (no `"*"` allow wildcard).
pub fn default_admin_policy(tenant_id: &str, actor_id: Option<i32>) -> RolePermissionPolicy {
    let mut allow = BTreeMap::new();
    allow.insert(
        "BillResource".to_string(),
        svec(&[
            action::CREATE,
            action::READ,
            action::UPDATE,
            action::CANCEL,
            action::CORRECT,
            action::PRINT,
            action::REPRINT,
            action::REPAIR_INVOICE,
            action::VOID,
        ]),
    );
    allow.insert(
        "MemberResource".to_string(),
        svec(&[
            action::CREATE,
            action::READ,
            action::UPDATE,
            action::DELETE,
            action::CHANGE_OWNER,
            action::RECONCILE,
            action::BLACKLIST,
            action::UNBLACKLIST,
        ]),
    );
    allow.insert(
        "VehicleResource".to_string(),
        svec(&[
            action::CREATE,
            action::READ,
            action::UPDATE,
            action::DELETE,
            action::CONFIRM_NUMBER_CHANGE,
            action::DISPLACE,
            action::BLACKLIST,
            action::UNBLACKLIST,
        ]),
    );
    allow.insert("VehicleCategoryResource".to_string(), svec(CRUD_BASIC));
    allow.insert(
        "BillingRuleResource".to_string(),
        svec(&[
            action::CREATE,
            action::READ,
            action::UPDATE,
            action::DELETE,
            action::ACTIVATE,
            action::DEACTIVATE,
        ]),
    );
    allow.insert("BillingEntityResource".to_string(), svec(CRUD_BASIC));
    allow.insert("BranchResource".to_string(), svec(CRUD_BASIC));
    allow.insert("ReportResource".to_string(), svec(&[action::READ, action::EXPORT]));
    allow.insert(
        "SettingResource".to_string(),
        svec(&[
            action::READ,
            action::UPDATE,
            action::MANAGE_FISCAL_YEAR,
            action::MANAGE_SMS,
        ]),
    );
    allow.insert(
        "UserResource".to_string(),
        svec(&[
            action::CREATE,
            action::READ,
            action::UPDATE,
            action::DELETE,
            action::RESET_PASSWORD,
            action::ACTIVATE,
            action::DEACTIVATE,
        ]),
    );
    allow.insert(
        "PermissionResource".to_string(),
        svec(&[
            action::CREATE,
            action::READ,
            action::UPDATE,
            action::ASSIGN,
            action::REPAIR_ACCESS_CONTROL,
        ]),
    );
    allow.insert("CacheResource".to_string(), svec(&[action::READ, action::CLEAR]));
    allow.insert(
        "ActionLogsResource".to_string(),
        svec(&[action::READ, action::EXPORT]),
    );
    allow.insert(
        "AuditTrailResource".to_string(),
        svec(&[action::READ, action::EXPORT]),
    );
    allow.insert(
        "VehicleHistoryResource".to_string(),
        svec(&[action::READ, action::CREATE, action::UPDATE]),
    );
    allow.insert("FileResource".to_string(), svec(CRUD_BASIC));

    let mut deny = BTreeMap::new();
    deny.insert("BillResource".to_string(), svec(&[action::DELETE]));
    deny.insert("PermissionResource".to_string(), svec(&[action::DELETE]));
    deny.insert(
        "ActionLogsResource".to_string(),
        svec(&[action::CREATE, action::UPDATE, action::DELETE]),
    );
    deny.insert(
        "AuditTrailResource".to_string(),
        svec(&[action::CREATE, action::UPDATE, action::DELETE]),
    );

    let require_audit = svec(&[
        "BillResource:CANCEL",
        "BillResource:CORRECT",
        "BillResource:REPAIR_INVOICE",
        "BillResource:VOID",
        "VehicleResource:CONFIRM_NUMBER_CHANGE",
        "MemberResource:CHANGE_OWNER",
        "PermissionResource:ASSIGN",
        "PermissionResource:REPAIR_ACCESS_CONTROL",
        "UserResource:RESET_PASSWORD",
    ]);

    RolePermissionPolicy {
        schema_version: PERMISSION_SCHEMA_VERSION.to_string(),
        tenant_id: normalize_tenant_id(tenant_id),
        role: "Admin".to_string(),
        status: "ACTIVE".to_string(),
        branch_ids: Vec::new(),
        allow,
        deny,
        require_audit,
        created_by: actor_id,
        updated_by: actor_id,
    }
}

/// Example Cashier role policy, optionally restricted to specific `branchIds`.
pub fn default_cashier_policy(
    tenant_id: &str,
    branch_ids: Vec<i64>,
    actor_id: Option<i32>,
) -> RolePermissionPolicy {
    let mut allow = BTreeMap::new();
    allow.insert(
        "BillResource".to_string(),
        svec(&[action::CREATE, action::READ, action::PRINT]),
    );
    allow.insert(
        "MemberResource".to_string(),
        svec(&[action::READ, action::CREATE, action::UPDATE]),
    );
    allow.insert(
        "VehicleResource".to_string(),
        svec(&[action::READ, action::CREATE, action::UPDATE]),
    );
    allow.insert("VehicleCategoryResource".to_string(), svec(&[action::READ]));
    allow.insert("BillingRuleResource".to_string(), svec(&[action::READ]));
    allow.insert("BillingEntityResource".to_string(), svec(&[action::READ]));
    allow.insert("ReportResource".to_string(), svec(&[action::READ]));
    allow.insert("VehicleHistoryResource".to_string(), svec(&[action::READ]));
    allow.insert("VehicleHistoryResource".to_string(), svec(&[action::READ]));

    RolePermissionPolicy {
        schema_version: PERMISSION_SCHEMA_VERSION.to_string(),
        tenant_id: normalize_tenant_id(tenant_id),
        role: "Cashier".to_string(),
        status: "ACTIVE".to_string(),
        branch_ids,
        allow,
        deny: BTreeMap::new(),
        require_audit: Vec::new(),
        created_by: actor_id,
        updated_by: actor_id,
    }
}

/// Example read-only Viewer role policy (explicitly denies User/Permission/Setting access).
pub fn default_viewer_policy(tenant_id: &str, actor_id: Option<i32>) -> RolePermissionPolicy {
    let mut allow = BTreeMap::new();
    allow.insert(action::WILDCARD.to_string(), svec(&[action::READ]));

    let mut deny = BTreeMap::new();
    deny.insert("UserResource".to_string(), svec(&[action::WILDCARD]));
    deny.insert("PermissionResource".to_string(), svec(&[action::WILDCARD]));
    deny.insert("SettingResource".to_string(), svec(&[action::WILDCARD]));

    RolePermissionPolicy {
        schema_version: PERMISSION_SCHEMA_VERSION.to_string(),
        tenant_id: normalize_tenant_id(tenant_id),
        role: "Viewer".to_string(),
        status: "ACTIVE".to_string(),
        branch_ids: Vec::new(),
        allow,
        deny,
        require_audit: Vec::new(),
        created_by: actor_id,
        updated_by: actor_id,
    }
}

fn parse_i64_value(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
}

pub fn extract_branch_id_from_value(val: &Value) -> Option<i64> {
    for key in ["branchId", "branch_id"] {
        if let Some(v) = val.get(key) {
            if let Some(id) = parse_i64_value(v) {
                return Some(id);
            }
        }
    }
    if let Some(branch_obj) = val.get("branch") {
        if let Some(id) = branch_obj.get("id").and_then(parse_i64_value) {
            return Some(id);
        }
        if let Some(id) = parse_i64_value(branch_obj) {
            return Some(id);
        }
    }
    None
}
