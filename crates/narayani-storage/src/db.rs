use narayani_core::resource::{DatabaseResource, ResourceName};
use futures_util::future::try_join_all;
use std::sync::Arc;
use tokio_postgres::{self}; // Add ResourceName import assuming it's in the same module

pub async fn connect_to_db(
    database_url: &str,
) -> Result<tokio_postgres::Client, tokio_postgres::Error> {
    let (client, connection) = tokio_postgres::connect(database_url, tokio_postgres::NoTls).await?;
    // Spawn a new thread to run the connection
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("Connection error: {}", e);
        }
    });
    Ok(client)
}

#[derive(Debug, Clone)]
pub struct Database {
    pub resource_name: Vec<ResourceName>,
    pub client: Arc<tokio_postgres::Client>,
}

impl Database {
    pub fn new(client: Arc<tokio_postgres::Client>) -> Self {
        let resource_name = vec![
            ResourceName::UserResource,
            ResourceName::PermissionResource,
            ResourceName::VehicleResource,
            ResourceName::BillResource,
            ResourceName::MemberResource,
            ResourceName::ReportResource,
            ResourceName::SettingResource,
            ResourceName::VehicleCategoryResource,
            ResourceName::BillingRuleResource,
            ResourceName::BillingEntityResource,
            ResourceName::BranchResource,
            ResourceName::ActionLogsResource,
            ResourceName::AuditTrailResource,
            ResourceName::VehicleHistoryResource,
            ResourceName::FileResource,
        ];
        Database {
            resource_name,
            client,
        }
    }

    pub async fn create_tables(&self) -> Result<(), tokio_postgres::Error> {
        for resource in &self.resource_name {
            let query = format!(
                "CREATE TABLE IF NOT EXISTS {} (id SERIAL PRIMARY KEY, inner_content JSONB, external_content JSONB);",
                resource.to_string().to_lowercase()
            );
            self.client.execute(&query[..], &[]).await?;
        }

        Ok(())
    }

    pub async fn read_resource(
        &self,
        resource_name: ResourceName,
        id: i32,
    ) -> Result<DatabaseResource, tokio_postgres::Error> {
        let query = format!(
            "SELECT * FROM {} WHERE id = $1;",
            resource_name.to_string().to_lowercase()
        );
        let rows = self.client.query(&query[..], &[&id]).await?;
        if rows.is_empty() {
            return Ok(DatabaseResource {
                id: None,
                resource_name: resource_name.into(),
                inner_content: None,
                external_content: None,
            });
        } else {
            let row = &rows[0];
            let id: i32 = row.get(0);
            let inner_content: Option<serde_json::Value> = row.get(1);
            let external_content: Option<serde_json::Value> = row.get(2);
            let resource = DatabaseResource {
                id: Some(id),
                resource_name: resource_name.into(),
                inner_content,
                external_content,
            };
            Ok(resource)
        }
    }

    pub async fn read_all_resource(
        &self,
        resource_name: ResourceName,
    ) -> Result<Vec<DatabaseResource>, tokio_postgres::Error> {
        let query = format!(
            "SELECT id, inner_content, external_content FROM {};",
            resource_name.clone().to_string().to_lowercase()
        );
        let rows = self.client.query(&query[..], &[]).await?;
        let mut resources = Vec::new();
        for row in rows {
            let id: i32 = row.get(0);
            let inner_content: Option<serde_json::Value> = row.get(1);
            let external_content: Option<serde_json::Value> = row.get(2);
            let resource = DatabaseResource {
                id: Some(id),
                resource_name: Some(resource_name.into()),
                inner_content,
                external_content,
            };
            resources.push(resource);
        }
        Ok(resources)
    }

    pub async fn create_resource(
        &self,
        resource_name: ResourceName,
        resource: DatabaseResource,
    ) -> Result<i32, tokio_postgres::Error> {
        let query = format!(
            "INSERT INTO {} (inner_content, external_content) VALUES ($1, $2) RETURNING id;",
            resource_name.to_string().to_lowercase()
        );
        //Add time of creation to the external content of the resource
        
        // Stamp creation time (second precision) into external_content as `createdTime`
        let mut external_content = resource.external_content;
        let created_time = serde_json::Value::String(
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        );
        match external_content {
            Some(serde_json::Value::Object(ref mut map)) => {
                map.insert("createdTime".to_string(), created_time);
            }
            None | Some(serde_json::Value::Null) => {
                external_content = Some(serde_json::json!({ "createdTime": created_time }));
            }
            // Non-object external content cannot hold a key; leave it untouched
            Some(_) => {}
        }
        let rows = self
            .client
            .query(
                &query[..],
                &[&resource.inner_content, &external_content],
            )
            .await?;
        let id: i32 = rows[0].get(0);
        Ok(id)
    }

