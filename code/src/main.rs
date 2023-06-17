#![deny(warnings)]
use pretty_env_logger;
use std::env;
use warp::{http::{StatusCode}, Filter, Rejection, Reply, reject};
use mongodb::{bson::doc, gridfs::GridFsBucket, gridfs::FilesCollectionDocument, Client};
use hyper::{Body,Response};
use futures::TryStreamExt;
use tokio_util::io::ReaderStream;
use tokio_util::compat::FuturesAsyncReadCompatExt;

use headers::{
    ContentLength, ContentType, HeaderMapExt
};

use mime_guess;

async fn download_handler(filename: String, bucket: GridFsBucket) -> Result<impl Reply, Rejection> {
    let filter = doc!{"filename": filename.clone()};
    println!("finding file");
    match bucket.find(filter, None).await {
        Ok(cursor) => {
            println!("found file");
            let file_collections: Result<Vec<FilesCollectionDocument>,_> = cursor.try_collect().await;
            match file_collections {
                Ok(metas) => {
                    let _chunk_size = metas[0].chunk_size_bytes;
                    let length = metas[0].length;
                    println!("opening file");
                    let download_stream = bucket
                        .open_download_stream_by_name(filename.clone(), None)
                        .await
                        .expect("should be able to download data to bucket");
                    println!("opened file");
                    println!("making stream");
                    let stream = ReaderStream::new(download_stream.compat());
                    println!("made stream");
                    println!("making body");
                    let body = Body::wrap_stream(stream);
                    println!("made body");
                    println!("setting response values");
                    let mut resp = Response::new(body);
                    *resp.status_mut() = StatusCode::OK;
                    resp.headers_mut().typed_insert(ContentLength(length));

                    let mime = mime_guess::from_path(filename.clone()).first_or_octet_stream();
                    resp.headers_mut().typed_insert(ContentType::from(mime));
                    
                    Ok(resp)
                },
                Err(_) => return Err(reject::not_found())
            }
        },
        Err(_) => {
            println!("no file");
            return Err(reject::not_found())
        }
    }
}

pub async fn health_handler() -> Result<impl Reply, Rejection> {
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

    let media = warp::path("violin");
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

    warp::serve(routes).run(([0, 0, 0, 0], 80)).await;

}