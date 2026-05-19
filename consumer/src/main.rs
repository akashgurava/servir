use echo_server::{AppState, app_routes, db};
use std::net::SocketAddr;

use servir::Servir;

#[tokio::main]
async fn main() {
    let app_db_url =
        std::env::var("APP_DATABASE_URL").unwrap_or_else(|_| "sqlite://app.db".to_string());
    let app_pool = db::connect(&app_db_url).await.expect("app db connect");
    db::migrate(&app_pool).await.expect("app db migrate");

    let state = AppState { pool: app_pool };

    Servir::builder()
        .service_name("echo-server")
        .addr(SocketAddr::from(([0, 0, 0, 0], 3000)))
        .routes(app_routes(state))
        .serve()
        .await
        .expect("server failed");
}
