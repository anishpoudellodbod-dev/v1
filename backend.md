# Narayani Billing Backend (`v1`) — Complete Architecture & Implementation Specification

This document consolidates the architecture, data models, endpoints, middleware, integrations, and runtime behaviors of the **Narayani Billing Backend (`v1`)** so an engineer or agent can understand or rebuild the backend from scratch in Rust.

---

## 1. Tech Stack & Project Layout

### 1.1 `Cargo.toml`
- **Package**: `name = "v1"`, `version = "0.1.0"`, `edition = "2021"`
- **Core Dependencies**:
  - Web & HTTP: `actix-web = { version = "4.3.1", features = ["rustls-0_21"] }`, `actix-cors = "0.6"`, `reqwest = { version = "0.12", features = ["json"] }`
  - Async & Streaming: `tokio = { version = "1.32", features = ["full"] }`, `futures = "0.3"`, `futures-util = "0.3.34"`, `bytes = "1"`
  - Serialization & Errors: `serde = { version = "1.0", features = ["derive"] }`, `serde_json = { version = "1.0", features = ["preserve_order"] }`, `anyhow = "1.0.104"`
  - Database: `tokio-postgres = { version = "0.7.8", features = ["with-serde_json-1"] }`
  - Auth & TLS: `jsonwebtoken = "9.2"`, `rustls = "0.21"`, `rustls-pemfile = "1"`
  - Time & Env: `chrono = { version = "0.4", features = ["serde"] }`, `dotenv = "0.15"`

### 1.2 Directory & Module Structure
Workspace root contains `Cargo.toml`, `Cargo.lock`, `.env`, `billing-narayani.service`, `cert.pem`, `key.pem`, `private.pem`, `public.pem`, `date_range.json`, `logs/`, `src/`, and `target/`.

**20 active Rust modules wired in `src/main.rs`:**
- Core & Storage: `main.rs`, `resource.rs`, `db.rs`, `cache.rs`
- Auth & Authorization: `roles.rs`, `auth.rs`, `middleware.rs`, `login.rs`, `access.rs`, `permissions.rs`
- Generic CRUD & Streaming: `actions.rs`, `api_stream.rs`
- Observability & Settings: `log.rs`, `audit.rs`, `settings.rs`
- Domain & Integrations: `billing_actions.rs`, `tqms_integration.rs`, `sms.rs`, `dashboard_statistics.rs`, `members.rs`, `date.rs`

> **Note:** Several legacy/unreferenced files sit in `src/` and are **not** declared as modules in `src/main.rs`: `billlingruleresource.rs`, `initialization.rs`, `meta_data.rs`, `permission_resource.rs`, `utility.rs`, and `vehicle_number.rs`. Do not wire these unless intentionally migrating logic from them.

---

## 2. Environment Variables & Deployment Configuration

| Variable | Default / Required | Used In | Purpose |
| :--- | :--- | :--- | :--- |
| `DATABASE_URL` | Required | `main.rs` | PostgreSQL connection URL |
| `APP_PORT` | `"5001"` | `main.rs` | HTTP listen port on `127.0.0.1:{APP_PORT}` |
| `TLS_CERT_PATH` | `"cert.pem"` | `main.rs` | TLS certificate path read by `load_tls_config()` |
| `TLS_KEY_PATH` | `"key.pem"` | `main.rs` | TLS private key path read by `load_tls_config()` |
| `JWT_USER_SECRET` | Optional env | `roles.rs` | Role secret helper (`Roles::jwt_secret()`) |
| `JWT_ADMIN_SECRET` | Optional env | `roles.rs` | Role secret helper (`Roles::jwt_secret()`) |
| `JWT_OPERATOR_SECRET` | Optional env | `roles.rs` | Role secret helper (`Roles::jwt_secret()`) |
| `LODBOD_SERVICE_TOKEN` | Optional | `billing_actions.rs` | Static Bearer token accepted for Lodbod endpoints alongside Admin/Operator JWT |
| `TQMS_BASE_URL` | Required for TQMS | `tqms_integration.rs` | Base URL for upstream TQMS API |
| `ACTION_LOG_FILE` | `"logs/actions.jsonl"` | `log.rs` | Optional local NDJSON mirror for action logs (outside `#[cfg(test)]`) |
| `SMS_API_BASE_URL` | SamayaSMS URL | `sms.rs` | Fallback SMS API URL (overridden by `SettingResource`) |
| `SMS_API_KEY` | Fallback constant | `sms.rs` | Fallback SamayaSMS API key |
| `SMS_SENDER_ID` | Optional | `sms.rs` | Fallback sender ID |
| `SMS_RESPONSE_TYPE` | Optional | `sms.rs` | Fallback SamayaSMS response type |
| `SMS_API_TYPE` | Optional | `sms.rs` | Fallback SamayaSMS API type |
| `SMS_API_SECRET` | Optional | `sms.rs` | Fallback SMS secret |
| `SMS_WEBHOOK_URL` | Optional | `sms.rs` | Fallback SMS webhook callback URL |
| `SMS_DEFAULT_COUNTRY_CODE` | Optional | `sms.rs` | Fallback default country code (e.g., `+977`) |
| `SMS_ROUTE_ID` | `"10244"` fallback | `sms.rs` | Route ID fallback when account requires explicit pricing route |
| `SMS_CAMPAIGN_ID` | `"8572"` fallback | `sms.rs` | Campaign ID fallback for SamayaSMS |

### Key & Certificate Files
- `private.pem` / `public.pem`: RSA keys in the working directory used by `auth.rs` for RS512 JWT signing and RS256/RS384/RS512 verification.
- `cert.pem` / `key.pem`: Loaded by `load_tls_config()` in `main.rs`. Note that `HttpServer::bind(format!("127.0.0.1:{}", app_port))` binds plain HTTP on loopback (intended behind a reverse proxy) while `load_tls_config()` validates TLS files.
- `billing-narayani.service`: Systemd service running `/home/ubuntu/billing-narayani/v1/target/release/v1` with `APP_PORT=5001`, `Restart=on-failure`, `RestartSec=10s`.

---

## 3. Application Startup, `ServerData` & Migrations (`src/main.rs`)

### 3.1 Shared State
```rust
pub struct ServerData {
    pub db: Database,
    pub tqms_client: TQMSClient,
    pub cache: RwLock<Cache>, // tokio::sync::RwLock
}
```
Wrapped in `Arc<ServerData>` and injected as `web::Data<Arc<ServerData>>`.