    pub async fn batch_create_resources(
        &self,
        resource_name: ResourceName,
        resources: Vec<DatabaseResource>,
    ) -> Result<Vec<i32>, tokio_postgres::Error> {
        let futures = resources
            .into_iter()
            .map(|resource| self.create_resource(resource_name.clone(), resource));

        let ids = try_join_all(futures).await?;

        Ok(ids)
    }

    pub async fn update_resource(
        &self,
        resource_name: ResourceName,
        id: i32,
        resource: DatabaseResource,
    ) -> Result<(), tokio_postgres::Error> {
        let query = format!(
            "UPDATE {} SET inner_content = $1, external_content = $2 WHERE id = $3;",
            resource_name.to_string().to_lowercase()
        );
        self.client
            .execute(
                &query[..],
                &[&resource.inner_content, &resource.external_content, &id],
            )
            .await?;
        Ok(())
    }

    pub async fn delete_resource(
        &self,
        resource_name: ResourceName,
        id: i32,
    ) -> Result<(), tokio_postgres::Error> {
        let query = format!(
            "DELETE FROM {} WHERE id = $1;",
            resource_name.to_string().to_lowercase()
        );
        self.client.execute(&query[..], &[&id]).await?;
        Ok(())
    }
    pub async fn delete_all_resources(
        &self,
        resource_name: ResourceName,
    ) -> Result<(), tokio_postgres::Error> {
        let query = format!("DELETE FROM {};", resource_name.to_string().to_lowercase());
        self.client.execute(&query[..], &[]).await?;
        Ok(())
    }
    pub async fn delete_tables(&self) -> Result<(), tokio_postgres::Error> {
        for resource in &self.resource_name {
            let query = format!(
                "DROP TABLE IF EXISTS {}",
                resource.to_string().to_lowercase()
            );
            self.client.execute(&query[..], &[]).await?;
        }
        Ok(())
    }

    pub async fn create_report_tables(&self) -> Result<(), tokio_postgres::Error> {
        let ddl = r#"
            CREATE TABLE IF NOT EXISTS report_receipts (
                receipt_id INTEGER PRIMARY KEY,
                receipt_number TEXT NOT NULL DEFAULT '',
                billing_type TEXT NOT NULL DEFAULT 'MEMBERSHIP',
                status TEXT NOT NULL DEFAULT 'PAID',
                branch_id TEXT NOT NULL DEFAULT '',
                branch_name_lower TEXT NOT NULL DEFAULT '',
                fiscal_year_id TEXT NOT NULL DEFAULT '',
                fiscal_year_canonical TEXT NOT NULL DEFAULT '',
                created_by_id TEXT NOT NULL DEFAULT '',
                created_by_name TEXT NOT NULL DEFAULT '',
                created_by_username TEXT NOT NULL DEFAULT '',
                vehicle_id TEXT NOT NULL DEFAULT '',
                vehicle_number_norm TEXT NOT NULL DEFAULT '',
                vehicle_type_norm TEXT NOT NULL DEFAULT '',
                customer_name TEXT NOT NULL DEFAULT '',
                customer_phone TEXT NOT NULL DEFAULT '',
                payment_date DATE,
                billing_date DATE,
                period_start DATE,
                period_end DATE,
                billing_months INTEGER NOT NULL DEFAULT 0,
                subtotal DOUBLE PRECISION NOT NULL DEFAULT 0,
                discount DOUBLE PRECISION NOT NULL DEFAULT 0,
                vat_amount DOUBLE PRECISION NOT NULL DEFAULT 0,
                total_amount DOUBLE PRECISION NOT NULL DEFAULT 0,
                payment_method TEXT NOT NULL DEFAULT '',
                paid_at TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL DEFAULT ''
            );

            CREATE TABLE IF NOT EXISTS report_receipt_items (
                receipt_id INTEGER NOT NULL REFERENCES report_receipts(receipt_id) ON DELETE CASCADE,
                item_index INTEGER NOT NULL,
                fee_head_id TEXT NOT NULL DEFAULT '',
                fee_title_norm TEXT NOT NULL DEFAULT '',
                billing_entity_id TEXT NOT NULL DEFAULT '',
                amount DOUBLE PRECISION NOT NULL DEFAULT 0,
                PRIMARY KEY (receipt_id, item_index)
            );

            CREATE TABLE IF NOT EXISTS report_vehicle_aliases (
                alias_key TEXT PRIMARY KEY,
                alias_kind TEXT NOT NULL DEFAULT '',
                alias_vehicle_id TEXT NOT NULL DEFAULT '',
                alias_vehicle_number_norm TEXT NOT NULL DEFAULT '',
                current_vehicle_id TEXT NOT NULL DEFAULT '',
                current_vehicle_number TEXT NOT NULL DEFAULT '',
                previous_vehicle_number TEXT NOT NULL DEFAULT ''
            );

            CREATE INDEX IF NOT EXISTS idx_rr_fy_branch_status_pay
                ON report_receipts (fiscal_year_canonical, branch_name_lower, status, payment_date DESC);
            CREATE INDEX IF NOT EXISTS idx_rr_fy_branch_status_bill
                ON report_receipts (fiscal_year_canonical, branch_name_lower, status, billing_date DESC);
            CREATE INDEX IF NOT EXISTS idx_rr_vehicle_id_type_pay
                ON report_receipts (vehicle_id, billing_type, payment_date DESC);
            CREATE INDEX IF NOT EXISTS idx_rr_vehicle_num_type
                ON report_receipts (vehicle_number_norm, billing_type);
            CREATE INDEX IF NOT EXISTS idx_rr_validity
                ON report_receipts (period_start, period_end);
            CREATE INDEX IF NOT EXISTS idx_rri_fee_head
                ON report_receipt_items (fee_head_id, receipt_id);
            CREATE INDEX IF NOT EXISTS idx_rri_receipt
                ON report_receipt_items (receipt_id);
            CREATE INDEX IF NOT EXISTS idx_rva_curr_vid
                ON report_vehicle_aliases (current_vehicle_id);
        "#;
        self.client.batch_execute(ddl).await
    }

