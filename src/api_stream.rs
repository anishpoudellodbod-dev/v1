use crate::resource::{DatabaseResource, ResourceName};
use crate::ServerData;

use actix_web::{
    error::ErrorInternalServerError,
    http::header,
    web,
    Error,
    HttpRequest,
    HttpResponse,
};

use bytes::Bytes;
use futures_util::stream::{self};
use std::{
    collections::VecDeque,
    sync::Arc,
};

pub async fn search(
    path: web::Path<(
        String,
        String,
        String,
        String,
        Option<String>,
    )>,
    req: HttpRequest,
    s_data: web::Data<Arc<ServerData>>,
) -> HttpResponse {
    let (
        resource_name,
        search_item,
        search_query,
        batch_limit,
        offset,
    ) = path.into_inner();

    let resource_name: ResourceName = match ResourceName::from_str_case_insensitive(&resource_name) {
        Some(resource_name) => resource_name,
        None => {
            return HttpResponse::BadRequest()
                .body("Invalid resource name");
        }
    };

    match crate::permissions::enforce_request_permission(
        &req,
        &s_data,
        crate::permissions::canonical_resource_name(resource_name),
        crate::permissions::action::READ,
        crate::permissions::extract_branch_id_from_req(&req),
    )
    .await
    {
        crate::permissions::PermissionCheckOutcome::Denied { message, .. } => {
            return HttpResponse::Forbidden().body(message);
        }
        crate::permissions::PermissionCheckOutcome::Allowed { .. } => {}
        crate::permissions::PermissionCheckOutcome::FallbackLegacy => {
            if resource_name == ResourceName::UserResource {
                return HttpResponse::Forbidden().body("Search in UserResource is forbidden");
            }
            if !crate::actions::is_admin(req) {
                return HttpResponse::Forbidden().body("Admin access required");
            }
        }
    }

    let batch_limit: usize = if batch_limit.eq_ignore_ascii_case("all") {
        5000
    } else {
        match batch_limit.parse::<usize>() {
            Ok(0) => {
                return HttpResponse::BadRequest()
                    .body("Batch limit must be greater than zero");
            }
            Ok(limit) => limit,
            Err(_) => {
                return HttpResponse::BadRequest()
                    .body("Invalid batch limit");
            }
        }
    };

    let starting_offset = match offset
        .as_deref()
        .unwrap_or("0")
        .parse::<usize>()
    {
        Ok(offset) => offset,
        Err(_) => {
            return HttpResponse::BadRequest()
                .body("Invalid offset");
        }
    };

    let first_batch = {
        let cache = s_data.cache.read().await;

        cache.search_innercontent_batch(
            resource_name.clone(),
            &search_item,
            &search_query,
            batch_limit,
            starting_offset,
        )
    };

    if first_batch.is_empty() {
        return HttpResponse::NotFound().body(format!(
            "No resources found for search query: {}",
            search_query
        ));
    }

    let first_batch_len = first_batch.len();

    let initial_state = SearchStreamState {
        next_offset: starting_offset + first_batch_len,
        pending: resources_to_ndjson_chunks(first_batch),
        finished: first_batch_len < batch_limit,
    };

    let response_stream = stream::unfold(
        initial_state,
        move |mut state| {
            let s_data = s_data.clone();
            let resource_name = resource_name.clone();
            let search_item = search_item.clone();
            let search_query = search_query.clone();

            async move {
                loop {
                    if let Some(chunk) = state.pending.pop_front() {
                        return Some((chunk, state));
                    }

                    if state.finished {
                        return None;
                    }

                    tokio::task::yield_now().await;

                    let resources = {
                        let cache = s_data.cache.read().await;

                        cache.search_innercontent_batch(
                            resource_name.clone(),
                            &search_item,
                            &search_query,
                            batch_limit,
                            state.next_offset,
                        )
                    };

                    if resources.is_empty() {
                        return None;
                    }

                    let resources_len = resources.len();

                    state.next_offset += resources_len;
                    state.finished = resources_len < batch_limit;
                    state.pending =
                        resources_to_ndjson_chunks(resources);
                }
            }
        },
    );

    HttpResponse::Ok()
        .insert_header((
            header::CONTENT_TYPE,
            "application/x-ndjson; charset=utf-8",
        ))
        .insert_header((
            header::CACHE_CONTROL,
            "no-cache, no-store, no-transform",
        ))
        .insert_header((
            header::CONTENT_ENCODING,
            "identity",
        ))
        .insert_header(("X-Accel-Buffering", "no"))
        .insert_header((
            "X-Batch-Size",
            batch_limit.to_string(),
        ))
        .insert_header((
            "X-Starting-Offset",
            starting_offset.to_string(),
        ))
        .streaming::<_, Error>(response_stream)
}

struct SearchStreamState {
    next_offset: usize,
    pending: VecDeque<Result<Bytes, Error>>,
    finished: bool,
}

fn resources_to_ndjson_chunks(
    resources: Vec<DatabaseResource>,
) -> VecDeque<Result<Bytes, Error>> {
    resources
        .into_iter()
        .map(|resource| {
            serde_json::to_vec(&resource)
                .map(|mut json| {
                    json.push(b'\n');
                    Bytes::from(json)
                })
                .map_err(ErrorInternalServerError)
        })
        .collect()
}