### 3.2 Boot & Shutdown Lifecycle
1. Load `.env` via `dotenv::dotenv().ok()`.
2. Connect to PostgreSQL via `db::connect_to_db(&database_url)` and construct `Database::new(client)`.
3. Call `db.create_tables().await` to ensure all PostgreSQL resource tables exist.
4. Run startup data migrations (also runnable standalone via `--run-migrations`):
   - `create_admin_user_if_not_exists(&db)` — seeds initial Admin user if absent.
   - `add_sthaniya_branch_to_billing_rule_resources(&db)` — ensures `inner_content.sthaniyaBranch = "Loadboard sthaniya"` on `BillingRuleResource` rows missing it.
   - `migrate_fiscal_year_id_to_fiscal_year(&db)` — removes case-insensitive `fiscalYearId` / `fiscalyearid` from `BillResource` and `BillingRuleResource` and sets `fiscalYear` (e.g., `"2082/2083"`) if missing.
5. Hydrate cache: `cache.load_from_db(&db).await`.
6. Hydrate v1.0 RBAC policies into `Cache.role_policies`: `permissions::hydrate_role_policies(&s_data).await`.
7. Start async MPSC workers and hydrate sequence counters:
   - `log::init_persistent_action_logger(&s_data).await`
   - `audit::init_persistent_audit_trail(&s_data).await`
8. Configure CORS (`Cors::permissive()` / exposed headers: `X-Stream-Format`, `X-Batch-Size`, `X-Concurrency`, `X-Total-Items`, `X-Total-Pages`, `X-Starting-Offset`) and bind Actix-web `HttpServer`.
9. On shutdown, drain MPSC queues cleanly:
   - `log::flush_action_logs().await;`
   - `audit::flush_audit_trail().await;`

---

## 4. Storage Model: PostgreSQL JSONB + In-Memory Cache (`src/resource.rs`, `src/db.rs`, `src/cache.rs`)

### 4.1 `ResourceName` & `DatabaseResource`
Every domain entity shares the same PostgreSQL envelope:
```rust
pub struct DatabaseResource {
    pub id: Option<i32>,
    pub resource_name: Option<ResourceName>,
    pub inner_content: Option<serde_json::Value>,
    pub external_content: Option<serde_json::Value>,
}
```
PostgreSQL table schema per DB-backed resource:
```sql
CREATE TABLE IF NOT EXISTS <lowercase_resource_name> (
    id SERIAL PRIMARY KEY,
    inner_content JSONB,
    external_content JSONB
);
```
`db.create_resource()` automatically stamps `external_content.createdTime` with an RFC 3339 timestamp (seconds precision).

**All 14 `ResourceName` Variants:**

| Variant | `Display` String | DB Table Created | Cached in `Cache` | Notes |
| :--- | :--- | :--- | :--- | :--- |
| `VehicleResource` | `"vehicleresource"` *(lowercase quirk)* | Yes | Yes (deduped) | Vehicle records (local + TQMS sync) |
| `UserResource` | `"UserResource"` | Yes | **No** (DB-only) | Never bulk-cached in `Cache.cache` |
| `BillResource` | `"BillResource"` | Yes | Yes | Receipts / bills |
| `MemberResource` | `"MemberResource"` | Yes | Yes | Members / drivers / owners |
| `ReportResource` | `"ReportResource"` | Yes | Yes | Saved/generated reports |
| `SettingResource` | `"SettingResource"` | Yes | Yes | Tenant/system settings (`id = 1` typical) |
| `PermissionResource` | `"PermissionResource"` | Yes | **No** (`cache.cache`) | Legacy per-user docs & v1.0 role policies (v1.0 policies live in `cache.role_policies`) |
| `CacheResource` | `"CacheResource"` | **No** | No | Internal-only marker; excluded from `Database::new` |
| `VehicleCategoryResource` | `"VehicleCategoryResource"` | Yes | Yes | Vehicle categories |
| `BillingRuleResource` | `"BillingRuleResource"` | Yes | Yes | Fee heads & billing rules |
| `BillingEntityResource` | `"BillingEntityResource"` | Yes | Yes | Billing entities (VAT applicability) |
| `BranchResource` | `"BranchResource"` | Yes | Yes | Branch/office records |
| `ActionLogsResource` | `"ActionLogsResource"` | Yes | Yes | Persistent `ACTLOG-*` entries |
| `AuditTrailResource` | `"AuditTrailResource"` | Yes | Yes | Immutable `AUD-*` entries |

### 4.2 `Cache` Mechanics (`src/cache.rs`)
- Fields:
  - `cache: HashMap<ResourceName, HashMap<i32, DatabaseResource>>`
  - `fully_loaded: HashSet<ResourceName>`
  - `role_policies: HashMap<(String, String), RolePermissionResource>` (keyed by `(normalized_tenant_id, role_lowercase)`)
- **Uncached Resource Protection**: `actions::is_uncached_resource()` returns `true` for `UserResource` and `PermissionResource`. Reads hit DB directly (`read_uncached_resource_from_db`), and creating/updating a `UserResource` or `PermissionResource` **never clears or evicts** unrelated cached resources (`VehicleResource`, `BillResource`, etc.).
- **Querying**: `search_innercontent` and `search_innercontent_batch` traverse dotted JSON paths (e.g., `vehicle.vehicleNumber` or `billing.vat.enabled`) against cached items.

### 4.3 `DatabaseResource` Envelope — Optional Components & Design Rationale
Every persistent resource uses `DatabaseResource` (`src/resource.rs`). All four top-level fields are wrapped in `Option<...>` for distinct lifecycle and compatibility reasons:

