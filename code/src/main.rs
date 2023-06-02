#![deny(warnings)]
use pretty_env_logger;
use std::env;
use warp::{http::StatusCode, Filter, Rejection, Reply};

type Result<T> = std::result::Result<T, Rejection>;
use mongodb::{gridfs::GridFsBucket, Client};

async fn download_handler(filename: String, bucket: GridFsBucket) -> Result<impl Reply> {
    let mut buf = Vec::new();
    
    bucket
        .download_to_futures_0_3_writer_by_name(filename, &mut buf, None)
        .await
        .expect("should be able to download data to bucket");

    Ok(buf)
}

pub async fn health_handler() -> Result<impl Reply> {
    Ok(StatusCode::OK)
}

#[tokio::main]    
async fn main() {
    pretty_env_logger::init();

    let mongo_endpoint = env::var("ENDPOINT").expect("$ENDPOINT is not set");
    let mongo_table = env::var("TABLE").expect("$TABLE is not set");

    let client = Client::with_uri_str(&mongo_endpoint).await.expect("should be able to setup MondoDB client");
    let db = client.database(&mongo_table);
    let bucket = db.gridfs_bucket(None);

    let bucket = warp::any().map(move || bucket.clone());

    let health_route = warp::path!("health").and_then(health_handler);

    let media = warp::path(env!("API_PATH"));
    let media_routes = media
        .and(warp::get())
        .and(warp::path::param())
        .and(bucket)
        .and_then(download_handler)
        .or(
            media.and(warp::options().map(warp::reply))
        );
        

    let routes = warp::path("api").and(
        warp::path("media").and(
            warp::path("v1").and(
                health_route
                .or(media_routes)
            )
        )
    );

    warp::serve(routes).run(([127, 0, 0, 1], 80)).await;

}