    pub async fn upsert_report_receipt(
        &self,
        proj: &crate::report_index::ProjectedReceipt,
    ) -> Result<(), tokio_postgres::Error> {
        let pdate_iso = proj
            .payment_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let bdate_iso = proj
            .billing_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let pstart_iso = proj
            .period_start
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let pend_iso = proj
            .period_end
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();

        let stmt = r#"
            INSERT INTO report_receipts (
                receipt_id, receipt_number, billing_type, status,
                branch_id, branch_name_lower, fiscal_year_id, fiscal_year_canonical,
                created_by_id, created_by_name, created_by_username,
                vehicle_id, vehicle_number_norm, vehicle_type_norm,
                customer_name, customer_phone,
                payment_date, billing_date, period_start, period_end,
                billing_months, subtotal, discount, vat_amount, total_amount,
                payment_method, paid_at, created_at
            ) VALUES (
                $1, $2, $3, $4,
                $5, $6, $7, $8,
                $9, $10, $11,
                $12, $13, $14,
                $15, $16,
                NULLIF($17, '')::date, NULLIF($18, '')::date, NULLIF($19, '')::date, NULLIF($20, '')::date,
                $21, $22, $23, $24, $25,
                $26, $27, $28
            )
            ON CONFLICT (receipt_id) DO UPDATE SET
                receipt_number = EXCLUDED.receipt_number,
                billing_type = EXCLUDED.billing_type,
                status = EXCLUDED.status,
                branch_id = EXCLUDED.branch_id,
                branch_name_lower = EXCLUDED.branch_name_lower,
                fiscal_year_id = EXCLUDED.fiscal_year_id,
                fiscal_year_canonical = EXCLUDED.fiscal_year_canonical,
                created_by_id = EXCLUDED.created_by_id,
                created_by_name = EXCLUDED.created_by_name,
                created_by_username = EXCLUDED.created_by_username,
                vehicle_id = EXCLUDED.vehicle_id,
                vehicle_number_norm = EXCLUDED.vehicle_number_norm,
                vehicle_type_norm = EXCLUDED.vehicle_type_norm,
                customer_name = EXCLUDED.customer_name,
                customer_phone = EXCLUDED.customer_phone,
                payment_date = EXCLUDED.payment_date,
                billing_date = EXCLUDED.billing_date,
                period_start = EXCLUDED.period_start,
                period_end = EXCLUDED.period_end,
                billing_months = EXCLUDED.billing_months,
                subtotal = EXCLUDED.subtotal,
                discount = EXCLUDED.discount,
                vat_amount = EXCLUDED.vat_amount,
                total_amount = EXCLUDED.total_amount,
                payment_method = EXCLUDED.payment_method,
                paid_at = EXCLUDED.paid_at,
                created_at = EXCLUDED.created_at;
        "#;

        self.client
            .execute(
                stmt,
                &[
                    &proj.receipt_id,
                    &proj.receipt_number,
                    &proj.billing_type,
                    &proj.status,
                    &proj.branch_id,
                    &proj.branch_name_lower,
                    &proj.fiscal_year_id,
                    &proj.fiscal_year_canonical,
                    &proj.created_by_id,
                    &proj.created_by_name,
                    &proj.created_by_username,
                    &proj.vehicle_id,
                    &proj.vehicle_number_norm,
                    &proj.vehicle_type_norm,
                    &proj.customer_name,
                    &proj.customer_phone,
                    &pdate_iso,
                    &bdate_iso,
                    &pstart_iso,
                    &pend_iso,
                    &proj.billing_months,
                    &proj.subtotal,
                    &proj.discount,
                    &proj.vat_amount,
                    &proj.total_amount,
                    &proj.payment_method,
                    &proj.paid_at,
                    &proj.created_at,
                ],
            )
            .await?;

        self.client
            .execute(
                "DELETE FROM report_receipt_items WHERE receipt_id = $1;",
                &[&proj.receipt_id],
            )
            .await?;

        let item_stmt = r#"
            INSERT INTO report_receipt_items (
                receipt_id, item_index, fee_head_id, fee_title_norm, billing_entity_id, amount
            ) VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (receipt_id, item_index) DO UPDATE SET
                fee_head_id = EXCLUDED.fee_head_id,
                fee_title_norm = EXCLUDED.fee_title_norm,
                billing_entity_id = EXCLUDED.billing_entity_id,
                amount = EXCLUDED.amount;
        "#;

        for item in &proj.items {
            let idx_i32 = item.item_index as i32;
            self.client
                .execute(
                    item_stmt,
                    &[
                        &proj.receipt_id,
                        &idx_i32,
                        &item.fee_head_id,
                        &item.fee_title_norm,
                        &item.billing_entity_id,
                        &item.amount,
                    ],
                )
                .await?;
        }
        Ok(())
    }