| Field | Type | Optionality & Why It Is Optional |
| :--- | :--- | :--- |
| `id` | `Option<i32>` | Clients omit `id` on `POST /api/v1/create/{resource_name}` because PostgreSQL assigns it via `SERIAL PRIMARY KEY`. `db.read_resource()` returns `id: None` as a "not found" sentinel (`db.rs:70-76`), and `members::synthesize_external_vehicle_resource()` builds synthetic external vehicle items without a local DB row ID. |
| `resource_name` | `Option<ResourceName>` | Clients omit it in JSON bodies on generic routes (`/create/{resource_name}`, `/update/{resource_name}/{id}`) because the URL path already specifies the table. Deserialization uses `ResourceName::from_str_case_insensitive()`, while `ToString` preserves the legacy lowercase `"vehicleresource"` table name quirk for `VehicleResource`. |
| `inner_content` | `Option<serde_json::Value>` | Stores the domain JSONB column (nullable in PostgreSQL). Remains `Option<Value>` so a single row wrapper handles heterogeneous resource payloads, legacy schema variants, partial updates, and "not found" (`None`) DB sentinels without brittle deserialization failures. |
| `external_content` | `Option<serde_json::Value>` | Isolates system-managed metadata from client-owned `inner_content`. Clients normally omit it (`None`/`Null`), and `db.create_resource()` stamps `external_content.createdTime` (UTC RFC 3339 seconds) when it is `None`, `Null`, or a JSON object (leaving non-object values untouched). |

### 4.4 Concrete Resource Structures, Optional Components & Reasons

#### 1. `BillResource` & `DatabaseBillResource` (`src/billing_actions.rs`)
`DatabaseBillResource` mirrors `DatabaseResource` with `resource_name: Option<String>` so custom bill handlers can deserialize loosely. Inside `inner_content`, `BillResource` requires only **`items: Vec<BillItem>`** (`title: String`, `amount: f64`) and **`total: f64`**:
- **Optional top-level fields**: `id`, `note`, `subtotal`, `discount`, `branch: Option<BranchInfo>`, `paidAt`, `vehicle: Option<BillVehicleInfo>`, `createdBy: Option<CreatedByInfo>`, `periodStart`, `periodEnd`, `billingDate`, `billingType`, `fiscalYearId` (plus migrated `fiscalYear`), `billingMonths`, `paymentMethod`, `receiptNumber`, `lodbodPaymentReferenceId`, `esewaTransactionId`, `sanghBranch` (alias `SanghBranch`, `skip_serializing_if = "Option::is_none"`), `status`.
- **Optional nested fields**:
  - `BillItem`: `ruleId`, `feeHeadId`, `billingEntity`, `billingEntityId`, `periodId` (`Option<String>`, `#[serde(default)]`).
  - `BranchInfo`: `id`, `name`, `phone`, `address`, `BranchCode`, `contactPerson` (`Option<String>`).
  - `BillVehicleInfo`: `id`, `phone`, `address`, `ownerName`, `vehicleType`, `vehicleNumber`, `sanghBranch` / `SanghBranch` (`Option<String>`).
  - `CreatedByInfo`: `id`, `name`, `username` (`Option<String>`).
- **Reasons**: Accommodates both counter/cash bills and online Lodbod/eSewa integrations (where gateway reference IDs, `createdBy`, or `receiptNumber` differ), supports both one-time and period-based fees (`periodStart`/`periodEnd`/`billingMonths`), tolerates legacy or ad-hoc line items missing `ruleId`/`feeHeadId`/`billingEntityId`, accepts partial vehicle/branch snapshots, and allows the `fiscalYearId` $\rightarrow$ `fiscalYear` migration without breaking older records.

#### 2. `PermissionResource` — Dual Schema (`src/permissions.rs` v1.0 & `src/access.rs` Legacy)
The `permissionresource` table intentionally supports two distinct inner schemas (`is_role_permission_payload()` distinguishes v1.0 by presence of `tenantId` without `userId`):
- **v1.0 Tenant/Role Policy (`RolePermissionResource` / `RolePermissionPolicy`)**:
  - **Required**: `tenantId` (`tenant_id`), `role`.
  - **Optional / Defaulted**: `id: Option<i32>`, `external_content: Option<Value>`, `schemaVersion` (default `"1.0"`), `status` (default `"ACTIVE"`), `branchIds` (`default` empty `Vec<i64>`), `allow` / `deny` (`default` empty `BTreeMap<String, Vec<String>>`), `requireAudit` (`default`, `skip_serializing_if = "Vec::is_empty"`), `createdBy` / `updatedBy` (`Option<i32>`, `skip_serializing_if = "Option::is_none"`).
  - **Reasons**: Keeps role policy JSON compact; empty `branchIds` intentionally means "all branches"; empty `allow`/`deny` enforces safe deny-by-default (`is_allowed()`); and optional metadata keeps seed/minimal policy documents clean. (`SuperAdmin` has no DB document and is checked in Rust code across tenants.)
- **Legacy Per-User Policy (`PermissionResource` / `PermissionInnerContent`)**:
  - **Required**: `userId` (`user_id`), `userRole` (`user_role`).
  - **Optional / Defaulted**: `id: Option<i32>`, `external_content: Option<Value>`, `userPermissions` (defaults to `{}`), `sthaniyaBranch` (aliases `sthaniya_branch`, `SanghBranch`, `sanghBranch`), `Allowed: Vec<AllowedRule>` (custom `deserialize_allowed_rules` accepts `null`, a single rule object, or an array; `AllowedScope` accepts `"all"`/`"*"` or `Vec<ScopeId>` mixing integer and string IDs).
  - **Reasons**: Preserves backward compatibility with older single-object `Allowed` payloads, mixed string/int UI IDs, and optional branch assignment inherited from the creator or `SettingResource`.

#### 3. `SettingResource` (`AppSettings` in `src/settings.rs`)
- **Structure & Optional / Defaulted Components**:
  - All top-level sections on `AppSettings` (`schemaVersion` = `1`, `billing`, `receipts`, `insurance`, `fiscalYear`, `integrations`, `notifications`, `vehicleManagement`) use `#[serde(default)]` and conservative defaults (`vat.rate = 13.0`, `cancellation.hours = 24`, `minPaidRules = 1`, `yearsBack = 4`, `yearsForward = 1`, `provider = "samayasms"`, `displacedTagging.enabled = true`).
  - Explicitly `Option<...>` fields: `MonthlyPaidArrearsSettings.{fiscal_year, billing_rule_id}`, `PaidExpiredArrearsSettings.billing_rule_id`, `LinkedFeeRule.fiscal_year`, `FiscalYearBillingAccessSettings.{next_year, previous_year}` (and `FiscalYearAccessEntry.{updated_at, updated_by}`), `TqmsIntegrationSettings.sangh_branch` (aliases `SanghBranch`/`sanghBranch`/`sthaniyaBranch`), `SmsNotificationSettings.{route_id, campaign_id}` (`skip_serializing_if = "Option::is_none"`, multi-case aliases).
  - Custom string/number deserializers (`deserialize_opt_string_id`, `deserialize_string_id`, `deserialize_string_id_vec`) coerce both JSON numbers and strings cleanly.
