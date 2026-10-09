use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Clone, Serialize)]
struct Item {
    id: Uuid,
    name: String,
    description: String,
}

#[derive(Deserialize)]
struct ItemInput {
    name: String,
    description: String,
}

type Db = Arc<RwLock<HashMap<Uuid, Item>>>;

fn app(db: Db) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/items", get(list_items).post(create_item))
        .route(
            "/items/{id}",
            get(get_item).put(update_item).delete(delete_item),
        )
        .with_state(db)
}

async fn health() -> &'static str {
    "ok"
}

async fn list_items(State(db): State<Db>) -> Json<Vec<Item>> {
    let items = db.read().await.values().cloned().collect();
    Json(items)
}

async fn create_item(
    State(db): State<Db>,
    Json(input): Json<ItemInput>,
) -> Result<(StatusCode, Json<Item>), StatusCode> {
    let name = input.name.trim().to_string();
    if name.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let item = Item {
        id: Uuid::new_v4(),
        name,
        description: input.description,
    };
    db.write().await.insert(item.id, item.clone());
    Ok((StatusCode::CREATED, Json(item)))
}

async fn get_item(
    State(db): State<Db>,
    Path(id): Path<Uuid>,
) -> Result<Json<Item>, StatusCode> {
    db.read()
        .await
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn update_item(
    State(db): State<Db>,
    Path(id): Path<Uuid>,
    Json(input): Json<ItemInput>,
) -> Result<Json<Item>, StatusCode> {
    let name = input.name.trim().to_string();
    if name.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut db = db.write().await;
    let item = db.get_mut(&id).ok_or(StatusCode::NOT_FOUND)?;
    item.name = name;
    item.description = input.description;
    Ok(Json(item.clone()))
}

async fn delete_item(State(db): State<Db>, Path(id): Path<Uuid>) -> StatusCode {
    match db.write().await.remove(&id) {
        Some(_) => StatusCode::NO_CONTENT,
        None => StatusCode::NOT_FOUND,
    }
}

#[tokio::main]
async fn main() {
    let db: Db = Arc::new(RwLock::new(HashMap::new()));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .expect("bind 0.0.0.0:3000");
    println!("listening on {}", listener.local_addr().expect("local addr"));
    axum::serve(listener, app(db)).await.expect("serve");
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_db() -> Db {
        Arc::new(RwLock::new(HashMap::new()))
    }

    async fn body_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn health_is_ok() {
        let response = app(test_db())
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn crud_roundtrip() {
        let db = test_db();

        let created = app(db.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/items")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"widget","description":"first"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let created_json = body_json(created).await;
        let id = created_json["id"].as_str().unwrap();

        let listed = app(db.clone())
            .oneshot(Request::builder().uri("/items").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        let listed_json = body_json(listed).await;
        assert_eq!(listed_json.as_array().unwrap().len(), 1);

        let updated = app(db.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/items/{id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"widget-2","description":"second"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(updated.status(), StatusCode::OK);
        assert_eq!(body_json(updated).await["name"], "widget-2");

        let deleted = app(db.clone())
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/items/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(deleted.status(), StatusCode::NO_CONTENT);

        let missing = app(db)
            .oneshot(
                Request::builder()
                    .uri(format!("/items/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn rejects_blank_name_and_unknown_id() {
        let db = test_db();
        let blank = app(db.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/items")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"  ","description":""}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(blank.status(), StatusCode::BAD_REQUEST);

        let missing = app(db)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/items/{}", Uuid::new_v4()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }
}
