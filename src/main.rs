use actix_cors::Cors;
use actix_web::{web, App, HttpResponse, HttpServer};
use dotenv::dotenv;
use rustls::{Certificate, PrivateKey, ServerConfig};
use rustls_pemfile::{certs, pkcs8_private_keys, rsa_private_keys};
use std::env;
use std::fs::File;
use std::io::{self, BufReader};
use std::sync::{Arc, RwLock};
mod db;
mod login;
use crate::db::{connect_to_db, Database};
use crate::resource::{DatabaseResource, ResourceName::VehicleResource};
mod actions;
mod auth;
mod cache;
mod middleware;
mod resource;
mod roles;
mod tqms_integration;
pub mod billing_actions;
pub mod dashboard_statistics;
pub mod members;
mod api_stream;
pub mod date;
pub mod log;
pub mod audit;
pub mod sms;
pub mod access;
pub mod settings;
pub mod permissions;
pub mod vehicle_history;
pub mod vehicle_number;

#[derive(Clone)]
pub struct ServerData {
    pub db: Arc<Database>,
    pub tqms_client: Arc<tqms_integration::TQMSClient>,
    pub cache: Arc<tokio::sync::RwLock<cache::Cache>>,
}

pub async fn server_routes(s_data: web::Data<Arc<ServerData>>) -> HttpResponse {
    let resource_name = s_data.db.resource_name.clone();
    let actions = [
        "method: POST,      api/v1/create/{}",
        "method: GET,       api/v1/read/{}/{id} supports id set as all",
        "method: PUT,       api/v1/update/{}/{id}",
        "method: DELETE,    api/v1/delete/{}/{id}",
        "method: GET,       api/v1/search/{}/{search_query}/{search_item}/{limit}",
        "method: GET,       api/v1/streams/search/{}/{search_query}/{search_item}/{limit}/{offset}",
    ];
    let mut routes = vec![
        "method: GET,       /".to_string(),
        "method: POST,      api/v1/auth/login".to_string(),
        "method: POST,      api/v1/auth/logout".to_string(),
        "method: GET,       api/v1/dashboard/statistics".to_string(),
        "method: GET,       api/v1/dashboard/statistics/{fiscal_year}".to_string(),
    ];

    let member_suffixes = [
        "",
        "/{fiscal_year}",
        "/total_users",
        "/total_users/{fiscal_year}",
        "/total",
        "/total/{fiscal_year}",
        "/total_active_users",
        "/total_active_users/{fiscal_year}",
        "/active",
        "/active/{fiscal_year}",
        "/new_users",
        "/new_users/{fiscal_year}",
        "/new",
        "/new/{fiscal_year}",
        "/displaced_users",
        "/displaced_users/{fiscal_year}",
        "/displaced",
        "/displaced/{fiscal_year}",
        "/inactive_year",
        "/inactive_year/{fiscal_year}",
        "/total_inactive_users",
        "/total_inactive_users/{fiscal_year}",
        "/inactive",
        "/inactive/{fiscal_year}",
    ];
    for prefix in ["api/v1/dashboard/members", "api/v1/members"] {
        for suffix in member_suffixes {
            routes.push(format!("method: GET,       {}{}", prefix, suffix));
        }
    }

    routes.extend(
        [
            "method: POST,      api/v1/tqms/search",
            "method: GET,       api/v1/tqms/validate/{vehicle_number}",
            "method: GET,       api/v1/tqms/vehicle_details/{vehicle_number}",
            "method: GET,       api/v1/tqms/sync",
            "method: GET,       api/v1/tqms/sync/{concurency}",
            "method: GET,       api/v1/read_all/VehicleTypes",
            "method: GET,       api/v1/get_vehicles_tqms",
            "method: GET,       api/v1/tqms/stream_sync/{batch_size}/{concurrency}",
            "method: GET,       api/v1/streams/search/{resource_name}/{search_query}/{search_item}/{limit}/{offset}",
            "method: POST,      api/integrations/lodbod/v1/vehicles/billing-details",
            "method: POST,      api/integrations/lodbod/v1/payments/vehicle-receipts",
            "method: GET,       api/integrations/lodbod/v1/payments/{lodbodPaymentReferenceId}/vehicle-receipts supports lodbodPaymentReferenceId set as all",
            "method: POST,      api/v1/bills/create",
            "method: PUT,       api/v1/bills/modify/{id}",
            "method: DELETE,    api/v1/bills/delete/{id}",
            "method: POST,      api/v1/bills/rollback/{id}",
            "method: GET,       api/v1/bills/logs",
            "method: GET,       api/v1/bills/logs/{bill_id}",
            "method: GET,       api/v1/sms/templates",
            "method: GET,       api/v1/sms/templates/{template_id}",
            "method: POST,      api/v1/sms/preview",
            "method: POST,      api/v1/sms/send",
            "method: POST,      api/v1/sms/send-raw",
            "method: GET,       api/v1/access/me",
            "method: GET,       api/v1/access/user/{user_id}",
            "method: POST,      api/v1/access/permission",
            "method: PUT,       api/v1/access/user/{user_id}",
            "method: GET,       api/v1/permissions/catalog",
            "method: GET,       api/v1/permissions/me",
            "method: GET,       api/v1/permissions",
            "method: POST,      api/v1/permissions",
            "method: GET,       api/v1/permissions/{tenant_id}/{role}",
            "method: PUT,       api/v1/permissions/{id}",
            "method: POST,      api/v1/permissions/check",
            "method: POST,      api/v1/permissions/seed-defaults",
            "method: POST,      api/v1/permissions/repair",
            "method: GET,       api/v1/logs/actions",
            "method: GET,       api/v1/logs/actions/{resource_name}",
            "method: GET,       api/v1/audit",
            "method: GET,       api/v1/audit/entity/{resource_name}/{resource_id}",
            "method: GET,       api/v1/audit/summary",
            "method: GET,       api/v1/settings/resolved",
            "method: POST,      api/v1/settings/billing/calculate-vat",
            "method: GET,       api/v1/settings/billing/arrears/{vehicle}",
            "method: POST,      api/v1/settings/billing/linked-fees/evaluate",
            "method: GET,       api/v1/settings/receipts/cancellation-check/{bill_id}",
            "method: GET,       api/v1/settings/insurance/eligibility/{vehicle}",
            "method: GET,       api/v1/settings/fiscal-year/selector",
            "method: GET,       api/v1/settings/fiscal-year/access/{fiscal_year}",
            "method: GET,       api/v1/settings/vehicle-management/displaced-tagging",
        ]
        .into_iter()
        .map(String::from),
    );

    for resource in resource_name {
        for action in &actions {
            // Replace {} with resource name
            let route = action.replace("{}", &resource.to_string());
            routes.push(route);
        }
    }

    HttpResponse::Ok().json(routes)
}