- **Reasons**: Partial or older `SettingResource` JSON documents remain valid as new modules (insurance, fiscal-year access, displaced tagging, SamayaSMS routing) are introduced; disabled toggles do not force operators to supply rule/fiscal IDs; and multi-case aliases preserve compatibility across UI/migration versions.

#### 4. `ActionLogsResource` (`ActionLogEntry` in `src/log.rs`)
- **Required**: `id` (`ACTLOG-<seq>`), `action` (`ResourceActionKind`), `performed_at`, `status_code`, `success`, `duration_ms`.
- **Optional**: `resource_name: Option<String>`, `resource_id: Option<String>`, `performed_by: Option<String>`, `user_id: Option<i32>`, `role: Option<String>`, `details: Option<String>`, `metadata: Option<Value>`.
- **Reasons**: Supports non-resource lifecycle events (`LOGIN` / `LOGOUT` where `resource_name`/`resource_id` are `None`), failed `CREATE` requests before an ID exists, unauthenticated/unknown-actor failures (`performed_by`/`user_id`/`role` as `None`), and optional diagnostic `metadata` (with `[REDACTED]` secret masking).

#### 5. `AuditTrailResource` (`AuditTrailEntry`, `AuditActor`, `AuditRequestContext` in `src/audit.rs`)
- **Required**: `schema_version` (`1`), `audit_id` (`AUD-<seq>`), `timestamp`, `category` (`AuditCategory`), `action` (`AuditAction`), `outcome` (`AuditOutcome`), `status_code`, `actor`, `request`, `summary`.
- **Optional (`skip_serializing_if = "Option::is_none"` / `"Vec::is_empty"`)**:
  - Entry fields: `resource_name`, `resource_id`, `before_state`, `after_state`, `changes: Vec<FieldChange>`, `metadata`.
  - `AuditActor`: `email`, `user_id`, `role`, `branch`, `ip_address`, `user_agent` (all `Option<...>`).
  - `AuditRequestContext`: `method`, `path`, `request_id`, `duration_ms` (all `Option<...>`).
- **Reasons**: Keeps immutable audit rows compact by omitting inapplicable snapshots (`before_state` on `Create`/`Login`, `after_state` on `Delete`/`AccessDenied`, `changes` when empty), supports internal/background/service-token events lacking full HTTP headers or user IDs, and pairs with `prepare_and_sanitize_entry()` to diff fields before redacting secrets.

#### 6. Schema-by-Convention JSONB Resources
These resources stay untyped `serde_json::Value` inside `DatabaseResource.inner_content` rather than fixed Rust `struct`s so the React/client UI and upstream syncs can evolve payloads without Rust recompiles:

| Resource | Observed Core / Typical Keys | Optional / Variant Components & Reasons |
| :--- | :--- | :--- |
| `UserResource` | `email`, `password` (stripped on login), `role` (`Admin`, `Operator`, `User`, `SuperAdmin`, or custom tenant role) | `tenantId`/`tenant_id`, `SanghBranch`/`sanghBranch`/`sthaniyaBranch` (auto-inherited via `access.rs` when omitted), `isSuperAdmin`: optional for single-tenant or legacy admin users. |
| `VehicleResource` | TQMS PascalCase (`VehicleId`, `VehicleNumber`, `NormalizedVehicleNumber`, `VehicleType`, `Registration`, `Owners`) and/or camelCase (`vehicleNumber`, `vehicleType`, `id`) | Upstream TQMS owner/registration subfields vary across vehicles; untyped JSONB prevents data loss during concurrent TQMS sync (`ensure_tqms_vehicles_in_cache_and_db`). |
| `MemberResource` | Member profile + vehicle link (`vehicleId`/`VehicleId`, `vehicleNumber`/`VehicleNumber`), optional displaced-tagging & timestamp metadata | Optional displaced status/vehicle references allow standalone members as well as vehicle-linked dashboard aggregation (`members.rs`). |
| `BillingRuleResource` | Discriminated by `resourceType` (`"FEE_HEAD"` or `"BILLING_RULE"`), `name`/`title`, `amount`, `scope`, `status`, `displayOrder`, `code`, `billingEntityId`, `fiscalYear` | Combines fee heads and billing rules in one table (`main.rs` tests); optional `sthaniyaBranch` (`"Loadboard sthaniya"`) and migrated legacy `fiscalYearId` are normalized on boot/create. |
| `BillingEntityResource`, `BranchResource`, `VehicleCategoryResource`, `ReportResource` | Flexible client-defined JSONB attributes (`name`, `code`, `BranchCode`, report payload/parameters, etc.) | Stored as `DatabaseResource` JSONB so CRUD, search (`search_innercontent`), and NDJSON streaming work uniformly without rigid Rust DTOs. |
| `CacheResource` | None (no PostgreSQL table created in `Database::new()`) | Internal authorization/routing sentinel in `ResourceName`; never granted in `PermissionResource` or stored on disk. |

---

## 5. Authentication & JWT Security (`src/roles.rs`, `src/auth.rs`, `src/middleware.rs`, `src/login.rs`)

### 5.1 Roles & JWT Claims
- `Roles`: `User`, `Admin`, `Operator` (`From<String>` strictly matches `"User"`, `"Admin"`, `"Operator"`).
- `Claims` (`src/auth.rs`):
  - `sub: String` (email)
  - `exp: usize`, `iat: usize` (24-hour expiration)
  - `role: Roles` (base Actix middleware role)
  - `user_id: i32`
  - `jti: String` (unique token ID)
  - `role_name: Option<String>` (effective role such as `"SuperAdmin"`, `"Cashier"`, `"Viewer"`, `"Admin"`)
  - `tenant_id: Option<String>` (tenant scope, normalized lowercase)
- `Claims::effective_role_name()` resolves the custom/platform `role_name` or falls back to base `role`.
- Signing & Verification:
  - `create_jwt_with_context`, `create_jwt_with_role_and_user_id`, `create_jwt_with_role`: RS512 signed with `private.pem`.
  - `verify_jwt`: verifies against `public.pem` allowing `RS256`, `RS384`, `RS512`.

