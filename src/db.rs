use crate::resource::{DatabaseResource, ResourceName};
use futures::future::try_join_all;
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
}