//Check if user with email admin@narayanisangh.com exists, if not create one with password Admin@123
pub async fn create_admin_user_if_not_exists(db: &Database) {
    let email = "admin@narayanisangh.com";
    let password = "Admin@123";
    let setting_branch = access::get_setting_sangh_branch_from_db(db).await;

    let db_user = db.read_all_resource(resource::ResourceName::UserResource).await;
    if let Ok(users) = db_user {
        if !users.is_empty() {
            println!("Admin user already exists");
            if let Some(admin) = users.iter().find(|u| {
                u.inner_content
                    .as_ref()
                    .and_then(|c| c.get("email"))
                    .and_then(|e| e.as_str())
                    .map(|e| e.eq_ignore_ascii_case(email))
                    .unwrap_or(false)
            }) {
                if let (Some(admin_id), Some(admin_inner)) = (admin.id, &admin.inner_content) {
                    if let Err(err) = access::ensure_permission_in_db_for_user(
                        db,
                        admin_id,
                        admin_inner,
                        setting_branch,
                    )
                    .await
                    {
                        eprintln!(
                            "Failed to ensure PermissionResource for existing admin user {}: {}",
                            admin_id, err
                        );
                    }
                }
            }
            return;
        }
    }
    let mut admin_map = serde_json::Map::new();
    admin_map.insert(
        "email".to_string(),
        serde_json::Value::String(email.to_string()),
    );
    admin_map.insert(
        "password".to_string(),
        serde_json::Value::String(password.to_string()),
    );
    admin_map.insert(
        "role".to_string(),
        serde_json::Value::String("Admin".to_string()),
    );
    if let Some(ref branch) = setting_branch {
        admin_map.insert(
            "SanghBranch".to_string(),
            serde_json::Value::String(branch.clone()),
        );
    }
    let admin_inner = serde_json::Value::Object(admin_map);

    let resource = DatabaseResource {
        id: None,
        inner_content: Some(admin_inner.clone()),
        external_content: None,
        resource_name: Some(resource::ResourceName::UserResource),
    };
    match db
        .create_resource(resource::ResourceName::UserResource, resource)
        .await
    {
        Ok(admin_id) => {
            println!("Admin user created successfully");
            if let Err(err) = access::ensure_permission_in_db_for_user(
                db,
                admin_id,
                &admin_inner,
                setting_branch,
            )
            .await
            {
                eprintln!(
                    "Failed to create 1:1 PermissionResource for admin user {}: {}",
                    admin_id, err
                );
            }
        }
        Err(err) => {
            println!("Failed to create admin user: {:?}", err);
        }
    }
}