### 5.2 Middleware (`src/middleware.rs`)
- `AuthMiddleware` constructors:
  - `AuthMiddleware::new(role)` — requires a single `Roles` variant
  - `AuthMiddleware::with_roles(vec![...])` — allows any of the specified base roles
- Execution flow:
  1. Extracts `Authorization: Bearer <token>`.
  2. Checks `!crate::access::is_token_revoked(token)` (returns `401 Unauthorized` if revoked on logout).
  3. Calls `auth::verify_jwt(token)` and checks `required_roles`.
  4. Inserts `Claims` into `req.extensions()` and passes request downstream.

### 5.3 Login & Logout
- **`POST /api/v1/auth/login`** (`login::login`, `LoginCredentials { email, password }`):
  - Finds user in `UserResource`, validates password, strips `password` from response payload (`LoginResponse`).
  - Supports built-in roles (`Admin`, `Operator`, `User`), `SuperAdmin` (`inner_content.isSuperAdmin == true`, cross-tenant bypass without needing a `PermissionResource`), and custom roles (`Cashier`, `Viewer`, etc., mapped to base `Roles::User` with `Claims.role_name` and `Claims.tenant_id`).
  - Checks if a v1.0 `RolePermissionPolicy` exists for `(tenant_id, role_name)`; otherwise falls back to `access::load_permission_on_login`.
  - Emits asynchronous `log` and `audit` events.
- **`POST /api/v1/auth/logout`** (`access::logout`):
  - Revokes the active JWT in the in-memory revocation store, cleans up legacy session permission cache if applicable, and emits a logout audit event.

---

## 6. Authorization & Permission System (`src/permissions.rs` & `src/access.rs`)

The backend implements a **dual-mode authorization architecture** so v1.0 role/tenant RBAC coexists seamlessly with legacy per-user permissions without breaking existing routes:

### 6.1 Role-Based Tenant RBAC v1.0 (`src/permissions.rs`)
- Constants:
  - `PERMISSION_SCHEMA_VERSION = "1.0"`
  - `PLATFORM_SUPER_ADMIN_ROLE = "SuperAdmin"`
- **Hardcoded Action Catalog (`ACTION_CATALOG: &[ResourceActionCatalogEntry]`)**:
  - 4 base endpoint actions:
    - `CREATE` $\rightarrow$ `POST /api/v1/create/{r}`
    - `READ` $\rightarrow$ `GET /api/v1/read/{r}/{id}`, `GET /api/v1/search/...`, `GET /api/v1/streams/search/...`
    - `UPDATE` $\rightarrow$ `PUT /api/v1/update/{r}/{id}`
    - `DELETE` $\rightarrow$ `DELETE /api/v1/delete/{r}/{id}`
  - Domain-specific actions checked in dedicated handlers:
    - `BillResource`: `CANCEL`, `PRINT` (plus bill lifecycle route checks `CORRECT`, `VOID`)
    - `ReportResource`: `EXPORT`
    - `PermissionResource`: `ASSIGN`
  - Restricted resources:
    - `CacheResource`: internal only — never granted to any role.
    - `ActionLogsResource` & `AuditTrailResource`: system-written and immutable — roles may only ever be granted `READ`, never `CREATE`, `UPDATE`, or `DELETE` (even `Admin`).
    - `SuperAdmin`: platform-level cross-tenant bypass evaluated in Rust code; **no** `PermissionResource` document is stored for it.

- **Minimal Document Schema (`RolePermissionPolicy`)**:
  ```json
  {
    "resource_name": "PermissionResource",
    "inner_content": {
      "schemaVersion": "1.0",
      "tenantId": "loadboard-sthaniya",
      "role": "Admin",
      "status": "ACTIVE",
      "branchIds": [],
      "allow": {
        "BillResource": ["CREATE", "READ", "UPDATE", "CANCEL", "PRINT"],
        "MemberResource": ["*"],
        "VehicleResource": ["*"],
        "VehicleCategoryResource": ["*"],
        "BillingRuleResource": ["*"],
        "BillingEntityResource": ["*"],
        "BranchResource": ["*"],
        "ReportResource": ["READ", "EXPORT"],
        "SettingResource": ["READ", "UPDATE"],
        "UserResource": ["*"],
        "PermissionResource": ["READ", "ASSIGN"],
        "ActionLogsResource": ["READ"],
        "AuditTrailResource": ["READ"]
      },
      "deny": {
        "BillResource": ["DELETE"]
      },
      "updatedBy": 3
    },
    "external_content": {}
  }
  ```
- **Policy Evaluation (`is_allowed`)**:
  1. Require `status == "ACTIVE"` (case-insensitive normalization during validation) and `tenant_id == tenant`.
  2. If `branch` is `Some(b)` and `policy.branch_ids` is non-empty, require `branch_ids.contains(&b)` (empty `branchIds` means all branches).
  3. Check `deny` across `[resource, "*"]` matching `[action, "*"]` $\rightarrow$ **deny always wins**.
  4. Check `allow` across `[resource, "*"]` matching `[action, "*"]` $\rightarrow$ otherwise **deny by default**.
- **Runtime Enforcement (`enforce_request_permission`)**:
  - Returns `PermissionCheckOutcome::{FallbackLegacy, Allowed, Denied}`.
  - If no v1.0 policy is stored for the caller's `(tenant_id, role)`, returns `FallbackLegacy` so legacy `AuthMiddleware` + per-user access continues working.
  - On `Denied`, automatically emits an `ACCESS_DENIED` entry via `audit.rs`.
- **v1.0 Permission Endpoints (`/api/v1/permissions`)**:
  - `GET /api/v1/permissions/catalog` — returns `ACTION_CATALOG`
  - `GET /api/v1/permissions/me` — caller's resolved role policy
  - `POST /api/v1/permissions/check` (`CheckPermissionRequest`) — programmatic check
  - `GET /api/v1/permissions` (`ListPoliciesQuery`) — list policies (`PermissionResource:READ`)
  - `GET /api/v1/permissions/{tenant_id}/{role}` — fetch specific policy
  - `POST /api/v1/permissions` — create/upsert normalized policy (`PermissionResource:ASSIGN`)
  - `PUT /api/v1/permissions/{id}` — update policy (`PermissionResource:ASSIGN`)
  - `POST /api/v1/permissions/seed-defaults` (`SeedDefaultsRequest`) — seeds `Admin`, `Cashier`, `Viewer` templates (`default_admin_policy`, `default_cashier_policy`, `default_viewer_policy`)
  - `POST /api/v1/permissions/repair` — audits/repairs missing/invalid policies

