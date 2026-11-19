use actix_cors::Cors;
use actix_web::{web, App, HttpResponse, HttpServer};
use dotenv::dotenv;
use rustls::{Certificate, PrivateKey, ServerConfig};
use rustls_pemfile::{certs, pkcs8_private_keys, rsa_private_keys};
use std::fs::File;
use std::io::{self, BufReader};
use std::env;
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
pub mod sms;


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
            "method: POST,      api/v1/sms/preview",
            "method: POST,      api/v1/sms/send",
            "method: POST,      api/v1/sms/send-raw",
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

    let db_user = db.read_all_resource(resource::ResourceName::UserResource).await;
    if let Ok(users) = db_user {
        if !users.is_empty() {
            println!("Admin user already exists");
            return;
        }
    }
    let resource = DatabaseResource {
        id: None,
        inner_content: Some(serde_json::json!({
            "email": email,
            "password": password,
            "role": "Admin",
        })),
        external_content: None,
        resource_name: Some(resource::ResourceName::UserResource),
    };
    let result = db.create_resource(resource::ResourceName::UserResource, resource);

    if result.await.is_ok() {
        println!("Admin user created successfully");
    } else {
        println!("Failed to create admin user");
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
    
    //Delete previous tables
    //db.delete_tables().await.expect("Failed to delete tables");
    //db.delete_all_resources(resource::ResourceName::UserResource).await.expect("Failed to delete user resource");

    if let Err(e) = db.create_tables().await {
        eprintln!("Error creating tables: {}", e);
    }
    create_admin_user_if_not_exists(&db).await;

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
    let tls_config = load_tls_config()?;
    let app_port = env::var("APP_PORT").unwrap_or_else(|_| "5001".to_owned());
    println!("Starting server on port {}", app_port);
    // Create and start HTTPS server
    HttpServer::new(move || {
        let cors = Cors::default()
            .allow_any_origin()
            .allow_any_method()
            .allow_any_header()
            .max_age(3600);
        App::new()
            .wrap(cors)
            //.app_data(web::Data::new(server_data.clone()))
            .app_data(web::Data::new(Arc::new(server_data.clone())))
            .route("/", web::get().to(server_routes))
            .service(web::scope("/api/v1/auth").route("/login", web::post().to(login::login)))
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
                        "/statistics/{fiscal_year}",
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
            )
            .service(
                web::scope("/api/v1/streams")
                .route("/search/{resource_name}/{search_query}/{search_item}/{limit}/{offset}", web::get().to(api_stream::search))
                .wrap(actix_web::middleware::Compress::default())
                .wrap(middleware::AuthMiddleware::with_roles(&[
                    roles::Roles::Admin
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
                            .wrap(middleware::AuthMiddleware::with_role(roles::Roles::Operator)),
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
    .await
}
