use std::net::SocketAddr;

use clap::Parser;
use echo_server::{AppState, app_routes, db};
use servir::Servir;

#[derive(Parser)]
#[command(name = "echo-server")]
struct Args {
    /// Path to the auth SQLite database.
    #[arg(long, env = "CONSUMER_AUTH_DB", default_value = "./data/db/auth.db")]
    auth_db: String,

    /// Path to the app data SQLite database.
    #[arg(long, env = "CONSUMER_DATA_DB", default_value = "./data/db/data.db")]
    data_db: String,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let auth_db_url = format!("sqlite://{}", args.auth_db);
    let data_db_url = format!("sqlite://{}", args.data_db);

    let app_pool = db::connect(&data_db_url).await.expect("app db connect");
    db::migrate(&app_pool).await.expect("app db migrate");

    let state = AppState { pool: app_pool };

    Servir::builder()
        .service_name("echo-server")
        .addr(SocketAddr::from(([0, 0, 0, 0], 3000)))
        .auth_database_url(auth_db_url)
        .routes(app_routes(state))
        .serve()
        .await
        .expect("server failed");
}