### 6.2 Legacy Per-User Permissions & Branch Inheritance (`src/access.rs`)
- Structs: `ScopeId`, `AllowedScope`, `AllowedRule`, `PermissionInnerContent`, `PermissionResource`, `PermissionError`, `CreatePermissionRequest`, `UpdatePermissionRequest`.
- Branch inheritance helpers: `extract_sangh_branch`, `resolve_creator_own_sangh_branch`, `resolve_branch_for_new_creation`, `get_setting_sangh_branch`. Used during `UserResource` and `BillingRuleResource` creation when branch metadata is omitted.
- Lifecycle helpers called by `login.rs` and `actions.rs`: `create_permission_in_db_strict`, `ensure_permission_in_db_for_user`, `load_permission_on_login`, `destroy_permission_on_logout`, `sync_permission_on_user_update`, `delete_permission_for_user`.
- Endpoints (`/api/v1/access`):
  - `GET /api/v1/access/me`
  - `GET /api/v1/access/user/{user_id}`
  - `PUT /api/v1/access/user/{user_id}` (enforces `PermissionResource:ASSIGN`)
  - `POST /api/v1/access/permission` (enforces `PermissionResource:ASSIGN`)

---

## 7. Generic CRUD & High-Volume Streaming (`src/actions.rs`, `src/api_stream.rs`)

### 7.1 Generic Resource Endpoints
All routes verify caller authentication (`AuthMiddleware`) and v1.0 `enforce_request_permission`:
- `POST /api/v1/create/{resource_name}` (`actions::write`)
- `GET /api/v1/read/{resource_name}/{id}` (`actions::read`, supports `"all"` or numeric `i32` ID)
- `PUT /api/v1/update/{resource_name}/{id}` (`actions::update`)
- `DELETE /api/v1/delete/{resource_name}/{id}` (`actions::delete`)
- `GET /api/v1/search/{resource_name}/{search_query}/{search_item}/{limit}` (`actions::search`, dotted-path matching)

**Important Resource-Specific Guards in `actions.rs`:**
- `UserResource` & `PermissionResource` always bypass `Cache.cache` via `is_uncached_resource` / `read_uncached_resource_from_db`; creating a `UserResource` never invalidates cached tables, and also syncs legacy permissions (`ensure_permission_in_db_for_user` / `sync_permission_on_user_update` / `delete_permission_for_user`).
- `AuditTrailResource` is strictly immutable (update/delete rejected).
- `BillingRuleResource` and `UserResource` auto-inherit creator `sanghBranch` / `sthaniyaBranch` via `access.rs`.
- `MemberResource` integrates with `SettingResource.vehicleManagement` for displaced tagging metadata.
- Every mutation asynchronously emits both an `ActionLogEntry` (`log.rs`) and an `AuditTrailEntry` (`audit.rs`) capturing sanitized `before_state` / `after_state`.

### 7.2 Generic NDJSON Streaming (`src/api_stream.rs`)
- **`GET /api/v1/streams/search/{resource_name}/{search_query}/{search_item}/{limit}/{offset}`**:
  - Streams matching resources from cache as **NDJSON** (`application/x-ndjson`) in batches (chunks of `5000` when limit is `"all"`).
  - Sets response headers: `X-Batch-Size`, `X-Starting-Offset`, `X-Accel-Buffering: no`.
  - Enforces `READ` permission via v1.0 policy when configured.

---

## 8. Persistent Action Logging & Structured Audit Trail (`src/log.rs`, `src/audit.rs`)

Both subsystems use an **unbounded, lock-free MPSC queue (`tokio::sync::mpsc::unbounded_channel`)** feeding a background Tokio worker that drains batches, persists to PostgreSQL + `Cache`, and flushes cleanly on shutdown.

### 8.1 Action Logs (`src/log.rs`)
- **Bill Rollback Store**: `ActionType`, `BillLogEntry`, `rollback_action`, and in-memory `LOG_STORE` supporting `/api/v1/bills/rollback/{id}` and `/api/v1/bills/logs`.
- **Persistent Action Logs (`ActionLogsResource`)**:
  - `ResourceActionKind`: action classification enum.
  - `ActionLogEntry`: records with sequential IDs `ACTLOG-<seq>` initialized from existing DB rows on startup.
  - `AsyncActionLogger` & `ActionLogDbSink`: background worker drains `mpsc::unbounded_channel`, batch-persists rows into PostgreSQL `actionlogsresource`, inserts them into `Cache`, and optionally appends NDJSON lines to `ACTION_LOG_FILE` (`logs/actions.jsonl` outside tests).
- **Endpoints (Admin-only)**:
  - `GET /api/v1/logs/actions` (`ActionLogQuery`)
  - `GET /api/v1/logs/actions/{resource_name}`

### 8.2 Audit Trail (`src/audit.rs`)
- `AUDIT_SCHEMA_VERSION = 1`
- **Core Enums & Structs**:
  - `AuditCategory`: `Resource`, `Billing`, `Auth`, `AccessControl`, `Settings`, `Notification`, `Integration`
  - `AuditAction`: 12 variants covering CRUD, billing lifecycle, login/logout, access denied, settings changes, notifications, sync
  - `AuditOutcome`: `Success`, `Denied`, `Failure`
  - `FieldChangeType`, `FieldChange`: structured field-level change records
  - `AuditActor`: `user_id`, `email`, `role` / `role_name`, `tenant_id`, etc.
  - `AuditRequestContext`: method, path, remote IP, user agent, etc.
  - `AuditTrailEntry`: sequential ID `AUD-<seq>`, sanitized `before_state` / `after_state`, `changes: Vec<FieldChange>`