pub async fn add_sthaniya_branch_to_billing_rule_resources(db: &Database) {
    let items = match db
        .read_all_resource(resource::ResourceName::BillingRuleResource)
        .await
    {
        Ok(items) => items,
        Err(err) => {
            eprintln!(
                "Failed to read BillingRuleResource for sthaniyaBranch migration: {:?}",
                err
            );
            return;
        }
    };

    let mut updated = 0usize;
    let mut skipped = 0usize;

    for mut item in items {
        let Some(id) = item.id else {
            continue;
        };

        let Some(serde_json::Value::Object(ref mut map)) = item.inner_content else {
            continue;
        };

        if map.get("sthaniyaBranch").and_then(|v| v.as_str()) == Some("Loadboard sthaniya") {
            skipped += 1;
            continue;
        }

        map.insert(
            "sthaniyaBranch".to_string(),
            serde_json::Value::String("Loadboard sthaniya".to_string()),
        );

        match db
            .update_resource(resource::ResourceName::BillingRuleResource, id, item)
            .await
        {
            Ok(()) => updated += 1,
            Err(err) => {
                eprintln!(
                    "Failed to update BillingRuleResource {} with sthaniyaBranch: {:?}",
                    id, err
                );
            }
        }
    }

    println!(
        "BillingRuleResource sthaniyaBranch migration complete: {} updated, {} already up-to-date",
        updated, skipped
    );
}

pub async fn migrate_fiscal_year_id_to_fiscal_year(db: &Database) {
    for resource_name in [
        resource::ResourceName::BillingRuleResource,
        resource::ResourceName::BillResource,
    ] {
        let items = match db.read_all_resource(resource_name).await {
            Ok(items) => items,
            Err(err) => {
                eprintln!(
                    "Failed to read {:?} for fiscalYearId migration: {:?}",
                    resource_name, err
                );
                continue;
            }
        };

        let mut updated = 0usize;
        let mut skipped = 0usize;

        for mut item in items {
            let Some(id) = item.id else {
                continue;
            };

            let Some(serde_json::Value::Object(ref map)) = item.inner_content else {
                continue;
            };

            let has_fiscal_year_id = map.keys().any(|k| k.eq_ignore_ascii_case("fiscalyearid"));
            if !has_fiscal_year_id {
                skipped += 1;
                continue;
            }

            let mut new_map = serde_json::Map::with_capacity(map.len());
            let mut inserted_fiscal_year = false;

            for (k, v) in map.iter() {
                if k.eq_ignore_ascii_case("fiscalyearid") {
                    if !inserted_fiscal_year {
                        new_map.insert(
                            "fiscalYear".to_string(),
                            serde_json::Value::String("2082/2083".to_string()),
                        );
                        inserted_fiscal_year = true;
                    }
                    continue;
                }
                if k.eq_ignore_ascii_case("fiscalyear") {
                    continue;
                }
                new_map.insert(k.clone(), v.clone());
            }

            item.inner_content = Some(serde_json::Value::Object(new_map));

            match db.update_resource(resource_name, id, item).await {
                Ok(()) => updated += 1,
                Err(err) => {
                    eprintln!(
                        "Failed to migrate fiscalYearId on {:?} {}: {:?}",
                        resource_name, id, err
                    );
                }
            }
        }

        println!(
            "{:?} fiscalYearId -> fiscalYear migration complete: {} updated, {} skipped",
            resource_name, updated, skipped
        );
    }
}