    pub async fn delete_report_receipt(&self, bill_id: i32) -> Result<(), tokio_postgres::Error> {
        self.client
            .execute(
                "DELETE FROM report_receipts WHERE receipt_id = $1;",
                &[&bill_id],
            )
            .await?;
        Ok(())
    }

    pub async fn upsert_report_vehicle_alias(
        &self,
        target: &narayani_vehicle_history::VehicleAliasTarget,
    ) -> Result<(), tokio_postgres::Error> {
        let stmt = r#"
            INSERT INTO report_vehicle_aliases (
                alias_key, alias_kind, alias_vehicle_id, alias_vehicle_number_norm,
                current_vehicle_id, current_vehicle_number, previous_vehicle_number
            ) VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (alias_key) DO UPDATE SET
                alias_kind = EXCLUDED.alias_kind,
                current_vehicle_id = EXCLUDED.current_vehicle_id,
                current_vehicle_number = EXCLUDED.current_vehicle_number,
                previous_vehicle_number = EXCLUDED.previous_vehicle_number;
        "#;
        let last_prev = target
            .previous_vehicle_numbers
            .last()
            .map(String::as_str)
            .unwrap_or("");
        for pid in &target.previous_vehicle_ids {
            let key = format!("prev_id:{}", pid.trim().to_ascii_lowercase());
            self.client
                .execute(
                    stmt,
                    &[
                        &key,
                        &"prev_id",
                        &pid.as_str(),
                        &"",
                        &target.vehicle_id,
                        &target.vehicle_number,
                        &last_prev,
                    ],
                )
                .await?;
        }
        for pnum in &target.previous_vehicle_numbers {
            if let Some(norm) = narayani_core::normalize_vehicle_number(pnum) {
                let key = format!("prev_num:{norm}");
                self.client
                    .execute(
                        stmt,
                        &[
                            &key,
                            &"prev_num",
                            &"",
                            &norm,
                            &target.vehicle_id,
                            &target.vehicle_number,
                            &pnum.as_str(),
                        ],
                    )
                    .await?;
            }
        }
        Ok(())
    }

    /// Non-destructive startup backfill: populates report_receipts & report_receipt_items
    /// ONLY when `report_receipts` is empty and the in-memory `ReportIndex` has projected receipts.
    pub async fn backfill_report_tables_if_empty(
        &self,
        index: &crate::report_index::ReportIndex,
    ) -> Result<usize, tokio_postgres::Error> {
        let count_row = self
            .client
            .query_one("SELECT COUNT(*) FROM report_receipts;", &[])
            .await?;
        let existing: i64 = count_row.get(0);
        if existing > 0 || index.receipts.is_empty() {
            return Ok(0);
        }
        let mut inserted = 0usize;
        for proj in index.receipts.values() {
            self.upsert_report_receipt(proj).await?;
            inserted += 1;
        }
        Ok(inserted)
    }
}