- **Diff-Before-Redact Pipeline**: `prepare_and_sanitize_entry` computes accurate field diffs *before* redacting sensitive credentials/tokens/secrets so changes on sensitive keys are recorded safely (`***REDACTED***`) without storing plaintext secrets.
- **Worker (`AsyncAuditTrailLogger` / `AuditTrailDbSink`)**: drains up to `256` entries per tick from `mpsc::unbounded_channel`, persists to `AuditTrailResource` in chunks of `32`, updates `Cache`, and advances `AUD-<seq>` across restarts.
- **Endpoints (Admin-only)**:
  - `GET /api/v1/audit` (`AuditTrailQuery` $\rightarrow$ `AuditTrailPage`)
  - `GET /api/v1/audit/summary` (`AuditSummaryQuery` $\rightarrow$ `AuditTrailSummary`)
  - `GET /api/v1/audit/entity/{resource_name}/{resource_id}` (`EntityHistoryQuery` $\rightarrow$ `EntityAuditHistory`)

---

## 9. Centralized Business Rules & Settings Engine (`src/settings.rs`)

All configurable tenant/billing/SMS/fiscal policies live in `SettingResource` and deserialize into `AppSettings`:

### 9.1 `AppSettings` Structure
- `billing: BillingSettings`:
  - `vat: VatSettings` (`rate`, `enabled`, `billingEntityIds`)
  - `arrears: ArrearsSettings` (`monthlyPaid: MonthlyPaidArrearsSettings`, `paidExpired: PaidExpiredArrearsSettings`)
  - `linkedFeeRules: LinkedFeeRulesSettings` (`rules: Vec<LinkedFeeRule>`, `enabled`)
- `receipts: ReceiptsSettings` (`cancellation: ReceiptCancellationSettings` with `hours`, `enabled`)
- `insurance: InsuranceSettings` (`eligibility: InsuranceEligibilitySettings` with `enabled`, `ruleIds`, `minPaidRules`)
- `fiscalYear: FiscalYearSettings`:
  - `selector: FiscalYearSelectorSettings` (`yearsBack`, `yearsForward`, `archivedYears`)
  - `billingAccess: FiscalYearBillingAccessSettings` (`nextYear`, `previousYear: Option<FiscalYearAccessEntry>`)
- `integrations: IntegrationsSettings` (`tqms: TqmsIntegrationSettings` with `SanghBranch`)
- `notifications: NotificationsSettings` (`sms: SmsNotificationSettings` with `apiKey`, `baseUrl`, `enabled`, `provider`, `senderId`, `apiSecret`, `templates`, `webhookUrl`, `defaultCountryCode`, etc.)
- `vehicleManagement: VehicleManagementSettings` (`DisplacedTaggingSettings`)

### 9.2 Settings Evaluators & API Endpoints (`/api/v1/settings`)
- **Evaluators**: `calculate_vat` (`VatCalculationResult`), `evaluate_vehicle_arrears` (`ArrearsEvaluationResult`), `evaluate_linked_fee_rules` (`LinkedFeeEvaluationResult`), `check_receipt_cancellation` (`ReceiptCancellationCheck`), `evaluate_insurance_eligibility` (`InsuranceEligibilityResult`), `generate_fiscal_year_selector` (`FiscalYearSelectorResult`), `check_fiscal_year_billing_access` (`FiscalYearBillingAccessResult`).
- **Endpoints**:
  - `GET /api/v1/settings/resolved` — returns full `AppSettings` (SMS `apiKey` / `apiSecret` redacted for non-Admin callers)
  - `POST /api/v1/settings/billing/calculate-vat` (`VatRequest`)
  - `GET /api/v1/settings/billing/arrears/{vehicle}` (`ArrearsQuery`)
  - `POST /api/v1/settings/billing/linked-fees/evaluate` (`LinkedFeeEvalRequest`)
  - `GET /api/v1/settings/receipts/cancellation-check/{bill_id}`
  - `GET /api/v1/settings/insurance/eligibility/{vehicle}` (`InsuranceEligibilityQuery`)
  - `GET /api/v1/settings/fiscal-year/selector`
  - `GET /api/v1/settings/fiscal-year/access/{fiscal_year:.*}`
  - `GET /api/v1/settings/vehicle-management/displaced-tagging`

---

## 10. Billing Actions & Lodbod External Integration (`src/billing_actions.rs`)

### 10.1 Core Bill Models & Endpoints
- Models: `DatabaseBillResource`, `BillResource`, `BillItem`, `BranchInfo`, `BillVehicleInfo`, `CreatedByInfo`.
- Constant: `REQUIRED_STHANIYA_BRANCH = "Loadboard sthaniya"`.
- **Bill Endpoints (`/api/v1/bills`)**:
  - `POST /api/v1/bills/create` (`create_bill`)
  - `PUT /api/v1/bills/modify/{id}` (`modify_bill`)
  - `DELETE /api/v1/bills/delete/{id}` (`delete_bill`)
  - `POST /api/v1/bills/rollback/{id}` (`rollback_bill`)
  - `GET /api/v1/bills/logs` (`get_bill_logs`)
  - `GET /api/v1/bills/logs/{bill_id}` (`get_bill_logs_by_bill_id`)
- Enforces `SettingResource` evaluators (VAT calculation, fiscal year billing access, linked fee rules, receipt cancellation window) and domain permissions (`CREATE`, `CORRECT`, `CANCEL`, `VOID`), and emits rollback logs, `ActionLogsResource`, and `AuditTrailResource` entries.

### 10.2 Lodbod Integration (`/api/integrations/lodbod/v1`)
- **Authentication**: Accepts either static `Authorization: Bearer {LODBOD_SERVICE_TOKEN}` or a valid `Admin`/`Operator` JWT.
- **Request/Response Models**:
  - Lookup: `LodbodVehicleQuery`, `GetVehicleBillingDetailsRequest`, `OwnerInfo`, `OwnersResolution`, `PeriodItem`, `FeeItem`, `PaymentHistoryItem`, `VehicleBillingDetails`
  - Payment Submission & Status: `SelectedFeePeriod`, `SelectedFee`, `SubmitVehiclePaymentRequest`, `VehiclePaymentResult`, `PaymentStatusVehicleItem`, `PaymentStatusResponseData`, `LodbodPaymentUpdatePayload`
  - Envelope: `ApiResponse<T>`, `ApiErrorDetails`, `ApiErrorResponse`
- **Reliability Features**:
  - Generates `ONL-XXXXXX` online receipt numbers.
  - In-memory `IDEMPOTENCY_STORE` and `RECONCILIATION_STORE` prevent duplicate payment creation on retries and track reconciliation state.
  - Webhook helper `send_payment_update_to_lodbod(&payload, receiver_url)` dispatches outbound payment callbacks.