fn load_tls_config() -> io::Result<ServerConfig> {
    let cert_path = env::var("TLS_CERT_PATH").unwrap_or_else(|_| "cert.pem".to_owned());
    let key_path = env::var("TLS_KEY_PATH").unwrap_or_else(|_| "key.pem".to_owned());

    let cert_chain = certs(&mut BufReader::new(File::open(cert_path)?))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
        .into_iter()
        .map(Certificate)
        .collect();

    let mut key_reader = BufReader::new(File::open(&key_path)?);
    let pkcs8_key = pkcs8_private_keys(&mut key_reader)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
        .into_iter()
        .next()
        .map(PrivateKey);
    let key = match pkcs8_key {
        Some(key) => key,
        None => {
            let mut key_reader = BufReader::new(File::open(&key_path)?);
            rsa_private_keys(&mut key_reader)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
                .into_iter()
                .next()
                .map(PrivateKey)
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "no private key found")
                })?
        }
    };

    ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenv().ok();

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let client = connect_to_db(&database_url).await.unwrap();

    // Create database tables if they don't exist
    let client = Arc::new(client);
    let db = Database::new(client.clone());

    if env::args().any(|arg| arg == "--run-migrations") {
        add_sthaniya_branch_to_billing_rule_resources(&db).await;
        migrate_fiscal_year_id_to_fiscal_year(&db).await;
        println!("One-off migrations finished; exiting (--run-migrations).");
        return Ok(());
    }
    
    //Delete previous tables
    //db.delete_tables().await.expect("Failed to delete tables");
    //db.delete_all_resources(resource::ResourceName::UserResource).await.expect("Failed to delete user resource");

    if let Err(e) = db.create_tables().await {
        eprintln!("Error creating tables: {}", e);
    }
    create_admin_user_if_not_exists(&db).await;
    add_sthaniya_branch_to_billing_rule_resources(&db).await;
    migrate_fiscal_year_id_to_fiscal_year(&db).await;

    let mut cache = cache::Cache::new();
    println!("Loading cache from database...");
    cache.load_from_db(&db).await;
    println!("Cache loaded from database");
    let tqms_client = Arc::new(tqms_integration::TQMSClient::new().await);
    let server_data = ServerData {
        db: Arc::new(db),
        tqms_client,
        cache: Arc::new(tokio::sync::RwLock::new(cache)),
    };
    permissions::hydrate_role_policies(&server_data).await;
    log::init_persistent_action_logger(
        server_data.db.clone(),
        Some(server_data.cache.clone()),
    )
    .await;
    audit::init_persistent_audit_trail(
        server_data.db.clone(),
        Some(server_data.cache.clone()),
    )
    .await;
    let tls_config = load_tls_config()?;
    let app_port = env::var("APP_PORT").unwrap_or_else(|_| "5001".to_owned());
    println!("Starting server on port {}", app_port);
    // Create and start HTTPS server
    let server_result = HttpServer::new(move || {
        let cors = Cors::default()
            .allow_any_origin()
            .allow_any_method()
            .allow_any_header()
            .expose_headers([
                "X-Stream-Format",
                "X-Batch-Size",
                "X-Concurrency",
                "X-Total-Items",
                "X-Total-Pages",
                "X-Starting-Offset",
            ])
            .max_age(3600);
        App::new()
            .wrap(cors)
            //.app_data(web::Data::new(server_data.clone()))
            .app_data(web::Data::new(Arc::new(server_data.clone())))
            .route("/", web::get().to(server_routes))
            .service(
                web::scope("/api/v1/auth")
                    .route("/login", web::post().to(login::login))
                    .route("/logout", web::post().to(access::logout)),
            )
            .service(
                web::scope("/api/v1/access")
                    .wrap(middleware::AuthMiddleware::with_roles(&[
                        roles::Roles::User,
                        roles::Roles::Admin,
                        roles::Roles::Operator,
                    ]))
                    .configure(access::configure_routes),
            )
            .service(
                web::scope("/api/v1/permissions")
                    .wrap(middleware::AuthMiddleware::with_roles(&[
                        roles::Roles::User,
                        roles::Roles::Admin,
                        roles::Roles::Operator,
                    ]))
                    .configure(permissions::configure_routes),
            )
            // .service(
            //     web::scope("/api/v1/Developer")
            //     .route("/create/user", web::post().to(routes::create_user))
            //     .route("/delete/user/{id}", web::post().to(routes::delete_user))
            //     .app_data(web::Data::new(Arc::new(vec![roles::Roles::Developer])))//Store Role
            //     .wrap(middleware::AuthMiddleware::with_role(roles::Roles::Developer))
            // )
            // .service(
            //     web::scope("/api/v1/Operator")
            //     .route("/Create/user", web::post().to(routes::create_user))
            //     .route("/Delete/user/{id}", web::post().to(routes::delete_user))
            //     .route("/Create/{resource_name}", web::post().to(routes::create_resource))
            //     .route("/Read/{resource_name}/{id}", web::post().to(routes::read_resource))
            //     .app_data(web::Data::new(Arc::new(vec![roles::Roles::Operator])))//Store Role
            //     .wrap(middleware::AuthMiddleware::with_roles(&[roles::Roles::Operator, roles::Roles::Administrator, roles::Roles::Developer]))
            // )
            // .service(
            //     web::scope("/api/v1/Administrator")
            //     .route("/Create/user", web::post().to(routes::create_user))
            //     .route("/Delete/user/{id}", web::post().to(routes::delete_user))
            //     .route("/Create/{resource_name}", web::post().to(routes::create_resource))
            //     .route("/Read/{resource_name}/{id}", web::post().to(routes::read_resource))
            //     .app_data(web::Data::new(Arc::new(vec![roles::Roles::Administrator])))//Store Role
            //     .wrap(middleware::AuthMiddleware::with_roles(&[roles::Roles::Administrator, roles::Roles::Developer]))
            // )
            // .service(
            //     web::scope("/api/v1/Staff")
            //     .route("/Create/{resource_name}", web::post().to(routes::create_resource))
            //     .route("/Read/{resource_name}/{id}", web::post().to(routes::read_resource))
            //     .app_data(web::Data::new(Arc::new(vec![roles::Roles::Staff])))//Store Role
            //     .wrap(middleware::AuthMiddleware::with_roles(&[roles::Roles::Administrator, roles::Roles::Developer, roles::Roles::Operator, roles::Roles::Staff]))
            // )
            // .service(
            //     web::scope("/api/v1/public")
            //     .route("/Read/{resource_name}/{id}", web::post().to(routes::read_resource))
            //     .app_data(web::Data::new(Arc::new(vec![roles::Roles::Visitor])))//Store Role
            // )
            .service(
                web::scope("/api/v1/dashboard")
                    .route(
                        "/statistics",
                        web::get().to(dashboard_statistics::get_dashboard_statistics),
                    )
                    .route(
                        "/statistics/{fiscal_year:.*}",
                        web::get().to(dashboard_statistics::get_dashboard_statistics_by_fy),
                    )
                    .service(web::scope("/members").configure(members::configure_routes)),
            )
            .service(web::scope("/api/v1/members").configure(members::configure_routes))
            .service(web::scope("/api/v1/sms").configure(sms::configure_routes))
            .service(
                web::scope("/api/v1/tqms")
                    .route("/search", web::post().to(tqms_integration::search_vehicles))
                    .route("/validate/{vehicle_number}", web::get().to(tqms_integration::validate_vehicle))
                    .route("/vehicle_details/{vehicle_number}", web::get().to(tqms_integration::get_vehicle_details))
                    .route("/sync", web::get().to(tqms_integration::sync_vehicles_tqms))
                    .route("/sync/{concurency}", web::get().to(tqms_integration::sync_vehicle_tqms_concurrency_limited))
                    .route("/stream_sync/{batch_size}/{concurrency}", web::get().to(tqms_integration::stream_sync_vehicles_tqms))
            )
            .service(
                web::scope("/api/v1/streams")
                .route("/search/{resource_name}/{search_query}/{search_item}/{limit}/{offset}", web::get().to(api_stream::search))
                .wrap(actix_web::middleware::Compress::default())
                .wrap(middleware::AuthMiddleware::with_roles(&[
                    roles::Roles::User,
                    roles::Roles::Admin,
                    roles::Roles::Operator,
                ]))
            )
            .service(
                web::scope("/api/integrations/lodbod/v1")
                    .route(
                        "/vehicles/billing-details",
                        web::post().to(billing_actions::get_vehicle_billing_details),
                    )
                    .route(
                        "/payments/vehicle-receipts",
                        web::post().to(billing_actions::submit_vehicle_payment),
                    )
                    .route(
                        "/payments/{lodbodPaymentReferenceId}/vehicle-receipts",
                        web::get().to(billing_actions::get_payment_receipt_status),
                    ),
            )
            .service(
                web::scope("/api/v1/bills")
                    .service(
                        web::resource("/create")
                            .route(web::post().to(billing_actions::create_bill))
                            .wrap(middleware::AuthMiddleware::with_roles(&[
                                roles::Roles::User,
                                roles::Roles::Admin,
                                roles::Roles::Operator,
                            ])),
                    )
                    .service(
                        web::resource("/modify/{id}")
                            .route(web::put().to(billing_actions::modify_bill))
                            .wrap(middleware::AuthMiddleware::with_roles(&[
                                roles::Roles::Admin,
                                roles::Roles::Operator,
                            ])),
                    )
                    .service(
                        web::resource("/delete/{id}")
                            .route(web::delete().to(billing_actions::delete_bill))
                            .wrap(middleware::AuthMiddleware::with_role(roles::Roles::Admin)),
                    )
                    .service(
                        web::resource("/rollback/{id}")
                            .route(web::post().to(billing_actions::rollback_bill))
                            .wrap(middleware::AuthMiddleware::with_role(roles::Roles::Admin)),
                    )
                    .service(
                        web::resource("/logs")
                            .route(web::get().to(billing_actions::get_bill_logs))
                            .wrap(middleware::AuthMiddleware::with_role(roles::Roles::Admin)),
                    )
                    .service(
                        web::resource("/logs/{bill_id}")
                            .route(web::get().to(billing_actions::get_bill_logs_by_bill_id))
                            .wrap(middleware::AuthMiddleware::with_role(roles::Roles::Admin)),
                    ),
            )
            .service(
                web::scope("/api/v1/vehicles/history")
                    .wrap(middleware::AuthMiddleware::with_roles(&[
                        roles::Roles::User,
                        roles::Roles::Admin,
                        roles::Roles::Operator,
                    ]))
                    .configure(vehicle_history::configure_routes),
            )
            .service(
                web::scope("/api/v1/settings")
                    .wrap(middleware::AuthMiddleware::with_roles(&[
                        roles::Roles::User,
                        roles::Roles::Admin,
                        roles::Roles::Operator,
                    ]))
                    .configure(settings::configure_routes),
            )
            .service(
                web::scope("/api/v1/logs")
                    .wrap(middleware::AuthMiddleware::with_role(roles::Roles::Admin))
                    .configure(log::configure_routes),
            )
            .service(
                web::scope("/api/v1/audit")
                    .wrap(middleware::AuthMiddleware::with_role(roles::Roles::Admin))
                    .configure(audit::configure_routes),
            )
            .service(
                web::scope("/api/v1")
                    .route(
                        "/update/{resource_name}/{id}",
                        web::put().to(actions::update),
                    )
                    .route(
                        "/delete/{resource_name}/{id}",
                        web::delete().to(actions::delete),
                    )
                    .wrap(middleware::AuthMiddleware::with_roles(&[
                        roles::Roles::User,
                        roles::Roles::Admin,
                        roles::Roles::Operator,
                    ]))
                    .route("/create/{resource_name}", web::post().to(actions::write))
                    .route(
                        "/search/{resource_name}/{search_query}/{search_item}/{limit}",
                        web::get().to(actions::search),
                    )
                    .route(
                        "/read_all/VehicleTypes",
                        web::get().to(tqms_integration::read_all_vehicle_types),
                    )
                    .route("/read/{resource_name}/{id}", web::get().to(actions::read))
                    .route(
                        "/get_vehicles_tqms",
                        web::get().to(tqms_integration::get_vehicle_from_tqms),
                    ),
            )
    })
    .bind(format!("127.0.0.1:{}", app_port))
    .expect("Failed to bind to port")
    .run()
    .await;

    log::flush_action_logs().await;
    audit::flush_audit_trail().await;
    server_result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::test_helpers::{create_mock_server_data_with_state, MockDbState};
    use crate::resource::ResourceName;

    #[actix_web::test]
    async fn test_add_sthaniya_branch_to_billing_rule_resources_updates_all_existing_items() {
        let mut state = MockDbState::default();
        state.seed(
            ResourceName::BillingRuleResource,
            DatabaseResource {
                id: Some(29),
                resource_name: Some(ResourceName::BillingRuleResource),
                inner_content: Some(serde_json::json!({
                    "code": "",
                    "name": "Entry",
                    "status": "ACTIVE",
                    "fiscalYear": "2082/2083",
                    "displayOrder": 11,
                    "resourceType": "FEE_HEAD"
                })),
                external_content: None,
            },
        );
        state.seed(
            ResourceName::BillingRuleResource,
            DatabaseResource {
                id: Some(16),
                resource_name: Some(ResourceName::BillingRuleResource),
                inner_content: Some(serde_json::json!({
                    "scope": "COMMON",
                    "amount": 0,
                    "status": "ACTIVE",
                    "fiscalYear": "2082/2083",
                    "resourceType": "BILLING_RULE",
                    "billingEntityId": 1
                })),
                external_content: Some(serde_json::json!({ "createdTime": "2026-11-16T10:11:27Z" })),
            },
        );
        state.seed(
            ResourceName::BillingRuleResource,
            DatabaseResource {
                id: Some(26),
                resource_name: Some(ResourceName::BillingRuleResource),
                inner_content: Some(serde_json::json!({
                    "scope": "COMMON",
                    "amount": 0,
                    "sthaniyaBranch": "Loadboard sthaniya"
                })),
                external_content: None,
            },
        );

        let (s_data, db_state) = create_mock_server_data_with_state(state).await;

        add_sthaniya_branch_to_billing_rule_resources(&s_data.db).await;

        {
            let guard = db_state.lock().await;
            let rows = guard.tables.get("billingruleresource").unwrap();
            assert_eq!(rows.len(), 3);
            for row in rows {
                let inner = row.inner_content.as_ref().unwrap();
                assert_eq!(
                    inner.get("sthaniyaBranch").and_then(|v| v.as_str()),
                    Some("Loadboard sthaniya")
                );
            }
            let row_29 = rows.iter().find(|r| r.id == Some(29)).unwrap();
            assert_eq!(row_29.inner_content.as_ref().unwrap()["name"], "Entry");
            let row_16 = rows.iter().find(|r| r.id == Some(16)).unwrap();
            assert_eq!(
                row_16.external_content,
                Some(serde_json::json!({ "createdTime": "2026-11-16T10:11:27Z" }))
            );
        }

        // Second run is idempotent
        add_sthaniya_branch_to_billing_rule_resources(&s_data.db).await;
        let guard = db_state.lock().await;
        for row in guard.tables.get("billingruleresource").unwrap() {
            assert_eq!(
                row.inner_content
                    .as_ref()
                    .unwrap()
                    .get("sthaniyaBranch")
                    .and_then(|v| v.as_str()),
                Some("Loadboard sthaniya")
            );
        }
    }

    #[actix_web::test]
    async fn test_migrate_fiscal_year_id_to_fiscal_year_in_billing_rule_and_bill_resources() {
        let mut state = MockDbState::default();

        // BillResource rows
        state.seed(
            ResourceName::BillResource,
            DatabaseResource {
                id: Some(149),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(serde_json::json!({
                    "total": 565,
                    "vehicle": { "id": "36010", "vehicleNumber": "PROVINCE-4-56-02-JHA-8241" },
                    "items": [{ "title": "Entry", "amount": 500 }],
                    "fiscalYearId": "15f21721-50e8-41af-9596-9d8cdb8d6a16",
                    "paymentMethod": "नगद"
                })),
                external_content: Some(serde_json::json!({ "createdTime": "2026-11-17T09:55:09Z" })),
            },
        );
        state.seed(
            ResourceName::BillResource,
            DatabaseResource {
                id: Some(150),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(serde_json::json!({
                    "total": 100,
                    "fiscalyearid": "any-value"
                })),
                external_content: None,
            },
        );
        state.seed(
            ResourceName::BillResource,
            DatabaseResource {
                id: Some(151),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(serde_json::json!({
                    "total": 200,
                    "fiscalYear": "2083/2084"
                })),
                external_content: None,
            },
        );

        // BillingRuleResource rows
        state.seed(
            ResourceName::BillingRuleResource,
            DatabaseResource {
                id: Some(24),
                resource_name: Some(ResourceName::BillingRuleResource),
                inner_content: Some(serde_json::json!({
                    "scope": "COMMON",
                    "amount": 0,
                    "fiscalYearId": "15f21721-50e8-41af-9596-9d8cdb8d6a16",
                    "resourceType": "BILLING_RULE"
                })),
                external_content: None,
            },
        );
        state.seed(
            ResourceName::BillingRuleResource,
            DatabaseResource {
                id: Some(25),
                resource_name: Some(ResourceName::BillingRuleResource),
                inner_content: Some(serde_json::json!({
                    "name": "Fee Head",
                    "fiscalYearId": null,
                    "resourceType": "FEE_HEAD"
                })),
                external_content: None,
            },
        );
        state.seed(
            ResourceName::BillingRuleResource,
            DatabaseResource {
                id: Some(29),
                resource_name: Some(ResourceName::BillingRuleResource),
                inner_content: Some(serde_json::json!({
                    "name": "Entry",
                    "fiscalYear": "2082/2083",
                    "resourceType": "FEE_HEAD"
                })),
                external_content: None,
            },
        );

        let (s_data, db_state) = create_mock_server_data_with_state(state).await;

        migrate_fiscal_year_id_to_fiscal_year(&s_data.db).await;

        {
            let guard = db_state.lock().await;
            let bill_rows = guard.tables.get("billresource").unwrap();
            assert_eq!(bill_rows.len(), 3);

            let b149 = bill_rows.iter().find(|r| r.id == Some(149)).unwrap();
            let b149_inner = b149.inner_content.as_ref().unwrap().as_object().unwrap();
            assert!(!b149_inner.keys().any(|k| k.eq_ignore_ascii_case("fiscalyearid")));
            assert_eq!(b149_inner.get("fiscalYear").and_then(|v| v.as_str()), Some("2082/2083"));
            assert_eq!(b149_inner.get("total").and_then(|v| v.as_i64()), Some(565));
            assert_eq!(
                b149.external_content,
                Some(serde_json::json!({ "createdTime": "2026-11-17T09:55:09Z" }))
            );

            let b150 = bill_rows.iter().find(|r| r.id == Some(150)).unwrap();
            let b150_inner = b150.inner_content.as_ref().unwrap().as_object().unwrap();
            assert!(!b150_inner.keys().any(|k| k.eq_ignore_ascii_case("fiscalyearid")));
            assert_eq!(b150_inner.get("fiscalYear").and_then(|v| v.as_str()), Some("2082/2083"));
            assert_eq!(b150_inner.get("total").and_then(|v| v.as_i64()), Some(100));

            let b151 = bill_rows.iter().find(|r| r.id == Some(151)).unwrap();
            assert_eq!(
                b151.inner_content
                    .as_ref()
                    .unwrap()
                    .get("fiscalYear")
                    .and_then(|v| v.as_str()),
                Some("2083/2084")
            );

            let rule_rows = guard.tables.get("billingruleresource").unwrap();
            assert_eq!(rule_rows.len(), 3);

            for rule_id in [24, 25] {
                let row = rule_rows.iter().find(|r| r.id == Some(rule_id)).unwrap();
                let obj = row.inner_content.as_ref().unwrap().as_object().unwrap();
                assert!(!obj.keys().any(|k| k.eq_ignore_ascii_case("fiscalyearid")));
                assert_eq!(obj.get("fiscalYear").and_then(|v| v.as_str()), Some("2082/2083"));
            }

            let r29 = rule_rows.iter().find(|r| r.id == Some(29)).unwrap();
            assert_eq!(
                r29.inner_content
                    .as_ref()
                    .unwrap()
                    .get("fiscalYear")
                    .and_then(|v| v.as_str()),
                Some("2082/2083")
            );
        }

        // Second run is idempotent
        migrate_fiscal_year_id_to_fiscal_year(&s_data.db).await;
        let guard = db_state.lock().await;
        for row in guard.tables.get("billresource").unwrap() {
            let obj = row.inner_content.as_ref().unwrap().as_object().unwrap();
            assert!(!obj.keys().any(|k| k.eq_ignore_ascii_case("fiscalyearid")));
        }
        let b151 = guard
            .tables
            .get("billresource")
            .unwrap()
            .iter()
            .find(|r| r.id == Some(151))
            .unwrap();
        assert_eq!(
            b151.inner_content
                .as_ref()
                .unwrap()
                .get("fiscalYear")
                .and_then(|v| v.as_str()),
            Some("2083/2084")
        );
    }
}