- **Lodbod Endpoints**:
  - `POST /api/integrations/lodbod/v1/vehicles/billing-details` (`get_vehicle_billing_details`)
  - `POST /api/integrations/lodbod/v1/payments/vehicle-receipts` (`submit_vehicle_payment`)
  - `GET /api/integrations/lodbod/v1/payments/{lodbodPaymentReferenceId}/vehicle-receipts` (`get_payment_receipt_status`)

---

## 11. TQMS Vehicle Integration & Streaming Sync (`src/tqms_integration.rs`)

- **Client**: `TQMSClient` authenticates with TQMS (`TQMS_BASE_URL`) via OAuth2 `cts_client` client credentials (`ensure_token`, `check_validity`).
- **Models**: `Status`, `VehicleSearchParameters`, `StreamedVehicleItem`, `StreamSyncFormat` (SSE default; selects NDJSON if `Accept: application/x-ndjson` or `?format=ndjson`).
- **Key Features**:
  - **`GET /api/v1/tqms/stream_sync/{batch_size}/{concurrency}`**:
    - Clamps `batch_size` and `concurrency` to safe maximums, fetches page 1 to discover `TotalItems` and `TotalPages`, then fetches remaining pages concurrently with exponential backoff retries.
    - Persists vehicle deltas to PostgreSQL and updates `Cache` incrementally via `ensure_tqms_vehicles_in_cache_and_db`.
    - Streams progress/items over **SSE** (`text/event-stream`) or **NDJSON** (`application/x-ndjson`) with `X-Stream-Format`, `X-Batch-Size`, `X-Concurrency`, `X-Total-Items`, `X-Total-Pages`, and `X-Accel-Buffering: no`.
  - Additional endpoints/handlers:
    - `sync_vehicles_tqms`, `sync_vehicle_tqms_concurrency_limited`
    - `validate_vehicle`, `get_vehicle_details`
    - `GET /api/v1/read_all/VehicleTypes` (`read_all_vehicle_types`)
    - `GET /api/v1/get_vehicles_tqms` (`get_vehicle_from_tqms`)

---

## 12. SMS Notifications (`src/sms.rs`)

- **Provider**: SamayaSMS (`DEFAULT_SMS_BASE_URL = "https://samayasms.com.np/smsapi/index.php"`, `MAX_SMS_CHAR_LEN = 720`, 500ms send throttling).
- **Route/Campaign Pricing Handling**: Includes `DEFAULT_SMS_CAMPAIGN_ID = "8572"` and `DEFAULT_SMS_ROUTE_ID = "10244"` (and env overrides `SMS_ROUTE_ID` / `SMS_CAMPAIGN_ID`) and handles account-specific route pricing API responses cleanly.
- **Settings-Driven Templates**:
  - Dynamically loads `notifications.sms` from `SettingResource` (`generate_templates_from_server_data`, `find_sms_template_in_server_data`, `find_sms_notification_settings_in_server_data`).
  - Supports Nepali/Unicode templates from `SettingResource` (e.g., `PAID`, `PAYMENT REMAINDER`, `URGENT`) with placeholder substitution (`{vehicleNumber}`, `{amount}`, `{organizationName}`, `{expiryDate}`, `{dueDate}`).
- **Structs**: `SmsType`, `SettingSmsTemplate`, `GeneratedSmsTemplate`, `SmsTemplate`, `SmsConfig`, `SmsSendResult`, `SmsClient`, `SendRawSmsRequest`.
- **Endpoints (`/api/v1/sms`)**:
  - `GET /api/v1/sms/templates` (`list_sms_templates`)
  - `GET /api/v1/sms/templates/{template_id}` (`get_sms_template_by_id`)
  - `POST /api/v1/sms/preview` (`preview_sms_template`)
  - `POST /api/v1/sms/send` (`send_template_sms`)
  - `POST /api/v1/sms/send-raw` (`send_raw_sms`)

---

## 13. Dashboard Statistics, Members & Nepali Date Mapping (`src/dashboard_statistics.rs`, `src/members.rs`, `src/date.rs`)

- **Nepali BS/AD Fiscal Date Mapping (`src/date.rs`)**:
  - Types: `DateType`, `Date`, `DateParams`, `DateRange`, `DateMapper`.
  - Note: Workspace root stores `date_range.json` while `DateMapper::new()` reads `"date_mappings.json"`; check file naming when deploying or updating mappings.
- **Dashboard Statistics (`src/dashboard_statistics.rs`)**:
  - Structs: `DashBoardStatistics`, `DashboardQuery`.
  - Routes:
    - `GET /api/v1/dashboard/statistics` (`get_dashboard_statistics`)
    - `GET /api/v1/dashboard/statistics/{fiscal_year}` (`get_dashboard_statistics_by_fy`)
- **Dashboard & Member Metrics (`src/members.rs`)**:
  - Struct: `DashboardMembers`.
  - Handlers cover members, total users, total active users, new users, displaced users, and inactive-year counts both overall and `by_fy`:
    - `get_members`, `get_members_by_fy`
    - `get_total_users`, `get_total_users_by_fy`
    - `get_total_active_users`, `get_total_active_users_by_fy`
    - `get_new_users`, `get_new_users_by_fy`
    - `get_displaced_users`, `get_displaced_users_by_fy`
    - `get_inactive_year`, `get_inactive_year_by_fy`

---

## 14. Build, Verification & Testing Guide

### 14.1 Compile & Run
```bash
# Typecheck application and unit/integration tests
cargo check --bin v1 --tests

# Run startup migrations only
cargo run --bin v1 -- --run-migrations

# Run server (requires PostgreSQL DATABASE_URL + PEM files)
cargo run --bin v1
```

### 14.2 Running Tests Safely
Most unit and integration tests use `access::test_helpers::create_mock_server_data_with_state` (`MockDbState`, an in-memory PostgreSQL wire-protocol mock), so no live PostgreSQL instance is needed for the test suite.

> **Important Pitfall:** `sms::tests::test_live_send_paid_sms_using_setting_resource_template` performs a **live outbound SMS HTTP request** via SamayaSMS. Always exclude `test_live_` tests during automated verification:
```bash
cargo test --bin v1 -- --skip test_live_
```
